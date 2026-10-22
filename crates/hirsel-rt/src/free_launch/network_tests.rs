use super::*;
use crate::host::{Host, Selection, Sink, Sinks};
use crate::io::{GrantResult, Request};
use crate::net_abi::*;
use crate::program::{Code, Program};
use hirsel_run_config::{ProgramFacts, Root, RunConfig};
use hirsel_run_schema::{
    CodeReach, IoGrants, LaunchKind, NetGrant, NetRule, RecordingLevel, Returned,
};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

static PEER: AtomicU16 = AtomicU16::new(0);

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
    roots: *mut u64,
) -> u8 {
    unsafe {
        let io = roots;
        let env = roots.add(2);
        let name = crate::text::allocate_bytes(ctx, b"MODE");
        let mut value = std::mem::MaybeUninit::<crate::io::EnvResult>::uninit();
        assert_eq!(
            crate::io::hirsel_rt_v2_env_get(
                ctx,
                std::ptr::null_mut(),
                value.as_mut_ptr(),
                env,
                name.owner,
                name.start,
                name.length
            ),
            crate::STATUS_OK
        );
        let value = value.assume_init();
        assert_eq!(value.tag, 0);
        assert_eq!(
            crate::io::slice_bytes(value.value.owner, value.value.start, value.value.length),
            b"network"
        );
        release(ctx, value.value);
        release(ctx, name);
        let mut first = GrantResult::default();
        let mut alias = GrantResult::default();
        crate::io::hirsel_rt_v2_io_net(ctx, std::ptr::null_mut(), &mut first, io);
        crate::io::hirsel_rt_v2_io_net(ctx, std::ptr::null_mut(), &mut alias, io);
        assert_eq!((first.tag, alias.tag), (0, 0));
        assert_ne!(first.value, alias.value);
        // A narrowed Env is a creation, rather than an invented initial root.
        {
            let _role = crate::tasks::role::Guard::new(ctx);
            let narrowed = crate::io::capability_call(
                ctx,
                Request::EnvNarrow {
                    env: *env,
                    names: vec!["MODE".into()],
                },
            )
            .unwrap();
            let crate::io::Completion::Handle(Ok(narrowed)) = narrowed else {
                panic!("Env narrow")
            };
            let observed = crate::io::capability_call(
                ctx,
                Request::EnvGet {
                    env: narrowed,
                    name: "MODE".into(),
                },
            )
            .unwrap();
            assert_eq!(
                observed,
                crate::io::Completion::Env(Ok(Some("network".into())))
            );
        }
        let addr = SocketAddrAbi::of(crate::net::SocketAddr::of(
            ([127, 0, 0, 1], PEER.load(Ordering::Relaxed)).into(),
        ));
        let mut connected = NetWordResult::default();
        hirsel_rt_v2_net_connect_addr(
            ctx,
            std::ptr::null_mut(),
            &mut connected,
            &first.value,
            &addr,
            &NetTimeoutAbi {
                tag: 1,
                padding: 0,
                nanos: 3_000_000_000,
            },
        );
        assert_eq!(connected.tag, 0, "{connected:?}");
        let mut stream = connected.value;
        let mut halves = [0; 2];
        assert_eq!(
            hirsel_rt_v2_net_split(
                ctx,
                std::ptr::null_mut(),
                &mut halves,
                &first.value,
                &mut stream
            ),
            crate::STATUS_OK
        );
        let text = crate::text::allocate_bytes(ctx, b"ping");
        let mut written = NetUnitResult::default();
        hirsel_rt_v2_net_write_all(
            ctx,
            std::ptr::null_mut(),
            &mut written,
            &first.value,
            &mut halves[1],
            text.owner,
            text.start,
            text.length,
        );
        assert_eq!(written.tag, 0);
        release(ctx, text);
        let mut bytes = [0xa5; 8];
        let mut count = NetWordResult::default();
        hirsel_rt_v2_net_read_into(
            ctx,
            std::ptr::null_mut(),
            &mut count,
            &first.value,
            &mut halves[0],
            bytes.as_mut_ptr(),
            8,
        );
        assert_eq!(count.tag, 0);
        assert_eq!(count.value, 4);
        assert_eq!(&bytes[..4], b"pong");
        assert_eq!(&bytes[4..], &[0xa5; 4]);
        let mut joined = NetWordResult::default();
        let [mut read, mut write] = halves;
        hirsel_rt_v2_net_join(
            ctx,
            std::ptr::null_mut(),
            &mut joined,
            &first.value,
            &mut read,
            &mut write,
        );
        assert_eq!(joined.tag, 0);
        crate::io::hirsel_rt_v2_io_close(ctx, joined.value, 0, 0);
        let mut listened = NetWordResult::default();
        let address = SocketAddrAbi::of(crate::net::SocketAddr::of(([127, 0, 0, 1], 0).into()));
        hirsel_rt_v2_net_listen(
            ctx,
            std::ptr::null_mut(),
            &mut listened,
            &alias.value,
            &address,
        );
        assert_eq!(listened.tag, 0);
        let mut local = NetAddrResult::default();
        hirsel_rt_v2_net_local_addr(
            ctx,
            std::ptr::null_mut(),
            &mut local,
            &alias.value,
            &listened.value,
        );
        assert_eq!(local.tag, 0);
        assert_ne!(local.value.port, 0);
        crate::io::hirsel_rt_v2_io_close(ctx, listened.value, 0, 0);
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
        program: [2; 32],
        entry: "free_network@1/main".into(),
        launch: LaunchKind::Main,
        roots: vec![Root::Io, Root::Env],
        code: CodeReach::Safe,
        fueled: false,
    }
}

#[test]
fn ordinary_free_network_and_initial_env_record_and_replay_without_live_peers() {
    let dir = tempfile::tempdir().unwrap();
    for width in [1, 8] {
        for level in [
            RecordingLevel::None,
            RecordingLevel::Record,
            RecordingLevel::Durable,
        ] {
            let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            PEER.store(port, Ordering::Relaxed);
            listener.set_nonblocking(true).unwrap();
            let server = std::thread::spawn(move || {
                let until = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < until, "native connect never reached peer");
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = [0; 4];
                stream.read_exact(&mut bytes).unwrap();
                assert_eq!(&bytes, b"ping");
                stream.write_all(b"pong").unwrap();
            });
            let mut record = RunConfig::empty([2; 32], facts().entry, LaunchKind::Main);
            record.env = Some(std::collections::BTreeMap::from([(
                "MODE".into(),
                Some("network".into()),
            )]));
            record.io = Some(IoGrants {
                tasks: Some("tasks.free@1".into()),
                net: Some(NetGrant {
                    provider: "net.stream@1".into(),
                    rules: vec![
                        NetRule {
                            address: "127.0.0.1".into(),
                            service: port.to_string(),
                            operation: "connect".into(),
                        },
                        NetRule {
                            address: "127.0.0.1".into(),
                            service: "0".into(),
                            operation: "listen".into(),
                        },
                    ],
                    peers: None,
                    listeners: vec![],
                }),
                ..Default::default()
            });
            record.placement = Some(
                hirsel_run_config::Placement::from_json(
                    format!("{{\"cores\":{width}}}").as_bytes(),
                )
                .unwrap(),
            );
            record.recording = Some(level);
            let record = record.validated().unwrap();
            let config = crate::launch::Config {
                run_identity: record.identity(),
                hash_seed: 0,
                random_seed: 0,
                test_seed: 0,
            };
            let mut program = Program::harness(
                Code::Roots(
                    entry,
                    crate::standalone::ROOT_IO | crate::standalone::ROOT_ENV,
                ),
                4,
                std::ptr::null(),
                0,
                config,
            );
            program.digest = [2; 32];
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
            server.join().unwrap(); // no live peer exists for either replay
            if level == RecordingLevel::None {
                assert!(!path.exists());
                continue;
            }
            let tape = hirsel_free::tape::parse(&std::fs::read(&path).unwrap()).unwrap();
            let creations = tape
                .logs
                .values()
                .flat_map(|bytes| hirsel_free::record::decode(bytes).unwrap())
                .filter_map(|r| match r {
                    hirsel_free::record::Record::Completion { bytes, .. } => {
                        Some(crate::tasks::free::host::native::test_net_creations(&bytes))
                    }
                    _ => None,
                })
                .flatten()
                .collect::<Vec<_>>();
            assert_eq!(creations.len(), 2);
            assert_eq!(creations[0].0, creations[1].0);
            assert_eq!(creations[0].1, creations[1].1);
            for replay_width in [1, 8] {
                let (io, run) = crate::host_launch::replay(&path).unwrap();
                let handle = Host::with_clock(|| panic!("replay admission clock")).start(
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
