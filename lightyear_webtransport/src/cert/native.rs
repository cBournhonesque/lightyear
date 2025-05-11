use {
    super::{BASE64, CertificateHash},
    base64::Engine,
    spki::der::Decode,
    wtransport::tls::Sha256Digest,
};

/// Encodes a SHA-256 digest of a certificate hash into a base 64 string which
/// can be decoded by `hash_from_b64`.
#[must_use]
pub fn hash_to_b64(hash: impl AsRef<CertificateHash>) -> String {
    BASE64.encode(hash.as_ref())
}

/// Calculates the fingerprint bytes of a certificate's public key.
///
/// This gets the raw bytes of the public key fingerprint - you may find that
/// [`spki_fingerprint_b64`] is typically more useful.
///
/// Returns [`None`] if the certificate cannot be converted to an
/// [`x509_cert::Certificate`].
#[must_use]
pub fn spki_fingerprint(cert: &wtransport::tls::Certificate) -> Option<spki::FingerprintBytes> {
    let cert = x509_cert::Certificate::from_der(cert.der()).ok()?;
    let fingerprint = cert
        .tbs_certificate
        .subject_public_key_info
        .fingerprint_bytes()
        .ok()?;
    Some(fingerprint)
}

/// Calculates the base 64 encoded form of the fingerprint bytes of a
/// certificate's public key.
///
/// To launch a Chromium-based browser which can connect to a server with this
/// self-signed certificate, use the flags:
/// ```text
/// --webtransport-developer-mode \
/// --ignore-certificate-errors-spki-list=[output of this function]
/// ```
#[must_use]
pub fn spki_fingerprint_b64(cert: &wtransport::tls::Certificate) -> Option<String> {
    spki_fingerprint(cert).map(|fingerprint| BASE64.encode(fingerprint))
}

/// Decodes a [`Sha256Digest`] from an SPKI fingerprint produced by
/// [`spki_fingerprint_b64`], for use in
/// [`ClientConfigBuilder::with_server_certificate_hashes`].
///
/// Returns [`None`] if the fingerprint is not valid base 64, or if it is not
/// 32 bytes in length.
///
/// [`ClientConfigBuilder::with_server_certificate_hashes`]: wtransport::config::ClientConfigBuilder::with_server_certificate_hashes
#[must_use]
pub fn digest_from_spki_fingerprint(fingerprint: &str) -> Option<Sha256Digest> {
    let bytes = BASE64.decode(fingerprint).ok()?;
    let bytes = <[u8; 32]>::try_from(bytes).ok()?;
    Some(Sha256Digest::new(bytes))
}
