//! Closed network grant data; execution engines enforce the validated rules.
use crate::{ProviderContract, RunRefusal, invalid};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetRule {
    pub address: String,
    pub service: String,
    pub operation: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetGrant {
    pub provider: String,
    pub rules: Vec<NetRule>,
    /// Required for net.script@2, forbidden for every other provider.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "peers"
    )]
    pub peers: Option<Vec<NetPeer>>,
    /// Scripted incoming clients, allowed only for net.script@2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listeners: Vec<NetListener>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetPeer {
    pub address: String,
    pub port: u16,
    /// Canonical IP literals, in resolver order, with duplicates preserved.
    pub resolves_to: Vec<String>,
    pub tls: PeerTls,
    pub connections: Vec<NetConnection>,
}

/// A scripted listen address and the clients that connect to it, in accept order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetListener {
    /// A canonical IP literal.
    pub address: String,
    pub port: u16,
    pub clients: Vec<NetClient>,
}

/// One incoming connection: its source address, its TLS client and what it sends and
/// expects.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetClient {
    /// A canonical IP literal.
    pub address: String,
    pub port: u16,
    /// `Plain` never starts TLS; `Certificate(names)` accepts a server certificate valid
    /// for one of `names`, sending the first as SNI when it is a DNS name;
    /// `Presents(names, chain)` does the same and presents the PEM `chain` as its client
    /// certificate; `Fails(f)` fails every server handshake with `Tls(f)`.
    pub tls: PeerTls,
    pub steps: Vec<NetStep>,
}

/// How Test reports name a listen address: `ip:port`, or a Unix listener's socket path.
pub fn listener_name(address: &str, port: u16) -> String {
    match address.parse::<IpAddr>() {
        Ok(ip) => std::net::SocketAddr::new(ip, port).to_string(),
        Err(_) => address.to_owned(),
    }
}

/// Scripted listeners name canonical IP literals or socket paths, each address/port once.
/// Port zero binds its clients to the first ephemeral listen on that IP. A Unix listener
/// has port 0 and its clients, being unnamed, come from `0.0.0.0` with port 0.
pub fn check_net_listeners(listeners: &[NetListener]) -> Result<(), String> {
    let mut seen = std::collections::BTreeMap::new();
    for (index, listener) in listeners.iter().enumerate() {
        if let Some(first) = seen.insert((&listener.address, listener.port), index) {
            return Err(format!(
                "duplicate listener address at entries {first} and {index}"
            ));
        }
        if canonical_unix_path(&listener.address) {
            if listener.port != 0
                || !listener
                    .clients
                    .iter()
                    .all(|c| c.address == "0.0.0.0" && c.port == 0)
            {
                return Err(format!(
                    "Unix listener {index} needs port 0 and clients from 0.0.0.0 port 0"
                ));
            }
        } else if !canonical_ip(&listener.address)
            || !listener.clients.iter().all(|c| canonical_ip(&c.address))
        {
            return Err(format!("listener {index} has a noncanonical IP literal"));
        }
        if listener.clients.iter().any(
            |c| matches!(&c.tls, PeerTls::Presents(_, chain) if crate::tls::chain(chain).is_none()),
        ) {
            return Err(format!(
                "listener {index} has a client presenting no well-formed PEM certificate"
            ));
        }
    }
    Ok(())
}

/// The Test report of a scripted client whose `Expect` the program's writes contradict
/// (docs/spec/core-library.md, scripted peers), with its trailing newline.
pub fn net_client_mismatch(client: usize, listener: &str, expected: &[u8], wrote: &[u8]) -> String {
    format!(
        "test failure: net client {client} of {listener} expected {} but the program wrote {}\n",
        byte_literal(expected),
        byte_literal(wrote)
    )
}

/// The Test report of an accepted stream closed before its client's `Expect` was met.
pub fn net_client_unmet(client: usize, listener: &str, expected: &[u8], wrote: &[u8]) -> String {
    format!(
        "test failure: net client {client} of {listener} expected {} but the program closed after writing {}\n",
        byte_literal(expected),
        byte_literal(wrote)
    )
}

/// The Test report of a listener closed before a client with an `Expect` connected.
pub fn net_client_unaccepted(client: usize, listener: &str) -> String {
    format!("test failure: net client {client} of {listener} was never accepted\n")
}

/// The first unaccepted client of `listener`, from `next` on, that expects bytes.
pub fn first_expecting_client(listener: &NetListener, next: usize) -> Option<usize> {
    (next..listener.clients.len()).find(|&i| {
        listener.clients[i]
            .steps
            .iter()
            .any(|step| matches!(step, NetStep::Expect(_)))
    })
}

// A Hirsel byte-string literal: printable ASCII as is, other bytes escaped.
pub(crate) fn byte_literal(bytes: &[u8]) -> String {
    let mut out = String::from("b\"");
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('"');
    out
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PeerTls {
    Plain,
    Certificate(Vec<String>),
    /// Every handshake with this peer fails with `Tls(failure)`.
    Fails(TlsFailure),
    /// A client only: `Certificate(names)` that also presents the PEM certificate chain
    /// (leaf first) as its client certificate.
    Presents(Vec<String>, Vec<u8>),
}

/// `TlsFailure`, in its docs/spec/core-library.md variant order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum TlsFailure {
    UnknownIssuer,
    Expired,
    NotYetValid,
    NameMismatch,
    Revoked,
    BadCertificate,
    AlertReceived(u8),
    HandshakeFailure,
    PeerMisbehaved,
    Truncated,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum NetConnection {
    Refused,
    Unanswered,
    Open(Vec<NetStep>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum NetStep {
    Send(Vec<u8>),
    Expect(Vec<u8>),
    Stall,
    Close,
    Reset,
    /// The peer closes without TLS close_notify: a TLS read is `Tls(Truncated())`.
    Truncate,
}

/// The ordered peer-data encoding used by scripted network tape requests.
/// Encoding data grants no authority and does not admit a RunConfig.
pub fn encode_net_peers(peers: &[NetPeer]) -> Vec<u8> {
    serde_json::to_vec(peers).expect("peer data has no failing JSON values")
}

/// Decode peer data without minting a validated run or accepting its endpoints.
/// The serving adapter validates the complete table before creating a world.
pub fn decode_net_peers(bytes: &[u8]) -> Result<Vec<NetPeer>, RunRefusal> {
    serde_json::from_slice(bytes).map_err(|error| invalid("io.net.peers", &error.to_string()))
}

/// The ordered listener-data encoding used by scripted network tape requests.
pub fn encode_net_listeners(listeners: &[NetListener]) -> Vec<u8> {
    serde_json::to_vec(listeners).expect("listener data has no failing JSON values")
}

/// Decode listener data without validating it, as `decode_net_peers` does.
pub fn decode_net_listeners(bytes: &[u8]) -> Result<Vec<NetListener>, RunRefusal> {
    serde_json::from_slice(bytes).map_err(|error| invalid("io.net.listeners", &error.to_string()))
}

/// A launcher's standalone `io.net` grant: the JSON of a RunConfig's `io.net` field, admitted
/// exactly as that field is (provider contract, canonical rules, peers only for
/// net.script@2). Peers supply data and never widen the rules.
pub fn validate_net_grant(bytes: &[u8]) -> Result<NetGrant, RunRefusal> {
    let mut grant: NetGrant =
        serde_json::from_slice(bytes).map_err(|error| invalid("io.net", &error.to_string()))?;
    if !matches!(
        ProviderContract::parse(&grant.provider),
        Some(ProviderContract::NetStream | ProviderContract::NetScript)
    ) {
        return Err(RunRefusal::Unsupported {
            field: "io.net.provider".into(),
        });
    }
    grant.validate()?;
    Ok(grant)
}

// Missing is None; an explicit null is not a peers array, under any provider.
fn peers<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Option<Vec<NetPeer>>, D::Error> {
    Vec::deserialize(de).map(Some)
}

impl NetGrant {
    pub(crate) fn validate(&mut self) -> Result<(), RunRefusal> {
        let script = ProviderContract::parse(&self.provider) == Some(ProviderContract::NetScript);
        if self.peers.is_some() != script {
            return Err(invalid(
                "io.net.peers",
                "peers is required only for net.script@2",
            ));
        }
        if !self.listeners.is_empty() && !script {
            return Err(invalid(
                "io.net.listeners",
                "listeners are allowed only for net.script@2",
            ));
        }
        check_net_listeners(&self.listeners).map_err(|e| invalid("io.net.listeners", &e))?;
        self.rules.sort();
        for rule in &self.rules {
            let unix = UNIX_OPERATIONS.contains(&rule.operation.as_str());
            if unix && !canonical_unix_path(&rule.address) {
                return Err(invalid(
                    "io.net.rules.address",
                    "expected a canonical absolute socket path",
                ));
            }
            if unix && rule.service != "0" {
                return Err(invalid(
                    "io.net.rules.service",
                    "a Unix socket rule has service 0",
                ));
            }
            if !unix && !canonical_address(&rule.address) {
                return Err(invalid(
                    "io.net.rules.address",
                    "expected a canonical host name or IP literal",
                ));
            }
            if rule.service.is_empty()
                || !rule.service.bytes().all(|b| b.is_ascii_digit())
                || (rule.service.len() > 1 && rule.service.starts_with('0'))
                || rule.service.parse::<u16>().is_err()
            {
                return Err(invalid(
                    "io.net.rules.service",
                    "expected a canonical decimal port 0–65535",
                ));
            }
            if !unix
                && !matches!(
                    rule.operation.as_str(),
                    "connect" | "connect_tls" | "listen" | "udp_bind" | "udp_peer" | "listen_tls"
                )
            {
                return Err(invalid(
                    "io.net.rules.operation",
                    "expected connect, connect_tls, listen, udp_bind, udp_peer, listen_tls, unix_connect, unix_listen or unix_listen_remove_stale",
                ));
            }
        }
        if self.rules.windows(2).any(|p| p[0] == p[1]) {
            return Err(invalid("io.net.rules", "duplicate restriction"));
        }
        if let Some(peers) = &self.peers {
            let mut seen = std::collections::BTreeMap::new();
            for (index, peer) in peers.iter().enumerate() {
                if let Some(first) = seen.insert((&peer.address, peer.port), index) {
                    return Err(invalid(
                        "io.net.peers",
                        &format!("duplicate address/port at entries {first} and {index}"),
                    ));
                }
                check_peer_address(peer).map_err(|e| invalid("io.net.peers.address", &e))?;
                if !peer.resolves_to.iter().all(|address| canonical_ip(address)) {
                    return Err(invalid(
                        "io.net.peers.resolves_to",
                        "expected canonical IP literals",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// The RunConfig operations whose rule address is a socket path.
pub const UNIX_OPERATIONS: [&str; 3] = ["unix_connect", "unix_listen", "unix_listen_remove_stale"];

/// The longest socket path: Linux `sun_path` holds 108 bytes, its terminating NUL included.
pub const UNIX_PATH_MAX: usize = 107;

/// A canonical Unix socket path: a canonical absolute path (as process grants name programs)
/// of at most `UNIX_PATH_MAX` bytes. It never names the abstract namespace.
pub fn canonical_unix_path(path: &str) -> bool {
    path.len() <= UNIX_PATH_MAX && crate::process::canonical_program(path)
}

/// A scripted peer names a host or IP literal, or a socket path: a Unix peer, which has
/// port 0 and resolves to nothing. A peer's TLS is never a client's `Presents`.
pub fn check_peer_address(peer: &NetPeer) -> Result<(), String> {
    if matches!(peer.tls, PeerTls::Presents(..)) {
        return Err("presents is a scripted client's TLS, not a peer's".into());
    }
    if canonical_unix_path(&peer.address) {
        if peer.port != 0 || !peer.resolves_to.is_empty() {
            return Err("a Unix peer has port 0 and no resolves_to".into());
        }
        return Ok(());
    }
    if !canonical_address(&peer.address) {
        return Err("expected a canonical host name, IP literal or socket path".into());
    }
    Ok(())
}

// Use Rust's mature parser and Display directly (library/core/src/net/{parser,
// ip_addr}.rs, f6c1107f), including RFC 5952 and IPv4-mapped IPv6 formatting.
fn canonical_ip(address: &str) -> bool {
    address
        .parse::<IpAddr>()
        .is_ok_and(|ip| ip.to_string() == address)
}

/// Canonical grant address: the exact host or literal form a rule may name.
pub fn canonical_address(address: &str) -> bool {
    // WHATWG URL ends-in-a-number checker (https://url.spec.whatwg.org/
    // #ends-in-a-number-checker), owner T5 addendum. Prevent getaddrinfo from
    // reinterpreting hexadecimal, short and leading-zero IPv4 spellings.
    let last = address
        .trim_end_matches('.')
        .rsplit('.')
        .next()
        .unwrap_or("");
    let numeric = !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit());
    let hexadecimal = last
        .strip_prefix("0x")
        .or_else(|| last.strip_prefix("0X"))
        .is_some_and(|digits| digits.bytes().all(|b| b.is_ascii_hexdigit()));
    if address.contains(':') || numeric || hexadecimal {
        return canonical_ip(address);
    }
    canonical_host(address)
}

// Adapted from Zig lib/std/Io/net/HostName.zig::validate, bff490a2 (MIT).
// Hirsel's exact grants additionally require lowercase ASCII and no root dot.
fn canonical_host(address: &str) -> bool {
    if address.is_empty() || address.len() > 253 {
        return false;
    }
    let alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let mut label_len = 0;
    let mut previous = 0;
    for byte in address.bytes() {
        match byte {
            b'.' => {
                if label_len == 0 || label_len > 63 || !alphanumeric(previous) {
                    return false;
                }
                label_len = 0;
            }
            b'-' => {
                if label_len == 0 {
                    return false;
                }
                label_len += 1;
            }
            _ => {
                if !alphanumeric(byte) {
                    return false;
                }
                label_len += 1;
            }
        }
        previous = byte;
    }
    label_len > 0 && label_len <= 63 && alphanumeric(previous)
}
