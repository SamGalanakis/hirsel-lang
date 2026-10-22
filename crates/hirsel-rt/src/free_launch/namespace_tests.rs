use super::*;
use crate::host::{Host, Selection, Sink, Sinks};
use crate::io::{Completion, Request};
use crate::program::{Code, Program};
use hirsel_run_config::{ProgramFacts, Root, RunConfig};
use hirsel_run_schema::{Access, CodeReach, IoGrants, LaunchKind, Mount, RecordingLevel, Returned};

unsafe extern "C" fn entry(
    ctx: *mut crate::TaskCtx,
    _: *mut u8,
    out: *mut u8,
    io: *const u64,
    _: *const u64,
) -> u8 {
    unsafe {
        let _role = crate::tasks::role::Guard::new(ctx);
        let directory = crate::io::capability_call(
            ctx,
            Request::Directory {
                io: *io,
                path: "scratch".into(),
            },
        )
        .unwrap();
        let Completion::Handle(Ok(directory)) = directory else {
            panic!("directory: {directory:?}")
        };
        let wrote = crate::io::capability_call(
            ctx,
            Request::Write {
                directory,
                path: "data".into(),
                bytes: b"coupled".to_vec(),
            },
        )
        .unwrap();
        assert_eq!(wrote, Completion::Unit(Ok(())));
        let read = crate::io::capability_call(
            ctx,
            Request::Read {
                directory,
                path: "data".into(),
            },
        )
        .unwrap();
        assert_eq!(read, Completion::Bytes(Ok(b"coupled".to_vec())));
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
        program: [4; 32],
        entry: "free_namespace@1/main".into(),
        launch: LaunchKind::Main,
        roots: vec![Root::Io, Root::Args],
        code: CodeReach::Safe,
        fueled: false,
    }
}

#[test]
fn ordinary_free_unknown_stdin_couples_every_namespace_request_and_replay_uses_the_bound_policy() {
    let scratch = tempfile::tempdir().unwrap();
    for width in [1, 8] {
        for coupled in [false, true] {
            for level in [
                RecordingLevel::None,
                RecordingLevel::Record,
                RecordingLevel::Durable,
            ] {
                let mount = tempfile::tempdir().unwrap();
                let mut record = RunConfig::empty([4; 32], facts().entry, LaunchKind::Main);
                record.args = Some(vec![]);
                record.io = Some(IoGrants {
                    tasks: Some("tasks.free@1".into()),
                    dirs: vec![Mount {
                        path: "scratch".into(),
                        host: mount.path().to_str().unwrap().into(),
                        access: Access::ReadWrite,
                    }],
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
                let mut program =
                    Program::harness(Code::Main(entry), 4, std::ptr::null(), 0, config);
                program.digest = [4; 32];
                program.entry_name = facts().entry;
                let host = Host::with_clock(|| Some(0));
                let output = sinks();
                let (admitted, mut io) = host
                    .admit(&record.clone().bind(&facts()).unwrap(), None, &output)
                    .unwrap();
                if coupled {
                    io.set_send_stdin(std::io::Cursor::new(Vec::<u8>::new()));
                }
                assert_eq!(io.namespace_coupled(), coupled);
                let path = scratch
                    .path()
                    .join(format!("{width}-{coupled}-{level:?}.tape"));
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
                assert_eq!(
                    std::fs::read(mount.path().join("data")).unwrap(),
                    b"coupled"
                );
                drop(mount);
                if level == RecordingLevel::None {
                    continue;
                }
                let bytes = std::fs::read(&path).unwrap();
                let tape = hirsel_free::tape::parse(&bytes).unwrap();
                assert_eq!(binding(&tape.binding).unwrap().1, coupled);
                assert_eq!(
                    tape.keys.iter().any(|key| key == "host-resource/1"),
                    coupled
                );
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
                // Reframe a valid tape with the opposite policy; do not rely on CRC failure.
                let prefix = hirsel_free::tape::header(&tape.binding);
                let mut forged_binding = tape.binding.clone();
                *forged_binding.last_mut().unwrap() = u8::from(!coupled);
                let mut forged = hirsel_free::tape::header(&forged_binding);
                forged.extend_from_slice(&bytes[prefix.len()..]);
                let corrupt = path.with_extension("forged");
                std::fs::write(&corrupt, forged).unwrap();
                let (io, run) = crate::host_launch::replay(&corrupt).unwrap();
                let outcome = Host::with_clock(|| panic!("replay clock"))
                    .start(
                        &program,
                        config,
                        Selection::Replay {
                            run: run.bind(&facts()).unwrap(),
                            io,
                        },
                        vec![],
                        sinks(),
                    )
                    .wait();
                assert!(matches!(outcome, RunOutcome::Diverged(_)), "{outcome:?}");
            }
        }
    }
}

static CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct ChangingPolicy;
impl crate::io_concurrent::ConcurrentAdapter for ChangingPolicy {
    fn namespace_coupled(&self) -> bool {
        CHANGED.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, crate::io::AdapterError> {
        panic!("policy mismatch reached preflight")
    }
    fn call(&self, _: Request) -> Result<Completion, crate::io::AdapterError> {
        panic!("policy mismatch reached host effect")
    }
}

#[test]
fn a_changed_namespace_policy_refuses_before_preparation_or_host_effect() {
    use crate::tasks::free::{enable, host, record};
    use std::sync::atomic::Ordering;
    CHANGED.store(false, Ordering::Relaxed);
    let recording = record::Recording::new(b"fixed namespace policy", false);
    let install = recording.clone();
    let adapter = Arc::new(ChangingPolicy);
    let mut admitted = Some(adapter.clone());
    let mut run = crate::host::BodyRun::new(8, false);
    run.free = true;
    unsafe { run.use_physical_dispatch() };
    let outcome = run.run(
        move |ctx| unsafe {
            enable(ctx);
            install.root(ctx);
        },
        move |ctx| unsafe {
            let _role = crate::tasks::role::Guard::new(ctx);
            {
                let _segment = crate::tasks::role::Segment::new(ctx);
                host::concurrent::Service::install(ctx, admitted.take().unwrap()).unwrap();
                host::native::enable(ctx);
            }
            CHANGED.store(true, Ordering::Relaxed);
            crate::stop::check(
                ctx,
                crate::host_waits::issue(
                    ctx,
                    Request::Directory {
                        io: 1,
                        path: "".into(),
                    },
                ),
            );
            unreachable!("changed policy admitted");
        },
    );
    assert_eq!(
        outcome.ended,
        Err(Stop::Refused("namespace policy changed during run".into()))
    );
    assert_eq!(outcome.pending, 0);
    assert!(recording.trace().is_empty());
    let (_, sweep) = recording.recorder.stop("refused", None).unwrap().unwrap();
    assert!(sweep.withdrawn.is_empty());
    assert!(sweep.in_doubt.is_empty());
    assert_eq!(Arc::strong_count(&adapter), 1);
}

#[test]
fn namespace_policy_only_adds_the_declared_exclusive_conservative_domain() {
    use crate::io_keys::{COUPLED, Key, KeySet, Mode, NAMESPACE};
    for keys in [
        KeySet::default(),
        KeySet::new(vec![(Key(7), Mode::Exclusive)]),
    ] {
        assert_eq!(
            crate::tasks::free::host::native::namespace_policy(keys.clone(), true),
            keys
        );
    }
    for mode in [Mode::Shared, Mode::Exclusive] {
        let keys = KeySet::new(vec![(NAMESPACE, mode), (Key(7), Mode::Exclusive)]);
        assert_eq!(
            crate::tasks::free::host::native::namespace_policy(keys.clone(), false),
            keys
        );
        assert_eq!(
            crate::tasks::free::host::native::namespace_policy(keys, true),
            KeySet::new(vec![
                (NAMESPACE, mode),
                (COUPLED, Mode::Exclusive),
                (Key(7), Mode::Exclusive)
            ])
        );
    }
}
