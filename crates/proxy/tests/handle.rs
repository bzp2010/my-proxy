use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::client::conn::http1 as client_http1;
use hyper::server::conn::http1 as server_http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};

use cluster::{Cluster, Endpoint};
use inbound::InboundServer;

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

#[tokio::test]
async fn handle_forwards_request_to_selected_endpoint() {
    let upstream_addr = spawn_fake_upstream().await;
    let cluster = Arc::new(Cluster::new(vec![Endpoint { addr: upstream_addr }]));

    let inbound_server = InboundServer::bind(
        inbound::ListenAddr::Http("127.0.0.1:0".parse().unwrap()),
        inbound::TimeoutConfig {
            header_read: Duration::from_secs(10),
            idle: Duration::from_secs(60),
        },
    )
    .await
    .unwrap();
    let proxy_addr = inbound_server.local_addr().unwrap();

    tokio::spawn(async move {
        let _ = inbound_server
            .serve(move |req| {
                let cluster = cluster.clone();
                async move { proxy::handle(cluster, req).await }
            })
            .await;
    });

    let stream = TcpStream::connect(proxy_addr).await.unwrap();
    let io = TokioIo::new(stream);
    let (mut sender, conn) = client_http1::handshake(io).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method("GET")
        .uri("/")
        .header("Host", proxy_addr.to_string())
        .body(
            Empty::<Bytes>::new()
                .map_err(|never: Infallible| -> hyper::Error { match never {} })
                .boxed(),
        )
        .unwrap();

    let response = sender.send_request(req).await.unwrap();
    assert_eq!(response.status(), 200);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body_bytes[..], b"hello from upstream");
}
