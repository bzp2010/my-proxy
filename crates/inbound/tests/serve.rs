use std::convert::Infallible;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::client::conn::http1 as client_http1;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use inbound::InboundServer;

#[tokio::test]
async fn serve_calls_handler_and_returns_its_response() {
    let server = InboundServer::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr().unwrap();

    tokio::spawn(async move {
        let _ = server
            .serve(|_req| async move {
                Response::new(
                    Full::new(Bytes::from("hello from handler"))
                        .map_err(|never: Infallible| match never {})
                        .boxed(),
                )
            })
            .await;
    });

    let stream = TcpStream::connect(addr).await.unwrap();
    let io = TokioIo::new(stream);
    let (mut sender, conn) = client_http1::handshake(io).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let body: BoxBody<Bytes, hyper::Error> = Empty::<Bytes>::new()
        .map_err(|never: Infallible| match never {})
        .boxed();
    let req = Request::builder()
        .method("GET")
        .uri("/")
        .header("Host", addr.to_string())
        .body(body)
        .unwrap();

    let response = sender.send_request(req).await.unwrap();
    assert_eq!(response.status(), 200);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body_bytes[..], b"hello from handler");
}
