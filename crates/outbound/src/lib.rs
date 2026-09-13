use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use hyper::body::Incoming;
use hyper::client::conn::http1;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

pub use cluster::Endpoint;

pub type RequestBody = BoxBody<Bytes, hyper::Error>;

#[derive(Debug)]
pub enum OutboundError {
    Connect(std::io::Error),
    Handshake(hyper::Error),
    Send(hyper::Error),
}

pub struct Connection {
    sender: http1::SendRequest<RequestBody>,
}

impl Connection {
    pub async fn send_request(
        &mut self,
        req: Request<RequestBody>,
    ) -> Result<Response<Incoming>, OutboundError> {
        self.sender.send_request(req).await.map_err(OutboundError::Send)
    }
}

pub async fn connect(endpoint: Endpoint) -> Result<Connection, OutboundError> {
    let stream = TcpStream::connect(endpoint.addr)
        .await
        .map_err(OutboundError::Connect)?;
    let io = TokioIo::new(stream);
    let (sender, conn) = http1::handshake(io).await.map_err(OutboundError::Handshake)?;
    tokio::spawn(async move {
        if let Err(err) = conn.await {
            eprintln!("outbound connection driver error: {err:?}");
        }
    });
    Ok(Connection { sender })
}
