use std::convert::Infallible;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use outbound::{connect, Endpoint};

async fn spawn_fake_upstream() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let io = TokioIo::new(stream);
        let service = service_fn(|_req: Request<Incoming>| async move {
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from("hello from upstream"))))
        });
        let _ = http1::Builder::new().serve_connection(io, service).await;
    });
    addr
}

#[tokio::test]
async fn connect_and_send_request_returns_upstream_response() {
    let addr = spawn_fake_upstream().await;
    let endpoint = Endpoint { addr };

    let mut connection = connect(endpoint).await.expect("connect should succeed");

    let req = Request::builder()
        .method("GET")
        .uri("/")
        .header("Host", addr.to_string())
        .body(
            http_body_util::Empty::<Bytes>::new()
                .map_err(|never: Infallible| match never {})
                .boxed(),
        )
        .unwrap();

    let response = connection.send_request(req).await.expect("request should succeed");
    assert_eq!(response.status(), 200);

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body_bytes[..], b"hello from upstream");
}
