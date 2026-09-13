use std::path::PathBuf;

use openssl::asn1::Asn1Time;
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::rsa::Rsa;
use openssl::x509::{X509, X509NameBuilder};

/// Generates a self-signed certificate/key pair for tests, written to
/// temporary files. Keep the returned `TempDir` alive for as long as the
/// paths are needed; dropping it deletes the files.
pub fn self_signed_cert() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let rsa = Rsa::generate(2048).expect("generate RSA key");
    let key = PKey::from_rsa(rsa).expect("wrap RSA key");

    let mut name_builder = X509NameBuilder::new().expect("create name builder");
    name_builder
        .append_entry_by_text("CN", "localhost")
        .expect("set CN");
    let name = name_builder.build();

    let mut builder = X509::builder().expect("create x509 builder");
    builder.set_version(2).expect("set version");
    builder.set_subject_name(&name).expect("set subject");
    builder.set_issuer_name(&name).expect("set issuer");
    builder.set_pubkey(&key).expect("set pubkey");
    builder
        .set_not_before(&Asn1Time::days_from_now(0).expect("compute not_before"))
        .expect("set not_before");
    builder
        .set_not_after(&Asn1Time::days_from_now(1).expect("compute not_after"))
        .expect("set not_after");
    builder
        .sign(&key, MessageDigest::sha256())
        .expect("sign certificate");
    let cert = builder.build();

    let dir = tempfile::tempdir().expect("create temp dir");
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.to_pem().expect("encode cert")).expect("write cert");
    std::fs::write(
        &key_path,
        key.private_key_to_pem_pkcs8().expect("encode key"),
    )
    .expect("write key");

    (dir, cert_path, key_path)
}
