mod config;

use std::env;
use std::sync::Arc;

use cluster::Cluster;
use inbound::InboundServer;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = env::args().nth(1).ok_or("usage: server <config.toml>")?;
    let config = config::load(std::path::Path::new(&config_path))?;
    let cluster = Arc::new(Cluster::new(config.backends));

    let mut tasks = Vec::new();
    for listen_addr in config.listeners {
        let server = InboundServer::bind(listen_addr, config.timeouts).await?;
        println!("listening on {}", server.local_addr()?);

        let cluster = cluster.clone();
        tasks.push(tokio::spawn(async move {
            if let Err(err) = server
                .serve(move |req| {
                    let cluster = cluster.clone();
                    async move { proxy::handle(cluster, req).await }
                })
                .await
            {
                eprintln!("inbound server error: {err:?}");
            }
        }));
    }

    for task in tasks {
        let _ = task.await;
    }

    Ok(())
}
