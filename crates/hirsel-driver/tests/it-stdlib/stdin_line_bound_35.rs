//! Go bufio.go:338-386 and bufio_test.go:918-1021, ebe2feef (BSD-3-Clause
//! (Go), NOTICES). Owned text, retained LF/CR and consumed overflow probe:
//! see fixtures/stdin-line-bound-35/README.md for the semantic adaptations.
use hirsel_driver::{native::*, *};
use hirsel_engine::{LoadedProgram, RunConfiguration, ToolError, with_compiler_stack};
use hirsel_rt::io::{Completion, Event, Request};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/stdin-line-bound-35/{name}.hir"))
}
fn checked(path: &Path, source: Option<&str>) -> LoadedProgram {
    let mut loaded = load_program_source(path, source).unwrap();
    loaded.check_for_execution().unwrap_or_else(|error| {
        if let ToolError::Query {
            error: hirsel_types::QueryError::Diagnostics(diagnostics),
            sources,
            ..
        } = &error
        {
            for diagnostic in diagnostics {
                eprintln!("{}", diagnostic.render_human(sources));
            }
        }
        panic!("{error}")
    });
    loaded
}
fn retained(label: &str) -> PathBuf {
    let root = std::env::var_os("STDIN_LINE_BOUND_35_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
    let dir = root.join(format!("{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}
fn hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}
// Optional recovery receipts let qualification resume individual failed/unrun
// cells without repeating accepted cells. Ordinary test runs select everything.
fn resume() -> (
    std::collections::BTreeSet<String>,
    std::collections::BTreeMap<String, String>,
) {
    std::env::var("STDIN_LINE_BOUND_35_RESUME").map_or_else(
        |_| Default::default(),
        |json| serde_json::from_str(&json).unwrap(),
    )
}
fn pending(id: &str) -> bool {
    if resume().0.contains(id) {
        eprintln!("STDIN_REUSED {id}");
        false
    } else {
        true
    }
}
fn retained_seed(id: &str) -> Vec<u8> {
    let hex = resume()
        .1
        .remove(id)
        .unwrap_or_else(|| panic!("missing retained tape for {id}"));
    assert_eq!(hex.len() % 2, 0);
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}
#[derive(Debug)]
struct Row {
    id: &'static str,
    data: &'static [u8],
    limit: u64,
    lines: usize,
}
fn rows() -> Vec<Row> {
    [
        ("eof-zero", b"".as_slice(), 0, 1),
        ("lf-zero", b"\nNEXT", 0, 1),
        ("over-zero", b"x\nNEXT", 0, 1),
        ("eof", b"", 3, 1),
        ("lf", b"\n", 3, 1),
        ("cap-lf", b"abc\nNEXT", 3, 1),
        ("cap-eof", b"abc", 3, 1),
        ("over-lf", b"abcd\nNEXT", 3, 1),
        ("over-eof", b"abcd", 3, 1),
        ("short-eof", b"ab", 3, 1),
        ("utf8-at-cap", "aé\nNEXT".as_bytes(), 3, 1),
        ("utf8-cross-cap", "abé\nNEXT".as_bytes(), 3, 1),
        ("invalid-below", b"\xff\nNEXT", 3, 1),
        ("invalid-at-cap", b"ab\xff\nNEXT", 3, 1),
        ("invalid-over", b"\xffabc\nNEXT", 3, 1),
        ("crlf", b"ab\r\nNEXT", 3, 1),
        ("cr-over", b"abc\r\nNEXT", 3, 1),
        ("adjacent", b"abc\nxy\nTAIL", 3, 2),
        ("limit-recovery", b"abcdxy\nTAIL", 3, 2),
        ("max", b"tiny\nNEXT", u64::MAX, 1),
        ("max-eof", b"tiny", u64::MAX, 1),
        ("go-short", b"012\n345\n678\n", 3, 2),
        (
            "go-long",
            b"0123456789012345678901234567890123456789",
            16,
            1,
        ),
        (
            "go-remainder",
            b"this is line1\nthis is line2\nthis is line 3\n",
            13,
            1,
        ),
    ]
    .into_iter()
    .map(|(id, data, limit, lines)| Row {
        id,
        data,
        limit,
        lines,
    })
    .collect()
}

// Independent delimiter/prefix oracle. It never emulates the reader's loop.
fn oracle(row: &Row, trace: bool) -> (Vec<u8>, Vec<(u64, Vec<u8>)>) {
    let mut cursor = 0;
    let mut output = String::new();
    let mut reads = vec![];
    for _ in 0..row.lines {
        let suffix = &row.data[cursor..];
        let content = suffix
            .iter()
            .position(|b| *b == b'\n')
            .unwrap_or(suffix.len());
        let over = content as u64 > row.limit;
        let accepted = if over { row.limit as usize } else { content };
        let delimited = !over && content < suffix.len();
        let consumed = accepted + usize::from(over || delimited);
        let successful = &suffix[..accepted + usize::from(delimited)];
        for (index, byte) in suffix[..consumed].iter().enumerate() {
            reads.push((1, vec![*byte]));
            if trace {
                output.push_str("probe 1\n");
                if index < successful.len() {
                    output.push_str(&format!("append {index} {byte}\n"));
                }
            }
        }
        if !over && !delimited {
            reads.push((1, vec![]));
        }
        if over {
            output.push_str("limit\n");
        } else if successful.is_empty() {
            output.push_str("none\n");
        } else if std::str::from_utf8(successful).is_err() {
            output.push_str("utf8\n");
        } else {
            output.push_str(&format!("some {}\n", hex(successful)));
        }
        cursor += consumed;
    }
    let tail = row.data[cursor..].to_vec();
    assert!(tail.len() <= 64);
    reads.push((64, tail.clone()));
    output.push_str(&format!("tail {}\n", hex(&tail)));
    (output.into_bytes(), reads)
}
fn recipe(row: &Row) -> Fixture {
    Fixture {
        stdin: row.data.to_vec(),
        args: vec![row.limit.to_string(), row.lines.to_string()],
        ..Default::default()
    }
}
fn simulations(
    prepared: &requests::TirExecution,
    id: &str,
    world: hirsel_engine::SimulationConfig,
    expected: &[u8],
) {
    let run = RunConfiguration {
        entry: prepared.entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut previous = None;
    for checked in [false, true] {
        if !pending(&format!(
            "{id}/{}",
            if checked { "checked" } else { "reference" }
        )) {
            continue;
        }
        eprintln!("STDIN_STARTED {id} checked={checked}");
        let result = testing::observe_tir(
            prepared,
            &run,
            world.clone(),
            hirsel_interp::Limits::default(),
            checked,
        )
        .unwrap();
        let observed = result
            .observation
            .unwrap_or_else(|| panic!("INCOMPLETE {id}: {:?}", result.outcome));
        assert_eq!(observed.exit_code, Some(0), "{id}: {observed:?}");
        assert_eq!(observed.output, expected, "{id}");
        assert!(
            observed.failures.is_empty() && observed.blocked.is_none(),
            "{observed:?}"
        );
        assert!(observed.final_state.as_ref().unwrap().stderr.is_empty());
        if let Some(before) = previous {
            assert_eq!(observed, before);
        }
        previous = Some(observed);
        eprintln!("STDIN_PASS {id} checked={checked}");
    }
}
fn assert_reads(events: &[Event], expected: &[(u64, Vec<u8>)]) {
    let actual: Vec<_> = events
        .iter()
        .filter_map(|event| match (&event.request, &event.completion) {
            (Request::ReadStdin { max, .. }, Completion::Input(Ok(bytes))) => {
                Some((*max, bytes.clone()))
            }
            (Request::ReadStdin { .. }, other) => panic!("unexpected read: {other:?}"),
            _ => None,
        })
        .collect();
    assert_eq!(
        actual, expected,
        "exact requests, consumed prefix and untouched suffix"
    );
}

#[test]
fn four_engine_lines_cursor_and_offline_replay() {
    with_compiler_stack(|| {
        let path = fixture("main");
        let mut loaded = checked(&path, None);
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input.clone()).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let entry = loaded.entry.clone();
        drop(validated);
        drop(loaded);
        let prepared = request_tir(&path, hirsel_engine::Verify::On, None).unwrap();
        let sites = Sites::new(&input);
        let dir = retained("matrix");
        let tools = Toolchain::in_tree_with_runtime(
            &dir.join("runtime"),
            &[&ready.target],
            RuntimeBuild::default(),
        )
        .unwrap();
        let objects: Vec<_> = [Backend::Llvm, Backend::Cranelift]
            .into_iter()
            .map(|backend| {
                (
                    backend,
                    emit_backend_objects(
                        &ready,
                        backend,
                        OptLevel::None,
                        Partition::Whole,
                        false,
                        None,
                    )
                    .unwrap(),
                )
            })
            .collect();
        for row in rows() {
            let passed = resume().0;
            if row.id != "cap-lf"
                && ["reference", "checked"]
                    .iter()
                    .all(|engine| passed.contains(&format!("{}/{engine}", row.id)))
                && ["Llvm", "Cranelift"].iter().all(|recorder| {
                    passed.contains(&format!("{}/{recorder}", row.id))
                        && ["reference", "checked", "Llvm", "Cranelift"]
                            .iter()
                            .all(|engine| {
                                passed.contains(&format!("{}/{recorder}/replay/{engine}", row.id))
                            })
                })
            {
                eprintln!("STDIN_REUSED_ROW {}", row.id);
                continue;
            }
            let (expected, reads) = oracle(&row, false);
            let fixture = recipe(&row);
            simulations(&prepared, row.id, (&fixture).into(), &expected);
            let mut runnables = vec![];
            for (backend, objects) in &objects {
                let launch = dir.join(format!("{}-{backend:?}", row.id));
                std::fs::create_dir_all(&launch).unwrap();
                let runnable =
                    link_fixture_runnable(&ready, &entry, objects, &tools, &launch, &fixture)
                        .unwrap();
                runnables.push((runnable, launch));
            }
            let offline = Fixture {
                stdin: vec![],
                ..fixture.clone()
            };
            for (runnable, launch) in &runnables {
                let recorder = launch
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .rsplit_once('-')
                    .unwrap()
                    .1;
                let cell = format!("{}/{recorder}", row.id);
                let tape = launch.join("recording");
                let observation = Observation {
                    exit_code: Some(0),
                    blocked: false,
                    budget: None,
                    signaled: None,
                    output: expected.clone(),
                    failures: vec![],
                    roots: vec![],
                };
                if pending(&cell) {
                    eprintln!("STDIN_STARTED {} native {}", row.id, launch.display());
                    let live = launch_fixture(
                        runnable,
                        &ready.target,
                        &tools,
                        launch,
                        true,
                        Tape::Live {
                            record: Some(&tape),
                            revocations: None,
                        },
                        &fixture,
                    )
                    .unwrap();
                    let bytes = std::fs::read(&tape).unwrap();
                    eprintln!("STDIN_TAPE {} {} {}", row.id, launch.display(), hex(&bytes));
                    assert_eq!(
                        observe(&live, &ready, &sites).unwrap(),
                        observation,
                        "{live:?}"
                    );
                    assert!(live.stderr.is_empty());
                    eprintln!("STDIN_PASS {} native {}", row.id, launch.display());
                } else {
                    std::fs::write(&tape, retained_seed(&cell)).unwrap();
                }
                let bytes = std::fs::read(&tape).unwrap();
                let recording = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
                assert_reads(&recording.events, &reads);
                for (label, checked) in [("reference", false), ("checked", true)] {
                    if !pending(&format!("{cell}/replay/{label}")) {
                        continue;
                    }
                    eprintln!(
                        "STDIN_STARTED {} replay {}->{label}",
                        row.id,
                        launch.display()
                    );
                    let replayed =
                        replay_prepared_observations(&prepared, &tape, Some(1_000_000), checked)
                            .unwrap();
                    assert_eq!(Observation::of_reference(&replayed), observation);
                    eprintln!("STDIN_PASS {} replay {}->{label}", row.id, launch.display());
                }
                for (replayer, replay_dir) in &runnables {
                    let engine = replay_dir
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .rsplit_once('-')
                        .unwrap()
                        .1;
                    if !pending(&format!("{cell}/replay/{engine}")) {
                        continue;
                    }
                    eprintln!(
                        "STDIN_STARTED {} replay {}->{}",
                        row.id,
                        launch.display(),
                        replay_dir.display()
                    );
                    let replayed = launch_fixture(
                        replayer,
                        &ready.target,
                        &tools,
                        replay_dir,
                        true,
                        Tape::Replay(&tape),
                        &offline,
                    )
                    .unwrap();
                    assert_eq!(observe(&replayed, &ready, &sites).unwrap(), observation);
                    assert!(replayed.stderr.is_empty());
                    eprintln!(
                        "STDIN_PASS {} replay {}->{}",
                        row.id,
                        launch.display(),
                        replay_dir.display()
                    );
                }
                if row.id == "cap-lf" {
                    refusal_controls(
                        &prepared,
                        &tape,
                        &recording,
                        &runnables,
                        (&ready, &tools, &sites),
                        &offline,
                        &cell,
                    );
                }
                assert_eq!(std::fs::read(&tape).unwrap(), bytes);
            }
        }
    });
}

fn refusal_controls(
    prepared: &requests::TirExecution,
    tape: &Path,
    record: &hirsel_rt::launch::Recording,
    runnables: &[(Runnable, PathBuf)],
    context: (&ReadyProgram, &Toolchain, &Sites),
    fixture: &Fixture,
    cell: &str,
) {
    let (ready, tools, sites) = context;
    for mode in ["input", "request", "missing", "extra"] {
        let mut altered = record.clone();
        let error = match mode {
            "input" => {
                altered.arguments.push("changed".into());
                "the header disagrees with its run record".to_string()
            }
            "request" => {
                let index = altered
                    .events
                    .iter()
                    .position(|e| matches!(e.request, Request::ReadStdin { .. }))
                    .unwrap();
                let Request::ReadStdin { max, .. } = &mut altered.events[index].request else {
                    unreachable!()
                };
                *max = 2;
                format!("ReplayMismatch({index})")
            }
            "missing" => {
                altered.events.pop().unwrap();
                format!("MissingInput({})", altered.events.len())
            }
            "extra" => {
                altered.events.push(altered.events.last().unwrap().clone());
                format!("UnconsumedInput({})", record.events.len())
            }
            _ => unreachable!(),
        };
        let changed = tape.with_file_name(format!("refused-{mode}"));
        std::fs::write(
            &changed,
            hirsel_rt::launch::render_recording(&altered).unwrap(),
        )
        .unwrap();
        eprintln!(
            "STDIN_CONTROL_TAPE {cell} {mode} {}",
            hex(&std::fs::read(&changed).unwrap())
        );
        for (label, checked) in [("reference", false), ("checked", true)] {
            if !pending(&format!("{cell}/refusal/{mode}/{label}")) {
                continue;
            }
            let refusal =
                replay_prepared_observations(prepared, &changed, Some(1_000_000), checked)
                    .unwrap_err()
                    .to_string();
            assert!(
                refusal.contains(&error),
                "{mode}/{label}: expected {error}, got {refusal}"
            );
            eprintln!("STDIN_REFUSAL_PASS {mode} {label} {refusal}");
        }
        for (runnable, dir) in runnables {
            let engine = dir
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .rsplit_once('-')
                .unwrap()
                .1;
            if !pending(&format!("{cell}/refusal/{mode}/{engine}")) {
                continue;
            }
            let refusal = match launch_fixture(
                runnable,
                &ready.target,
                tools,
                dir,
                true,
                Tape::Replay(&changed),
                fixture,
            ) {
                Err(e) => e.message,
                Ok(result) => {
                    assert_eq!(result.exit, Some(3));
                    match (mode, observe_run(&result, ready, sites).unwrap()) {
                        (
                            "request" | "missing",
                            Observed::Stopped(hirsel_run_schema::RunOutcome::Diverged(reason)),
                        ) => {
                            assert_eq!(reason, error);
                            reason
                        }
                        ("extra", Observed::Refused(reason)) => {
                            assert_eq!(reason, format!("the replayed tape: {error}"));
                            reason
                        }
                        ("input", Observed::Refused(reason)) => reason,
                        (_, other) => panic!("expected exact {mode} refusal, got {other:?}"),
                    }
                }
            };
            assert!(
                refusal.contains(&error),
                "{mode}/{}: expected {error}, got {refusal}",
                dir.display()
            );
            eprintln!("STDIN_REFUSAL_PASS {mode} {} {refusal}", dir.display());
        }
    }
}

#[test]
fn underlying_input_error_kinds_and_limit_precedence() {
    with_compiler_stack(|| {
        use hirsel_interp::library::simulation::Fault;
        let prepared = request_tir(&fixture("main"), hirsel_engine::Verify::On, None).unwrap();
        for (id, fault, expected) in [
            (
                "fault",
                Fault::Fault {
                    code: 73,
                    partial: 0,
                },
                "fault 73\ntail ff0a\n",
            ),
            ("requested", Fault::Canceled(0), "canceled 0\ntail ff0a\n"),
            ("parent", Fault::Canceled(1), "canceled 1\ntail ff0a\n"),
            ("child", Fault::Canceled(2), "canceled 2\ntail ff0a\n"),
            ("deadline", Fault::Canceled(3), "canceled 3\ntail ff0a\n"),
        ] {
            let mut world: hirsel_engine::SimulationConfig = (&Fixture {
                args: vec!["0".into(), "1".into()],
                stdin: b"\xff\n".to_vec(),
                ..Default::default()
            })
                .into();
            world.faults.insert("stdin.read".into(), [fault].into());
            simulations(&prepared, id, world, expected.as_bytes());
        }
    });
}

#[test]
fn pending_stdin_scripted_cancel_and_four_engine_replay() {
    with_compiler_stack(|| {
        use std::process::{Command, Stdio};
        let path = fixture("cancel");
        let mut loaded = checked(&path, None);
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let dir = retained("pending-cancel");
        let tools = Toolchain::in_tree_with_runtime(
            &dir.join("runtime"),
            &[&ready.target],
            RuntimeBuild::default(),
        )
        .unwrap();
        let grants = serde_json::json!({"args":[],"io":{
            "stdin":"stdin.bytes@1","stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1",
            "signals":{"provider":"signals.script@1","kinds":["interrupt"],"script":["interrupt"]},"dirs":[]
        }}).to_string();
        let mut executables = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let out = dir.join(format!("{backend:?}"));
            std::fs::create_dir_all(&out).unwrap();
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            executables.push(
                link_standalone_objects(&ready, &loaded.entry, &objects, &tools, &out).unwrap(),
            );
        }
        let expected = b"reading\ncanceled requested\n";
        for (index, executable) in executables.iter().enumerate() {
            let tape = dir.join(format!("recorder-{index}.tape"));
            eprintln!("STDIN_STARTED pending-cancel recorder={index}");
            let mut child = Command::new(executable)
                .arg("--hirsel-record")
                .arg(&tape)
                .arg("--hirsel-grants")
                .arg(&grants)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            // Keep the writer open with no bytes: the read must park, never see EOF.
            let writer = child.stdin.take().unwrap();
            let result = child.wait_with_output().unwrap();
            drop(writer);
            let bytes = std::fs::read(&tape).unwrap();
            eprintln!("STDIN_TAPE pending-cancel recorder-{index} {}", hex(&bytes));
            assert_eq!(result.status.code(), Some(0), "{result:?}");
            assert_eq!(result.stdout, expected);
            assert!(result.stderr.is_empty(), "{result:?}");
            let record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
            let blocked = record
                .events
                .iter()
                .position(|e| {
                    matches!(
                        (&e.request, &e.completion),
                        (
                            Request::ReadStdin { max: 1, .. },
                            Completion::Blocked { committed: false }
                        )
                    )
                })
                .expect("stdin never parked");
            // host_waits::poll_binding_observed retires the wait and synthesizes
            // Input(Canceled(Requested)) without reissuing a provider read.
            // The child checks that exact kind; the live root reports its result.
            assert_eq!(
                record
                    .events
                    .iter()
                    .filter(|e| matches!(e.request, Request::ReadStdin { .. }))
                    .count(),
                1
            );
            assert!(record.events[blocked + 1..].iter().any(|e| matches!(
                (&e.request, &e.completion),
                (
                    Request::SignalWait { .. },
                    Completion::Signal(Ok(hirsel_run_schema::SignalKind::Interrupt))
                )
            )));
            let (_, binary) = hirsel_rt::tape_binary::Tape::read(bytes.clone().into()).unwrap();
            assert!(binary.complete);
            assert_eq!(
                binary.outcome,
                Some(hirsel_run_schema::RunOutcome::Returned(
                    hirsel_run_schema::Returned::Ok
                ))
            );
            eprintln!("STDIN_PASS pending-cancel recorder={index} blocked={blocked}");
            for (label, replay) in [
                (
                    "reference",
                    replay_observations as fn(&Path, &Path, Option<u64>) -> _,
                ),
                ("checked", replay_observations_checked),
            ] {
                eprintln!("STDIN_STARTED pending-cancel replay {index}->{label}");
                let observed = replay(&path, &tape, Some(1_000_000)).unwrap();
                assert_eq!(observed.exit_code, Some(0));
                assert_eq!(observed.output, expected);
                assert!(observed.failures.is_empty() && observed.blocked.is_none());
                eprintln!("STDIN_PASS pending-cancel replay {index}->{label}");
            }
            for replayer in &executables {
                let replayed = Command::new(replayer)
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .output()
                    .unwrap();
                assert_eq!(replayed.status.code(), Some(0), "{replayed:?}");
                assert_eq!(replayed.stdout, expected);
                assert!(replayed.stderr.is_empty());
                eprintln!(
                    "STDIN_PASS pending-cancel replay {index}->{}",
                    replayer.display()
                );
            }
            assert_eq!(std::fs::read(&tape).unwrap(), bytes);
        }
    });
}

#[test]
fn real_stdin_error_after_prefix_discards_partial_text() {
    with_compiler_stack(|| {
        use std::io::Write;
        use std::os::fd::FromRawFd;
        use std::process::{Command, Stdio};
        let path = fixture("fault");
        let mut loaded = checked(&path, None);
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let dir = retained("real-error");
        let tools = Toolchain::in_tree_with_runtime(
            &dir.join("runtime"),
            &[&ready.target],
            RuntimeBuild::default(),
        )
        .unwrap();
        let grants = serde_json::json!({"args":["3"],"io":{"stdin":"stdin.bytes@1","stdout":"stdout.bytes@1","dirs":[]}}).to_string();
        let mut executables = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let out = dir.join(format!("{backend:?}"));
            std::fs::create_dir_all(&out).unwrap();
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            executables.push(
                link_standalone_objects(&ready, &loaded.entry, &objects, &tools, &out).unwrap(),
            );
        }
        for (row, (prefix, expected, fault)) in [
            (b"".as_slice(), b"fault 1\n".as_slice(), true),
            (b"a", b"fault 1\n", true),
            (b"\xff", b"fault 1\n", true),
            (b"abc", b"fault 1\n", true),
            (b"\xffabc", b"limit\n", false),
            (b"ab\nTAIL", b"some 61620a\n", false),
        ]
        .into_iter()
        .enumerate()
        {
            for (recorder, executable) in executables.iter().enumerate() {
                // A real PTY master returns its queued prefix, then EIO after
                // the last slave closes. No fabricated completion or fault script.
                let (mut master, mut slave) = (-1, -1);
                let mut term = std::mem::MaybeUninit::<libc::termios>::uninit();
                assert_eq!(
                    unsafe {
                        libc::openpty(
                            &mut master,
                            &mut slave,
                            std::ptr::null_mut(),
                            std::ptr::null(),
                            std::ptr::null(),
                        )
                    },
                    0
                );
                let input = unsafe { std::fs::File::from_raw_fd(master) };
                let mut source = unsafe { std::fs::File::from_raw_fd(slave) };
                assert_eq!(unsafe { libc::tcgetattr(slave, term.as_mut_ptr()) }, 0);
                let mut term = unsafe { term.assume_init() };
                unsafe { libc::cfmakeraw(&mut term) };
                assert_eq!(unsafe { libc::tcsetattr(slave, libc::TCSANOW, &term) }, 0);
                source.write_all(prefix).unwrap();
                drop(source);
                let tape = dir.join(format!("row-{row}-recorder-{recorder}.tape"));
                eprintln!("STDIN_STARTED real-error row={row} recorder={recorder}");
                let result = Command::new(executable)
                    .arg("--hirsel-record")
                    .arg(&tape)
                    .arg("--hirsel-grants")
                    .arg(&grants)
                    .arg("3")
                    .stdin(Stdio::from(input))
                    .output()
                    .unwrap();
                let bytes = std::fs::read(&tape).unwrap();
                eprintln!(
                    "STDIN_TAPE real-error-{row} recorder-{recorder} {}",
                    hex(&bytes)
                );
                assert_eq!(result.status.code(), Some(0), "{result:?}");
                assert_eq!(result.stdout, expected);
                assert!(result.stderr.is_empty(), "{result:?}");
                let record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
                let reads: Vec<_> = record
                    .events
                    .iter()
                    .filter(|e| matches!(e.request, Request::ReadStdin { .. }))
                    .collect();
                let consumed = if fault {
                    prefix.len()
                } else if row == 4 {
                    4
                } else {
                    3
                };
                assert_eq!(reads.len(), consumed + usize::from(fault));
                for (index, event) in reads[..consumed].iter().enumerate() {
                    assert!(matches!(event.request, Request::ReadStdin { max: 1, .. }));
                    assert_eq!(event.completion, Completion::Input(Ok(vec![prefix[index]])));
                }
                if fault {
                    let last = reads.last().unwrap();
                    assert!(matches!(last.request, Request::ReadStdin { max: 1, .. }));
                    // The existing host provider translates stream errno to InputFault(1).
                    assert_eq!(
                        last.completion,
                        Completion::Input(Err(hirsel_rt::io::InputError::Fault(1)))
                    );
                }
                eprintln!("STDIN_PASS real-error row={row} recorder={recorder}");
                for (label, replay) in [
                    (
                        "reference",
                        replay_observations as fn(&Path, &Path, Option<u64>) -> _,
                    ),
                    ("checked", replay_observations_checked),
                ] {
                    let observed = replay(&path, &tape, Some(1_000_000)).unwrap();
                    assert_eq!(observed.exit_code, Some(0));
                    assert_eq!(observed.output, expected);
                    assert!(observed.failures.is_empty() && observed.blocked.is_none());
                    eprintln!("STDIN_PASS real-error row={row} replay {recorder}->{label}");
                }
                for (index, replayer) in executables.iter().enumerate() {
                    let replayed = Command::new(replayer)
                        .arg("--hirsel-replay")
                        .arg(&tape)
                        .output()
                        .unwrap();
                    assert_eq!(replayed.status.code(), Some(0), "{replayed:?}");
                    assert_eq!(replayed.stdout, expected);
                    assert!(replayed.stderr.is_empty());
                    eprintln!("STDIN_PASS real-error row={row} replay {recorder}->native-{index}");
                }
                assert_eq!(std::fs::read(&tape).unwrap(), bytes);
            }
        }
    });
}
