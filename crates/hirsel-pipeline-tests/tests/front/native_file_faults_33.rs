//! Full-entry file errors. Install/body/check separation follows SQLite's public-domain
//! test/malloc_common.tcl:107-157 and test/ioerr.test:29-46 (014644d8).
//! No SQL/pager semantics: only Hirsel's documented file effect boundaries.
//! Upstream SQLite notice: The author disclaims copyright to this source code.
//! In place of a legal notice, here is a blessing:
//! May you do good and not evil.
//! May you find forgiveness for yourself and forgive others.
//! May you share freely, never taking more than you give.
use hirsel_driver::{native::*, replay_observations, replay_observations_checked};
use hirsel_engine::{
    RunConfiguration, run_observed_checked, run_observed_snapshot, with_compiler_stack,
};
use hirsel_run_schema::{Access, FileFault, FileFaultError as E, FileFaults, FileOperation as O};
use std::collections::{BTreeMap, VecDeque};

fn step(requested: Option<u64>, error: Option<E>, partial: u64) -> FileFault {
    FileFault {
        requested,
        error,
        partial,
    }
}
fn script(op: O, steps: Vec<FileFault>) -> FileFaults {
    BTreeMap::from([(op, VecDeque::from(steps))])
}
const HELPERS: &str = r#"
@noalloc
fn grant_error(e: GrantError) -> FileError uses {} { NotGranted() }
@noalloc
fn argument_error(e: ArgError) -> FileError uses {} { FileFault(9u32) }
"#;
fn main_source(body: &str) -> String {
    format!(
        r#"pub fn main(rt: Io, args: Args) -> Result[(), FileError] {{
        let input = io.dir(rt, arg.path(args, 0u64)? argument_error)?
        let output = io.dir(rt, arg.path(args, 1u64)? argument_error)?
        {body}
        let _ = out.print(io.stdout(rt)? grant_error, "file faults checked\n")
        Ok(())
    }} {HELPERS}"#
    )
}
fn campaign(id: &str, source: &str, fixture: Fixture, controls: bool) {
    campaign_output(id, source, fixture, controls, 0, b"");
}
fn campaign_output(
    id: &str,
    source: &str,
    fixture: Fixture,
    controls: bool,
    exit: i32,
    stderr: &[u8],
) {
    if controls && std::env::var_os("HIRSEL_FILE_FAULTS_SUFFIX_LLVM").is_some() {
        suffix_recovery(source, &fixture);
        return;
    }
    with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        // Retention is opt-in for developers; qualification always sets this path.
        let retained = std::env::var_os("HIRSEL_FILE_FAULTS_EVIDENCE");
        let root = retained.map_or_else(
            || scratch.path().join(id),
            |p| std::path::PathBuf::from(p).join(id),
        );
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("main.hir");
        std::fs::write(&path, source).unwrap();
        std::fs::write(
            root.join("faults.json"),
            serde_json::to_vec_pretty(&fixture.file_faults).unwrap(),
        )
        .unwrap();
        eprintln!(
            "FILE_FAULT_SOURCE {id} {}",
            source
                .as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let expected = if std::env::var_os("HIRSEL_FILE_FAULTS_SKIP_SIMULATIONS").is_some() {
            assert_eq!(
                id, "stderr",
                "only the retained stderr simulator cells may be skipped"
            );
            Observation {
                exit_code: Some(exit),
                blocked: false,
                budget: None,
                signaled: None,
                output: b"file faults checked\n".to_vec(),
                failures: vec![],
                roots: vec![],
            }
        } else {
            eprintln!("FILE_FAULT_STARTED {id} reference");
            let reference =
                run_observed_snapshot(&mut loaded.engine, &run, (&fixture).into()).unwrap();
            let observable = reference.observation.as_ref().unwrap();
            if !fixture.file_reads.is_empty() {
                assert!(observable.effects.iter().any(|effect| matches!(&effect.effect,
                hirsel_interp::observation::Effect::CapabilityConsumed(event) if event.operation == "file.read.pending")), "read never parked");
            }
            let expected = Observation::of_reference(observable);
            assert_eq!(expected.exit_code, Some(exit), "{expected:?}");
            assert!(expected.failures.is_empty(), "{expected:?}");
            assert_eq!(expected.output, b"file faults checked\n");
            assert_eq!(observable.final_state.as_ref().unwrap().stderr, stderr);
            assert!(
                !observable
                    .final_state
                    .as_ref()
                    .unwrap()
                    .handles
                    .values()
                    .any(|h| matches!(
                        h,
                        hirsel_interp::library::simulation::ObservedGrant::File { .. }
                            | hirsel_interp::library::simulation::ObservedGrant::AtomicFile { .. }
                    )),
                "file handle leaked"
            );
            eprintln!("FILE_FAULT_PASS {id} reference");
            eprintln!("FILE_FAULT_STARTED {id} checked");
            let checked =
                run_observed_checked(&mut loaded.engine, &run, (&fixture).into()).unwrap();
            assert_eq!(checked.observation, reference.observation);
            eprintln!("FILE_FAULT_PASS {id} checked");
            expected
        };
        let native_stderr = [
            stderr,
            if exit == 1 {
                b"error: FileFault(7)\n".as_slice()
            } else {
                b"".as_slice()
            },
        ]
        .concat();
        let input = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::None,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        let ready = build(&input, &options).unwrap();
        let sites = Sites::new(input.snapshot());
        let mut runnables = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let dir = root.join(format!("{backend:?}"));
            std::fs::create_dir_all(&dir).unwrap();
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let runnable =
                link_fixture_runnable(&ready, &loaded.entry, &objects, &tools, &dir, &fixture)
                    .unwrap();
            runnables.push((runnable, dir));
        }
        let mut incomplete = vec![];
        for (recorder, dir) in &runnables {
            eprintln!("FILE_FAULT_STARTED {id} native {}", dir.display());
            let tape = dir.join("recording");
            let recorded = launch_fixture(
                recorder,
                &ready.target,
                &tools,
                dir,
                true,
                Tape::Live {
                    record: Some(&tape),
                    revocations: None,
                },
                &fixture,
            )
            .unwrap();
            assert_eq!(observe(&recorded, &ready, &sites).unwrap(), expected);
            if !stderr.is_empty() {
                assert_eq!(recorded.stderr, native_stderr);
            }
            eprintln!("FILE_FAULT_PASS {id} native {}", dir.display());
            let bytes = std::fs::read(&tape).unwrap();
            eprintln!(
                "FILE_FAULT_TAPE {id} {} {}",
                dir.file_name().unwrap().to_string_lossy(),
                bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
            );
            let record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
            let recorded_stderr: Vec<_> = record
                .events
                .iter()
                .filter_map(|event| match &event.request {
                    hirsel_rt::io::Request::PrintStderr { bytes, .. } => Some(bytes.as_slice()),
                    _ => None,
                })
                .flatten()
                .copied()
                .collect();
            assert_eq!(recorded_stderr, stderr);
            assert!(
                record.events.iter().any(|event| event.fault.is_some()),
                "fault was never delivered"
            );
            if !fixture.file_reads.is_empty() {
                let file = record
                    .events
                    .iter()
                    .find_map(|e| match (&e.request, &e.completion) {
                        (
                            hirsel_rt::io::Request::FileRead { file, .. },
                            hirsel_rt::io::Completion::Blocked { committed: false },
                        ) => Some(*file),
                        _ => None,
                    })
                    .expect("read never parked");
                assert!(record.events.iter().any(|e| matches!(e.request,hirsel_rt::io::Request::Close { handle } if handle == file)), "parked file leaked");
            }
            assert_eq!(
                record
                    .bound_run()
                    .unwrap()
                    .record()
                    .inputs
                    .fixture_file_faults
                    .as_ref(),
                Some(&fixture.file_faults)
            );
            let replay_fixture = Fixture {
                files: BTreeMap::new(),
                file_reads: BTreeMap::new(),
                file_faults: BTreeMap::new(),
                ..fixture.clone()
            };
            for (runnable, replay_dir) in &runnables {
                let replayed = launch_fixture(
                    runnable,
                    &ready.target,
                    &tools,
                    replay_dir,
                    true,
                    Tape::Replay(&tape),
                    &replay_fixture,
                )
                .unwrap();
                assert_eq!(observe(&replayed, &ready, &sites).unwrap(), expected);
                if !stderr.is_empty() {
                    assert_eq!(replayed.stderr, native_stderr);
                }
                eprintln!(
                    "FILE_FAULT_PASS {id} replay {} -> {}",
                    dir.display(),
                    replay_dir.display()
                );
            }
            for (name, replay) in [
                (
                    "reference",
                    replay_observations as fn(&std::path::Path, &std::path::Path, Option<u64>) -> _,
                ),
                ("checked", replay_observations_checked),
            ] {
                match replay(&path, &tape, None) {
                    Ok(result) => {
                        assert_eq!(Observation::of_reference(&result), expected);
                        eprintln!("FILE_FAULT_PASS {id} replay {} -> {name}", dir.display());
                    }
                    Err(error) => {
                        eprintln!(
                            "FILE_FAULT_INCOMPLETE {id} replay {} -> {name}: {error}",
                            dir.display()
                        );
                        incomplete.push(format!("{id}/{}->{name}: {error}", dir.display()));
                    }
                }
            }
            if !stderr.is_empty() {
                let mut suffix = record.clone();
                suffix.events.push(suffix.events.last().unwrap().clone());
                let path = tape.with_file_name("extra-event");
                std::fs::write(&path, hirsel_rt::launch::render_recording(&suffix).unwrap())
                    .unwrap();
                for (runnable, replay_dir) in &runnables {
                    let result = launch_fixture(
                        runnable,
                        &ready.target,
                        &tools,
                        replay_dir,
                        true,
                        Tape::Replay(&path),
                        &replay_fixture,
                    )
                    .unwrap();
                    assert_eq!(result.exit, Some(3));
                    assert_eq!(result.stdout, expected.output);
                    assert_eq!(result.stderr, native_stderr);
                    let refusal = format!(
                        "the replayed tape: UnconsumedInput({})",
                        record.events.len()
                    );
                    match observe_run(&result, &ready, &sites).unwrap() {
                        Observed::Refused(reason) => assert_eq!(reason, refusal),
                        other => panic!("expected exact late refusal, got {other:?}"),
                    }
                    eprintln!(
                        "FILE_FAULT_NEW_SUFFIX_PASS {id} {} -> {}",
                        dir.display(),
                        replay_dir.display()
                    );
                }
            }
            if controls {
                incomplete.extend(refusal_controls(
                    &path,
                    &tape,
                    &record,
                    &runnables,
                    &ready,
                    &tools,
                    &replay_fixture,
                    &expected,
                ));
            }
            assert_eq!(std::fs::read(&tape).unwrap(), bytes);
        }
        assert!(incomplete.is_empty(), "{incomplete:#?}");
    });
}

fn refusal_controls(
    path: &std::path::Path,
    tape: &std::path::Path,
    record: &hirsel_rt::launch::Recording,
    runnables: &[(Runnable, std::path::PathBuf)],
    ready: &ReadyProgram,
    tools: &Toolchain,
    fixture: &Fixture,
    expected: &Observation,
) -> Vec<String> {
    let mut failures = vec![];
    for mode in ["binding", "script-binding", "missing", "suffix", "tamper"] {
        let mut altered = record.clone();
        let expected_error = match mode {
            "binding" => {
                altered.arguments.push("changed".into());
                "the header disagrees with its run record".to_string()
            }
            "script-binding" => {
                let binding = altered.binding.as_mut().unwrap();
                let mut run: serde_json::Value =
                    serde_json::from_slice(&binding.canonical).unwrap();
                run["inputs"]["fixture_file_faults"]["file.read"][0]["error"] =
                    serde_json::json!({"Fault":24});
                binding.canonical =
                    hirsel_run_config::RunConfig::validate(&serde_json::to_vec(&run).unwrap())
                        .unwrap()
                        .canonical()
                        .to_vec();
                "recording.run_identity".to_string()
            }
            "missing" => {
                altered.events.pop().unwrap();
                format!("MissingInput({})", altered.events.len())
            }
            "suffix" => {
                altered.events.push(altered.events.last().unwrap().clone());
                format!("UnconsumedInput({})", record.events.len())
            }
            "tamper" => {
                altered
                    .events
                    .iter_mut()
                    .find(|e| e.fault.is_some())
                    .unwrap()
                    .fault
                    .as_mut()
                    .unwrap()
                    .partial = 1;
                "InvalidCompletion".to_string()
            }
            _ => unreachable!(),
        };
        let changed = tape.with_file_name(format!("refused-{mode}"));
        std::fs::write(
            &changed,
            hirsel_rt::launch::render_recording(&altered).unwrap(),
        )
        .unwrap();
        for (name, replay) in [
            (
                "reference",
                replay_observations as fn(&std::path::Path, &std::path::Path, Option<u64>) -> _,
            ),
            ("checked", replay_observations_checked),
        ] {
            let error = match replay(path, &changed, None) {
                Err(error) => error.to_string(),
                Ok(value) => format!("unexpected semantic success: {value:?}"),
            };
            if error.contains(&expected_error) {
                eprintln!("FILE_FAULT_REFUSAL_PASS {mode} {name} {error}");
            } else {
                failures.push(format!(
                    "{mode}/{name}: expected {expected_error}, got {error}"
                ));
            }
        }
        for (runnable, dir) in runnables {
            let result = launch_fixture(
                runnable,
                &ready.target,
                tools,
                dir,
                true,
                Tape::Replay(&changed),
                fixture,
            );
            let (error, prefix_ok) = match result {
                Err(error) => (error.message, mode != "suffix"),
                Ok(result) => (
                    result.record,
                    result.exit == Some(3)
                        && (mode != "suffix" || result.stdout == expected.output),
                ),
            };
            if error.contains(&expected_error) && prefix_ok {
                eprintln!(
                    "FILE_FAULT_REFUSAL_PASS {mode} native {} {error}",
                    dir.display()
                );
            } else {
                failures.push(format!(
                    "{mode}/{}: expected {expected_error} prefix_ok={prefix_ok}, got {error}",
                    dir.display()
                ));
            }
        }
    }
    failures
}

fn fixture(faults: FileFaults) -> Fixture {
    Fixture {
        args: vec!["input".into(), "output".into()],
        files: BTreeMap::from([
            ("input/a".into(), b"abc".to_vec()),
            ("output/old".into(), b"old".to_vec()),
        ]),
        mounts: BTreeMap::from([
            ("input".into(), Access::Read),
            ("output".into(), Access::ReadWrite),
        ]),
        file_faults: faults,
        ..Default::default()
    }
}
#[test]
fn read_before_and_after_prefix() {
    campaign(
        "read",
        &main_source(
            r#"
        let mut f = file.open(input, path.of("a"))?
        let mut bytes: List[u8] = []
        failure.check(file.read(input, mut f, mut bytes, 2u64) == Error(FileFault(23u32)), Arithmetic())
        failure.check(bytes == [], Arithmetic())
        failure.check(file.read(input, mut f, mut bytes, 2u64) == Ok(2u64), Arithmetic())
        failure.check(bytes == [97u8,98u8], Arithmetic())
        failure.check(file.read(input, mut f, mut bytes, 2u64) == Error(NotFound()), Arithmetic())
        failure.check(file.read(input, mut f, mut bytes, 2u64) == Error(Canceled(DeadlineExceeded())), Arithmetic())
        failure.check(bytes == [97u8,98u8], Arithmetic())
        failure.check(file.read(input, mut f, mut bytes, 2u64) == Ok(1u64), Arithmetic())
        failure.check(bytes == [97u8,98u8,99u8], Arithmetic())
    "#,
        ),
        fixture(script(
            O::Read,
            vec![
                step(Some(2), Some(E::Fault(23)), 0),
                step(Some(2), None, 0),
                step(Some(2), Some(E::NotFound), 0),
                step(Some(2), Some(E::Canceled(3)), 0),
            ],
        )),
        true,
    );
}
#[test]
fn atomic_prefix_drop_and_commit_refusal() {
    campaign(
        "atomic",
        &main_source(
            r#"
        { let mut f = file.create_atomic(output, path.of("old"))?
          failure.check(file.write_all(output, mut f, b"new") == Error(FileFault(7u32)), Arithmetic())
          let mut saw_prefix = false
          for p in take file.walk(output)? {
            if file.read_bytes(output, p?)? == b"ne" { saw_prefix = true }
          }
          failure.check(saw_prefix, Arithmetic()) }
        failure.check(file.read_bytes(output, path.of("old"))? == b"old", Arithmetic())
        let mut f = file.create_atomic(output, path.of("old"))?
        file.write_all(output, mut f, b"new")?
        failure.check(file.commit(output, take f) == Error(PermissionDenied()), Arithmetic())
        failure.check(file.read_bytes(output, path.of("old"))? == b"old", Arithmetic())
        let mut count = 0u64
        for entry in take file.walk(output)? { let _ = entry?; count = count + 1u64 }
        failure.check(count == 1u64, Arithmetic())
    "#,
        ),
        fixture(BTreeMap::from([
            (
                O::WriteAll,
                VecDeque::from([step(Some(3), Some(E::Fault(7)), 2)]),
            ),
            (
                O::Commit,
                VecDeque::from([step(None, Some(E::PermissionDenied), 0)]),
            ),
        ])),
        false,
    );
}
#[test]
fn put_new_error_and_existing_paths() {
    campaign(
        "put-new",
        &main_source(
            r#"
        failure.check(file.put_new(output, path.of("new"), b"new", Durable()) == Error(FileFault(7u32)), Arithmetic())
        failure.check(file.read_bytes(output, path.of("new")) == Error(NotFound()), Arithmetic())
        file.put_new(output, path.of("old"), b"old", Durable())?
        failure.check(file.put_new(output, path.of("old"), b"different", Durable()) == Error(AlreadyExists()), Arithmetic())
        failure.check(file.read_bytes(output, path.of("old"))? == b"old", Arithmetic())
        file.put_new(output, path.of("new"), b"new", Durable())?
        failure.check(file.read_bytes(output, path.of("new"))? == b"new", Arithmetic())
    "#,
        ),
        fixture(script(O::PutNew, vec![step(Some(3), Some(E::Fault(7)), 0)])),
        false,
    );
}
#[test]
fn append_partial_sync_and_cancellation() {
    campaign(
        "append",
        &main_source(
            r#"
        failure.check(file.write_bytes(input, path.of("denied"), b"abc") == Error(NotGranted()), Arithmetic())
        let mut f = file.open_append(output, path.of("old"))?
        failure.check(file.append(output, mut f, b"xyz") == Error(FileFault(7u32)), Arithmetic())
        failure.check(file.sync(output, mut f) == Error(FileFault(8u32)), Arithmetic())
        failure.check(file.read_bytes(output, path.of("old"))? == b"oldxy", Arithmetic())
        failure.check(file.write_bytes(output, path.of("old"), b"abc") == Error(Canceled(Parent())), Arithmetic())
        failure.check(file.read_bytes(output, path.of("old"))? == b"oldxy", Arithmetic())
        failure.check(file.write_bytes(output, path.of("old"), b"abc") == Error(FileFault(9u32)), Arithmetic())
        failure.check(file.read_bytes(output, path.of("old"))? == b"a", Arithmetic())
    "#,
        ),
        fixture(BTreeMap::from([
            (
                O::Append,
                VecDeque::from([step(Some(3), Some(E::Fault(7)), 2)]),
            ),
            (O::Sync, VecDeque::from([step(None, Some(E::Fault(8)), 0)])),
            (
                O::WriteBytes,
                VecDeque::from([
                    step(Some(3), Some(E::Canceled(1)), 0),
                    step(Some(3), Some(E::Fault(9)), 1),
                ]),
            ),
        ])),
        false,
    );
}
#[test]
fn admission_refuses_unsupported_scripts() {
    use hirsel_run_schema::validate_file_faults;
    for (faults, message) in [
        (script(O::Read, vec![]), "empty file fault queue"),
        (
            script(O::WriteAll, vec![step(Some(2), Some(E::Fault(7)), 3)]),
            "unsupported partial file fault",
        ),
        (
            script(O::Commit, vec![step(Some(1), Some(E::Fault(7)), 0)]),
            "expected request count",
        ),
        (
            script(O::Read, vec![step(Some(2), Some(E::Fault(7)), 1)]),
            "unsupported partial file fault",
        ),
        (
            script(O::PutNew, vec![step(Some(2), Some(E::Fault(7)), 1)]),
            "unsupported partial file fault",
        ),
        (
            script(O::Append, vec![step(Some(2), Some(E::Canceled(1)), 1)]),
            "unsupported partial file fault",
        ),
        (
            script(O::Open, vec![step(None, Some(E::Canceled(4)), 0)]),
            "invalid cancellation cause",
        ),
    ] {
        assert!(validate_file_faults(&faults).unwrap_err().contains(message));
    }
    let mut faults = script(O::WriteAll, vec![step(Some(3), Some(E::Fault(7)), 2)]);
    let original = faults.clone();
    assert_eq!(
        hirsel_run_schema::take_file_fault(&mut faults, O::WriteAll, Some(1)).unwrap_err(),
        "file.write_all: file fault request count mismatch: expected Some(3), got Some(1)"
    );
    assert_eq!(faults, original);
}

const PENDING: &str = r#"
resource type Joined { dir: Dir }
drop(x) { let _ = file.write_bytes(x.dir, path.of("joined"), b"yes") }
pub fn main(rt: Io, args: Args) -> Result[(), FileError] {
  failure.check(arg.len(args) == 4u64, Arithmetic())
  failure.check(arg.text(args, 2u64) == Ok("--hirsel-run"), Arithmetic())
  failure.check(arg.text(args, 3u64) == Ok("羊"), Arithmetic())
  failure.check(arg.text(args, 4u64) == Error(MissingArgument()), Arithmetic())
  let tasks = io.tasks(rt)? grant_error
  let d = io.dir(rt, arg.path(args, 0u64)? argument_error)?
  let output = io.dir(rt, arg.path(args, 1u64)? argument_error)?
  failure.check(file.write_bytes(d, path.of("denied"), b"no") == Error(NotGranted()), Arithmetic())
  failure.check(match io.dir(rt, path.of("ungranted")) { Error(NotGranted()) => true, _ => false }, Arithmetic())
  let problem = task.scope(tasks, fn(s: Scope) -> bool {
    let mut pending: TaskSet[Result[(), FileError]] of s = task.set(s)
    task.add(s, mut pending, task.async(s, fn() -> Result[(), FileError] {
      let guard = Joined(dir: output)
      let mut f = file.open(d, path.of("a"))?
      let mut bytes: List[u8] = []
      match file.read(d, mut f, mut bytes, 1u64) {
        Error(Canceled(Requested())) => {
          failure.check(list.len(bytes) == 0u64, Arithmetic())
          Ok(())
        },
        _ => { failure.check(false, Arithmetic()); Ok(()) },
      }
    }))
    task.add(s, mut pending, task.async(s, fn() -> Result[(), FileError] {
      let info = file.metadata(d, path.of("b"))?
      failure.check(info.kind == Regular(), Arithmetic())
      let mut f = file.open(d, path.of("b"))?
      let mut bytes: List[u8] = []
      let n = file.read(d, mut f, mut bytes, 1u64)?
      if n != 1u64 { Error(FileFault(7u32)) } else { Ok(()) }
    }))
    let slot = match task.await_any(s, pending) { Some(i) => i, None => 99u64 }
    failure.check(slot == 1u64, Arithmetic())
    let problem = match task.await_at(s, mut pending, slot) {
      Error(FileFault(7u32)) => true,
      _ => false,
    }
    task.cancel_all(s, take pending)
    if !problem { let _ = file.write_bytes(output, path.of("c"), b"queued") }
    problem
  })
  failure.check(problem, Arithmetic())
  failure.check(file.read_bytes(output, path.of("joined"))? == b"yes", Arithmetic())
  failure.check(file.read_bytes(output, path.of("c")) == Error(NotFound()), Arithmetic())
  let _ = out.print(io.stdout(rt)? grant_error, "file faults checked\n")
  Ok(())
}

@noalloc
fn grant_error(e: GrantError) -> FileError uses {} { NotGranted() }
@noalloc
fn argument_error(e: ArgError) -> FileError uses {} { FileFault(9u32) }
"#;
#[test]
fn pending_does_not_consume_fault_before_sibling_and_cancel() {
    let mut recipe = fixture(script(O::Read, vec![step(Some(1), Some(E::Fault(7)), 0)]));
    recipe.args.extend(["--hirsel-run".into(), "羊".into()]);
    recipe.files.insert("input/b".into(), vec![]);
    recipe
        .file_reads
        .insert("input/a".into(), hirsel_run_schema::FileReadScript::Pending);
    campaign("pending", PENDING, recipe, false);
}

#[test]
fn acquisitions_and_stream_write_errors() {
    campaign(
        "acquire-write",
        &main_source(
            r#"
        failure.check(match file.open(input, path.of("a")) { Error(PermissionDenied()) => true, _ => false }, Arithmetic())
        failure.check(file.read_bytes(input, path.of("a")) == Error(FileFault(5u32)), Arithmetic())
        failure.check(match file.create(output, path.of("new")) { Error(Canceled(ChildFailed())) => true, _ => false }, Arithmetic())
        failure.check(match file.open_append(output, path.of("old")) { Error(NotDirectory()) => true, _ => false }, Arithmetic())
        failure.check(match file.create_atomic(output, path.of("old")) { Error(CrossDevice()) => true, _ => false }, Arithmetic())
        let mut f = file.create(output, path.of("new"))?
        failure.check(file.write(output, mut f, b"xyz") == Error(FileFault(42u32)), Arithmetic())
        failure.check(file.read_bytes(output, path.of("new"))? == b"xy", Arithmetic())
        failure.check(file.write(output, mut f, b"z") == Ok(1u64), Arithmetic())
        failure.check(file.read_bytes(output, path.of("new"))? == b"xyz", Arithmetic())
    "#,
        ),
        fixture(BTreeMap::from([
            (
                O::Open,
                VecDeque::from([step(None, Some(E::PermissionDenied), 0)]),
            ),
            (
                O::ReadBytes,
                VecDeque::from([step(None, Some(E::Fault(5)), 0)]),
            ),
            (
                O::Create,
                VecDeque::from([step(None, Some(E::Canceled(2)), 0)]),
            ),
            (
                O::OpenAppend,
                VecDeque::from([step(None, Some(E::NotDirectory), 0)]),
            ),
            (
                O::CreateAtomic,
                VecDeque::from([step(None, Some(E::CrossDevice), 0)]),
            ),
            (
                O::Write,
                VecDeque::from([step(Some(3), Some(E::Fault(42)), 2)]),
            ),
        ])),
        false,
    );
}

// Recovery consumes the original log-retained bytes. No live or accepted replay
// cell runs again merely to qualify the launcher's late-refusal diagnostic.
fn suffix_recovery(source: &str, fixture: &Fixture) {
    with_compiler_stack(|| {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.hir");
        std::fs::write(&path, source).unwrap();
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let input = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let tools = Toolchain::for_matrix(
            &root.path().join("runtime"),
            &[
                Configuration {
                    options: options.clone(),
                    engine: Engine::Native {
                        opt: OptLevel::None,
                        partition: Partition::Whole,
                        runtime: Runtime::Diagnostic,
                    },
                },
                Configuration {
                    options: options.clone(),
                    engine: Engine::Cranelift {
                        partition: Partition::Whole,
                        runtime: Runtime::Diagnostic,
                    },
                },
            ],
        )
        .unwrap();
        let ready = build(&input, &options).unwrap();
        let mut runnables = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let dir = root.path().join(format!("{backend:?}"));
            std::fs::create_dir(&dir).unwrap();
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let runnable =
                link_fixture_runnable(&ready, &loaded.entry, &objects, &tools, &dir, fixture)
                    .unwrap();
            runnables.push((runnable, dir));
        }
        for key in [
            "HIRSEL_FILE_FAULTS_SUFFIX_LLVM",
            "HIRSEL_FILE_FAULTS_SUFFIX_CRANELIFT",
        ] {
            let hex = std::env::var(key).unwrap();
            let bytes: Vec<_> = hex
                .as_bytes()
                .chunks_exact(2)
                .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
                .collect();
            let mut record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
            let count = record.events.len();
            record.events.push(record.events.last().unwrap().clone());
            let tape = root.path().join(key);
            std::fs::write(&tape, hirsel_rt::launch::render_recording(&record).unwrap()).unwrap();
            let recipe = Fixture {
                files: Default::default(),
                file_reads: Default::default(),
                file_faults: Default::default(),
                ..fixture.clone()
            };
            for (runnable, dir) in &runnables {
                let result = launch_fixture(
                    runnable,
                    &ready.target,
                    &tools,
                    dir,
                    true,
                    Tape::Replay(&tape),
                    &recipe,
                )
                .unwrap();
                let linked =
                    hirsel_run_config::RunConfig::read_path(&dir.join("run.json")).unwrap();
                let original = record.bound_run().unwrap();
                eprintln!(
                    "FILE_FAULT_BINDING_ORIGINAL {}",
                    String::from_utf8_lossy(original.canonical())
                );
                eprintln!(
                    "FILE_FAULT_BINDING_RELINKED {}",
                    String::from_utf8_lossy(linked.canonical())
                );
                eprintln!(
                    "FILE_FAULT_RECOVERY_DIAGNOSTIC {key} {} {result:?}",
                    dir.display()
                );
                assert_eq!(result.exit, Some(3));
                assert_eq!(result.stdout, b"file faults checked\n");
                assert!(
                    result.record.contains(&format!("UnconsumedInput({count})")),
                    "{}",
                    result.record
                );
                eprintln!(
                    "FILE_FAULT_RECOVERY_PASS suffix {key} -> {} {}",
                    dir.display(),
                    result.record
                );
            }
        }
    });
}

#[test]
fn nonzero_file_error_preserves_stderr_and_prefix() {
    let source = main_source(r#"
        failure.check(file.put_new(output, path.of("new"), b"new", Durable()) == Error(FileFault(7u32)), Arithmetic())
        let _ = err.print(io.stderr(rt)? grant_error, "file fault: 7\n")
    "#).replace("Ok(())", "Error(FileFault(7u32))");
    campaign_output(
        "stderr",
        &source,
        fixture(script(O::PutNew, vec![step(Some(3), Some(E::Fault(7)), 0)])),
        false,
        1,
        b"file fault: 7\n",
    );
}
