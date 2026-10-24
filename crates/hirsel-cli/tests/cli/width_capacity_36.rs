//! Shared exact-capacity launch and evidence transport for dogfood tests.
use hirsel_driver::native::*;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

// Export on success and unwinding: remote temporary directories disappear.
pub(super) struct Evidence(pub PathBuf);
impl Drop for Evidence {
    fn drop(&mut self) {
        let result = Command::new("python3")
            .arg("-c")
            .arg(include_str!("../support/width_capacity_36/export.py"))
            .arg(&self.0)
            .output();
        match result {
            Ok(output) => {
                eprint!("{}", String::from_utf8_lossy(&output.stdout));
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
                if !output.status.success() {
                    eprintln!("WIDTH36_EXPORT_FAILED {:?}", output.status);
                }
            }
            Err(error) => eprintln!("WIDTH36_EXPORT_FAILED {error}"),
        }
    }
}

pub(super) fn selection(name: &str) -> (Backend, u64, String) {
    let value = std::env::var(name).expect("select backend:width:kind");
    let parts: Vec<_> = value.split(':').collect();
    assert_eq!(parts.len(), 3);
    let backend = match parts[0] {
        "llvm" => Backend::Llvm,
        "cranelift" => Backend::Cranelift,
        _ => panic!("invalid backend"),
    };
    let width = parts[1].parse().unwrap();
    assert!([1, 2, 4].contains(&width));
    assert!(["success", "domain", "cancel"].contains(&parts[2]));
    (backend, width, parts[2].to_owned())
}

// Same validated Fixture -> RunConfig -> placement -> link/launch route as
// discrete_simulation_33::ordered. Never substitutes fixture semantics.
pub(super) fn fixture_capacity(
    manifest: &Path,
    root: &Path,
    fixture: &Fixture,
    backend: Backend,
    width: u64,
    verify: impl FnOnce(&Observation, &hirsel_rt::launch::Recording),
) {
    capacity(manifest, root, fixture, (backend, width, false), verify);
}
fn capacity(
    manifest: &Path,
    root: &Path,
    fixture: &Fixture,
    cell: (Backend, u64, bool),
    verify: impl FnOnce(&Observation, &hirsel_rt::launch::Recording),
) {
    let (backend, width, host) = cell;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut loaded = hirsel_driver::load_program(manifest).unwrap();
    loaded.check_for_execution().unwrap();
    let input = loaded.engine.export_native_input().unwrap();
    let input = loaded.engine.import_native_input(input).unwrap();
    let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    let matrix = vec![Configuration {
        options: options.clone(),
        engine: Engine::Native {
            opt: OptLevel::None,
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
    }];
    let mut tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
    tools.execution = Execution::Executable;
    tools.launch_timeout = std::time::Duration::from_secs(60);
    let ready = build(&input, &options).unwrap();
    let sites = Sites::new(input.snapshot());
    let objects = emit_backend_objects(
        &ready,
        backend,
        OptLevel::None,
        Partition::Whole,
        false,
        None,
    )
    .unwrap();
    let dir = root.join("cell");
    fs::create_dir(&dir).unwrap();
    let short = tempfile::tempdir().unwrap();
    let control = short.path().join("ctl");
    link_fixture_runnable(&ready, &loaded.entry, &objects, &tools, &dir, fixture).unwrap();
    let bound = hirsel_run_config::RunConfig::read_path(&dir.join("run.json")).unwrap();
    let mut record = bound.record().clone();
    if host {
        record.io = Some(hirsel_run_schema::IoGrants {
            tasks: Some("tasks.cooperative@1".into()),
            ..Default::default()
        });
    }
    record.placement = Some(
        hirsel_run_config::Placement::from_json(
            &serde_json::to_vec(&json!({
                "cores": width, "commit": "shared", "reorder": "off", "control": control,
            }))
            .unwrap(),
        )
        .unwrap(),
    );
    let requested = record.validated().unwrap();
    let runnable = link_runnable(
        &ready,
        &loaded.entry,
        &objects,
        &tools,
        &dir,
        Some(&requested),
    )
    .unwrap();
    let tape = dir.join("recording.tape");
    let mut replies = Vec::new();
    let launched = std::thread::scope(|scope| {
        let child = scope.spawn(|| {
            if host {
                Ok(launch_host(
                    &runnable,
                    &dir,
                    Tape::Live {
                        record: Some(&tape),
                        revocations: None,
                    },
                ))
            } else {
                launch_fixture(
                    &runnable,
                    &ready.target,
                    &tools,
                    &dir,
                    true,
                    Tape::Live {
                        record: Some(&tape),
                        revocations: None,
                    },
                    fixture,
                )
            }
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !child.is_finished() && std::time::Instant::now() < deadline {
            if let Ok(mut connection) = UnixStream::connect(&control) {
                connection
                    .set_read_timeout(Some(std::time::Duration::from_millis(100)))
                    .unwrap();
                if connection.write_all(b"{\"metrics\":true}\n").is_ok() {
                    let mut line = String::new();
                    if BufReader::new(connection).read_line(&mut line).is_ok()
                        && let Ok(value) = serde_json::from_str::<Value>(&line)
                    {
                        replies.push(value);
                    }
                }
            }
            if replies.iter().any(|m| running(m, width)) {
                break;
            }
            std::thread::yield_now();
        }
        child.join().unwrap()
    });
    let frozen_admission =
        hirsel_rt::launch::parse_recording_bytes(&fs::read(&tape).unwrap()).unwrap();
    let placement = frozen_admission
        .binding
        .as_ref()
        .unwrap()
        .placement
        .as_ref()
        .unwrap();
    let capacity = placement.proves(width);
    fs::write(
        root.join("capacity.json"),
        serde_json::to_vec_pretty(&json!({
            "backend": format!("{backend:?}"), "requested": width, "control": replies,
            "placement": placement, "capacity": capacity,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        capacity,
        "INCOMPLETE: exact admission capacity missing: {placement:?}"
    );
    let launched = launched.unwrap();
    assert!(launched.record.contains(&format!(
        "placement {}",
        hirsel_run_schema::hex(&placement.to_json())
    )));
    fs::write(dir.join("stdout"), &launched.stdout).unwrap();
    fs::write(dir.join("stderr"), &launched.stderr).unwrap();
    let observed = observe(&launched, &ready, &sites).unwrap();
    fs::write(dir.join("observation.txt"), format!("{observed:#?}")).unwrap();
    let frozen = fs::read(&tape).unwrap();
    let recording = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
    verify(&observed, &recording);
    let replay_fixture = Fixture {
        files: Default::default(),
        file_reads: Default::default(),
        ..fixture.clone()
    };
    let replayed = if host {
        Ok(launch_host(&runnable, &dir, Tape::Replay(&tape)))
    } else {
        launch_fixture(
            &runnable,
            &ready.target,
            &tools,
            &dir,
            true,
            Tape::Replay(&tape),
            &replay_fixture,
        )
    }
    .unwrap();
    assert_eq!(observe(&replayed, &ready, &sites).unwrap(), observed);
    assert_eq!(fs::read(&tape).unwrap(), frozen);
    assert_eq!(
        recording.bound_run().unwrap().identity(),
        requested.identity()
    );
    let encoded = placement.to_json();
    let replacement = String::from_utf8(encoded.clone())
        .unwrap()
        .replace(
            &format!("\"effective\":{width}"),
            &format!("\"effective\":{}", width + 1),
        )
        .into_bytes();
    let frame =
        hirsel_run_schema::tape::read(&frozen, hirsel_run_schema::tape::MAGIC.len(), 0).unwrap();
    let offset = frame
        .payload
        .windows(encoded.len())
        .position(|w| w == encoded)
        .unwrap();
    assert_eq!(encoded.len(), replacement.len());
    let mut payload = frame.payload.to_vec();
    payload[offset..offset + encoded.len()].copy_from_slice(&replacement);
    let mut forged = hirsel_run_schema::tape::MAGIC.to_vec();
    hirsel_run_schema::tape::frame(&mut forged, 0, 1, &payload).unwrap();
    forged.extend_from_slice(&frozen[frame.range.end..]);
    assert!(
        hirsel_rt::launch::parse_recording_bytes(&forged)
            .unwrap_err()
            .contains("inconsistent physical capacity")
    );
    assert_eq!(fs::read(&tape).unwrap(), frozen);

    assert!(capacity, "INCOMPLETE: exact admission capacity missing");
    eprintln!("WIDTH36_CAPACITY_PASS {:?} {width}", backend);
}
fn launch_host(runnable: &Runnable, dir: &Path, tape: Tape<'_>) -> Launched {
    use std::os::unix::process::ExitStatusExt;
    let Runnable::Executable(executable) = runnable else {
        panic!("expected executable");
    };
    let record = dir.join("host-observation");
    let mut command = Command::new(executable);
    command
        .env_clear()
        .env("HIRSEL_RT_OBSERVATION", &record)
        .env("HIRSEL_RT_DIAGNOSTIC", "1");
    match tape {
        Tape::Live {
            record: Some(tape),
            revocations: None,
        } => {
            command
                .arg("--hirsel-record")
                .arg(tape)
                .arg("--hirsel-run")
                .arg(dir.join("run.json"));
        }
        Tape::Replay(tape) => {
            command.arg("--hirsel-replay").arg(tape);
        }
        _ => panic!("expected recorded host launch"),
    }
    let output = command.output().unwrap();
    Launched {
        audit_required: true,
        exit: output.status.code(),
        signal: output.status.signal(),
        stdout: output.stdout,
        stderr: output.stderr,
        record: fs::read_to_string(record).unwrap(),
    }
}
fn running(m: &Value, width: u64) -> bool {
    m["worker_phase"] == "running"
        && m["width"]["requested"] == width
        && m["width"]["effective"] == width
        && m["width"]["pending"] == 0
}

#[test]
fn fixture_placement_starts_exact_workers() {
    immediate(false);
}
#[test]
fn immediate_host_records_exact_admission_and_replays() {
    immediate(true);
}
fn immediate(host: bool) {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path();
        let _evidence = Evidence(root.to_owned());
        fs::write(
            root.join("main.hir"),
            r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  Ok(())
}"#,
        )
        .unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = root.join("hirsel.json");
        fs::write(&manifest, json!({"schema":1,"package":{"name":"fixture_capacity","version":"0"},
            "sources":[{"path":"main.hir","module":[]}],
            "entry":{"module":[],"name":"main"},
            "dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}]
        }).to_string()).unwrap();
        capacity(
            &manifest,
            root,
            &Fixture::default(),
            (Backend::Llvm, 4, host),
            |observed, recording| {
                assert_eq!(observed.exit_code, Some(0));
                assert!(observed.failures.is_empty());
                assert_eq!(
                    recording
                        .binding
                        .as_ref()
                        .unwrap()
                        .placement
                        .as_ref()
                        .unwrap()
                        .request
                        .cores,
                    hirsel_run_config::Cores::Fixed(4)
                );
            },
        );
    });
}
