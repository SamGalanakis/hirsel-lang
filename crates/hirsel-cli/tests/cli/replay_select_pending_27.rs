//! A completed winner must not turn a pending connect into a fresh connection attempt.
use std::{fs, path::Path, process::Command};

fn success(command: &mut Command, expected: &str) {
    command.env("HIRSEL_JOBS", "4");
    eprintln!("RSP27 COMMAND {command:?}");
    let output = command.output().unwrap();
    assert!(output.status.success(), "{command:?}: {output:?}");
    assert_eq!(output.stdout, expected.as_bytes(), "{command:?}");
    assert!(output.stderr.is_empty(), "{command:?}: {output:?}");
}

const SOURCE: &str = include_str!("../support/replay_select_pending_27/main.hir");
const DEADLINE: &str = include_str!("../support/replay_select_pending_27/deadline.hir");
const WRITE: &str = "      let _ = net.write_all(n, mut stream, b\"ping\")\n";
const READ: &str =
    "      let mut buf: List[u8] = []\n      net.read(n, mut stream, mut buf, 1u64) == Ok(1u64)";

#[test]
fn write_read() {
    verify("write-read", SOURCE.to_owned(), "0\n");
}
#[test]
fn write_only() {
    verify("write-only", SOURCE.replace(READ, "      true"), "0\n");
}
#[test]
fn read_only() {
    verify("read-only", SOURCE.replace(WRITE, ""), "0\n");
}
#[test]
fn connect_only() {
    verify(
        "connect-only",
        SOURCE.replace(WRITE, "").replace(READ, "      true"),
        "0\n",
    );
}
#[test]
fn deadline() {
    verify("deadline", DEADLINE.to_owned(), "done\n");
}
#[test]
fn deadline_no_write() {
    verify("deadline-no-write", DEADLINE.replace(WRITE, ""), "done\n");
}

fn verify(name: &str, source: String, expected: &str) {
    hirsel_engine::with_compiler_stack(|| {
        eprintln!("RSP27 STARTED {name}");
        let root = tempfile::Builder::new().prefix("rsp27-").tempdir().unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let cli = env!("CARGO_BIN_EXE_hirsel");
        let dir = root.path().join(name);
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("main.hir"), source).unwrap();
        let manifest = dir.join("hirsel.json");
        fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"select_replay","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let mut grants: serde_json::Value = serde_json::from_str(include_str!(
            "../support/replay_select_pending_27/grants.json"
        ))
        .unwrap();
        grants["io"]["monotonic_clock"] = "clock.monotonic.ns@1".into();
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap();
        loaded.check_for_execution().unwrap();
        let run = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut world = hirsel_engine::SimulationConfig {
            net: Some(serde_json::from_value(grants["io"]["net"].clone()).unwrap()),
            ..Default::default()
        };
        world.grants.insert("MonotonicClock".into());
        world.projections.insert("monotonic_clock".into());
        let mut reference = None;
        for checked in [false, true] {
            let execution = if checked {
                hirsel_engine::run_observed_checked(&mut loaded.engine, &run, world.clone())
            } else {
                hirsel_engine::run_observed_snapshot(&mut loaded.engine, &run, world.clone())
            }
            .unwrap();
            let observed = execution.observation.unwrap();
            assert_eq!(
                observed.output,
                expected.as_bytes(),
                "{name} checked={checked}"
            );
            assert_eq!(observed.exit_code, Some(0));
            assert!(observed.failures.is_empty() && observed.blocked.is_none());
            if let Some(reference) = &reference {
                assert_eq!(reference, &observed);
            } else {
                reference = Some(observed);
            }
        }
        for backend in ["llvm", "cranelift"] {
            let executable = dir.join(backend);
            success(
                Command::new(cli)
                    .args(["build", "--backend", backend])
                    .arg(&manifest)
                    .arg("-o")
                    .arg(&executable),
                "",
            );
        }
        for backend in ["llvm", "cranelift"] {
            let executable = dir.join(backend);
            for width in [1, 2, 4] {
                grants["placement"] =
                    serde_json::json!({"cores":width,"commit":"shared","reorder":"auto"});
                let grant_path = dir.join("grants.json");
                fs::write(&grant_path, grants.to_string()).unwrap();
                let tape = dir.join(format!("{backend}-{width}.tape"));
                success(
                    Command::new(&executable)
                        .arg("--hirsel-record")
                        .arg(&tape)
                        .arg("--hirsel-grants")
                        .arg(&grant_path),
                    expected,
                );
                let original = fs::read(&tape).unwrap();
                let record = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
                assert!(!record.events.iter().any(|event| matches!(&event.request, hirsel_rt::io::Request::UnixConnect { path, .. } if path.ends_with("1.sock")) && matches!(event.completion, hirsel_rt::io::Completion::NetHandle(_))), "unanswered connect completed: {name} {backend} {width}");
                success(
                    Command::new(cli)
                        .arg("run")
                        .arg(&manifest)
                        .arg("--replay")
                        .arg(&tape)
                        .arg("--cores")
                        .arg(width.to_string()),
                    expected,
                );
                for target in ["llvm", "cranelift"] {
                    success(
                        Command::new(dir.join(target))
                            .env("HIRSEL_RT_WORKERS", width.to_string())
                            .arg("--hirsel-replay")
                            .arg(&tape),
                        expected,
                    );
                }
                assert_eq!(fs::read(&tape).unwrap(), original);
                // Re-encode a valid tape envelope with the wrong output bytes. Exact
                // request matching must reject it, independently of checksum checks.
                let mut tampered = record;
                let event = tampered
                    .events
                    .iter_mut()
                    .find(|event| matches!(event.request, hirsel_rt::io::Request::Print { .. }))
                    .unwrap();
                let hirsel_rt::io::Request::Print { bytes, .. } = &mut event.request else {
                    unreachable!()
                };
                bytes.push(b'!');
                let bad = dir.join("tampered.tape");
                fs::write(
                    &bad,
                    hirsel_rt::launch::render_recording(&tampered).unwrap(),
                )
                .unwrap();
                let rejected = Command::new(cli)
                    .arg("run")
                    .arg(&manifest)
                    .arg("--replay")
                    .arg(&bad)
                    .output()
                    .unwrap();
                assert_eq!(rejected.status.code(), Some(3));
                assert!(
                    String::from_utf8_lossy(&rejected.stderr).contains("ReplayMismatch"),
                    "{rejected:?}"
                );
                eprintln!("RSP27 COMPLETED {name} {backend} width={width} replay+tamper");
            }
        }
        eprintln!("RSP27 FINISHED {name}");
    });
}
