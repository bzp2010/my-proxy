mod common;

use std::pin::Pin;

use inbound::tls::{build_acceptor, TlsConfig};
use openssl::ssl::{Ssl, SslConnector, SslMethod, SslVersion};
use tokio::net::{TcpListener, TcpStream};
use tokio_openssl::SslStream;

async fn handshake_pair(
    acceptor: &openssl::ssl::SslAcceptor,
    client_max_version: Option<SslVersion>,
) -> (SslStream<TcpStream>, SslStream<TcpStream>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let mut connector_builder = SslConnector::builder(SslMethod::tls()).unwrap();
    connector_builder.set_verify(openssl::ssl::SslVerifyMode::NONE);
    // SslConnector::builder() does not advertise ALPN by default; offer
    // h2 ahead of http/1.1 so the ALPN test proves the acceptor picks
    // http/1.1 instead of merely completing a handshake.
    connector_builder
        .set_alpn_protos(b"\x02h2\x08http/1.1")
        .unwrap();
    if let Some(version) = client_max_version {
        connector_builder.set_max_proto_version(Some(version)).unwrap();
        // OpenSSL's default security level (1) refuses to negotiate below
        // TLS 1.1 regardless of the max version above; a real legacy
        // client has no such modern restriction, so relax it here to
        // actually exercise the acceptor's lowered minimum version.
        connector_builder.set_security_level(0);
    }
    let connector = connector_builder.build();

    let server_accept = async {
        let (stream, _) = listener.accept().await.unwrap();
        let ssl = Ssl::new(acceptor.context()).unwrap();
        let mut server_stream = SslStream::new(ssl, stream).unwrap();
        Pin::new(&mut server_stream).accept().await.unwrap();
        server_stream
    };

    let client_connect = async {
        let stream = TcpStream::connect(addr).await.unwrap();
        let ssl = connector.configure().unwrap().into_ssl("localhost").unwrap();
        let mut client_stream = SslStream::new(ssl, stream).unwrap();
        Pin::new(&mut client_stream).connect().await.unwrap();
        client_stream
    };

    tokio::join!(server_accept, client_connect)
}

#[tokio::test]
async fn modern_client_negotiates_http1_1_via_alpn() {
    let (_dir, cert_path, key_path) = common::self_signed_cert();
    let acceptor = build_acceptor(&TlsConfig { cert_path, key_path }).unwrap();

    let (server_stream, _client_stream) = handshake_pair(&acceptor, None).await;

    let negotiated = server_stream.ssl().selected_alpn_protocol();
    assert_eq!(negotiated, Some(b"http/1.1".as_slice()));
}

#[tokio::test]
async fn legacy_tls1_0_client_can_still_complete_a_handshake() {
    let (_dir, cert_path, key_path) = common::self_signed_cert();
    let acceptor = build_acceptor(&TlsConfig { cert_path, key_path }).unwrap();

    // Forcing the client to offer only TLS 1.0 proves the acceptor's
    // lowered minimum version is actually reachable, not just configured.
    let (_server_stream, _client_stream) =
        handshake_pair(&acceptor, Some(SslVersion::TLS1)).await;
}

#[test]
fn missing_certificate_file_is_a_clear_error() {
    let result = build_acceptor(&TlsConfig {
        cert_path: "/nonexistent/cert.pem".into(),
        key_path: "/nonexistent/key.pem".into(),
    });

    assert!(result.is_err());
}
