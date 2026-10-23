use super::*;

#[test]
fn trace_record_27_argument_reads_and_deadline_scopes_preserve_canonical_evidence() {
    let cases = [
        (
            "args",
            r#"pub fn main(args: Args) -> Result[(), ()] { failure.check(arg.len(args) == 0, Arithmetic()); Ok(()) }"#,
            "",
            0,
        ),
        (
            "args-values",
            r#"pub fn main(args: Args) -> Result[(), ()] {
            failure.check(arg.len(args) == 0, Arithmetic())
            let _ = arg.text(args, 0)
            let _ = arg.path(args, 0)
            Ok(())
        }"#,
            "",
            0,
        ),
        (
            "args-present",
            r#"pub fn main(args: Args) -> Result[(), ArgError] {
                let text = arg.text(args, 0)?
                let _ = arg.path(args, 0)?
                failure.check(text == "x", Arithmetic())
                Ok(())
            }"#,
            "",
            0,
        ),
        (
            "deadline-min",
            r#"pub fn main(rt: Io) -> Result[(), GrantError] {
                let tasks = io.tasks(rt)?
                let c = io.monotonic_clock(rt)?
                let Ok(at) = clock.monotonic(c) else { return Ok(()) }
                let _ = task.scope_until(tasks, Deadline(clock: c, at: at), fn(s: Scope) { () })
                Ok(())
            }"#,
            "",
            0,
        ),
        (
            "deadline",
            r#"pub fn main(rt: Io) -> Result[(), GrantError] {
            let out = io.stdout(rt)?
            let tasks = io.tasks(rt)?
            let c = io.monotonic_clock(rt)?
            let Ok(start) = clock.monotonic(c) else { return Ok(()) }
            let Some(at) = instant.checked_add(c, start, 1000000000u64) else { return Ok(()) }
            let _ = task.scope_until(tasks, Deadline(clock: c, at: at), fn(s: Scope) { () })
            let _ = out.print(out, "done\n")
            Ok(())
        }"#,
            "done\n",
            0,
        ),
        (
            "scope-args-status",
            r#"pub fn main(rt: Io, args: Args) -> Result[std@1/process.ExitCode, GrantError] {
            let tasks = io.tasks(rt)?
            let out = io.stdout(rt)?
            let n = arg.len(args)
            let _ = task.scope(tasks, fn(s: Scope) { () })
            let _ = out.print(out, "ready\n")
            Ok(std@1/process.exit_code(2u8))
        }"#,
            "ready\n",
            2,
        ),
    ];
    for (name, source, expected, status) in cases {
        let source = std_file(source);
        let dir = tempfile::tempdir().unwrap();
        let args: &[&str] = if name == "args-present" { &["x"] } else { &[] };
        reference_and_checked_replay(&source, args, expected, status);
        for backend in ["llvm", "cranelift"] {
            let executable = build_executable(&source, &["--backend", backend]);
            for width in [1, 2, 4] {
                let grants = serde_json::json!({"io":{"tasks":"tasks.cooperative@1","stdout":"stdout.bytes@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[]},"placement":{"cores":width,"commit":"shared","reorder":"auto"}}).to_string();
                for traced in [false, true] {
                    for recorded in [false, true] {
                        let trace = dir.path().join("events.jsonl");
                        let tape = dir.path().join("run.tape");
                        let mut command = clean(&executable);
                        if traced {
                            command.arg("--hirsel-trace-events").arg(&trace);
                        }
                        if recorded {
                            command.arg("--hirsel-record").arg(&tape);
                        }
                        command.arg("--hirsel-grants").arg(&grants).args(args);
                        let output = command.output().unwrap();
                        assert_eq!(
                            output.status.code(),
                            Some(status),
                            "{name} {backend} w{width} trace={traced} record={recorded}: {output:?}"
                        );
                        assert_eq!(output.stdout, expected.as_bytes());
                        assert!(output.stderr.is_empty(), "{output:?}");
                        let evidence = traced.then(|| std::fs::read(&trace).unwrap());
                        if let Some(bytes) = &evidence {
                            let rows: Vec<serde_json::Value> = std::str::from_utf8(bytes)
                                .unwrap()
                                .lines()
                                .map(|line| serde_json::from_str(line).unwrap())
                                .collect();
                            assert_eq!(rows.last().unwrap()["outcome"]["kind"], "root_end");
                            if name.starts_with("args") || name == "scope-args-status" {
                                assert!(rows.iter().any(|row| row["op"] == "arg.read"), "{rows:?}");
                            }
                            if name.starts_with("deadline") {
                                assert_eq!(
                                    rows.iter()
                                        .filter(|row| row["outcome"]["kind"] == "deadline_armed")
                                        .count(),
                                    1
                                );
                            }
                        }
                        if recorded {
                            let original_tape = std::fs::read(&tape).unwrap();
                            let mut replay = clean(&executable);
                            replay.env("HIRSEL_RT_WORKERS", width.to_string());
                            if traced {
                                replay.arg("--hirsel-trace-events").arg(&trace);
                            }
                            let output = replay
                                .args([
                                    "--hirsel-replay-policy",
                                    r#"{"commit":"shared","reorder":"auto"}"#,
                                ])
                                .arg("--hirsel-replay")
                                .arg(&tape)
                                .output()
                                .unwrap();
                            assert_eq!(
                                output.status.code(),
                                Some(status),
                                "{name} {backend} replay: {output:?}"
                            );
                            assert_eq!(output.stdout, expected.as_bytes());
                            assert!(output.stderr.is_empty(), "{output:?}");
                            assert_eq!(std::fs::read(&tape).unwrap(), original_tape);
                            if let Some(evidence) = evidence {
                                assert_eq!(
                                    std::fs::read(&trace).unwrap(),
                                    evidence,
                                    "{name} {backend} w{width}"
                                );
                            }
                        }
                    }
                }
            }
            std::fs::remove_file(executable).unwrap();
        }
        std::fs::remove_file(source.with_extension("hir.json")).unwrap();
        std::fs::remove_file(source).unwrap();
    }
}

fn clean(executable: &std::path::Path) -> Command {
    let mut command = Command::new(executable);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("HIRSEL_RT_") {
            command.env_remove(key);
        }
    }
    command.env("HIRSEL_JOBS", "4");
    command
}

fn reference_and_checked_replay(
    source: &std::path::Path,
    args: &[&str],
    expected: &str,
    status: i32,
) {
    hirsel_engine::with_compiler_stack(|| {
        let mut loaded = hirsel_driver::load_program(source).unwrap();
        loaded.check_for_execution().unwrap();
        let mut run = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut world = hirsel_engine::SimulationConfig::default();
        world.grants.insert("MonotonicClock".into());
        world.projections.insert("monotonic_clock".into());
        world.args = args.iter().map(|arg| (*arg).to_owned()).collect();
        let mut baseline = None;
        for checked in [false, true] {
            for replay in [false, true] {
                if !replay {
                    run.inputs.clear();
                }
                let execution = if checked {
                    hirsel_engine::run_observed_checked(&mut loaded.engine, &run, world.clone())
                } else {
                    hirsel_engine::run_observed_snapshot(&mut loaded.engine, &run, world.clone())
                }
                .unwrap();
                let observed = execution.observation.as_ref().unwrap();
                assert_eq!(
                    observed.exit_code,
                    Some(status),
                    "checked={checked} replay={replay}: {execution:?}"
                );
                assert_eq!(observed.output, expected.as_bytes());
                assert!(observed.failures.is_empty());
                run.inputs = observed
                    .effects
                    .iter()
                    .filter_map(|effect| match &effect.effect {
                        hirsel_interp::observation::Effect::CapabilityConsumed(input) => {
                            Some(input.clone())
                        }
                        _ => None,
                    })
                    .collect();
                if let Some(baseline) = &baseline {
                    assert_eq!(observed, baseline);
                } else {
                    baseline = Some(observed.clone());
                }
            }
        }
    });
}
