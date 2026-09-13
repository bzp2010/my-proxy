use std::future::Future;
use std::net::SocketAddr;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

pub type ResponseBody = BoxBody<Bytes, hyper::Error>;

pub struct InboundServer {
    listener: TcpListener,
}

impl InboundServer {
    pub async fn bind(addr: SocketAddr) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self { listener })
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
            let (stream, _peer_addr) = self.listener.accept().await?;
            let io = TokioIo::new(stream);
            let handler = handler.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req| {
                    let handler = handler.clone();
                    async move { Ok::<_, std::convert::Infallible>(handler(req).await) }
                });
                if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                    eprintln!("inbound connection error: {err:?}");
                }
            });
        }
    }
}
