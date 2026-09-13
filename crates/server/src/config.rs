use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cluster::Endpoint;
use inbound::tls::TlsConfig;
use inbound::{ListenAddr, TimeoutConfig};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RawConfig {
    listen: Vec<RawListen>,
    backends: RawBackends,
    #[serde(default)]
    timeouts: RawTimeouts,
}

#[derive(Debug, Deserialize)]
struct RawListen {
    addr: String,
    cert_path: Option<PathBuf>,
    key_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct RawBackends {
    addrs: Vec<SocketAddr>,
}

#[derive(Debug, Deserialize)]
struct RawTimeouts {
    #[serde(default = "default_header_read_secs")]
    header_read_secs: u64,
    #[serde(default = "default_idle_secs")]
    idle_secs: u64,
}

fn default_header_read_secs() -> u64 {
    10
}

fn default_idle_secs() -> u64 {
    60
}

impl Default for RawTimeouts {
    fn default() -> Self {
        Self {
            header_read_secs: default_header_read_secs(),
            idle_secs: default_idle_secs(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("listen address {0:?} is missing a scheme (expected http:// or https://)")]
    MissingScheme(String),
    #[error("unsupported listen scheme {0:?} (only http and https are supported)")]
    UnsupportedScheme(String),
    #[error("listen address {0:?} could not be parsed: {1}")]
    InvalidAddr(String, std::net::AddrParseError),
    #[error("https listener {0:?} is missing cert_path")]
    MissingCertPath(String),
    #[error("https listener {0:?} is missing key_path")]
    MissingKeyPath(String),
    #[error("at least one backend address is required")]
    EmptyBackends,
}

pub struct Config {
    pub listeners: Vec<ListenAddr>,
    pub backends: Vec<Endpoint>,
    pub timeouts: TimeoutConfig,
}

pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let raw: RawConfig = toml::from_str(&text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    if raw.backends.addrs.is_empty() {
        return Err(ConfigError::EmptyBackends);
    }

    let listeners = raw
        .listen
        .into_iter()
        .map(parse_listen)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Config {
        listeners,
        backends: raw
            .backends
            .addrs
            .into_iter()
            .map(|addr| Endpoint { addr })
            .collect(),
        timeouts: TimeoutConfig {
            header_read: Duration::from_secs(raw.timeouts.header_read_secs),
            idle: Duration::from_secs(raw.timeouts.idle_secs),
        },
    })
}

fn parse_listen(raw: RawListen) -> Result<ListenAddr, ConfigError> {
    let (scheme, rest) = raw
        .addr
        .split_once("://")
        .ok_or_else(|| ConfigError::MissingScheme(raw.addr.clone()))?;

    let socket_addr: SocketAddr = rest
        .parse()
        .map_err(|err| ConfigError::InvalidAddr(raw.addr.clone(), err))?;

    match scheme {
        "http" => Ok(ListenAddr::Http(socket_addr)),
        "https" => {
            let cert_path = raw
                .cert_path
                .ok_or_else(|| ConfigError::MissingCertPath(raw.addr.clone()))?;
            let key_path = raw
                .key_path
                .ok_or_else(|| ConfigError::MissingKeyPath(raw.addr.clone()))?;
            Ok(ListenAddr::Https {
                addr: socket_addr,
                tls: TlsConfig { cert_path, key_path },
            })
        }
        other => Err(ConfigError::UnsupportedScheme(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_config(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        (dir, path)
    }

    #[test]
    fn parses_a_minimal_http_only_config() {
        let (_dir, path) = write_temp_config(
            r#"
            [[listen]]
            addr = "http://127.0.0.1:9080"

            [backends]
            addrs = ["127.0.0.1:8080"]
            "#,
        );

        let config = load(&path).expect("config should parse");
        assert_eq!(config.listeners.len(), 1);
        assert!(matches!(config.listeners[0], ListenAddr::Http(_)));
        assert_eq!(config.backends.len(), 1);
        assert_eq!(config.timeouts.header_read, std::time::Duration::from_secs(10));
        assert_eq!(config.timeouts.idle, std::time::Duration::from_secs(60));
    }

    #[test]
    fn parses_an_https_listener_with_explicit_timeouts() {
        let (_dir, path) = write_temp_config(
            r#"
            [[listen]]
            addr = "https://0.0.0.0:9443"
            cert_path = "/tmp/cert.pem"
            key_path = "/tmp/key.pem"

            [backends]
            addrs = ["127.0.0.1:8080", "127.0.0.1:8081"]

            [timeouts]
            header_read_secs = 5
            idle_secs = 30
            "#,
        );

        let config = load(&path).expect("config should parse");
        assert!(matches!(config.listeners[0], ListenAddr::Https { .. }));
        assert_eq!(config.backends.len(), 2);
        assert_eq!(config.timeouts.header_read, std::time::Duration::from_secs(5));
        assert_eq!(config.timeouts.idle, std::time::Duration::from_secs(30));
    }

    #[test]
    fn rejects_an_unsupported_scheme() {
        let (_dir, path) = write_temp_config(
            r#"
            [[listen]]
            addr = "quic://0.0.0.0:9443"

            [backends]
            addrs = ["127.0.0.1:8080"]
            "#,
        );

        let err = match load(&path) {
            Err(err) => err,
            Ok(_) => panic!("unsupported scheme must fail"),
        };
        assert!(matches!(err, ConfigError::UnsupportedScheme(scheme) if scheme == "quic"));
    }

    #[test]
    fn rejects_an_https_listener_missing_cert_path() {
        let (_dir, path) = write_temp_config(
            r#"
            [[listen]]
            addr = "https://0.0.0.0:9443"
            key_path = "/tmp/key.pem"

            [backends]
            addrs = ["127.0.0.1:8080"]
            "#,
        );

        let err = match load(&path) {
            Err(err) => err,
            Ok(_) => panic!("missing cert_path must fail"),
        };
        assert!(matches!(err, ConfigError::MissingCertPath(_)));
    }

    #[test]
    fn rejects_an_empty_backend_list() {
        let (_dir, path) = write_temp_config(
            r#"
            [[listen]]
            addr = "http://127.0.0.1:9080"

            [backends]
            addrs = []
            "#,
        );

        let err = match load(&path) {
            Err(err) => err,
            Ok(_) => panic!("empty backend list must fail"),
        };
        assert!(matches!(err, ConfigError::EmptyBackends));
    }

    #[test]
    fn rejects_a_missing_file() {
        let err = match load(std::path::Path::new("/nonexistent/config.toml")) {
            Err(err) => err,
            Ok(_) => panic!("missing file must fail"),
        };
        assert!(matches!(err, ConfigError::Read { .. }));
    }
}
