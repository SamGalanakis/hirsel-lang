//! Pure TLS argument checks. Certificate verification, time and handshakes belong to providers.
/// A loaded server identity, as the host provider presents it.
pub use rustls::sign::CertifiedKey as Identity;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName, TrustAnchor, pem::PemObject};

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

/// Load a server identity as rustls's `ServerConfig::with_single_cert` does: every PEM
/// certificate of `chain` (leaf first), the first PEM private key of `key`, signing with the
/// ring provider. An empty or malformed chain or key, an unsupported key type, and a key whose
/// public half differs from the leaf's SubjectPublicKeyInfo (or cannot be derived) are invalid.
pub fn identity(chain: &[u8], key: &[u8]) -> Option<Identity> {
    let chain = CertificateDer::pem_slice_iter(chain)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if chain.is_empty() {
        return None;
    }
    let key = PrivateKeyDer::from_pem_slice(key).ok()?;
    let signing = rustls::crypto::ring::default_provider()
        .key_provider
        .load_private_key(key)
        .ok()?;
    let identity = Identity::new(chain, signing);
    identity.keys_match().ok()?;
    Some(identity)
}

/// Whether the identity's leaf certificate names `name` (a DNS or IP subject alternative
/// name), as a verifying client checks it; no time, chain or key usage is consulted.
pub fn serves(identity: &Identity, name: &str) -> bool {
    let (Some(name), Ok(leaf)) = (server_name(name), identity.end_entity_cert()) else {
        return false;
    };
    webpki::EndEntityCert::try_from(leaf)
        .is_ok_and(|cert| cert.verify_is_valid_for_subject_name(&name).is_ok())
}
