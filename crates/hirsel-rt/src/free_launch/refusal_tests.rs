use super::*;
use crate::host::{Host, Selection, Sink, Sinks};
use crate::program::{Code, Program};
use hirsel_run_config::{ProgramFacts, Root, RunConfig};
use hirsel_run_schema::{CodeReach, IoGrants, LaunchKind, NetGrant, RecordingLevel};

const FAILURE_TABLE: [u8; 40] = {
    let mut bytes = [0; 40];
    bytes[0] = 1; // One site-zero row, no user rules; CAP-2 is a runtime rule.
    bytes
};

unsafe extern "C" fn entry(
    ctx: *mut crate::TaskCtx,
    _: *mut u8,
    _: *mut u8,
    io: *const u64,
    _: *const u64,
) -> u8 {
    unsafe {
        let _role = crate::tasks::role::Guard::new(ctx);
        let mut net = crate::io::GrantResult::default();
        crate::io::hirsel_rt_v2_io_net(ctx, std::ptr::null_mut(), &mut net, io);
        assert_eq!(net.tag, 0);
        let mut clock = crate::io::ClockResult {
            tag: 99,
            padding: 0,
            value: 0,
        };
        crate::io::hirsel_rt_v2_io_monotonic_clock(ctx, std::ptr::null_mut(), &mut clock, io);
        assert_eq!(clock.tag, 0);
        let mut result = crate::net_abi::NetWordResult {
            tag: 99,
            padding: 0,
            value: 0,
            rest: 0,
        };
        let mut byte = [0xa5];
        // A known authority with the wrong brand must reach CAP-2, not a missing mapping.
        assert_eq!(
            crate::net_abi::hirsel_rt_v2_net_read_into(
                ctx,
                std::ptr::null_mut(),
                &mut result,
                &net.value,
                &mut clock.value,
                byte.as_mut_ptr(),
                1
            ),
            crate::STATUS_FAILURE
        );
        assert_eq!(byte, [0xa5]);
        assert_eq!(result.tag, 99);
        crate::failure::record_runtime(ctx, "CAP-2", 0, 0);
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
fn facts() -> ProgramFacts {
    ProgramFacts {
        program: [5; 32],
        entry: "free_refusal@1/main".into(),
        launch: LaunchKind::Main,
        roots: vec![Root::Io, Root::Args],
        code: CodeReach::Safe,
        fueled: false,
    }
}

#[test]
fn ordinary_free_wrong_brand_refusal_preserves_cap2_and_replays_logical_error_handles() {
    let dir = tempfile::tempdir().unwrap();
    for width in [1, 8] {
        for level in [
            RecordingLevel::None,
            RecordingLevel::Record,
            RecordingLevel::Durable,
        ] {
            let mut run = RunConfig::empty([5; 32], facts().entry, LaunchKind::Main);
            run.args = Some(vec![]);
            run.io = Some(IoGrants {
                tasks: Some("tasks.free@1".into()),
                monotonic_clock: Some("clock.monotonic.ns@1".into()),
                net: Some(NetGrant {
                    provider: "net.stream@1".into(),
                    rules: vec![],
                    peers: None,
                    listeners: vec![],
                }),
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
            let mut program = Program::harness(Code::Main(entry), 4, std::ptr::null(), 0, config);
            program.digest = [5; 32];
            program.entry_name = facts().entry;
            program.failure_table =
                Some(crate::isolate::FailureTable::new(&FAILURE_TABLE).unwrap());
            let host = Host::with_clock(|| Some(0));
            let output = sinks();
            let (admitted, io) = host
                .admit(&run.clone().bind(&facts()).unwrap(), None, &output)
                .unwrap();
            let path = dir.path().join(format!("{width}-{level:?}.tape"));
            let expected = host
                .start(
                    &program,
                    config,
                    Selection::Live {
                        admitted,
                        io,
                        tape: (level != RecordingLevel::None).then(|| path.clone()),
                    },
                    vec![],
                    output,
                )
                .wait();
            let RunOutcome::Failed { primary, .. } = &expected else {
                panic!("{expected:?}")
            };
            assert_eq!(primary.rule, "CAP-2");
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
                assert_eq!(handle.wait(), expected);
            }
        }
    }
}
