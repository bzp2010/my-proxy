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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::sleep;

use inbound::{InboundServer, ListenAddr, TimeoutConfig};

async fn spawn_fake_upstream() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let io = TokioIo::new(stream);
        let service = service_fn(|_req: Request<Incoming>| async move {
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from("ok"))))
        });
        let _ = server_http1::Builder::new().serve_connection(io, service).await;
    });
    addr
}

async fn start_proxy(timeouts: TimeoutConfig) -> std::net::SocketAddr {
    let upstream_addr = spawn_fake_upstream().await;
    let cluster = Arc::new(cluster::Cluster::new(vec![cluster::Endpoint {
        addr: upstream_addr,
    }]));
    let server = InboundServer::bind(
        ListenAddr::Http("127.0.0.1:0".parse().unwrap()),
        timeouts,
    )
    .await
    .unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = server
            .serve(move |req| {
                let cluster = cluster.clone();
                async move { proxy::handle(cluster, req).await }
            })
            .await;
    });
    addr
}

#[tokio::test]
async fn header_read_deadline_closes_a_slow_drip_connection() {
    let addr = start_proxy(TimeoutConfig {
        header_read: Duration::from_millis(200),
        idle: Duration::from_secs(10),
    })
    .await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    // Trickle the request line one byte at a time, slower than the
    // 200ms hard deadline in total, well under it per gap.
    for byte in b"GET / HTTP/1.1\r\n" {
        let _ = stream.write_all(&[*byte]).await;
        sleep(Duration::from_millis(60)).await;
    }

    let mut buf = [0u8; 1];
    let result = stream.read(&mut buf).await;
    // The server must have closed the connection before the request
    // line (let alone the full header block) ever completed.
    assert!(matches!(result, Ok(0) | Err(_)));
}

#[tokio::test]
async fn idle_timeout_closes_a_connection_with_no_next_request() {
    let addr = start_proxy(TimeoutConfig {
        header_read: Duration::from_secs(10),
        idle: Duration::from_millis(200),
    })
    .await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    sleep(Duration::from_millis(500)).await;

    let mut buf = [0u8; 1];
    let result = stream.read(&mut buf).await;
    assert!(matches!(result, Ok(0) | Err(_)));
}
