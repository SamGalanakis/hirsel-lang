//! Network grants, the host provider, tape v4 and exact replay:
//! record against an in-process loopback server, replay with no sockets, and refuse an
//! edited, truncated or extended tape. Denials are checked against a provider call counter.
use hirsel_rt::io::*;
use hirsel_rt::io_real::RealIo;
use hirsel_rt::io_sim::SimIo;
use hirsel_rt::launch::{parse_tape, render_tape};
use hirsel_rt::net::Provided;
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
        Ok(Completion::NetHandle(Ok(id))) => id,
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
    assert!(text.starts_with("hirsel-io-tape 5\n"));
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
    fn tls_connect(&mut self, s: u64, name: &str, roots: &RootsDigest) -> Provided<()> {
        self.tick();
        self.inner.tls_connect(s, name, roots)
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
        roots: RootsDigest::Pem([0; 32]),
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
        io.call(Request::Close { handle: l }),
        Ok(Completion::Unit(Ok(())))
    );
}

#[test]
fn tape_v4_round_trips_every_network_event_and_other_versions_are_refused() {
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
        NetError::Fault(7),
    ];
    let requests = vec![
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
        },
        Request::TlsConnect {
            net: 2,
            stream: 3,
            server_name: "example.com".into(),
            roots: RootsDigest::Pem([0xab; 32]),
        },
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
        Completion::Addresses(Ok(vec![])),
        Completion::Addresses(Ok(vec![a4, a6])),
        Completion::NetBytes(Ok(vec![])),
        Completion::NetBytes(Ok(b"\r\n".to_vec())),
        Completion::Count(Ok(3)),
        Completion::NetUnit(Ok(())),
        Completion::Address(Ok(a6)),
        Completion::Accepted(Ok((5, a4))),
    ];
    completions.extend(errors.iter().flat_map(|e| {
        [
            Completion::NetHandle(Err(*e)),
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
    let text = render_tape(&events).unwrap();
    assert_eq!(parse_tape(&text).unwrap(), events);

    assert!(parse_tape("hirsel-io-tape 4\n").is_err());
    assert!(parse_tape("hirsel-io-tape 6\n").is_err());
    // Exactly one spelling of each value.
    for bad in [
        "hirsel-io-tape 5\nnet_read 2 3 01 -> net_bytes ok \n",
        "hirsel-io-tape 5\nlisten 2 [0::1]:80 -> net_handle ok 3\n",
        "hirsel-io-tape 5\nnet_read 2 3 1 -> net_bytes err not_granted\n",
        "hirsel-io-tape 5\nnet_read 2 3 1 -> net_unit ok x\n",
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
    assert!(!ready.ready(2).unwrap());
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
    assert!(!ready.ready(3).unwrap());
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
