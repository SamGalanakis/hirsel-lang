//! hirsel-rt's TLS tests whose recorded tapes the interpreter replays (`rt_replay`), beside
//! the rest of them in `crates/hirsel-rt/tests/it/tls.rs`.
use crate::rt_support::tls_peer::*;
use hirsel_rt::io::*;
use hirsel_rt::io_real::RealIo;
use hirsel_rt::net_tls::WEBPKI_VERSION;
/// Replay a host tape in the interpreter from scripted peers derived from
/// `fixture` alone, comparing it with `tape`.
fn replay(tape: &[Event], fixture: &RealIo) {
    use crate::rt_replay::{ReplayFixture, replay_interpreter_with_fixture};
    let tree = Tree::new(Default::default()).unwrap();
    replay_interpreter_with_fixture(
        tape,
        ReplayFixture {
            events: fixture.tape(),
            pem_roots: &[PEM],
            tls_identities: &[],
        },
        &tree,
        &tree,
        &fixture.handles().observed(),
        b"",
        &Default::default(),
    );
}

fn refuses(tape: &[Event], fixture: &RealIo) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| replay(tape, fixture))).is_err()
}

#[test]
fn a_host_unknown_issuer_tape_replays_independently_in_the_interpreter() {
    let (port, thread) = server(false, false);
    let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
    crate::rt_support::host(&mut io);
    io.set_net_rules(rules(port));
    let Ok(Completion::Handle(Ok(net))) = io.call(Request::Net { io: 1 }) else {
        panic!("net")
    };
    let Ok(Completion::NetHandle(Ok(stream))) = io.call(Request::ConnectAddr {
        net,
        addr: SocketAddr {
            ip: std::net::Ipv4Addr::LOCALHOST.into(),
            port,
        },
        timeout: NetTimeout::Nanos(5_000_000_000),
    }) else {
        panic!("connect")
    };
    assert_eq!((net, stream), (2, 3));
    assert_eq!(
        io.call(Request::TlsConnect {
            net,
            stream,
            server_name: "localhost".into(),
            roots: RootsDigest::WebPki(WEBPKI_VERSION.into()),
            identity: None,
            alpn: vec![],
        }),
        Ok(Completion::Protocol(Err(NetError::Tls(
            TlsFailure::UnknownIssuer
        ))))
    );
    thread.join().unwrap();
    replay(io.tape(), &io);
    // Altered observations refuse against the original fixture.
    for altered in [
        TlsFailure::Expired,
        TlsFailure::NameMismatch,
        TlsFailure::AlertReceived(48),
    ] {
        let mut tape = io.tape().to_vec();
        tape.last_mut().unwrap().completion = Completion::Protocol(Err(NetError::Tls(altered)));
        assert!(refuses(&tape, &io), "{altered:?}");
    }
}

#[test]
fn host_success_and_truncated_tapes_replay_independently_in_the_interpreter() {
    for (truncated, tls12) in [(false, false), (true, false), (false, true), (true, true)] {
        let (port, thread) = server(truncated, tls12);
        let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        crate::rt_support::host(&mut io);
        io.set_net_rules(rules(port));
        exchange(&mut io, port, truncated);
        thread.join().unwrap();
        replay(io.tape(), &io);
        // One altered byte of the delivered plaintext refuses.
        let mut tape = io.tape().to_vec();
        let read = tape
            .iter_mut()
            .find_map(|e| match &mut e.completion {
                Completion::NetBytes(Ok(bytes)) if !bytes.is_empty() => Some(bytes),
                _ => None,
            })
            .unwrap();
        read[0] ^= 1;
        assert!(refuses(&tape, &io), "truncated {truncated}");
        // A truncation recorded as a clean end of stream refuses, and vice versa.
        let mut tape = io.tape().to_vec();
        let end = tape
            .iter_mut()
            .filter(|e| matches!(e.request, Request::NetRead { .. }))
            .last()
            .unwrap();
        end.completion = Completion::NetBytes(if truncated {
            Ok(vec![])
        } else {
            Err(NetError::Tls(TlsFailure::Truncated))
        });
        assert!(refuses(&tape, &io), "truncated {truncated}");
    }
}
