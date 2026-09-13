mod common;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1 as server_http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use openssl::ssl::{Ssl, SslConnector, SslMethod, SslVerifyMode};
use tokio::net::{TcpListener, TcpStream};
use tokio_openssl::SslStream;

use inbound::{InboundServer, ListenAddr, TimeoutConfig};

async fn spawn_fake_upstream() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let io = TokioIo::new(stream);
        let service = service_fn(|_req: Request<Incoming>| async move {
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from("hello from upstream"))))
        });
        let _ = server_http1::Builder::new().serve_connection(io, service).await;
    });
    addr
}

fn default_timeouts() -> TimeoutConfig {
    TimeoutConfig {
        header_read: Duration::from_secs(10),
        idle: Duration::from_secs(60),
    }
}

#[tokio::test]
async fn https_listener_forwards_a_real_request() {
    let upstream_addr = spawn_fake_upstream().await;
    let cluster = Arc::new(cluster::Cluster::new(vec![cluster::Endpoint {
        addr: upstream_addr,
    }]));

    let (_dir, cert_path, key_path) = common::self_signed_cert();
    let listen_addr = ListenAddr::Https {
        addr: "127.0.0.1:0".parse().unwrap(),
        tls: inbound::tls::TlsConfig { cert_path, key_path },
    };
    let server = InboundServer::bind(listen_addr, default_timeouts())
        .await
        .expect("bind should succeed");
    let proxy_addr = server.local_addr().unwrap();

    tokio::spawn(async move {
        let _ = server
            .serve(move |req| {
                let cluster = cluster.clone();
                async move { proxy::handle(cluster, req).await }
            })
            .await;
    });

    let mut connector_builder = SslConnector::builder(SslMethod::tls()).unwrap();
    connector_builder.set_verify(SslVerifyMode::NONE);
    // SslConnector::builder() does not advertise ALPN by default; the
    // server can only select http/1.1 if the client actually offers it.
    connector_builder.set_alpn_protos(b"\x08http/1.1").unwrap();
    let connector = connector_builder.build();

    let tcp = TcpStream::connect(proxy_addr).await.unwrap();
    let ssl = connector.configure().unwrap().into_ssl("localhost").unwrap();
    let mut tls_stream = SslStream::new(ssl, tcp).unwrap();
    std::pin::Pin::new(&mut tls_stream).connect().await.unwrap();

    assert_eq!(
        tls_stream.ssl().selected_alpn_protocol(),
        Some(b"http/1.1".as_slice())
    );

    let io = TokioIo::new(tls_stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method("GET")
        .uri("/")
        .header("Host", proxy_addr.to_string())
        .body(
            Empty::<Bytes>::new()
                .map_err(|never: Infallible| match never {})
                .boxed(),
        )
        .unwrap();

    let response = sender.send_request(req).await.unwrap();
    assert_eq!(response.status(), 200);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body_bytes[..], b"hello from upstream");
}
