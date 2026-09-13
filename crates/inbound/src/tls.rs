use std::path::PathBuf;

use openssl::ssl::{AlpnError, SslAcceptor, SslFiletype, SslMethod, SslOptions, SslVersion};

/// Where to find the certificate and private key for a TLS listener.
#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum TlsSetupError {
    #[error("failed to configure TLS context: {0}")]
    Context(#[source] openssl::error::ErrorStack),
    #[error("failed to load certificate from {path}: {source}")]
    Certificate {
        path: String,
        #[source]
        source: openssl::error::ErrorStack,
    },
    #[error("failed to load private key from {path}: {source}")]
    PrivateKey {
        path: String,
        #[source]
        source: openssl::error::ErrorStack,
    },
}

const ALPN_WIRE_FORMAT: &[u8] = b"\x08http/1.1";

/// Builds an acceptor that only speaks HTTP/1.1 over ALPN and keeps
/// TLS 1.0/1.1 reachable for legacy clients.
pub fn build_acceptor(config: &TlsConfig) -> Result<SslAcceptor, TlsSetupError> {
    let mut builder =
        SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).map_err(TlsSetupError::Context)?;

    builder
        .set_min_proto_version(Some(SslVersion::TLS1))
        .map_err(TlsSetupError::Context)?;

    // The intermediate preset also disables TLS 1.0/1.1 via the legacy
    // SSL_OP_NO_TLSv1* options, independently of the min version above;
    // clear them so those versions are actually reachable.
    builder.clear_options(SslOptions::NO_TLSV1 | SslOptions::NO_TLSV1_1);

    // The preset's cipher list has no suite a TLS 1.0 client can use;
    // widen it so a legacy-only handshake can still complete.
    builder
        .set_cipher_list("DEFAULT:@SECLEVEL=0")
        .map_err(TlsSetupError::Context)?;

    builder
        .set_certificate_file(&config.cert_path, SslFiletype::PEM)
        .map_err(|source| TlsSetupError::Certificate {
            path: config.cert_path.display().to_string(),
            source,
        })?;

    builder
        .set_private_key_file(&config.key_path, SslFiletype::PEM)
        .map_err(|source| TlsSetupError::PrivateKey {
            path: config.key_path.display().to_string(),
            source,
        })?;

    builder
        .set_alpn_protos(ALPN_WIRE_FORMAT)
        .map_err(TlsSetupError::Context)?;
    builder.set_alpn_select_callback(|_ssl, client_protos| {
        openssl::ssl::select_next_proto(ALPN_WIRE_FORMAT, client_protos).ok_or(AlpnError::NOACK)
    });

    Ok(builder.build())
}
