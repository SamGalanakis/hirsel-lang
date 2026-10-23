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
        // This is plain owned storage, independent of reserved runtime layouts
        // (index one belongs to the streaming-walk finalizer).
        let block = crate::text::allocate_bytes(ctx, &[0; 128]).owner;
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
                // Previous ABI tapes refuse before root or host execution.
                for legacy in [true, false] {
                    let mut binding = parsed.binding.clone();
                    if legacy {
                        binding[..4].copy_from_slice(b"F1B2");
                        binding.drain(4..8);
                    } else {
                        binding[4..8].copy_from_slice(&(crate::ABI_VERSION - 1).to_le_bytes());
                    }
                    let mut old = hirsel_free::tape::header(&binding);
                    old.extend_from_slice(&tape_bytes[offset..]);
                    assert_eq!(
                        Session::replay(&old).err().unwrap(),
                        "Free recording runtime ABI version mismatch"
                    );
                }
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

#[test]
fn replay_observation_preserves_returned_codes_and_bound_ordered_failure_sites() {
    use hirsel_run_schema::{Failure, Returned};
    let config = crate::launch::Config {
        run_identity: [0; 32],
        hash_seed: 0,
        random_seed: 0,
        test_seed: 0,
    };
    let mut program = Program::harness(Code::Main(entry), 8, std::ptr::null(), 0, config);
    for (kind, offset, size) in [(0, 0, 4), (1, 0, 1), (2, 4, 8)] {
        program.return_kind = kind;
        program.success_offset = offset;
        program.result_size = size;
        for code in [0, 1, 37] {
            if kind == 0 && code > 1 {
                continue;
            }
            let outcome = RunOutcome::Returned(Returned::from_exit_code(code));
            let text = replay_observation(&program, &outcome).unwrap();
            let mut result = vec![0u8; size as usize];
            result[offset as usize] = code;
            assert_eq!(
                text,
                format!(
                    "status 0\nresult {}\nexit_code {code}\n",
                    hirsel_run_schema::hex(&result)
                )
            );
        }
    }

    // Three distinct sites make primary, cause ordering and operation-site
    // identity independently visible in the ordinary observation rows.
    let definition = "free_production@1/main";
    let file = "fixture.hir";
    let rule = "FREE-TEST";
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    for code in 1u64..=3 {
        bytes.extend_from_slice(&code.to_le_bytes());
        for word in [
            7 + code as u32,
            9,
            0,
            definition.len() as u32,
            definition.len() as u32,
            file.len() as u32,
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&((definition.len() + file.len()) as u32).to_le_bytes());
    bytes.extend_from_slice(&(rule.len() as u32).to_le_bytes());
    bytes.extend_from_slice(definition.as_bytes());
    bytes.extend_from_slice(file.as_bytes());
    bytes.extend_from_slice(rule.as_bytes());
    let table = crate::isolate::FailureTable::new(Box::leak(bytes.into_boxed_slice())).unwrap();
    program.failure_table = Some(table);
    let primary = Failure {
        rule: rule.into(),
        site: table.located(1),
        operation: Some(table.located(3)),
    };
    let causes = vec![
        Failure {
            rule: hirsel_core_contract::RUNTIME_RULES[0].into(),
            site: table.located(2),
            operation: None,
        },
        Failure {
            rule: rule.into(),
            site: table.located(3),
            operation: Some(table.located(2)),
        },
    ];
    let outcome = RunOutcome::Failed {
        primary: primary.clone(),
        causes: causes.clone(),
    };
    let text = replay_observation(&program, &outcome).unwrap();
    let mut rows = text.lines();
    assert_eq!(rows.next(), Some("status 1"));
    for (line, expected) in rows.zip(std::iter::once(&primary).chain(&causes)) {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 5);
        assert_eq!(fields[0], "failure");
        let record = crate::failure::FailureRecord {
            kind: fields[1].parse().unwrap(),
            rule: fields[2].parse().unwrap(),
            site: u64::from_str_radix(fields[3], 16).unwrap(),
            operation_site: u64::from_str_radix(fields[4], 16).unwrap(),
        };
        assert_eq!(table.failure(&record), *expected);
    }
    assert_eq!(text.lines().count(), 4);
    for field in ["rule", "site", "operation"] {
        let mut unknown = primary.clone();
        match field {
            "rule" => unknown.rule = "ABSENT-RULE".into(),
            "site" => unknown.site.column += 1,
            "operation" => unknown.operation.as_mut().unwrap().file = "absent.hir".into(),
            _ => unreachable!(),
        }
        assert_eq!(
            replay_observation(
                &program,
                &RunOutcome::Failed {
                    primary: unknown,
                    causes: vec![]
                }
            ),
            Err("replayed failure differs from bound program table".into())
        );
    }
    program.failure_table = None;
    assert_eq!(
        replay_observation(&program, &outcome),
        Err("replayed failure has no bound program table".into())
    );
}

unsafe extern "C" fn projection_entry(
    ctx: *mut crate::TaskCtx,
    _: *mut u8,
    out: *mut u8,
    io: *const u64,
    _: *const u64,
) -> u8 {
    unsafe {
        let _role = crate::tasks::role::Guard::new(ctx);
        let stdout =
            crate::io::capability_call(ctx, crate::io::Request::Stdout { io: *io }).unwrap();
        let crate::io::Completion::Handle(Ok(stdout)) = stdout else {
            panic!("stdout")
        };
        assert_eq!(
            crate::io::capability_call(
                ctx,
                crate::io::Request::Print {
                    stdout,
                    bytes: b"projection\n".to_vec()
                }
            )
            .unwrap(),
            crate::io::Completion::Unit(Ok(()))
        );
        out.cast::<u32>().write(0);
        crate::STATUS_OK
    }
}

#[test]
fn projection_channel_preserves_exact_recording_and_outcome() {
    const CHILD: &str = "HIRSEL_F1_PROJECTION_EQUIVALENCE";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "free_launch::lifetime_tests::projection_channel_preserves_exact_recording_and_outcome", "--nocapture"])
            .env(CHILD, "1").env_remove(crate::free_replay::Report::VAR).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    let scratch = tempfile::tempdir().unwrap();
    for width in [1, 8] {
        for level in [RecordingLevel::Record, RecordingLevel::Durable] {
            let facts = ProgramFacts {
                program: [23; 32],
                entry: "projection@1/main".into(),
                launch: LaunchKind::Main,
                roots: vec![Root::Io, Root::Args],
                code: CodeReach::Safe,
                fueled: false,
            };
            let mut run = RunConfig::empty(facts.program, facts.entry.clone(), facts.launch);
            run.args = Some(vec![]);
            run.io = Some(IoGrants {
                tasks: Some("tasks.free@1".into()),
                stdout: Some("stdout.bytes@1".into()),
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
                ..Default::default()
            };
            let mut program =
                Program::harness(Code::Main(projection_entry), 4, std::ptr::null(), 0, config);
            program.digest = facts.program;
            program.entry_name = facts.entry.clone();
            let mut expected = None;
            for observe in [false, true] {
                let report = scratch.path().join("projection");
                unsafe {
                    if observe {
                        std::env::set_var(crate::free_replay::Report::VAR, &report);
                    } else {
                        std::env::remove_var(crate::free_replay::Report::VAR);
                    }
                }
                let host = Host::with_clock(|| Some(0));
                let output = sinks();
                let (admitted, io) = host
                    .admit(&run.clone().bind(&facts).unwrap(), None, &output)
                    .unwrap();
                let tape = scratch
                    .path()
                    .join(if observe { "observed" } else { "plain" });
                let outcome = host
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
                assert_eq!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                );
                let actual = (outcome, std::fs::read(&tape).unwrap());
                if let Some(expected) = &expected {
                    assert_eq!(&actual, expected);
                } else {
                    expected = Some(actual);
                }
                if observe {
                    let live = std::fs::read_to_string(&report).unwrap();
                    assert!(live.starts_with("Projection {"), "{live}");
                    assert!(live.contains("host-resource/"), "{live}");
                    let (io, bound) = crate::host_launch::replay(&tape).unwrap();
                    assert_eq!(
                        host.start(
                            &program,
                            config,
                            Selection::Replay {
                                run: bound.bind(&facts).unwrap(),
                                io
                            },
                            vec![],
                            sinks()
                        )
                        .wait(),
                        expected.as_ref().unwrap().0
                    );
                    assert_eq!(std::fs::read_to_string(&report).unwrap(), live);
                }
            }
        }
    }
    unsafe {
        std::env::remove_var(crate::free_replay::Report::VAR);
    }
}
