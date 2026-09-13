use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cluster::{Cluster, Endpoint};
use inbound::{InboundServer, ListenAddr, TimeoutConfig};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut args = env::args().skip(1);
    let listen_addr: SocketAddr = args
        .next()
        .expect("usage: server <listen_addr> <backend_addr>...")
        .parse()
        .expect("invalid listen address");
    let backend_addrs: Vec<SocketAddr> = args
        .map(|a| a.parse().expect("invalid backend address"))
        .collect();
    assert!(
        !backend_addrs.is_empty(),
        "at least one backend address is required"
    );

    let endpoints = backend_addrs.into_iter().map(|addr| Endpoint { addr }).collect();
    let cluster = Arc::new(Cluster::new(endpoints));

    let timeouts = TimeoutConfig {
        header_read: Duration::from_secs(10),
        idle: Duration::from_secs(60),
    };
    let server = InboundServer::bind(ListenAddr::Http(listen_addr), timeouts)
        .await
        .expect("failed to bind listener");
    println!("listening on {}", server.local_addr()?);

    server
        .serve(move |req| {
            let cluster = cluster.clone();
            async move { proxy::handle(cluster, req).await }
        })
        .await
}
