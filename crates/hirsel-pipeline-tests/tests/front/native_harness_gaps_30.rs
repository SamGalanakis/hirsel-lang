//! Full Io+Args fixture launches and exact stop diagnostics through public APIs.
use hirsel_driver::{native::*, replay_observations, replay_observations_checked};
use hirsel_engine::{
    RunConfiguration, run_observed_checked, run_observed_snapshot, with_compiler_stack,
};
use hirsel_run_schema::{Access, FileReadScript};
use std::collections::BTreeMap;

const CANCEL: &str = r#"
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
        Error(Canceled(_)) => {
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
  let _ = out.print(io.stdout(rt)? grant_error, "joined; queued suppressed\n")
  Ok(())
}

@noalloc
fn grant_error(e: GrantError) -> FileError uses {} { NotGranted() }
@noalloc
fn argument_error(e: ArgError) -> FileError uses {} { FileFault(9u32) }
"#;

const RELEASE: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), FileError] {
  let tasks = io.tasks(rt)? grant_error
  let d = io.dir(rt, arg.path(args, 0u64)? argument_error)?
  let output = io.dir(rt, arg.path(args, 1u64)? argument_error)?
  task.scope(tasks, fn(s: Scope) -> Result[(), FileError] {
    let h = task.async(s, fn() -> Result[(), FileError] {
      let mut f = file.open(d, path.of("a"))?
      let mut bytes: List[u8] = []
      failure.check(file.read(d, mut f, mut bytes, 2u64)? == 2u64, Arithmetic())
      failure.check(bytes == [97u8, 98u8], Arithmetic())
      failure.check(file.read(d, mut f, mut bytes, 2u64)? == 1u64, Arithmetic())
      failure.check(bytes == [97u8, 98u8, 99u8], Arithmetic())
      Ok(())
    })
    let _ = file.metadata(d, path.of("a"))?
    let _ = file.metadata(d, path.of("a"))?
    file.write_bytes(output, path.of("release"), b"go")?
    task.await(s, take h)
  })?
  let _ = out.print(io.stdout(rt)? grant_error, "released\n")
  Ok(())
}

@noalloc
fn grant_error(e: GrantError) -> FileError uses {} { NotGranted() }
@noalloc
fn argument_error(e: ArgError) -> FileError uses {} { FileFault(9u32) }
"#;

fn campaign(source: &str, gate: FileReadScript, expected: &[u8]) {
    with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().join("main.hir");
        std::fs::write(&path, source).unwrap();
        let fixture = Fixture {
            args: vec![
                "input".into(),
                "output".into(),
                "--hirsel-run".into(),
                "羊".into(),
            ],
            files: BTreeMap::from([
                ("input/a".into(), b"abc".to_vec()),
                ("input/b".into(), vec![]),
            ]),
            mounts: BTreeMap::from([
                ("input".into(), Access::Read),
                ("output".into(), Access::ReadWrite),
            ]),
            file_reads: BTreeMap::from([("input/a".into(), gate)]),
            ..Default::default()
        };
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let reference = run_observed_snapshot(&mut loaded.engine, &run, (&fixture).into()).unwrap();
        let observable = reference.observation.as_ref().unwrap();
        assert_eq!(observable.output, expected);
        assert!(
            observable.effects.iter().any(|effect| matches!(
                &effect.effect, hirsel_interp::observation::Effect::CapabilityConsumed(event)
                    if event.operation == "file.read.pending"
            )),
            "reference read never parked"
        );
        let observation = Observation::of_reference(observable);
        assert_eq!(observation.exit_code, Some(0), "{observation:?}");
        assert!(observation.failures.is_empty(), "{observation:?}");
        let checked = run_observed_checked(&mut loaded.engine, &run, (&fixture).into()).unwrap();
        assert_eq!(checked.observation, reference.observation);
        let input = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: false },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
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
        let tools = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
        for (name, outcome) in differential_in(
            &input,
            &loaded.entry,
            &observation,
            &matrix,
            &tools,
            scratch.path(),
            &fixture,
        ) {
            assert_eq!(outcome, Outcome::Agree, "{name}");
        }
        let ready = build(&input, &options).unwrap();
        let sites = Sites::new(input.snapshot());
        let mut runnables = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let dir = scratch.path().join(format!("{backend:?}"));
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
                link_fixture_runnable(&ready, &loaded.entry, &objects, &tools, &dir, &fixture)
                    .unwrap();
            runnables.push((runnable, dir));
        }
        for (recorder, dir) in &runnables {
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
            assert_eq!(observe(&recorded, &ready, &sites).unwrap(), observation);
            let bytes = std::fs::read(&tape).unwrap();
            let record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
            assert_eq!(record.arguments, fixture.args);
            record.bound_run().unwrap();
            assert_eq!(record.tree.files, fixture.files);
            assert!(
                record.events.iter().any(|e| matches!(
                    (&e.request, &e.completion),
                    (
                        hirsel_rt::io::Request::FileRead { .. },
                        hirsel_rt::io::Completion::Blocked { committed: false }
                    )
                )),
                "read never parked"
            );
            // Replays need neither initial bytes nor readiness scripts.
            let replay_fixture = Fixture {
                files: BTreeMap::new(),
                file_reads: BTreeMap::new(),
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
                assert_eq!(observe(&replayed, &ready, &sites).unwrap(), observation);
            }
            for replay in [replay_observations, replay_observations_checked] {
                let observed = replay(&path, &tape, None).unwrap();
                assert_eq!(Observation::of_reference(&observed), observation);
                let error = replay(&path, &tape, Some(1)).unwrap_err().to_string();
                assert!(
                    error.contains("ToolingLimit")
                        && error.contains("Steps")
                        && error.contains("limit: 1"),
                    "{error}"
                );
            }
            let mut invalid = record.clone();
            invalid.arguments.push("not in the bound run".into());
            let refused_tape = dir.join("refused-recording");
            std::fs::write(
                &refused_tape,
                hirsel_rt::launch::render_recording(&invalid).unwrap(),
            )
            .unwrap();
            for replay in [replay_observations, replay_observations_checked] {
                let error = replay(&path, &refused_tape, None).unwrap_err().to_string();
                assert!(
                    error.contains("the header disagrees with its run record"),
                    "{error}"
                );
                assert!(
                    !error.contains("Steps") && !error.contains("no semantic observation"),
                    "{error}"
                );
            }
            for (runnable, replay_dir) in &runnables {
                let error = launch_fixture(
                    runnable,
                    &ready.target,
                    &tools,
                    replay_dir,
                    true,
                    Tape::Replay(&refused_tape),
                    &replay_fixture,
                )
                .unwrap_err();
                assert!(
                    error
                        .message
                        .contains("the header disagrees with its run record"),
                    "{error}"
                );
            }
            assert_eq!(std::fs::read(&tape).unwrap(), bytes);
        }
    });
}

#[test]
fn full_args_pending_sibling_cancel_records_and_replays() {
    campaign(
        CANCEL,
        FileReadScript::Pending,
        b"joined; queued suppressed\n",
    );
}

#[test]
fn full_args_after_write_records_and_replays() {
    campaign(
        RELEASE,
        FileReadScript::AfterWrite {
            path: "output/release".into(),
        },
        b"released\n",
    );
}

#[test]
fn fixture_keeps_root_free_entries_and_validates_snapshot_paths() {
    assert!(
        hirsel_rt::io::Tree::fixture(
            BTreeMap::from([("../escape".into(), vec![])]),
            BTreeMap::new()
        )
        .is_err()
    );
    assert!(
        hirsel_rt::io::Tree::fixture(
            BTreeMap::from([("input".into(), vec![])]),
            BTreeMap::from([("input".into(), Access::Read)])
        )
        .is_err()
    );
    with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().join("main.hir");
        std::fs::write(&path, "pub fn main() -> Result[(), ()] { Ok(()) }").unwrap();
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let reference =
            run_observed_snapshot(&mut loaded.engine, &run, Default::default()).unwrap();
        let observation = Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(input).unwrap();
        let matrix = smoke_matrix();
        let tools = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
        for (name, outcome) in differential_in(
            &input,
            &loaded.entry,
            &observation,
            &matrix,
            &tools,
            scratch.path(),
            &Fixture::default(),
        ) {
            assert_eq!(outcome, Outcome::Agree, "{name}");
        }
    });
}
