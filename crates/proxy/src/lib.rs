use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};

use cluster::{Cluster, Outcome};
use outbound::RequestBody;

pub type ResponseBody = inbound::ResponseBody;

pub async fn handle(cluster: Arc<Cluster>, req: Request<Incoming>) -> Response<ResponseBody> {
    let endpoint = cluster.pick_endpoint();

    let (parts, body) = req.into_parts();
    let outbound_req: Request<RequestBody> = Request::from_parts(parts, body.boxed());

    let mut connection = match outbound::connect(endpoint).await {
        Ok(connection) => connection,
        Err(_err) => {
            cluster.report_outcome(endpoint, Outcome::Failure);
            return bad_gateway();
        }
    };

    match connection.send_request(outbound_req).await {
        Ok(response) => {
            cluster.report_outcome(endpoint, Outcome::Success);
            let (parts, body) = response.into_parts();
            Response::from_parts(parts, body.boxed())
        }
        Err(_err) => {
            cluster.report_outcome(endpoint, Outcome::Failure);
            bad_gateway()
        }
    }
}

fn bad_gateway() -> Response<ResponseBody> {
    let body: ResponseBody = Empty::<Bytes>::new()
        .map_err(|never: std::convert::Infallible| match never {})
        .boxed();
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(body)
        .unwrap()
}
