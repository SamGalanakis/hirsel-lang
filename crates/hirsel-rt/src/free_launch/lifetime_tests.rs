use super::*;
use crate::host::{Host, Selection, Sink, Sinks};
use crate::program::{Code, Program};
use hirsel_run_config::{ProgramFacts, Root, RunConfig};
use hirsel_run_schema::{CodeReach, IoGrants, LaunchKind, RecordingLevel};
use std::sync::atomic::{AtomicBool, Ordering};

static INTERNAL: AtomicBool = AtomicBool::new(false);
unsafe extern "C" fn entry(
    ctx: *mut crate::TaskCtx,
    _: *mut u8,
    _: *mut u8,
    _: *const u64,
    _: *const u64,
) -> u8 {
    unsafe {
        let _role = crate::tasks::role::Guard::new(ctx);
        let block = crate::alloc::hirsel_rt_v2_alloc(ctx, 1, 128, 8, crate::DOMAIN_HEAP, 0);
        crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 1, 1, 0);
        if INTERNAL.load(Ordering::Relaxed) {
            // FAIL-3: this still-owned allocation is reclaimed by abandonment.
            crate::stop::raise(ctx, Stop::Failed);
        }
        crate::release::hirsel_rt_v2_release(ctx, block, (*block).index(), std::ptr::null(), 0, 0);
        crate::STATUS_FAILURE
    }
}
fn sinks() -> Sinks {
    Sinks {
        stdout: Sink::capture().0,
        stderr: Sink::capture().0,
        report: Sink::capture().0,
    }
}

#[test]
fn internal_failure_replay_abandons_live_storage_and_normal_failed_root_keeps_exit_audit() {
    const CHILD: &str = "HIRSEL_F1_FAILURE_LIFETIME";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "free_launch::lifetime_tests::internal_failure_replay_abandons_live_storage_and_normal_failed_root_keeps_exit_audit", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    let scratch = tempfile::tempdir().unwrap();
    for internal in [false, true] {
        INTERNAL.store(internal, Ordering::Relaxed);
        for width in [1, 8] {
            for level in [RecordingLevel::Record, RecordingLevel::Durable] {
                let facts = ProgramFacts {
                    program: [19; 32],
                    entry: "free_production@1/main".into(),
                    launch: LaunchKind::Main,
                    roots: vec![Root::Io, Root::Args],
                    code: CodeReach::Safe,
                    fueled: false,
                };
                let mut run = RunConfig::empty(facts.program, facts.entry.clone(), facts.launch);
                run.args = Some(vec![]);
                run.io = Some(IoGrants {
                    tasks: Some("tasks.free@1".into()),
                    ..Default::default()
                });
                run.recording = Some(level);
                run.placement = Some(
                    hirsel_run_config::Placement::from_json(
                        format!("{{\"cores\":{width}}}").as_bytes(),
                    )
                    .unwrap(),
                );
                let run = run.validated().unwrap();
                let config = crate::launch::Config {
                    run_identity: run.identity(),
                    hash_seed: 0,
                    random_seed: 0,
                    test_seed: 0,
                };
                let mut program =
                    Program::harness(Code::Main(entry), 4, std::ptr::null(), 0, config);
                program.digest = facts.program;
                program.entry_name = facts.entry.clone();
                program.failure_table = Some(super::tests::failure_table());
                let host = Host::with_clock(|| Some(0));
                let output = sinks();
                let (admitted, io) = host
                    .admit(&run.bind(&facts).unwrap(), None, &output)
                    .unwrap();
                let tape = scratch.path().join("failure.tape");
                let observation = scratch.path().join("observation");
                // Isolated child: no other test or launch observes this environment.
                unsafe {
                    std::env::set_var("HIRSEL_RT_OBSERVATION", &observation);
                }
                let live = host
                    .start(
                        &program,
                        config,
                        Selection::Live {
                            admitted,
                            io,
                            tape: Some(tape.clone()),
                        },
                        vec![],
                        output,
                    )
                    .wait();
                assert!(matches!(live, RunOutcome::Failed { .. }), "{live:?}");
                let tape_bytes = std::fs::read(&tape).unwrap();
                let parsed = hirsel_free::tape::parse(&tape_bytes).unwrap();
                let end = &parsed.end.as_ref().unwrap().outcome;
                assert!(end.starts_with(if internal {
                    "native-failure-v2/"
                } else {
                    "native-outcome-v2/"
                }));
                let mut old_binding = parsed.binding.clone();
                old_binding[..4].copy_from_slice(b"F1B1");
                // Preserve every record and terminal frame, recomputing only
                // the old binding's CRC: reject its version before terminal use.
                let offset = hirsel_free::tape::header(&parsed.binding).len();
                let mut old = hirsel_free::tape::header(&old_binding);
                old.extend_from_slice(&tape_bytes[offset..]);
                assert_eq!(
                    Session::replay(&old).err().unwrap(),
                    "unsupported Free admission binding"
                );
                let forged = terminal_bytes(
                    "native-failure-v2",
                    &RunOutcome::Returned(hirsel_run_schema::Returned::Ok),
                    &Default::default(),
                );
                assert!(terminal(&forged).is_err());
                let recorded = std::fs::read_to_string(&observation).unwrap();
                assert_eq!(
                    recorded.lines().any(|line| line == "ended"),
                    internal,
                    "{recorded}"
                );
                for replay_width in [1, 8] {
                    let (io, run) = crate::host_launch::replay(&tape).unwrap();
                    let handle = Host::with_clock(|| panic!("replay clock")).start(
                        &program,
                        config,
                        Selection::Replay {
                            run: run.bind(&facts).unwrap(),
                            io,
                        },
                        vec![],
                        sinks(),
                    );
                    handle.control().resize(replay_width).unwrap();
                    assert_eq!(handle.wait(), live);
                    let replayed = std::fs::read_to_string(&observation).unwrap();
                    assert_eq!(
                        replayed.lines().any(|line| line == "ended"),
                        internal,
                        "{replayed}"
                    );
                    if internal {
                        assert!(
                            replayed.lines().any(|line| line == "audit unavailable"),
                            "{replayed}"
                        );
                    } else {
                        assert!(
                            replayed.lines().any(|line| line == "audit 0 0 0 0 0"),
                            "{replayed}"
                        );
                    }
                }
            }
        }
    }
}
