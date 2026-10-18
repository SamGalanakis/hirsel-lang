//! Pure TLS argument checks. Certificate verification, time and handshakes belong to providers.
use rustls_pki_types::{CertificateDer, ServerName, TrustAnchor, pem::PemObject};

/// Parse a TLS verification/SNI name without consulting a resolver or clock.
pub fn server_name(name: &str) -> Option<ServerName<'static>> {
    ServerName::try_from(name.to_owned()).ok()
}

/// Parse every PEM certificate as a trust anchor, using the same parser as rustls's
/// RootCertStore::add. An empty bundle, malformed PEM or malformed certificate is invalid.
pub fn pem_roots(pem: &[u8]) -> Option<Vec<TrustAnchor<'static>>> {
    let roots = CertificateDer::pem_slice_iter(pem)
        .map(|cert| {
            Some(
                webpki::anchor_from_trusted_cert(&cert.ok()?)
                    .ok()?
                    .to_owned(),
            )
        })
        .collect::<Option<Vec<_>>>()?;
    (!roots.is_empty()).then_some(roots)
}
