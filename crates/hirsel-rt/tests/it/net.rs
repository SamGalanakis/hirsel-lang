//! Network grants, the host provider, tape v4 and exact replay:
//! record against an in-process loopback server, replay with no sockets, and refuse an
//! edited, truncated or extended tape. Denials are checked against a provider call counter.
use hirsel_rt::io::*;
use hirsel_rt::io_real::RealIo;
use hirsel_rt::io_sim::SimIo;
use hirsel_rt::launch::{parse_tape, render_tape};
use hirsel_rt::net::{ClientAuth, Provided, TlsServerDigest};
use hirsel_rt::net_host::HostNet;
use std::cell::Cell;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::rc::Rc;

const IO: u64 = 1;

fn rule(address: &str, port: u16, operation: NetOperation) -> NetRule {
    NetRule {
        address: address.into(),
        port,
        operation,
    }
}
fn loopback(port: u16) -> SocketAddr {
    SocketAddr {
        ip: std::net::Ipv4Addr::LOCALHOST.into(),
        port,
    }
}
fn real(rules: impl IntoIterator<Item = NetRule>) -> RealIo {
    let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
    io.set_net_rules(rules.into_iter().collect());
    io
}
fn net(io: &mut impl Adapter) -> u64 {
    match io.call(Request::Net { io: IO }) {
        Ok(Completion::Handle(Ok(id))) => id,
        other => panic!("io.net: {other:?}"),
    }
}
fn handle(completion: Result<Completion, AdapterError>) -> u64 {
    match completion {
        Ok(Completion::NetHandle(Ok(id)) | Completion::Listened(Ok((id, _)))) => id,
        other => panic!("expected a handle: {other:?}"),
    }
}
fn read_to_end(io: &mut impl Adapter, net: u64, stream: u64, max: u64) -> Vec<Vec<u8>> {
    let mut chunks = vec![];
    loop {
        match io.call(Request::NetRead { net, stream, max }) {
            Ok(Completion::NetBytes(Ok(bytes))) if bytes.is_empty() => return chunks,
            Ok(Completion::NetBytes(Ok(bytes))) => {
                assert!(bytes.len() as u64 <= max);
                chunks.push(bytes);
            }
            other => panic!("read: {other:?}"),
        }
    }
}

/// A one-connection server: read the request until the client half-closes, then send
/// the response and close.
fn serve(response: &'static [u8]) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = vec![];
        stream.read_to_end(&mut request).unwrap();
        stream.write_all(response).unwrap();
        request
    });
    (port, thread)
}

/// The recorded program: connect, send, half-close, read to EOF in small reads, close.
fn exchange(io: &mut impl Adapter, port: u16, message: &[u8]) -> Vec<Vec<u8>> {
    let n = net(io);
    let s = handle(io.call(Request::ConnectAddr {
        net: n,
        addr: loopback(port),
        timeout: NetTimeout::Nanos(5_000_000_000),
    }));
    assert_eq!(
        io.call(Request::NetWriteAll {
            net: n,
            stream: s,
            bytes: message.to_vec(),
        }),
        Ok(Completion::NetUnit(Ok(())))
    );
    assert_eq!(
        io.call(Request::Shutdown {
            net: n,
            stream: s,
            how: ShutdownHow::Write,
        }),
        Ok(Completion::NetUnit(Ok(())))
    );
    let chunks = read_to_end(io, n, s, 4);
    assert_eq!(
        io.call(Request::Close { handle: s }),
        Ok(Completion::Unit(Ok(())))
    );
    chunks
}

fn record_exchange() -> (BTreeSet<NetRule>, u16, Vec<Event>, Vec<Vec<u8>>) {
    let (port, server) = serve(b"pong and more");
    let rules = BTreeSet::from([rule("127.0.0.1", port, NetOperation::Connect)]);
    let mut io = real(rules.clone());
    let chunks = exchange(&mut io, port, b"ping");
    assert_eq!(server.join().unwrap(), b"ping");
    assert_eq!(chunks.concat(), b"pong and more");
    (rules, port, io.tape().to_vec(), chunks)
}

fn replay(rules: &BTreeSet<NetRule>, tape: Vec<Event>) -> SimIo {
    let mut sim = SimIo::new(Tree::new(Default::default()).unwrap(), tape);
    sim.set_net_rules(rules.clone());
    sim
}

#[test]
fn loopback_recording_replays_without_sockets() {
    let (rules, port, tape, chunks) = record_exchange();
    // The server is gone: replay must not touch the network at all.
    let text = render_tape(&tape).unwrap();
    assert!(text.starts_with("hirsel-io-tape 7\n"));
    let parsed = parse_tape(&text).unwrap();
    assert_eq!(parsed, tape);
    let mut sim = replay(&rules, parsed);
    assert_eq!(exchange(&mut sim, port, b"ping"), chunks);
    sim.finish().unwrap();
    let initial = Tree::new(Default::default()).unwrap();
    crate::support::io::replay_interpreter(
        &tape,
        &initial,
        sim.tree(),
        sim.handles().observed(),
        b"",
    );
    let mut altered = tape.clone();
    let bytes = altered
        .iter_mut()
        .find_map(|event| match &mut event.completion {
            Completion::NetBytes(Ok(bytes)) if !bytes.is_empty() => Some(bytes),
            _ => None,
        })
        .unwrap();
    bytes[0] ^= 1;
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::support::io::replay_interpreter_with_fixture(
                &altered,
                crate::support::io::ReplayFixture {
                    events: &tape,
                    pem_roots: &[],
                    tls_identities: &[],
                },
                &initial,
                sim.tree(),
                sim.handles().observed(),
                b"",
                &std::collections::BTreeMap::from([(
                    String::new(),
                    hirsel_run_config::Access::ReadWrite,
                )]),
            );
        }))
        .is_err(),
        "altered read bytes passed the independent interpreter comparison"
    );
}

#[test]
fn replay_refuses_an_edited_write_a_missing_and_an_unconsumed_event() {
    let (rules, port, tape, _) = record_exchange();
    let write_at = tape
        .iter()
        .position(|e| matches!(e.request, Request::NetWriteAll { .. }))
        .unwrap();
    // One written byte differs: the program's request no longer matches the tape.
    let mut sim = replay(&rules, tape.clone());
    let n = net(&mut sim);
    let s = handle(sim.call(Request::ConnectAddr {
        net: n,
        addr: loopback(port),
        timeout: NetTimeout::Nanos(5_000_000_000),
    }));
    assert_eq!(
        sim.call(Request::NetWriteAll {
            net: n,
            stream: s,
            bytes: b"pinG".to_vec(),
        }),
        Err(AdapterError::ReplayMismatch(write_at))
    );
    // Truncated: the last event is missing.
    let mut truncated = tape.clone();
    truncated.pop();
    let mut sim = replay(&rules, truncated);
    for event in &tape[..tape.len() - 1] {
        sim.call(event.request.clone()).unwrap();
    }
    assert_eq!(
        sim.call(tape.last().unwrap().request.clone()),
        Err(AdapterError::MissingInput(tape.len() - 1))
    );
    // Extended: an appended event is never consumed.
    let mut extended = tape.clone();
    extended.push(tape[1].clone());
    let mut sim = replay(&rules, extended);
    exchange(&mut sim, port, b"ping");
    assert_eq!(sim.finish(), Err(AdapterError::UnconsumedInput(tape.len())));
}

#[test]
fn replay_re_derives_every_grant_decision() {
    let (rules, port, tape, chunks) = record_exchange();
    // A wider rule set that makes the same decisions replays.
    let mut wider = rules.clone();
    wider.insert(rule("example.com", 443, NetOperation::ConnectTls));
    assert_eq!(
        exchange(&mut replay(&wider, tape.clone()), port, b"ping"),
        chunks
    );
    // Rules that deny the recorded connect refuse at that event, before any provider input.
    let mut sim = replay(&BTreeSet::new(), tape.clone());
    let n = net(&mut sim);
    assert_eq!(
        sim.call(Request::ConnectAddr {
            net: n,
            addr: loopback(port),
            timeout: NetTimeout::Nanos(5_000_000_000),
        }),
        Err(AdapterError::ReplayMismatch(1))
    );
    // A recorded denial cannot be replayed under rules that allow it, and a provider
    // never reports NotGranted.
    let mut forged = tape.clone();
    forged[1].completion = Completion::NetHandle(Err(NetError::NotGranted));
    let mut sim = replay(&rules, forged);
    let n = net(&mut sim);
    assert_eq!(
        sim.call(Request::ConnectAddr {
            net: n,
            addr: loopback(port),
            timeout: NetTimeout::Nanos(5_000_000_000),
        }),
        Err(AdapterError::InvalidCompletion)
    );
}

/// Counts every provider effect; `close` is the drop of a handle the policy issued.
struct Counting {
    inner: HostNet,
    calls: Rc<Cell<usize>>,
}
impl Counting {
    fn tick(&self) {
        self.calls.set(self.calls.get() + 1);
    }
}
impl NetProvider for Counting {
    fn udp_bind(&mut self, id: u64, addr: SocketAddr) -> Provided<()> {
        self.tick();
        self.inner.udp_bind(id, addr)
    }
    fn udp_send(&mut self, id: u64, bytes: &[u8], addr: SocketAddr) -> Provided<u64> {
        self.tick();
        self.inner.udp_send(id, bytes, addr)
    }
    fn udp_recv(&mut self, id: u64) -> Provided<(Vec<u8>, SocketAddr)> {
        self.tick();
        self.inner.udp_recv(id)
    }
    fn resolve(&mut self, host: &str, port: u16) -> Provided<Vec<SocketAddr>> {
        self.tick();
        self.inner.resolve(host, port)
    }
    fn connect(&mut self, id: u64, host: &str, port: u16, t: NetTimeout) -> Provided<()> {
        self.tick();
        self.inner.connect(id, host, port, t)
    }
    fn connect_addr(&mut self, id: u64, addr: SocketAddr, t: NetTimeout) -> Provided<()> {
        self.tick();
        self.inner.connect_addr(id, addr, t)
    }
    fn read(&mut self, s: u64, max: u64) -> Provided<Vec<u8>> {
        self.tick();
        self.inner.read(s, max)
    }
    fn write(&mut self, s: u64, b: &[u8]) -> Provided<u64> {
        self.tick();
        self.inner.write(s, b)
    }
    fn write_all(&mut self, s: u64, b: &[u8]) -> Provided<()> {
        self.tick();
        self.inner.write_all(s, b)
    }
    fn shutdown(&mut self, s: u64, how: ShutdownHow) -> Provided<()> {
        self.tick();
        self.inner.shutdown(s, how)
    }
    fn set_timeout(&mut self, s: u64, w: TimeoutKind, t: NetTimeout) -> Provided<()> {
        self.tick();
        self.inner.set_timeout(s, w, t)
    }
    fn peer_addr(&mut self, s: u64) -> Provided<SocketAddr> {
        self.tick();
        self.inner.peer_addr(s)
    }
    fn tls_connect(
        &mut self,
        s: u64,
        name: &str,
        roots: &RootsDigest,
        identity: Option<&std::sync::Arc<rustls::sign::CertifiedKey>>,
    ) -> Provided<()> {
        self.tick();
        self.inner.tls_connect(s, name, roots, identity)
    }
    fn tls_accept(&mut self, s: u64, server: &hirsel_run_config::tls::Server) -> Provided<()> {
        self.tick();
        self.inner.tls_accept(s, server)
    }
    fn tls_peer(&mut self, s: u64) -> Provided<Option<Vec<u8>>> {
        self.tick();
        self.inner.tls_peer(s)
    }
    fn unix_connect(&mut self, id: u64, path: &str) -> Provided<()> {
        self.tick();
        self.inner.unix_connect(id, path)
    }
    fn unix_listen(&mut self, id: u64, path: &str, remove_stale: bool) -> Provided<()> {
        self.tick();
        self.inner.unix_listen(id, path, remove_stale)
    }
    fn tls_close(&mut self, s: u64) -> Provided<()> {
        self.tick();
        self.inner.tls_close(s)
    }
    fn listen(&mut self, id: u64, addr: SocketAddr) -> Provided<()> {
        self.tick();
        self.inner.listen(id, addr)
    }
    fn accept(&mut self, l: u64, id: u64) -> Provided<SocketAddr> {
        self.tick();
        self.inner.accept(l, id)
    }
    fn local_addr(&mut self, l: u64) -> Provided<SocketAddr> {
        self.tick();
        self.inner.local_addr(l)
    }
    fn close(&mut self, handle: u64) {
        self.inner.close(handle)
    }
}
fn counted(rules: impl IntoIterator<Item = NetRule>) -> (RealIo, Rc<Cell<usize>>) {
    let mut io = real(rules);
    let calls = Rc::new(Cell::new(0));
    io.set_net_provider(Counting {
        inner: HostNet::new(),
        calls: calls.clone(),
    });
    (io, calls)
}

#[test]
fn denied_and_invalid_operations_issue_no_provider_call() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (mut io, calls) = counted([
        rule("localhost", port, NetOperation::Connect),
        rule("127.0.0.1", port, NetOperation::ConnectTls),
    ]);
    let n = net(&mut io);
    let t = NetTimeout::Nanos(5_000_000_000);
    let denied = |c: Result<Completion, AdapterError>| match c {
        Ok(
            Completion::NetHandle(Err(e))
            | Completion::Listened(Err(e))
            | Completion::NetUnit(Err(e))
            | Completion::NetBytes(Err(e))
            | Completion::Addresses(Err(e))
            | Completion::Count(Err(e))
            | Completion::Address(Err(e)),
        ) => e,
        other => panic!("expected an error: {other:?}"),
    };
    // Ungranted name, port and operation.
    for (host, port) in [("example.com", port), ("localhost", port + 1)] {
        let c = io.call(Request::Connect {
            net: n,
            host: host.into(),
            port,
            timeout: t,
        });
        assert_eq!(denied(c), NetError::NotGranted);
        let c = io.call(Request::Resolve {
            net: n,
            host: host.into(),
            port,
        });
        assert_eq!(denied(c), NetError::NotGranted);
    }
    let c = io.call(Request::Listen {
        net: n,
        addr: loopback(port),
    });
    assert_eq!(denied(c), NetError::NotGranted);
    // A host-name rule never authorizes the address it resolves to.
    let c = io.call(Request::ConnectAddr {
        net: n,
        addr: SocketAddr {
            ip: std::net::Ipv6Addr::LOCALHOST.into(),
            port,
        },
        timeout: t,
    });
    assert_eq!(denied(c), NetError::NotGranted);
    // Invalid inputs fail before the grant check.
    for host in ["LOCALHOST", "localhost.", "127.1", "", "a b"] {
        let c = io.call(Request::Connect {
            net: n,
            host: host.into(),
            port,
            timeout: t,
        });
        assert_eq!(denied(c), NetError::InvalidInput, "{host:?}");
    }
    let c = io.call(Request::Connect {
        net: n,
        host: "localhost".into(),
        port,
        timeout: NetTimeout::Nanos(0),
    });
    assert_eq!(denied(c), NetError::InvalidInput);
    // Narrowing beyond the parent mints nothing; an empty narrow denies everything.
    let c = io.call(Request::NetNarrow {
        net: n,
        rules: vec![rule("localhost", port, NetOperation::Listen)],
    });
    assert_eq!(denied(c), NetError::NotGranted);
    let c = io.call(Request::NetNarrow {
        net: n,
        rules: vec![rule("127.0.0.1", port, NetOperation::Connect)],
    });
    assert_eq!(denied(c), NetError::NotGranted);
    let empty = handle(io.call(Request::NetNarrow {
        net: n,
        rules: vec![],
    }));
    let c = io.call(Request::Connect {
        net: empty,
        host: "localhost".into(),
        port,
        timeout: t,
    });
    assert_eq!(denied(c), NetError::NotGranted);
    // ConnectTls narrows Connect.
    let tls_only = handle(io.call(Request::NetNarrow {
        net: n,
        rules: vec![rule("localhost", port, NetOperation::ConnectTls)],
    }));
    assert_eq!(calls.get(), 0, "no denial reached the provider");

    // A ConnectTls stream carries no plaintext before its handshake, and its handshake
    // must name the rule's address.
    let s = handle(io.call(Request::ConnectAddr {
        net: n,
        addr: loopback(port),
        timeout: t,
    }));
    let (_accepted, _) = listener.accept().unwrap();
    assert_eq!(calls.get(), 1);
    for request in [
        Request::NetRead {
            net: n,
            stream: s,
            max: 1,
        },
        Request::NetWrite {
            net: n,
            stream: s,
            bytes: b"x".to_vec(),
        },
        Request::NetWriteAll {
            net: n,
            stream: s,
            bytes: b"x".to_vec(),
        },
        Request::Shutdown {
            net: n,
            stream: s,
            how: ShutdownHow::Both,
        },
    ] {
        assert_eq!(denied(io.call(request)), NetError::NotGranted);
    }
    // Owner 385441: metadata and timeout options carry no plaintext and may
    // bound the TLS handshake before an attenuated stream is upgraded.
    for which in [TimeoutKind::Read, TimeoutKind::Write] {
        assert_eq!(
            io.call(Request::SetTimeout {
                net: n,
                stream: s,
                which,
                timeout: t
            }),
            Ok(Completion::NetUnit(Ok(())))
        );
    }
    assert!(matches!(
        io.call(Request::PeerAddr { net: n, stream: s }),
        Ok(Completion::Address(Ok(_)))
    ));
    let c = io.call(Request::NetRead {
        net: n,
        stream: s,
        max: 0,
    });
    assert_eq!(denied(c), NetError::InvalidInput);
    assert_eq!(calls.get(), 4);
    // A stream is reachable only through the Net that brands it (CAP-2 refusal).
    assert_eq!(
        io.call(Request::NetRead {
            net: tls_only,
            stream: s,
            max: 1,
        }),
        Err(AdapterError::InvalidHandle(s))
    );
    let c = io.call(Request::TlsConnect {
        net: n,
        stream: s,
        server_name: "localhost".into(),
        roots: RootsDigest::WebPki("webpki-roots@1.0.9".into()),
        identity: None,
    });
    assert_eq!(denied(c), NetError::NotGranted);
    assert_eq!(calls.get(), 4, "a refused handshake sends no TLS byte");
    // The taken stream was closed.
    assert_eq!(
        io.call(Request::PeerAddr { net: n, stream: s }),
        Err(AdapterError::InvalidHandle(s))
    );
}

#[test]
fn connect_by_name_tries_resolved_addresses_in_order() {
    let (port, server) = serve(b"by name");
    let (mut io, calls) = counted([rule("localhost", port, NetOperation::Connect)]);
    let n = net(&mut io);
    let addresses = match io.call(Request::Resolve {
        net: n,
        host: "localhost".into(),
        port,
    }) {
        Ok(Completion::Addresses(Ok(a))) => a,
        other => panic!("resolve: {other:?}"),
    };
    assert!(addresses.contains(&loopback(port)), "{addresses:?}");
    assert!(addresses.iter().all(|a| a.port == port));
    // Whether or not ::1 comes first and is refused, 127.0.0.1 is reached.
    let s = handle(io.call(Request::Connect {
        net: n,
        host: "localhost".into(),
        port,
        timeout: NetTimeout::Unbounded,
    }));
    assert_eq!(
        io.call(Request::PeerAddr { net: n, stream: s }),
        Ok(Completion::Address(Ok(loopback(port))))
    );
    io.call(Request::NetWrite {
        net: n,
        stream: s,
        bytes: b"hi".to_vec(),
    })
    .unwrap();
    io.call(Request::Shutdown {
        net: n,
        stream: s,
        how: ShutdownHow::Write,
    })
    .unwrap();
    assert_eq!(read_to_end(&mut io, n, s, 1024).concat(), b"by name");
    assert_eq!(server.join().unwrap(), b"hi");
    assert!(calls.get() >= 7);
    // The stream is closed on drop of the adapter; recorded events replay.
    let tape = io.tape().to_vec();
    let mut sim = replay(
        &[rule("localhost", port, NetOperation::Connect)].into(),
        tape.clone(),
    );
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(event.completion.clone())
        );
    }
    sim.finish().unwrap();
}

#[test]
fn host_errors_and_timeouts_are_recorded_outcomes() {
    // Refused: nothing listens on a just-released port.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let open = listener.local_addr().unwrap().port();
    let mut io = real([
        rule("127.0.0.1", port, NetOperation::Connect),
        rule("127.0.0.1", open, NetOperation::Connect),
        rule("no-such-host.invalid", 80, NetOperation::Connect),
    ]);
    let n = net(&mut io);
    assert_eq!(
        io.call(Request::ConnectAddr {
            net: n,
            addr: loopback(port),
            timeout: NetTimeout::Unbounded,
        }),
        Ok(Completion::NetHandle(Err(NetError::ConnectionRefused)))
    );
    match io.call(Request::Resolve {
        net: n,
        host: "no-such-host.invalid".into(),
        port: 80,
    }) {
        Ok(Completion::Addresses(Err(NetError::UnknownHost | NetError::ResolverFailure))) => {}
        other => panic!("resolve of an invalid name: {other:?}"),
    }
    // A read timeout is the outcome TimedOut and leaves the stream usable.
    let s = handle(io.call(Request::ConnectAddr {
        net: n,
        addr: loopback(open),
        timeout: NetTimeout::Unbounded,
    }));
    let (mut peer, _) = listener.accept().unwrap();
    assert_eq!(
        io.call(Request::SetTimeout {
            net: n,
            stream: s,
            which: TimeoutKind::Read,
            timeout: NetTimeout::Nanos(20_000_000),
        }),
        Ok(Completion::NetUnit(Ok(())))
    );
    assert_eq!(
        io.call(Request::NetRead {
            net: n,
            stream: s,
            max: 8,
        }),
        Ok(Completion::NetBytes(Err(NetError::TimedOut)))
    );
    // The timeout assertion is complete. Wait for the peer's data and FIN,
    // whose delivery is an event rather than a 20 ms scheduling guarantee.
    assert_eq!(
        io.call(Request::SetTimeout {
            net: n,
            stream: s,
            which: TimeoutKind::Read,
            timeout: NetTimeout::Unbounded,
        }),
        Ok(Completion::NetUnit(Ok(())))
    );
    peer.write_all(b"late").unwrap();
    assert_eq!(
        io.call(Request::NetRead {
            net: n,
            stream: s,
            max: 8,
        }),
        Ok(Completion::NetBytes(Ok(b"late".to_vec())))
    );
    drop(peer);
    assert_eq!(
        io.call(Request::NetRead {
            net: n,
            stream: s,
            max: 8,
        }),
        Ok(Completion::NetBytes(Ok(vec![])))
    );
    let tape = io.tape().to_vec();
    assert_eq!(parse_tape(&render_tape(&tape).unwrap()).unwrap(), tape);
    let rules = [
        rule("127.0.0.1", port, NetOperation::Connect),
        rule("127.0.0.1", open, NetOperation::Connect),
        rule("no-such-host.invalid", 80, NetOperation::Connect),
    ]
    .into();
    let mut sim = replay(&rules, tape.clone());
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(event.completion.clone())
        );
    }
    sim.finish().unwrap();
}

#[test]
fn listeners_accept_under_a_listen_rule() {
    let mut io = real([rule("127.0.0.1", 0, NetOperation::Listen)]);
    let n = net(&mut io);
    let l = handle(io.call(Request::Listen {
        net: n,
        addr: loopback(0),
    }));
    let port = match io.call(Request::LocalAddr {
        net: n,
        listener: l,
    }) {
        Ok(Completion::Address(Ok(a))) => a.port,
        other => panic!("local_addr: {other:?}"),
    };
    let client = std::thread::spawn(move || {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(b"hello").unwrap();
        s.local_addr().unwrap()
    });
    let (s, peer) = match io.call(Request::Accept {
        net: n,
        listener: l,
    }) {
        Ok(Completion::Accepted(Ok(v))) => v,
        other => panic!("accept: {other:?}"),
    };
    assert_eq!(peer.std(), client.join().unwrap());
    assert_eq!(read_to_end(&mut io, n, s, 64).concat(), b"hello");
    assert_eq!(
        io.call(Request::Close { handle: s }),
        Ok(Completion::Unit(Ok(())))
    );
    assert_eq!(
        io.call(Request::Close { handle: l }),
        Ok(Completion::Unit(Ok(())))
    );
    let mut sim = replay(
        &BTreeSet::from([rule("127.0.0.1", 0, NetOperation::Listen)]),
        io.tape().to_vec(),
    );
    for event in io.tape() {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(event.completion.clone())
        );
    }
    sim.finish().unwrap();
}

#[test]
fn tape_v7_round_trips_every_network_event_and_scheduling_input_and_other_versions_are_refused() {
    let a4 = loopback(8080);
    let a6 = SocketAddr {
        ip: "2001:db8::1".parse().unwrap(),
        port: 443,
    };
    let errors = [
        NetError::NotGranted,
        NetError::Canceled(2),
        NetError::InvalidInput,
        NetError::UnknownHost,
        NetError::ResolverFailure,
        NetError::ConnectionRefused,
        NetError::ConnectionReset,
        NetError::ConnectionAborted,
        NetError::NotConnected,
        NetError::AddrInUse,
        NetError::AddrNotAvailable,
        NetError::HostUnreachable,
        NetError::NetworkUnreachable,
        NetError::NetworkDown,
        NetError::BrokenPipe,
        NetError::TimedOut,
        NetError::Tls(TlsFailure::UnknownIssuer),
        NetError::Tls(TlsFailure::Expired),
        NetError::Tls(TlsFailure::NotYetValid),
        NetError::Tls(TlsFailure::NameMismatch),
        NetError::Tls(TlsFailure::Revoked),
        NetError::Tls(TlsFailure::BadCertificate),
        NetError::Tls(TlsFailure::AlertReceived(40)),
        NetError::Tls(TlsFailure::HandshakeFailure),
        NetError::Tls(TlsFailure::PeerMisbehaved),
        NetError::Tls(TlsFailure::Truncated),
        NetError::Tls(TlsFailure::CertificateRequired),
        NetError::Fault(7),
    ];
    let requests = vec![
        Request::TestNet {
            test: 1,
            peers: vec![],
            listeners: vec![hirsel_run_config::NetListener {
                address: "::1".into(),
                port: 8080,
                clients: vec![hirsel_run_config::NetClient {
                    address: "192.0.2.1".into(),
                    port: 40001,
                    tls: hirsel_run_config::PeerTls::Plain,
                    steps: vec![
                        hirsel_run_config::NetStep::Send(b"GET".to_vec()),
                        hirsel_run_config::NetStep::Expect(vec![0, 0xff]),
                    ],
                }],
            }],
        },
        Request::NetNarrow {
            net: 2,
            rules: vec![
                rule("api.example.com", 443, NetOperation::ConnectTls),
                rule("::1", 0, NetOperation::Listen),
            ],
        },
        Request::NetNarrow {
            net: 2,
            rules: vec![],
        },
        Request::Resolve {
            net: 2,
            host: "example.com".into(),
            port: 80,
        },
        Request::Connect {
            net: 2,
            host: "example.com".into(),
            port: 80,
            timeout: NetTimeout::Unbounded,
        },
        Request::ConnectAddr {
            net: 2,
            addr: a6,
            timeout: NetTimeout::Nanos(1),
        },
        Request::NetRead {
            net: 2,
            stream: 3,
            max: u64::MAX,
        },
        Request::NetWrite {
            net: 2,
            stream: 3,
            bytes: vec![],
        },
        Request::NetWriteAll {
            net: 2,
            stream: 3,
            bytes: vec![0, 0xff],
        },
        Request::Shutdown {
            net: 2,
            stream: 3,
            how: ShutdownHow::Both,
        },
        Request::SetTimeout {
            net: 2,
            stream: 3,
            which: TimeoutKind::Write,
            timeout: NetTimeout::Nanos(5),
        },
        Request::PeerAddr { net: 2, stream: 3 },
        Request::TlsConnect {
            net: 2,
            stream: 3,
            server_name: "example.com".into(),
            roots: RootsDigest::WebPki("webpki-roots 1.0.9".into()),
            identity: None,
        },
        Request::TlsConnect {
            net: 2,
            stream: 3,
            server_name: "example.com".into(),
            roots: RootsDigest::Pem([0xab; 32]),
            identity: Some([0xcd; 32]),
        },
        Request::TlsAccept {
            net: 2,
            stream: 3,
            server: TlsServerDigest {
                identities: vec![[1; 32], [2; 32]],
                clients: ClientAuth::Require([3; 32]),
            },
        },
        Request::TlsAccept {
            net: 2,
            stream: 3,
            server: TlsServerDigest {
                identities: vec![],
                clients: ClientAuth::Anonymous,
            },
        },
        Request::TlsPeer { net: 2, stream: 3 },
        Request::TlsClose { net: 2, stream: 3 },
        Request::Listen { net: 2, addr: a4 },
        Request::Accept {
            net: 2,
            listener: 4,
        },
        Request::LocalAddr {
            net: 2,
            listener: 4,
        },
    ];
    let mut completions = vec![
        Completion::NetHandle(Ok(9)),
        Completion::Listened(Ok((10, a4))),
        Completion::Addresses(Ok(vec![])),
        Completion::Addresses(Ok(vec![a4, a6])),
        Completion::NetBytes(Ok(vec![])),
        Completion::NetBytes(Ok(b"\r\n".to_vec())),
        Completion::Count(Ok(3)),
        Completion::NetUnit(Ok(())),
        Completion::Certificates(Ok(None)),
        Completion::Certificates(Ok(Some(vec![0x30, 0x03, 1, 2, 3]))),
        Completion::Certificates(Err(NetError::NotConnected)),
        Completion::Address(Ok(a6)),
        Completion::Accepted(Ok((5, a4))),
    ];
    completions.extend(errors.iter().flat_map(|e| {
        [
            Completion::NetHandle(Err(*e)),
            Completion::Listened(Err(*e)),
            Completion::Addresses(Err(*e)),
            Completion::NetBytes(Err(*e)),
            Completion::Count(Err(*e)),
            Completion::NetUnit(Err(*e)),
            Completion::Address(Err(*e)),
            Completion::Accepted(Err(*e)),
        ]
    }));
    let events: Vec<Event> = requests
        .iter()
        .cycle()
        .zip(&completions)
        .map(|(request, completion)| Event {
            request: request.clone(),
            completion: completion.clone(),
            fault: None,
        })
        .collect();
    // Cooperative scheduling inputs: a parked request and a ready one.
    let mut events = events;
    events.extend([
        Event {
            request: requests[0].clone(),
            completion: Completion::Blocked { committed: false },
            fault: None,
        },
        Event {
            request: requests[1].clone(),
            completion: Completion::Blocked { committed: true },
            fault: None,
        },
        Event {
            request: Request::Ready { wait: 1, points: 7 },
            completion: Completion::Ready,
            fault: None,
        },
    ]);
    let text = render_tape(&events).unwrap();
    assert!(text.contains(" -> blocked 0\n") && text.contains("\nready 1 7 -> ready\n"));
    assert_eq!(parse_tape(&text).unwrap(), events);
    // A version 6 tape holds no scheduling inputs and reads unchanged.
    let six = text.replacen("hirsel-io-tape 7", "hirsel-io-tape 6", 1);
    assert_eq!(parse_tape(&six).unwrap(), events);

    assert!(parse_tape("hirsel-io-tape 5\n").is_err());
    assert!(parse_tape("hirsel-io-tape 8\n").is_err());
    // Exactly one spelling of each value.
    for bad in [
        "hirsel-io-tape 6\nnet_read 2 3 01 -> net_bytes ok \n",
        "hirsel-io-tape 6\nlisten 2 [0::1]:80 -> net_handle ok 3\n",
        "hirsel-io-tape 6\nnet_read 2 3 1 -> net_bytes err not_granted\n",
        "hirsel-io-tape 6\nnet_read 2 3 1 -> net_unit ok x\n",
        "hirsel-io-tape 7\nnet_read 2 3 1 -> blocked 2\n",
        "hirsel-io-tape 7\nnet_read 2 3 1 -> blocked\n",
        "hirsel-io-tape 7\nready 1 -> ready\n",
        "hirsel-io-tape 7\nready 1 x -> ready\n",
    ] {
        assert!(parse_tape(bad).is_err(), "{bad}");
    }
}

/// Ported from Rust std tcp nonblocking tests; additionally exercise RealIo's
/// logical handle allocation while a provider operation is suspended.
#[test]
fn cooperative_accept_read_timeout_and_interleaved_handles() {
    use hirsel_rt::{net_host::CooperativeNet, readiness::Readiness};
    let mut io = real([rule("127.0.0.1", 0, NetOperation::Listen)]);
    let ready = Readiness::default();
    io.set_net_provider(CooperativeNet::new(ready.clone()));
    let n = net(&mut io);
    ready.enter(1);
    let listener = handle(io.call(Request::Listen {
        net: n,
        addr: loopback(0),
    }));
    ready.finish(1);
    let address = match io.call(Request::LocalAddr { net: n, listener }).unwrap() {
        Completion::Address(Ok(a)) => a,
        other => panic!("{other:?}"),
    };
    ready.enter(2);
    let accept = Request::Accept { net: n, listener };
    assert_eq!(io.call(accept.clone()), Err(AdapterError::Blocked));
    ready.parked(2);
    assert!(ready.ready().unwrap().is_empty());
    // No identity was published by the blocked accept. An unrelated projection
    // can allocate the next identity before this accept completes.
    let other_net = net(&mut io);
    let mut peer = std::net::TcpStream::connect(address.std()).unwrap();
    assert!(ready.wait().unwrap());
    let stream = match io.call(accept).unwrap() {
        Completion::Accepted(Ok((id, _))) => id,
        other => panic!("{other:?}"),
    };
    assert_ne!(stream, other_net);
    ready.finish(2);
    ready.enter(3);
    let read = Request::NetRead {
        net: n,
        stream,
        max: 4,
    };
    assert_eq!(io.call(read.clone()), Err(AdapterError::Blocked));
    ready.parked(3);
    assert!(ready.ready().unwrap().is_empty());
    peer.write_all(b"ping").unwrap();
    assert!(ready.wait().unwrap());
    assert_eq!(
        io.call(read.clone()),
        Ok(Completion::NetBytes(Ok(b"ping".to_vec())))
    );
    ready.finish(3);
    assert_eq!(
        io.call(Request::SetTimeout {
            net: n,
            stream,
            which: TimeoutKind::Read,
            timeout: NetTimeout::Nanos(1)
        }),
        Ok(Completion::NetUnit(Ok(())))
    );
    ready.enter(4);
    let mut result = io.call(read.clone());
    if result == Err(AdapterError::Blocked) {
        assert!(ready.wait().unwrap());
        result = io.call(read);
    }
    assert_eq!(result, Ok(Completion::NetBytes(Err(NetError::TimedOut))));
    ready.finish(4);
    assert!(!ready.wait().unwrap());
    io.call(Request::Close { handle: stream }).unwrap();
    io.call(Request::Close { handle: listener }).unwrap();
}

#[test]
fn cooperative_connect_retains_socket_across_handle_allocation() {
    use hirsel_rt::{net_host::CooperativeNet, readiness::Readiness};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = SocketAddr::of(listener.local_addr().unwrap());
    let mut io = real([rule("127.0.0.1", addr.port, NetOperation::Connect)]);
    let ready = Readiness::default();
    io.set_net_provider(CooperativeNet::new(ready.clone()));
    let n = net(&mut io);
    ready.enter(1);
    let request = Request::ConnectAddr {
        net: n,
        addr,
        timeout: NetTimeout::Nanos(5_000_000_000),
    };
    let mut result = io.call(request.clone());
    let other_net = net(&mut io);
    if result == Err(AdapterError::Blocked) {
        assert!(
            !ready.cancellable(1),
            "connect has already crossed its packet boundary"
        );
        assert!(ready.wait().unwrap());
        result = io.call(request);
    }
    let stream = handle(result);
    assert_ne!(stream, other_net);
    ready.finish(1);
    let (mut peer, _) = listener.accept().unwrap();
    assert_eq!(
        io.call(Request::NetWriteAll {
            net: n,
            stream,
            bytes: b"ok".to_vec()
        }),
        Ok(Completion::NetUnit(Ok(())))
    );
    let mut bytes = [0; 2];
    peer.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"ok");
    io.call(Request::Close { handle: stream }).unwrap();
}

/// Host UDP syscalls and tape validation cannot be exercised by a corpus script.
#[test]
fn udp_host_datagrams_discard_outside_senders_and_replay_without_sockets() {
    use std::net::UdpSocket;
    let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
    peer.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let peer_addr = SocketAddr::of(peer.local_addr().unwrap());
    let outside = UdpSocket::bind("127.0.0.1:0").unwrap();
    let rules = BTreeSet::from([
        rule("127.0.0.1", 0, NetOperation::UdpBind),
        rule("127.0.0.1", peer_addr.port, NetOperation::UdpPeer),
    ]);
    let mut io = real(rules.clone());
    let n = net(&mut io);
    let s = handle(io.call(Request::UdpBind {
        net: n,
        addr: loopback(0),
    }));
    assert_eq!(
        io.call(Request::UdpSend {
            net: n,
            socket: s,
            bytes: b"query".to_vec(),
            addr: peer_addr
        }),
        Ok(Completion::Count(Ok(5)))
    );
    let mut received = [0; 32];
    let (count, local) = peer.recv_from(&mut received).unwrap();
    assert_eq!(&received[..count], b"query");
    outside.send_to(b"discard me", local).unwrap();
    peer.send_to(b"abcdef", local).unwrap();
    peer.send_to(b"", local).unwrap();
    peer.send_to(b"next", local).unwrap();
    for (max, bytes, truncated) in [
        (3, b"abc".as_slice(), true),
        (8, b"".as_slice(), false),
        (8, b"next".as_slice(), false),
    ] {
        assert_eq!(
            io.call(Request::UdpRecv {
                net: n,
                socket: s,
                max
            }),
            Ok(Completion::Datagram(Ok((
                bytes.to_vec(),
                peer_addr,
                truncated
            ))))
        );
    }
    assert_eq!(
        io.call(Request::Close { handle: s }),
        Ok(Completion::Unit(Ok(())))
    );
    let tape = io.tape().to_vec();
    assert_eq!(
        tape.iter()
            .filter(|e| matches!(e.completion, Completion::Datagram(_)))
            .count(),
        3
    );
    let text = render_tape(&tape).unwrap();
    assert!(!text.contains(&hirsel_run_config::hex(b"discard me")));
    assert_eq!(parse_tape(&text).unwrap(), tape);
    drop(io);
    drop(peer);
    drop(outside);
    let mut sim = replay(&rules, parse_tape(&text).unwrap());
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(event.completion.clone())
        );
    }
    sim.finish().unwrap();
    // A tape cannot claim input from a source the bound Net does not authorize.
    let mut forged = tape.clone();
    let event = forged
        .iter_mut()
        .find(|e| matches!(e.completion, Completion::Datagram(_)))
        .unwrap();
    let Completion::Datagram(Ok((_, peer, _))) = &mut event.completion else {
        unreachable!()
    };
    peer.port = if peer.port == 1 { 2 } else { 1 };
    let mut sim = replay(&rules, forged);
    for event in &tape[..3] {
        sim.call(event.request.clone()).unwrap();
    }
    assert!(matches!(
        sim.call(tape[3].request.clone()),
        Err(AdapterError::InvalidCompletion)
    ));
}

/// A fresh directory for socket files, short enough for `sun_path`.
fn socket_dir(name: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(format!("/tmp/hirsel-unix-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    dir
}

/// Connect, write, accept and read over a real Unix socket through the host provider (Rust
/// std os/unix/net/tests.rs `basic`): the listener and its peers are unnamed, and closing
/// the listener removes its socket file.
fn unix_exchange(io: &mut impl Adapter, path: &str) -> Vec<u8> {
    let n = net(io);
    let (l, local) = match io.call(Request::UnixListen {
        net: n,
        path: path.into(),
    }) {
        Ok(Completion::Listened(Ok(listened))) => listened,
        other => panic!("unix listen: {other:?}"),
    };
    assert_eq!(local, SocketAddr::UNNAMED);
    let c = handle(io.call(Request::UnixConnect {
        net: n,
        path: path.into(),
    }));
    let (s, peer) = match io.call(Request::Accept {
        net: n,
        listener: l,
    }) {
        Ok(Completion::Accepted(Ok(accepted))) => accepted,
        other => panic!("accept: {other:?}"),
    };
    assert_eq!(peer, SocketAddr::UNNAMED);
    assert_eq!(
        io.call(Request::PeerAddr { net: n, stream: c }),
        Ok(Completion::Address(Ok(SocketAddr::UNNAMED)))
    );
    // Rust std os/unix/net/tests.rs test_read_timeout: an idle Unix stream times out
    // and stays usable.
    assert_eq!(
        io.call(Request::SetTimeout {
            net: n,
            stream: c,
            which: TimeoutKind::Read,
            timeout: NetTimeout::Nanos(1_000_000),
        }),
        Ok(Completion::NetUnit(Ok(())))
    );
    assert_eq!(
        io.call(Request::NetRead {
            net: n,
            stream: c,
            max: 10,
        }),
        Ok(Completion::NetBytes(Err(NetError::TimedOut)))
    );
    assert_eq!(
        io.call(Request::NetWriteAll {
            net: n,
            stream: c,
            bytes: b"over a socket file".to_vec(),
        }),
        Ok(Completion::NetUnit(Ok(())))
    );
    for handle in [c, l] {
        assert_eq!(
            io.call(Request::Close { handle }),
            Ok(Completion::Unit(Ok(())))
        );
    }
    let chunks = read_to_end(io, n, s, 64);
    assert_eq!(
        io.call(Request::Close { handle: s }),
        Ok(Completion::Unit(Ok(())))
    );
    chunks.concat()
}

#[test]
fn unix_streams_connect_accept_and_replay_without_socket_files() {
    let dir = socket_dir("exchange");
    let path = dir.join("app.sock").to_str().unwrap().to_owned();
    let rules = BTreeSet::from([
        rule(&path, 0, NetOperation::UnixConnect),
        rule(&path, 0, NetOperation::UnixListen),
    ]);
    let mut io = real(rules.clone());
    assert_eq!(unix_exchange(&mut io, &path), b"over a socket file");
    // The listener's close removed the file it bound.
    assert!(!std::path::Path::new(&path).exists());
    let tape = io.tape().to_vec();
    let parsed = parse_tape(&render_tape(&tape).unwrap()).unwrap();
    assert_eq!(parsed, tape);
    let mut sim = replay(&rules, parsed);
    assert_eq!(unix_exchange(&mut sim, &path), b"over a socket file");
    sim.finish().unwrap();
    // Replay binds nothing.
    assert!(!std::path::Path::new(&path).exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unix_listen_removes_only_a_stale_socket_and_only_under_its_rule() {
    let dir = socket_dir("stale");
    let at = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    let (stale, kept, live, file, moved, missing) = (
        at("stale.sock"),
        at("kept.sock"),
        at("live.sock"),
        at("file.sock"),
        at("moved.sock"),
        at("none/x.sock"),
    );
    let mut rules = BTreeSet::from([
        rule(&kept, 0, NetOperation::UnixListen),
        rule(&moved, 0, NetOperation::UnixListen),
        rule(&missing, 0, NetOperation::UnixConnect),
    ]);
    for path in [&stale, &live, &file, &missing] {
        rules.insert(rule(path, 0, NetOperation::UnixListenRemoveStale));
    }
    // Rust std leaves a dropped listener's socket file behind: a stale socket.
    for path in [&stale, &kept] {
        drop(std::os::unix::net::UnixListener::bind(path).unwrap());
    }
    let alive = std::os::unix::net::UnixListener::bind(&live).unwrap();
    std::fs::write(&file, b"not a socket").unwrap();
    let mut io = real(rules);
    let n = net(&mut io);
    let listen = |io: &mut RealIo, path: &str| match io.call(Request::UnixListen {
        net: n,
        path: path.into(),
    }) {
        Ok(Completion::Listened(result)) => result.map(|(id, _)| id),
        other => panic!("unix listen: {other:?}"),
    };
    let reclaimed = listen(&mut io, &stale).unwrap();
    assert_eq!(listen(&mut io, &kept), Err(NetError::AddrInUse));
    assert_eq!(listen(&mut io, &live), Err(NetError::AddrInUse));
    assert_eq!(listen(&mut io, &file), Err(NetError::AddrInUse));
    assert_eq!(std::fs::read(&file).unwrap(), b"not a socket");
    assert_eq!(listen(&mut io, &missing), Err(NetError::AddrNotAvailable));
    assert_eq!(
        io.call(Request::UnixConnect {
            net: n,
            path: missing.clone(),
        }),
        Ok(Completion::NetHandle(Err(NetError::ConnectionRefused)))
    );
    // A listener whose path now names another file leaves that file alone.
    std::fs::remove_file(&kept).unwrap();
    let replaced = listen(&mut io, &moved).unwrap();
    std::fs::rename(&moved, dir.join("elsewhere.sock")).unwrap();
    std::fs::write(&moved, b"newcomer").unwrap();
    for handle in [reclaimed, replaced] {
        assert_eq!(
            io.call(Request::Close { handle }),
            Ok(Completion::Unit(Ok(())))
        );
    }
    assert!(!std::path::Path::new(&stale).exists());
    assert_eq!(std::fs::read(&moved).unwrap(), b"newcomer");
    assert!(std::path::Path::new(&live).exists());
    drop(alive);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A cooperative Unix connect facing a full backlog has queued nothing, so it parks on a
/// timer, stays cancellable and connects once an accept makes room. A listen's stale probe
/// of that live path refuses at once instead of waiting for room.
#[test]
fn cooperative_unix_connect_parks_on_a_full_backlog_and_the_stale_probe_never_waits() {
    use hirsel_rt::{net_host::CooperativeNet, readiness::Readiness};
    use std::os::fd::AsRawFd;
    let dir = socket_dir("backlog");
    let path = dir.join("app.sock").to_str().unwrap().to_owned();
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    // SAFETY: listener owns its descriptor; listening again sets the smallest backlog.
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
    let mut io = real([
        rule(&path, 0, NetOperation::UnixConnect),
        rule(&path, 0, NetOperation::UnixListenRemoveStale),
    ]);
    let ready = Readiness::default();
    io.set_net_provider(CooperativeNet::new(ready.clone()));
    let n = net(&mut io);
    let connect = Request::UnixConnect {
        net: n,
        path: path.clone(),
    };
    let mut held = vec![];
    let waiting = loop {
        let token = held.len() as u64 + 1;
        assert!(token < 100, "the backlog never filled");
        ready.enter(token);
        match io.call(connect.clone()) {
            Err(AdapterError::Blocked) => break token,
            result => held.push(handle(result)),
        }
        ready.finish(token);
    };
    assert!(!held.is_empty());
    assert!(ready.cancellable(waiting));
    ready.parked(waiting);
    assert!(ready.ready().unwrap().is_empty());
    ready.enter(waiting + 1);
    assert_eq!(
        io.call(Request::UnixListen {
            net: n,
            path: path.clone(),
        }),
        Ok(Completion::Listened(Err(NetError::AddrInUse)))
    );
    ready.finish(waiting + 1);
    let accepted = listener.accept().unwrap();
    while !ready.ready().unwrap().contains(&waiting) {
        assert!(ready.wait().unwrap());
    }
    ready.enter(waiting);
    let connected = handle(io.call(connect));
    ready.finish(waiting);
    for handle in held.into_iter().chain([connected]) {
        io.call(Request::Close { handle }).unwrap();
    }
    drop((accepted, listener));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unix_paths_are_checked_before_grants_and_grants_before_any_effect() {
    let calls = Rc::new(Cell::new(0));
    let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
    io.set_net_rules(BTreeSet::from([
        rule("/run/granted.sock", 0, NetOperation::UnixConnect),
        rule("127.0.0.1", 0, NetOperation::Listen),
    ]));
    io.set_net_provider(Counting {
        inner: HostNet::new(),
        calls: calls.clone(),
    });
    let n = net(&mut io);
    let error = |c: Result<Completion, AdapterError>| match c {
        Ok(Completion::NetHandle(Err(e)) | Completion::Listened(Err(e))) => e,
        other => panic!("expected an error: {other:?}"),
    };
    for path in [
        "run/granted.sock",
        "/run//granted.sock",
        "/run/./granted.sock",
        "",
    ] {
        let c = io.call(Request::UnixConnect {
            net: n,
            path: path.into(),
        });
        assert_eq!(error(c), NetError::InvalidInput, "{path:?}");
    }
    let long = format!("/{}", "s".repeat(107));
    let c = io.call(Request::UnixListen { net: n, path: long });
    assert_eq!(error(c), NetError::InvalidInput);
    // A UnixConnect rule never authorizes listening, nor a TCP rule a socket path.
    let c = io.call(Request::UnixListen {
        net: n,
        path: "/run/granted.sock".into(),
    });
    assert_eq!(error(c), NetError::NotGranted);
    let c = io.call(Request::UnixConnect {
        net: n,
        path: "/run/other.sock".into(),
    });
    assert_eq!(error(c), NetError::NotGranted);
    assert_eq!(calls.get(), 0);
}
