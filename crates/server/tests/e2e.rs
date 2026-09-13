use std::convert::Infallible;
use std::net::SocketAddr;
use std::process::Stdio;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::client::conn::http1 as client_http1;
use hyper::server::conn::http1 as server_http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;

async fn spawn_fake_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let io = TokioIo::new(stream);
            let service = service_fn(|_req: Request<Incoming>| async move {
                Ok::<_, Infallible>(Response::new(Full::new(Bytes::from("hello from upstream"))))
            });
            tokio::spawn(async move {
                let _ = server_http1::Builder::new().serve_connection(io, service).await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn proxies_request_through_the_compiled_binary() {
    let upstream_addr = spawn_fake_upstream().await;

    let mut child = Command::new(env!("CARGO_BIN_EXE_server"))
        .arg("127.0.0.1:0")
        .arg(upstream_addr.to_string())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to start server binary");

    let stdout = child.stdout.take().expect("child stdout should be piped");
    let mut lines = BufReader::new(stdout).lines();
    let listening_line = lines
        .next_line()
        .await
        .expect("failed to read child stdout")
        .expect("child exited before printing listening address");
    let proxy_addr: SocketAddr = listening_line
        .strip_prefix("listening on ")
        .expect("unexpected startup message")
        .parse()
        .expect("failed to parse listening address");

    let stream = TcpStream::connect(proxy_addr).await.expect("proxy should be listening");
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

    child.kill().await.expect("failed to kill server process");
}
