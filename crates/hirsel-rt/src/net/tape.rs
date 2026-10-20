//! Tape version 6 network events: one line per event, as the
//! file events, with hex strings and bytes, canonical socket addresses and named errors.
use super::*;
use crate::io::{Completion, Request};
use hirsel_run_schema::hex;

fn unhex(word: &str) -> Result<Vec<u8>, String> {
    if !word.is_ascii() || word.len() % 2 != 0 {
        return Err("bad tape bytes".into());
    }
    (0..word.len() / 2)
        .map(|i| {
            u8::from_str_radix(&word[2 * i..2 * i + 2], 16).map_err(|_| "bad tape bytes".into())
        })
        .collect()
}
fn text(word: &str) -> Result<String, String> {
    String::from_utf8(unhex(word)?).map_err(|_| "bad tape UTF-8".into())
}
fn number<T: std::str::FromStr>(word: &str) -> Result<T, String> {
    // Canonical decimal only, so every tape has exactly one spelling.
    match word.parse::<T>() {
        Ok(n) if !(word.len() > 1 && word.starts_with('0')) && !word.starts_with('+') => Ok(n),
        _ => Err(format!("bad tape number {word:?}")),
    }
}

fn address(a: &SocketAddr) -> String {
    a.std().to_string()
}
fn parse_address(word: &str) -> Result<SocketAddr, String> {
    let parsed: std::net::SocketAddr = word.parse().map_err(|_| "bad tape address")?;
    let a = SocketAddr::of(parsed);
    if address(&a) != word {
        return Err("noncanonical tape address".into());
    }
    Ok(a)
}
fn addresses(list: &[SocketAddr]) -> String {
    list.iter().map(address).collect::<Vec<_>>().join(",")
}
fn parse_addresses(word: &str) -> Result<Vec<SocketAddr>, String> {
    if word.is_empty() {
        return Ok(vec![]);
    }
    word.split(',').map(parse_address).collect()
}
fn timeout(t: &NetTimeout) -> String {
    match t {
        NetTimeout::Unbounded => "unbounded".into(),
        NetTimeout::Nanos(n) => format!("nanos:{n}"),
    }
}
fn parse_timeout(word: &str) -> Result<NetTimeout, String> {
    match word.split_once(':') {
        None if word == "unbounded" => Ok(NetTimeout::Unbounded),
        Some(("nanos", n)) => Ok(NetTimeout::Nanos(number(n)?)),
        _ => Err("bad tape timeout".into()),
    }
}
fn rules(list: &[NetRule]) -> String {
    list.iter()
        .map(|r| {
            format!(
                "{}:{}:{}",
                r.operation.name(),
                hex(r.address.as_bytes()),
                r.port
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}
fn parse_rules(word: &str) -> Result<Vec<NetRule>, String> {
    if word.is_empty() {
        return Ok(vec![]);
    }
    word.split(',')
        .map(|rule| {
            let mut parts = rule.split(':');
            match (parts.next(), parts.next(), parts.next(), parts.next()) {
                (Some(op), Some(a), Some(p), None) => Ok(NetRule {
                    operation: NetOperation::parse(op).ok_or("bad tape rule operation")?,
                    address: text(a)?,
                    port: number(p)?,
                }),
                _ => Err("bad tape rule".into()),
            }
        })
        .collect()
}
fn roots(r: &RootsDigest) -> String {
    match r {
        RootsDigest::WebPki(version) => format!("webpki:{}", hex(version.as_bytes())),
        RootsDigest::Pem(digest) => format!("pem:{}", hex(digest)),
    }
}
fn parse_digest(word: &str) -> Result<[u8; 32], String> {
    unhex(word)?
        .try_into()
        .map_err(|_| "bad tape SHA-256 digest".into())
}
fn parse_roots(word: &str) -> Result<RootsDigest, String> {
    match word.split_once(':') {
        Some(("webpki", v)) => Ok(RootsDigest::WebPki(text(v)?)),
        Some(("pem", d)) => Ok(RootsDigest::Pem(
            unhex(d)?.try_into().map_err(|_| "bad tape PEM digest")?,
        )),
        _ => Err("bad tape roots".into()),
    }
}

/// An ALPN list word: `alpn:` and each protocol's hex, comma separated; absent when empty.
fn alpn(list: &[String]) -> String {
    if list.is_empty() {
        return String::new();
    }
    format!(
        " alpn:{}",
        list.iter()
            .map(|p| hex(p.as_bytes()))
            .collect::<Vec<_>>()
            .join(",")
    )
}
/// An absent word is the empty list, so `alpn:` is the list of one empty protocol (an
/// invalid offer the request still records).
fn parse_alpn(word: &str) -> Result<Vec<String>, String> {
    let list = word.strip_prefix("alpn:").ok_or("bad tape ALPN list")?;
    list.split(',').map(text).collect()
}
fn protocol(p: &Option<String>) -> String {
    p.as_ref()
        .map_or("none".into(), |p| format!("alpn:{}", hex(p.as_bytes())))
}
fn parse_protocol(word: &str) -> Result<Option<String>, String> {
    match word {
        "none" => Ok(None),
        p => Ok(Some(text(
            p.strip_prefix("alpn:").ok_or("bad tape protocol")?,
        )?)),
    }
}

const TLS: [(&str, TlsFailure); 10] = [
    ("unknown-issuer", TlsFailure::UnknownIssuer),
    ("expired", TlsFailure::Expired),
    ("not-yet-valid", TlsFailure::NotYetValid),
    ("name-mismatch", TlsFailure::NameMismatch),
    ("revoked", TlsFailure::Revoked),
    ("bad-certificate", TlsFailure::BadCertificate),
    ("handshake-failure", TlsFailure::HandshakeFailure),
    ("peer-misbehaved", TlsFailure::PeerMisbehaved),
    ("truncated", TlsFailure::Truncated),
    ("certificate-required", TlsFailure::CertificateRequired),
];
const ERRORS: [(&str, NetError); 15] = [
    ("not-granted", NetError::NotGranted),
    ("invalid-input", NetError::InvalidInput),
    ("unknown-host", NetError::UnknownHost),
    ("resolver-failure", NetError::ResolverFailure),
    ("connection-refused", NetError::ConnectionRefused),
    ("connection-reset", NetError::ConnectionReset),
    ("connection-aborted", NetError::ConnectionAborted),
    ("not-connected", NetError::NotConnected),
    ("addr-in-use", NetError::AddrInUse),
    ("addr-not-available", NetError::AddrNotAvailable),
    ("host-unreachable", NetError::HostUnreachable),
    ("network-unreachable", NetError::NetworkUnreachable),
    ("network-down", NetError::NetworkDown),
    ("broken-pipe", NetError::BrokenPipe),
    ("timed-out", NetError::TimedOut),
];
fn error(e: &NetError) -> String {
    match e {
        NetError::Canceled(n) => format!("canceled:{n}"),
        NetError::Fault(n) => format!("fault:{n}"),
        NetError::Tls(TlsFailure::AlertReceived(n)) => format!("tls:alert:{n}"),
        NetError::Tls(f) => format!(
            "tls:{}",
            TLS.iter()
                .find(|(_, v)| v == f)
                .expect("named TLS failure")
                .0
        ),
        e => ERRORS
            .iter()
            .find(|(_, v)| v == e)
            .expect("named net error")
            .0
            .into(),
    }
}
fn parse_error(word: &str) -> Result<NetError, String> {
    if let Some(n) = word.strip_prefix("canceled:") {
        return Ok(NetError::Canceled(number(n)?));
    }
    if let Some(n) = word.strip_prefix("fault:") {
        return Ok(NetError::Fault(number(n)?));
    }
    if let Some(n) = word.strip_prefix("tls:alert:") {
        return Ok(NetError::Tls(TlsFailure::AlertReceived(number(n)?)));
    }
    if let Some(name) = word.strip_prefix("tls:") {
        return TLS
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| NetError::Tls(*v))
            .ok_or_else(|| "bad tape TLS failure".into());
    }
    ERRORS
        .iter()
        .find(|(k, _)| *k == word)
        .map(|(_, v)| *v)
        .ok_or_else(|| "bad tape net error".into())
}
fn outcome<T>(value: &Result<T, NetError>, ok: impl FnOnce(&T) -> String) -> String {
    match value {
        Ok(v) => format!("ok {}", ok(v)),
        Err(e) => format!("err {}", error(e)),
    }
}
fn parse_outcome<T>(
    words: &[&str],
    ok: impl FnOnce(&[&str]) -> Result<T, String>,
) -> Result<Result<T, NetError>, String> {
    match words {
        ["ok", rest @ ..] => Ok(Ok(ok(rest)?)),
        ["err", e] => Ok(Err(parse_error(e)?)),
        _ => Err("bad tape net result".into()),
    }
}
fn one<'a>(words: &[&'a str]) -> Result<&'a str, String> {
    match words {
        [w] => Ok(w),
        _ => Err("bad tape net result".into()),
    }
}
fn how(h: ShutdownHow) -> &'static str {
    match h {
        ShutdownHow::Read => "read",
        ShutdownHow::Write => "write",
        ShutdownHow::Both => "both",
    }
}
fn which(w: TimeoutKind) -> &'static str {
    match w {
        TimeoutKind::Read => "read",
        TimeoutKind::Write => "write",
    }
}

pub(crate) fn render_request(request: &Request) -> String {
    use Request as R;
    match request {
        R::TestNet {
            test,
            peers,
            listeners,
        } => format!(
            "test_net {test} {} {}",
            hex(&hirsel_run_schema::encode_net_peers(peers)),
            hex(&hirsel_run_schema::encode_net_listeners(listeners))
        ),
        R::UdpBind { net, addr } => format!("udp_bind {net} {}", address(addr)),
        R::UdpSend {
            net,
            socket,
            bytes,
            addr,
        } => format!("udp_send {net} {socket} {} {}", hex(bytes), address(addr)),
        R::UdpRecv { net, socket, max } => format!("udp_recv {net} {socket} {max}"),
        R::NetNarrow { net, rules: r } => format!("net_narrow {net} {}", rules(r)),
        R::Resolve { net, host, port } => format!("resolve {net} {} {port}", hex(host.as_bytes())),
        R::Connect {
            net,
            host,
            port,
            timeout: t,
        } => format!(
            "connect {net} {} {port} {}",
            hex(host.as_bytes()),
            timeout(t)
        ),
        R::ConnectAddr {
            net,
            addr,
            timeout: t,
        } => format!("connect_addr {net} {} {}", address(addr), timeout(t)),
        R::NetRead { net, stream, max } => format!("net_read {net} {stream} {max}"),
        R::NetWrite { net, stream, bytes } => format!("net_write {net} {stream} {}", hex(bytes)),
        R::NetWriteAll { net, stream, bytes } => {
            format!("net_write_all {net} {stream} {}", hex(bytes))
        }
        R::Shutdown {
            net,
            stream,
            how: h,
        } => format!("shutdown {net} {stream} {}", how(*h)),
        R::SetTimeout {
            net,
            stream,
            which: w,
            timeout: t,
        } => format!("set_timeout {net} {stream} {} {}", which(*w), timeout(t)),
        R::PeerAddr { net, stream } => format!("peer_addr {net} {stream}"),
        R::NetSplit { net, stream } => format!("net_split {net} {stream}"),
        R::NetJoin { net, read, write } => format!("net_join {net} {read} {write}"),
        R::TlsConnect {
            net,
            stream,
            server_name,
            roots: r,
            identity,
            alpn: a,
        } => format!(
            "tls_connect {net} {stream} {} {}{}{}",
            hex(server_name.as_bytes()),
            roots(r),
            identity.map_or(String::new(), |d| format!(" pem:{}", hex(&d))),
            alpn(a)
        ),
        R::TlsAccept {
            net,
            stream,
            server,
        } => {
            let identities = server
                .identities
                .iter()
                .map(|d| format!("pem:{}", hex(d)))
                .collect::<Vec<_>>()
                .join(",");
            let clients = match server.clients {
                ClientAuth::Anonymous => "anonymous".into(),
                ClientAuth::Request(d) => format!("request:{}", hex(&d)),
                ClientAuth::Require(d) => format!("require:{}", hex(&d)),
            };
            format!(
                "tls_accept {net} {stream} {} {clients}{}",
                if identities.is_empty() {
                    "none"
                } else {
                    &identities
                },
                alpn(&server.alpn)
            )
        }
        R::TlsPeer { net, stream } => format!("tls_peer {net} {stream}"),
        R::TlsProtocol { net, stream } => format!("tls_protocol {net} {stream}"),
        R::TlsClose { net, stream } => format!("tls_close {net} {stream}"),
        R::Listen { net, addr } => format!("listen {net} {}", address(addr)),
        R::Accept { net, listener } => format!("accept {net} {listener}"),
        R::LocalAddr { net, listener } => format!("local_addr {net} {listener}"),
        R::UnixConnect { net, path } => format!("unix_connect {net} {}", hex(path.as_bytes())),
        R::UnixListen { net, path } => format!("unix_listen {net} {}", hex(path.as_bytes())),
        _ => unreachable!("a file or grant request has its own tape form"),
    }
}

pub(crate) fn render_completion(completion: &Completion) -> String {
    use Completion as C;
    match completion {
        C::Listened(v) => format!(
            "listened {}",
            outcome(v, |(id, local)| format!("{id} {}", address(local)))
        ),
        C::Datagram(v) => format!(
            "datagram {}",
            outcome(v, |(bytes, peer, truncated)| format!(
                "{} {} {}",
                hex(bytes),
                address(peer),
                u8::from(*truncated)
            ))
        ),
        C::NetHandle(v) => format!("net_handle {}", outcome(v, u64::to_string)),
        C::Addresses(v) => format!("addresses {}", outcome(v, |a| addresses(a))),
        C::NetBytes(v) => format!("net_bytes {}", outcome(v, |b| hex(b))),
        C::Count(v) => format!("count {}", outcome(v, u64::to_string)),
        C::NetUnit(v) => format!("net_unit {}", outcome(v, |()| String::new())),
        C::Certificates(v) => format!(
            "certificates {}",
            outcome(v, |chain| chain
                .as_ref()
                .map_or("none".into(), |der| format!("der:{}", hex(der))))
        ),
        C::Protocol(v) => format!("protocol {}", outcome(v, protocol)),
        C::Address(v) => format!("address {}", outcome(v, address)),
        C::Halves(v) => format!("halves {}", outcome(v, |(r, w)| format!("{r} {w}"))),
        C::Accepted(v) => format!(
            "accepted {}",
            outcome(v, |(id, peer)| format!("{id} {}", address(peer)))
        ),
        _ => unreachable!("a file or grant completion has its own tape form"),
    }
}

/// `Ok(None)`: not a network request word; the caller reports its own error.
pub(crate) fn parse_request(words: &[&str]) -> Result<Option<Request>, String> {
    use Request as R;
    Ok(Some(match words {
        ["test_net", test, peers, listeners] => R::TestNet {
            test: number(test)?,
            peers: hirsel_run_schema::decode_net_peers(&unhex(peers)?)
                .map_err(|e| e.to_string())?,
            listeners: hirsel_run_schema::decode_net_listeners(&unhex(listeners)?)
                .map_err(|e| e.to_string())?,
        },
        ["udp_bind", n, a] => R::UdpBind {
            net: number(n)?,
            addr: parse_address(a)?,
        },
        ["udp_send", n, s, b, a] => R::UdpSend {
            net: number(n)?,
            socket: number(s)?,
            bytes: unhex(b)?,
            addr: parse_address(a)?,
        },
        ["udp_recv", n, s, m] => R::UdpRecv {
            net: number(n)?,
            socket: number(s)?,
            max: number(m)?,
        },
        ["net_narrow", n, r] => R::NetNarrow {
            net: number(n)?,
            rules: parse_rules(r)?,
        },
        ["resolve", n, h, p] => R::Resolve {
            net: number(n)?,
            host: text(h)?,
            port: number(p)?,
        },
        ["connect", n, h, p, t] => R::Connect {
            net: number(n)?,
            host: text(h)?,
            port: number(p)?,
            timeout: parse_timeout(t)?,
        },
        ["connect_addr", n, a, t] => R::ConnectAddr {
            net: number(n)?,
            addr: parse_address(a)?,
            timeout: parse_timeout(t)?,
        },
        ["net_read", n, s, m] => R::NetRead {
            net: number(n)?,
            stream: number(s)?,
            max: number(m)?,
        },
        ["net_write", n, s, b] => R::NetWrite {
            net: number(n)?,
            stream: number(s)?,
            bytes: unhex(b)?,
        },
        ["net_write_all", n, s, b] => R::NetWriteAll {
            net: number(n)?,
            stream: number(s)?,
            bytes: unhex(b)?,
        },
        ["shutdown", n, s, h] => R::Shutdown {
            net: number(n)?,
            stream: number(s)?,
            how: match *h {
                "read" => ShutdownHow::Read,
                "write" => ShutdownHow::Write,
                "both" => ShutdownHow::Both,
                _ => return Err("bad tape shutdown".into()),
            },
        },
        ["set_timeout", n, s, w, t] => R::SetTimeout {
            net: number(n)?,
            stream: number(s)?,
            which: match *w {
                "read" => TimeoutKind::Read,
                "write" => TimeoutKind::Write,
                _ => return Err("bad tape timeout kind".into()),
            },
            timeout: parse_timeout(t)?,
        },
        ["peer_addr", n, s] => R::PeerAddr {
            net: number(n)?,
            stream: number(s)?,
        },
        ["net_split", n, s] => R::NetSplit {
            net: number(n)?,
            stream: number(s)?,
        },
        ["net_join", n, r, w] => R::NetJoin {
            net: number(n)?,
            read: number(r)?,
            write: number(w)?,
        },
        ["tls_connect", n, s, name, r, rest @ ..] => {
            let (identity, rest) = match rest {
                [d, rest @ ..] if d.starts_with("pem:") => {
                    (Some(parse_digest(d.strip_prefix("pem:").unwrap())?), rest)
                }
                rest => (None, rest),
            };
            R::TlsConnect {
                net: number(n)?,
                stream: number(s)?,
                server_name: text(name)?,
                roots: parse_roots(r)?,
                identity,
                alpn: match rest {
                    [] => vec![],
                    [a] => parse_alpn(a)?,
                    _ => return Err("bad tape tls_connect".into()),
                },
            }
        }
        ["tls_accept", n, s, identities, clients, rest @ ..] => R::TlsAccept {
            net: number(n)?,
            stream: number(s)?,
            server: TlsServerDigest {
                identities: match *identities {
                    "none" => vec![],
                    list => list
                        .split(',')
                        .map(|d| parse_digest(d.strip_prefix("pem:").ok_or("bad tape identity")?))
                        .collect::<Result<_, String>>()?,
                },
                clients: match clients.split_once(':') {
                    None if *clients == "anonymous" => ClientAuth::Anonymous,
                    Some(("request", d)) => ClientAuth::Request(parse_digest(d)?),
                    Some(("require", d)) => ClientAuth::Require(parse_digest(d)?),
                    _ => return Err("bad tape client auth".into()),
                },
                alpn: match rest {
                    [] => vec![],
                    [a] => parse_alpn(a)?,
                    _ => return Err("bad tape tls_accept".into()),
                },
            },
        },
        ["tls_peer", n, s] => R::TlsPeer {
            net: number(n)?,
            stream: number(s)?,
        },
        ["tls_protocol", n, s] => R::TlsProtocol {
            net: number(n)?,
            stream: number(s)?,
        },
        ["tls_close", n, s] => R::TlsClose {
            net: number(n)?,
            stream: number(s)?,
        },
        ["listen", n, a] => R::Listen {
            net: number(n)?,
            addr: parse_address(a)?,
        },
        ["accept", n, l] => R::Accept {
            net: number(n)?,
            listener: number(l)?,
        },
        ["local_addr", n, l] => R::LocalAddr {
            net: number(n)?,
            listener: number(l)?,
        },
        ["unix_connect", n, p] => R::UnixConnect {
            net: number(n)?,
            path: text(p)?,
        },
        ["unix_listen", n, p] => R::UnixListen {
            net: number(n)?,
            path: text(p)?,
        },
        _ => return Ok(None),
    }))
}

/// `Ok(None)`: not a network completion word.
pub(crate) fn parse_completion(words: &[&str]) -> Result<Option<Completion>, String> {
    use Completion as C;
    Ok(Some(match words {
        ["listened", r @ ..] => C::Listened(parse_outcome(r, |w| match w {
            [id, local] => Ok((number(id)?, parse_address(local)?)),
            _ => Err("bad tape listened".into()),
        })?),
        ["datagram", r @ ..] => C::Datagram(parse_outcome(r, |w| match w {
            [bytes, peer, truncated] => Ok((
                unhex(bytes)?,
                parse_address(peer)?,
                match *truncated {
                    "0" => false,
                    "1" => true,
                    _ => return Err("bad datagram truncation".into()),
                },
            )),
            _ => Err("bad tape datagram".into()),
        })?),
        ["net_handle", r @ ..] => C::NetHandle(parse_outcome(r, |w| number(one(w)?))?),
        ["addresses", r @ ..] => C::Addresses(parse_outcome(r, |w| parse_addresses(one(w)?))?),
        ["net_bytes", r @ ..] => C::NetBytes(parse_outcome(r, |w| unhex(one(w)?))?),
        ["count", r @ ..] => C::Count(parse_outcome(r, |w| number(one(w)?))?),
        ["net_unit", r @ ..] => C::NetUnit(parse_outcome(r, |w| match w {
            [""] => Ok(()),
            _ => Err("bad unit result".into()),
        })?),
        ["certificates", r @ ..] => C::Certificates(parse_outcome(r, |w| match one(w)? {
            "none" => Ok(None),
            der => Ok(Some(unhex(
                der.strip_prefix("der:").ok_or("bad tape chain")?,
            )?)),
        })?),
        ["protocol", r @ ..] => C::Protocol(parse_outcome(r, |w| parse_protocol(one(w)?))?),
        ["address", r @ ..] => C::Address(parse_outcome(r, |w| parse_address(one(w)?))?),
        ["halves", r @ ..] => C::Halves(parse_outcome(r, |w| match w {
            [read, write] => Ok((number(read)?, number(write)?)),
            _ => Err("bad tape halves".into()),
        })?),
        ["accepted", r @ ..] => C::Accepted(parse_outcome(r, |w| match w {
            [id, peer] => Ok((number(id)?, parse_address(peer)?)),
            _ => Err("bad tape accepted".into()),
        })?),
        _ => return Ok(None),
    }))
}
