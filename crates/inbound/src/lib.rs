pub mod timeout;
pub mod tls;

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use openssl::ssl::{Ssl, SslAcceptor};
use tokio::net::{TcpListener, TcpStream};
use tokio_openssl::SslStream;

use timeout::{ConnectionTimeout, Handle};
use tls::{build_acceptor, TlsConfig, TlsSetupError};

pub type ResponseBody = BoxBody<Bytes, hyper::Error>;

/// The two read timeouts that apply to every accepted connection.
#[derive(Debug, Clone, Copy)]
pub struct TimeoutConfig {
    pub header_read: Duration,
    pub idle: Duration,
}

/// Where to listen, and how (plaintext or TLS-terminated).
pub enum ListenAddr {
    Http(SocketAddr),
    Https { addr: SocketAddr, tls: TlsConfig },
}

#[derive(Debug, thiserror::Error)]
pub enum InboundError {
    #[error("failed to bind {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Tls(#[from] TlsSetupError),
}

pub struct InboundServer {
    listener: TcpListener,
    acceptor: Option<SslAcceptor>,
    timeouts: TimeoutConfig,
}

impl InboundServer {
    pub async fn bind(addr: ListenAddr, timeouts: TimeoutConfig) -> Result<Self, InboundError> {
        let (socket_addr, acceptor) = match addr {
            ListenAddr::Http(socket_addr) => (socket_addr, None),
            ListenAddr::Https { addr, tls } => (addr, Some(build_acceptor(&tls)?)),
        };
        let listener = TcpListener::bind(socket_addr)
            .await
            .map_err(|source| InboundError::Bind {
                addr: socket_addr,
                source,
            })?;
        Ok(Self {
            listener,
            acceptor,
            timeouts,
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub async fn serve<F, Fut>(self, handler: F) -> std::io::Result<()>
    where
        F: Fn(Request<Incoming>) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Response<ResponseBody>> + Send + 'static,
    {
        loop {
            match self.listener.accept().await {
                Ok((stream, _peer_addr)) => {
                    let handler = handler.clone();
                    let timeouts = self.timeouts;
                    match self.acceptor.clone() {
                        None => {
                            tokio::spawn(serve_plain(stream, timeouts, handler));
                        }
                        Some(acceptor) => {
                            tokio::spawn(serve_tls(stream, acceptor, timeouts, handler));
                        }
                    }
                }
                Err(err) => {
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::Interrupted
                    ) {
                        continue;
                    }
                    eprintln!("inbound accept error: {err:?}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            }
        }
    }
}

async fn serve_plain<F, Fut>(stream: TcpStream, timeouts: TimeoutConfig, handler: F)
where
    F: Fn(Request<Incoming>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Response<ResponseBody>> + Send + 'static,
{
    let (timed, handle) = ConnectionTimeout::new(stream, timeouts.idle, timeouts.header_read);
    run_http1(TokioIo::new(timed), handle, handler).await;
}

async fn serve_tls<F, Fut>(
    stream: TcpStream,
    acceptor: SslAcceptor,
    timeouts: TimeoutConfig,
    handler: F,
) where
    F: Fn(Request<Incoming>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Response<ResponseBody>> + Send + 'static,
{
    let ssl = match Ssl::new(acceptor.context()) {
        Ok(ssl) => ssl,
        Err(err) => {
            eprintln!("inbound TLS setup error: {err:?}");
            return;
        }
    };
    let mut tls_stream = match SslStream::new(ssl, stream) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("inbound TLS setup error: {err:?}");
            return;
        }
    };
    if let Err(err) = Pin::new(&mut tls_stream).accept().await {
        eprintln!("inbound TLS handshake error: {err:?}");
        return;
    }

    let (timed, handle) = ConnectionTimeout::new(tls_stream, timeouts.idle, timeouts.header_read);
    run_http1(TokioIo::new(timed), handle, handler).await;
}

async fn run_http1<IO, F, Fut>(io: TokioIo<IO>, handle: Handle, handler: F)
where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + 'static,
    F: Fn(Request<Incoming>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Response<ResponseBody>> + Send + 'static,
{
    let service = service_fn(move |req| {
        let handler = handler.clone();
        let handle = handle.clone();
        handle.begin_processing();
        async move {
            let response = handler(req).await;
            handle.end_request();
            Ok::<_, std::convert::Infallible>(response)
        }
    });
    if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
        eprintln!("inbound connection error: {err:?}");
    }
}
