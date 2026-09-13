mod common;

use std::convert::Infallible;
use std::io::Write;
use std::net::SocketAddr;
use std::process::Stdio;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1 as server_http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio_openssl::SslStream;

async fn spawn_fake_upstream() -> SocketAddr {
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

fn write_config(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(contents.as_bytes())
        .unwrap();
    (dir, path)
}

#[tokio::test]
async fn https_listener_works_through_the_compiled_binary() {
    let upstream_addr = spawn_fake_upstream().await;
    let (_cert_dir, cert_path, key_path) = common::self_signed_cert();
    let (_config_dir, config_path) = write_config(&format!(
        r#"
        [[listen]]
        addr = "https://127.0.0.1:0"
        cert_path = "{}"
        key_path = "{}"

        [backends]
        addrs = ["{upstream_addr}"]
        "#,
        cert_path.display(),
        key_path.display()
    ));

    let mut child = Command::new(env!("CARGO_BIN_EXE_server"))
        .arg(&config_path)
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

    let mut connector_builder = SslConnector::builder(SslMethod::tls()).unwrap();
    connector_builder.set_verify(SslVerifyMode::NONE);
    let connector = connector_builder.build();

    let tcp = TcpStream::connect(proxy_addr).await.unwrap();
    let ssl = connector.configure().unwrap().into_ssl("localhost").unwrap();
    let mut tls_stream = SslStream::new(ssl, tcp).unwrap();
    std::pin::Pin::new(&mut tls_stream).connect().await.unwrap();

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

    child.kill().await.expect("failed to kill server process");
}
