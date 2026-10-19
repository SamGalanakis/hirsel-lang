//! Pure TLS argument checks, identity selection and client-certificate verification at a
//! caller-supplied time. Handshakes, sockets and the host clock belong to providers.
use crate::TlsFailure;
use rustls::server::danger::ClientCertVerifier;
/// A loaded server identity, as the host provider presents it.
pub use rustls::sign::CertifiedKey as Identity;
use rustls_pki_types::{
    CertificateDer, PrivateKeyDer, ServerName, TrustAnchor, UnixTime, pem::PemObject,
};
use std::sync::Arc;

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
    let chain = self::chain(chain)?;
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

/// A `TlsClientAuth`: whether a server handshake asks for a client certificate, and the
/// trust set it verifies one against.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientAuth<T> {
    #[default]
    Anonymous,
    Request(T),
    Require(T),
}
impl<T> ClientAuth<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ClientAuth<U> {
        match self {
            Self::Anonymous => ClientAuth::Anonymous,
            Self::Request(t) => ClientAuth::Request(f(t)),
            Self::Require(t) => ClientAuth::Require(f(t)),
        }
    }
    pub fn as_ref(&self) -> ClientAuth<&T> {
        match self {
            Self::Anonymous => ClientAuth::Anonymous,
            Self::Request(t) => ClientAuth::Request(t),
            Self::Require(t) => ClientAuth::Require(t),
        }
    }
    pub fn is_anonymous(&self) -> bool {
        matches!(self, Self::Anonymous)
    }
}

/// A loaded `TlsServer`: its identities, and the client verifier of a `Request` or
/// `Require` trust set.
#[derive(Clone)]
pub struct Server {
    pub identities: Vec<Arc<Identity>>,
    pub clients: Option<Arc<dyn ClientCertVerifier>>,
}
impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("identities", &self.identities.len())
            .field("clients", &self.clients.is_some())
            .finish()
    }
}

/// Load a `TlsServer` from its identities' PEM `(chain, key)` pairs and its client policy.
/// An empty identity list, any invalid identity and an empty or malformed trust set are
/// invalid.
pub fn server(identities: &[(&[u8], &[u8])], clients: ClientAuth<&[u8]>) -> Option<Server> {
    let identities = identities
        .iter()
        .map(|(chain, key)| identity(chain, key).map(Arc::new))
        .collect::<Option<Vec<_>>>()?;
    if identities.is_empty() {
        return None;
    }
    let verifier = |pem: &[u8], required| client_verifier(pem_roots(pem)?, required);
    let clients = match clients {
        ClientAuth::Anonymous => None,
        ClientAuth::Request(pem) => Some(verifier(pem, false)?),
        ClientAuth::Require(pem) => Some(verifier(pem, true)?),
    };
    Some(Server {
        identities,
        clients,
    })
}

/// Load a `TlsServer`'s data as `server` does.
pub fn load(server: &crate::TlsServer) -> Option<Server> {
    let identities = server
        .identities
        .iter()
        .map(|identity| (identity.chain.as_slice(), identity.key.as_slice()))
        .collect::<Vec<_>>();
    self::server(&identities, server.clients.as_ref().map(Vec::as_slice))
}

/// Every PEM certificate of `pem`, in order; empty or malformed PEM is invalid.
pub fn chain(pem: &[u8]) -> Option<Vec<CertificateDer<'static>>> {
    let chain = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (!chain.is_empty()).then_some(chain)
}

/// What `tls.peer` returns for a verified chain: its DER certificates concatenated, leaf
/// first.
pub fn chain_der(chain: &[CertificateDer<'_>]) -> Vec<u8> {
    chain.iter().flat_map(|cert| cert.iter().copied()).collect()
}

/// The SNI a client sends for a verification name: the name itself when it is a DNS name;
/// an IP literal sends none (RFC 6066 section 3).
pub fn sni(name: &str) -> Option<&str> {
    matches!(server_name(name)?, ServerName::DnsName(_)).then_some(name)
}

/// The identity a server presents: the first whose leaf names the client's SNI, otherwise
/// the first. `identities` is nonempty.
pub fn select<'a>(identities: &'a [Arc<Identity>], sni: Option<&str>) -> &'a Arc<Identity> {
    sni.and_then(|name| identities.iter().find(|identity| serves(identity, name)))
        .unwrap_or(&identities[0])
}

/// The client-certificate verifier of a `Request` (`required` false) or `Require` trust set,
/// as rustls's `WebPkiClientVerifier` with the ring provider builds it. An empty trust set
/// is invalid.
pub fn client_verifier(
    trust: Vec<TrustAnchor<'static>>,
    required: bool,
) -> Option<Arc<dyn ClientCertVerifier>> {
    let roots = Arc::new(rustls::RootCertStore { roots: trust });
    let builder = rustls::server::WebPkiClientVerifier::builder_with_provider(
        roots,
        Arc::new(rustls::crypto::ring::default_provider()),
    );
    if required {
        builder.build()
    } else {
        builder.allow_unauthenticated().build()
    }
    .ok()
}

/// Verify a presented client chain as a server handshake does, at `now`.
pub fn verify_client(
    verifier: &dyn ClientCertVerifier,
    chain: &[CertificateDer<'_>],
    now: UnixTime,
) -> Result<(), rustls::Error> {
    let (leaf, intermediates) = chain
        .split_first()
        .ok_or(rustls::Error::NoCertificatesPresented)?;
    verifier
        .verify_client_cert(leaf, intermediates, now)
        .map(|_| ())
}

/// The alert a server sends refusing a client certificate, as rustls 0.23.46's
/// `CommonState::send_cert_verify_error_alert` chooses it, and `certificate_required` for
/// none under a mandatory policy (src/server/tls13.rs, tls12.rs).
fn refusal_alert(error: &rustls::Error) -> u8 {
    use rustls::{AlertDescription as A, Error as E};
    u8::from(match error {
        E::InvalidCertificate(cert) => A::from(cert.clone()),
        E::PeerMisbehaved(_) => A::IllegalParameter,
        E::NoCertificatesPresented => A::CertificateRequired,
        _ => A::HandshakeFailure,
    })
}

/// The time at which a scripted world verifies a client chain, where time is not checked:
/// the latest `notBefore` of its certificates, an instant at which a time-consistent chain
/// is valid. A certificate whose validity does not parse gives the epoch.
pub fn issued(chain: &[CertificateDer<'_>]) -> UnixTime {
    let latest = chain
        .iter()
        .map(|cert| not_before(cert).unwrap_or(0))
        .max()
        .unwrap_or(0);
    UnixTime::since_unix_epoch(std::time::Duration::from_secs(latest))
}

/// A scripted client's answer to a server handshake (docs/spec/core-library.md, scripted
/// peers): the client verifies the identity its SNI selects by name, then the server
/// verifies the chain it presents, if asked, at `issued`. Success carries what `tls.peer`
/// returns.
pub fn scripted_accept(
    client: &crate::PeerTls,
    server: &Server,
) -> Result<Option<Vec<u8>>, TlsFailure> {
    use crate::PeerTls;
    let (names, presented) = match client {
        // A peer's Serves, which no validated client has, runs no TLS client either.
        PeerTls::Plain | PeerTls::Serves(_) => return Err(TlsFailure::HandshakeFailure),
        PeerTls::Fails(failure) => return Err(*failure),
        PeerTls::Certificate(names) => (names, None),
        PeerTls::Presents(names, chain) => (names, Some(chain)),
    };
    let identity = select(&server.identities, names.first().and_then(|name| sni(name)));
    if !names.iter().any(|name| serves(identity, name)) {
        // A verifying client rejecting the certificate sends bad_certificate.
        return Err(TlsFailure::AlertReceived(42));
    }
    let Some(verifier) = server.clients.as_deref() else {
        return Ok(None);
    };
    match presented {
        None if verifier.client_auth_mandatory() => Err(TlsFailure::CertificateRequired),
        None => Ok(None),
        Some(pem) => {
            let chain = chain(pem).ok_or(TlsFailure::BadCertificate)?;
            verify_client(verifier, &chain, issued(&chain)).map_err(|e| failure(&e))?;
            Ok(Some(chain_der(&chain)))
        }
    }
}

/// A scripted peer's answer to a client handshake for `server_name` presenting `identity`
/// when asked (docs/spec/core-library.md, scripted peers). A `Serves` server presents the
/// identity the SNI selects, which the client verifies by name alone, then verifies the
/// client's chain, if it asks for one, at `issued`, refusing with the alert a rustls server
/// sends. Success carries what `tls.peer` returns: the presented chain, and nothing for a
/// `Certificate` peer, which sends no certificate bytes.
pub fn scripted_connect(
    peer: &crate::PeerTls,
    server_name: &str,
    identity: Option<&Identity>,
) -> Result<Option<Vec<u8>>, TlsFailure> {
    use crate::PeerTls;
    let server = match peer {
        // A client's Presents, which no validated peer has, runs no TLS server either.
        PeerTls::Plain | PeerTls::Presents(..) => return Err(TlsFailure::HandshakeFailure),
        PeerTls::Fails(failure) => return Err(*failure),
        PeerTls::Certificate(names) if names.iter().any(|name| name == server_name) => {
            return Ok(None);
        }
        PeerTls::Certificate(_) => return Err(TlsFailure::NameMismatch),
        PeerTls::Serves(server) => load(server).ok_or(TlsFailure::HandshakeFailure)?,
    };
    let presented = select(&server.identities, sni(server_name));
    if !serves(presented, server_name) {
        return Err(TlsFailure::NameMismatch);
    }
    if let Some(verifier) = server.clients.as_deref() {
        let refused = match identity {
            None if verifier.client_auth_mandatory() => {
                Some(rustls::Error::NoCertificatesPresented)
            }
            None => None,
            Some(identity) => verify_client(verifier, &identity.cert, issued(&identity.cert)).err(),
        };
        if let Some(error) = refused {
            return Err(TlsFailure::AlertReceived(refusal_alert(&error)));
        }
    }
    Ok(Some(chain_der(&presented.cert)))
}

/// Condense rustls Error/CertificateError into TLS failures.
pub fn failure(error: &rustls::Error) -> TlsFailure {
    use rustls::{CertificateError as C, Error as E};
    match error {
        E::InvalidCertificate(cert) => match cert {
            C::UnknownIssuer => TlsFailure::UnknownIssuer,
            C::Expired | C::ExpiredContext { .. } => TlsFailure::Expired,
            C::NotValidYet | C::NotValidYetContext { .. } => TlsFailure::NotYetValid,
            C::NotValidForName | C::NotValidForNameContext { .. } => TlsFailure::NameMismatch,
            C::Revoked => TlsFailure::Revoked,
            _ => TlsFailure::BadCertificate,
        },
        E::NoCertificatesPresented => TlsFailure::CertificateRequired,
        E::AlertReceived(alert) => TlsFailure::AlertReceived(u8::from(*alert)),
        E::PeerMisbehaved(_)
        | E::InappropriateMessage { .. }
        | E::InappropriateHandshakeMessage { .. }
        | E::InvalidMessage(_)
        | E::PeerSentOversizedRecord
        | E::DecryptError => TlsFailure::PeerMisbehaved,
        _ => TlsFailure::HandshakeFailure,
    }
}

/// A certificate's `notBefore` in Unix seconds (RFC 5280 section 4.1: Certificate ->
/// tbsCertificate -> [0] version?, serialNumber, signature, issuer, validity).
fn not_before(cert: &[u8]) -> Option<u64> {
    let (_, certificate, _) = der(cert)?;
    let (_, mut tbs, _) = der(certificate)?;
    let mut field = || {
        let (tag, body, rest) = der(tbs)?;
        tbs = rest;
        Some((tag, body))
    };
    let (mut tag, _) = field()?;
    if tag == 0xa0 {
        (tag, _) = field()?;
    }
    if tag != 0x02 {
        return None;
    }
    field()?;
    field()?;
    let (0x30, validity) = field()? else {
        return None;
    };
    let (tag, time, _) = der(validity)?;
    let digits = std::str::from_utf8(time.strip_suffix(b"Z")?).ok()?;
    let (year, rest) = match tag {
        // UTCTime YYMMDDHHMMSSZ: 50-99 are 19YY (RFC 5280 section 4.1.2.5.1).
        0x17 if digits.len() == 12 => {
            let yy: i64 = digits[..2].parse().ok()?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &digits[2..])
        }
        0x18 if digits.len() == 14 => (digits[..4].parse().ok()?, &digits[4..]),
        _ => return None,
    };
    if !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let part = |i: usize| rest[i..i + 2].parse::<i64>().ok();
    let (month, day) = (part(0)?, part(2)?);
    // Howard Hinnant's days_from_civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + part(4)? * 3600 + part(6)? * 60 + part(8)?).ok()
}

/// One DER TLV: its tag, contents and the bytes after it. Definite lengths only.
fn der(bytes: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = bytes.split_first()?;
    let (&first, mut rest) = rest.split_first()?;
    let len = if first < 0x80 {
        usize::from(first)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || rest.len() < count {
            return None;
        }
        let (len, after) = rest.split_at(count);
        rest = after;
        len.iter().fold(0usize, |n, &b| (n << 8) | usize::from(b))
    };
    (rest.len() >= len).then(|| (tag, &rest[..len], &rest[len..]))
}

#[cfg(test)]
mod tests {
    /// A certificate whose tbsCertificate has a version, serial, signature, issuer and the
    /// validity whose notBefore is `time` (tag and digits); nothing after it is read.
    fn certificate(tag: u8, time: &[u8]) -> Vec<u8> {
        let mut time = [&[tag, time.len() as u8][..], time].concat();
        let mut validity = vec![0x30, time.len() as u8];
        validity.append(&mut time);
        let mut tbs = vec![0xa0, 3, 2, 1, 2, 2, 1, 7, 0x30, 0, 0x30, 0];
        tbs.extend(validity);
        let mut tbs = [&[0x30, tbs.len() as u8][..], &tbs].concat();
        let mut out = vec![0x30, tbs.len() as u8];
        out.append(&mut tbs);
        out
    }

    #[test]
    fn not_before_reads_both_time_forms_with_the_utctime_century_pivot() {
        let cases: [(u8, &[u8], Option<u64>); 5] = [
            (0x17, b"700101000000Z", Some(0)),
            // RFC 5280 4.1.2.5.1: UTCTime 49 is 2049 and 50 is 1950.
            (0x17, b"491231235959Z", Some(2_524_607_999)),
            (0x17, b"500101000000Z", None),
            (0x18, b"20510101000000Z", Some(2_556_144_000)),
            (0x18, b"2051010100000Z", None),
        ];
        for (tag, time, expected) in cases {
            assert_eq!(
                super::not_before(&certificate(tag, time)),
                expected,
                "{}",
                String::from_utf8_lossy(time)
            );
        }
    }
}
