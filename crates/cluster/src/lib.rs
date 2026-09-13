use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub addr: SocketAddr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure,
}

pub struct Cluster {
    endpoints: Vec<Endpoint>,
    cursor: AtomicUsize,
}

impl Cluster {
    pub fn new(endpoints: Vec<Endpoint>) -> Self {
        assert!(!endpoints.is_empty(), "cluster must have at least one endpoint");
        Self {
            endpoints,
            cursor: AtomicUsize::new(0),
        }
    }

    pub fn pick_endpoint(&self) -> Endpoint {
        let idx = self.cursor.fetch_add(1, Ordering::Relaxed) % self.endpoints.len();
        self.endpoints[idx]
    }

    pub fn report_outcome(&self, _endpoint: Endpoint, _outcome: Outcome) {
        // Placeholder: no health-based downgrading yet.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(port: u16) -> SocketAddr {
        format!("127.0.0.1:{port}").parse().unwrap()
    }

    #[test]
    fn pick_endpoint_cycles_round_robin() {
        let endpoints = vec![
            Endpoint { addr: addr(8001) },
            Endpoint { addr: addr(8002) },
            Endpoint { addr: addr(8003) },
        ];
        let cluster = Cluster::new(endpoints.clone());

        let picked: Vec<_> = (0..6).map(|_| cluster.pick_endpoint()).collect();

        assert_eq!(
            picked,
            vec![
                endpoints[0], endpoints[1], endpoints[2],
                endpoints[0], endpoints[1], endpoints[2],
            ]
        );
    }

    #[test]
    #[should_panic(expected = "at least one endpoint")]
    fn new_panics_on_empty_endpoints() {
        Cluster::new(vec![]);
    }
}
