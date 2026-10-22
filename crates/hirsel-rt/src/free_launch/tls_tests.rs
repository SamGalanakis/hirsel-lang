//! Reuse the rustls peer and public fixtures from tests/support/tls_peer.rs:11-61.
//! The change here is ordinary configured Free admission through exported TLS ABI.
use super::*;
use crate::host::{Host, Selection, Sink, Sinks};
use crate::io::GrantResult;
use crate::net_abi::*;
use crate::program::{Code, Program};
use hirsel_run_config::{ProgramFacts, Root, RunConfig};
use hirsel_run_schema::{
    CodeReach, IoGrants, LaunchKind, NetGrant, NetRule, RecordingLevel, Returned,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};
const CA: &[u8] = include_bytes!("../../tests/it/tls-data/ca.cert");
static PORT: AtomicU16 = AtomicU16::new(0);

fn server() -> (u16, std::thread::JoinHandle<()>) {
    let certs =
        CertificateDer::pem_slice_iter(include_bytes!("../../tests/it/tls-data/end.fullchain"))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
    let key =
        PrivateKeyDer::from_pem_slice(include_bytes!("../../tests/it/tls-data/end.key")).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let thread = std::thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(5);
        let socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < until, "TLS connection not issued");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => panic!("{e}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let conn = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        let mut tls = rustls::StreamOwned::new(conn, socket);
        let mut bytes = [0; 4];
        tls.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ping");
        tls.write_all(b"pong").unwrap();
        tls.flush().unwrap();
        tls.conn.send_close_notify();
        tls.flush().unwrap();
        assert_eq!(tls.read(&mut [0]).unwrap(), 0);
    });
    (port, thread)
}
unsafe fn release(ctx: *mut crate::TaskCtx, text: crate::views::Slice) {
    unsafe {
        crate::release::hirsel_rt_v2_release(
            ctx,
            text.owner,
            (*text.owner).index(),
            std::ptr::null(),
            0,
            0,
        );
    }
}
unsafe extern "C" fn entry(
    ctx: *mut crate::TaskCtx,
    _: *mut u8,
    out: *mut u8,
    io: *const u64,
    _: *const u64,
) -> u8 {
    unsafe {
        let mut net = GrantResult::default();
        crate::io::hirsel_rt_v2_io_net(ctx, std::ptr::null_mut(), &mut net, io);
        assert_eq!(net.tag, 0);
        let address = SocketAddrAbi::of(crate::net::SocketAddr::of(
            ([127, 0, 0, 1], PORT.load(Ordering::Relaxed)).into(),
        ));
        let mut connected = NetWordResult::default();
        hirsel_rt_v2_net_connect_addr(
            ctx,
            std::ptr::null_mut(),
            &mut connected,
            &net.value,
            &address,
            &NetTimeoutAbi {
                tag: 1,
                padding: 0,
                nanos: 3_000_000_000,
            },
        );
        assert_eq!(connected.tag, 0);
        let name = crate::text::allocate_bytes(ctx, b"localhost");
        let pem = crate::text::allocate_bytes(ctx, CA);
        let mut stream = connected.value;
        let mut secure = NetWordResult::default();
        assert_eq!(
            crate::tls_abi::hirsel_rt_v2_tls_connect(
                ctx,
                std::ptr::null_mut(),
                &mut secure,
                &net.value,
                &mut stream,
                name.owner,
                name.start,
                name.length,
                &crate::tls_abi::TlsRootsAbi { pem }
            ),
            crate::STATUS_OK
        );
        assert_eq!(secure.tag, 0, "{secure:?}");
        release(ctx, name);
        release(ctx, pem);
        let mut stream = secure.value;
        let text = crate::text::allocate_bytes(ctx, b"ping");
        let mut written = NetUnitResult::default();
        hirsel_rt_v2_net_write_all(
            ctx,
            std::ptr::null_mut(),
            &mut written,
            &net.value,
            &mut stream,
            text.owner,
            text.start,
            text.length,
        );
        assert_eq!(written.tag, 0);
        release(ctx, text);
        let mut data = [0; 4];
        let mut used = 0;
        while used < data.len() {
            let mut result = NetWordResult::default();
            hirsel_rt_v2_net_read_into(
                ctx,
                std::ptr::null_mut(),
                &mut result,
                &net.value,
                &mut stream,
                data.as_mut_ptr().add(used),
                (data.len() - used) as u64,
            );
            assert_eq!(result.tag, 0);
            assert!(result.value > 0);
            used += result.value as usize;
        }
        assert_eq!(&data, b"pong");
        let mut closed = NetUnitResult::default();
        crate::tls_abi::hirsel_rt_v2_tls_close(
            ctx,
            std::ptr::null_mut(),
            &mut closed,
            &net.value,
            &mut stream,
        );
        assert_eq!(closed.tag, 0);
        out.cast::<u32>().write(0);
        crate::STATUS_OK
    }
}
fn sinks() -> Sinks {
    Sinks {
        stdout: Sink::capture().0,
        stderr: Sink::capture().0,
        report: Sink::capture().0,
    }
}
fn facts() -> ProgramFacts {
    ProgramFacts {
        program: [3; 32],
        entry: "free_tls@1/main".into(),
        launch: LaunchKind::Main,
        roots: vec![Root::Io, Root::Args],
        code: CodeReach::Safe,
        fueled: false,
    }
}
#[test]
fn ordinary_free_tls_routes_pem_material_to_concurrent_host_and_replays_without_a_server() {
    let dir = tempfile::tempdir().unwrap();
    for width in [1, 8] {
        for level in [
            RecordingLevel::None,
            RecordingLevel::Record,
            RecordingLevel::Durable,
        ] {
            let (port, server) = server();
            PORT.store(port, Ordering::Relaxed);
            let mut record = RunConfig::empty([3; 32], facts().entry, LaunchKind::Main);
            record.args = Some(vec![]);
            record.io = Some(IoGrants {
                tasks: Some("tasks.free@1".into()),
                net: Some(NetGrant {
                    provider: "net.stream@1".into(),
                    rules: vec![NetRule {
                        address: "127.0.0.1".into(),
                        service: port.to_string(),
                        operation: "connect".into(),
                    }],
                    peers: None,
                    listeners: vec![],
                }),
                ..Default::default()
            });
            record.recording = Some(level);
            record.placement = Some(
                hirsel_run_config::Placement::from_json(
                    format!("{{\"cores\":{width}}}").as_bytes(),
                )
                .unwrap(),
            );
            let record = record.validated().unwrap();
            let config = crate::launch::Config {
                run_identity: record.identity(),
                hash_seed: 0,
                random_seed: 0,
                test_seed: 0,
            };
            let mut program = Program::harness(Code::Main(entry), 4, std::ptr::null(), 0, config);
            program.digest = [3; 32];
            program.entry_name = facts().entry;
            let host = Host::with_clock(|| Some(0));
            let output = sinks();
            let (admitted, io) = host
                .admit(&record.clone().bind(&facts()).unwrap(), None, &output)
                .unwrap();
            let path = dir.path().join(format!("{width}-{level:?}.tape"));
            assert_eq!(
                host.start(
                    &program,
                    config,
                    Selection::Live {
                        admitted,
                        io,
                        tape: (level != RecordingLevel::None).then(|| path.clone())
                    },
                    vec![],
                    output
                )
                .wait(),
                RunOutcome::Returned(Returned::Ok)
            );
            server.join().unwrap();
            if level == RecordingLevel::None {
                continue;
            }
            for replay_width in [1, 8] {
                let (io, run) = crate::host_launch::replay(&path).unwrap();
                let handle = Host::with_clock(|| panic!("replay clock")).start(
                    &program,
                    config,
                    Selection::Replay {
                        run: run.bind(&facts()).unwrap(),
                        io,
                    },
                    vec![],
                    sinks(),
                );
                handle.control().resize(replay_width).unwrap();
                assert_eq!(handle.wait(), RunOutcome::Returned(Returned::Ok));
            }
        }
    }
}
