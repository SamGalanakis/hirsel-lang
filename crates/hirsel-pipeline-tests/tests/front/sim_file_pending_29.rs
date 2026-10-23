//! Scripted regular files: release, structured cancellation, and replay on every engine.
use hirsel_driver::{compiler_adapters, native::*};
use hirsel_engine::{
    QueryEngine, RunConfiguration, SimulationConfig, check_progress, default_context,
    default_environment, entry_path, run_observed_checked, run_observed_snapshot,
    with_compiler_stack,
};
use hirsel_run_schema::FileReadScript;
use std::{collections::BTreeMap, path::Path};

const RELEASE: &str = r#"
pub fn trial(t: Test) -> Result[(), FileError] {
  let tasks = test.tasks(t)
  let d = test.dir(t, [])
  file.write_bytes(d, path.of("a"), b"abc")?
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
    file.write_bytes(d, path.of("release"), b"go")?
    task.await(s, take h)
  })?
  test.log(t, "released")
  Ok(())
}
"#;

// Two admitted regular files at jobs=2. a parks; b's declared size is one but
// its body is empty. The error cancels and joins a before the next batch c.
const CANCEL: &str = r#"
resource type Joined { dir: Dir }
drop(x) { let _ = file.write_bytes(x.dir, path.of("joined"), b"yes") }
pub fn trial(t: Test) -> Result[(), FileError] {
  let tasks = test.tasks(t)
  let d = test.dir(t, [])
  file.write_bytes(d, path.of("a"), b"x")?
  file.write_bytes(d, path.of("b"), b"")?
  let problem = task.scope(tasks, fn(s: Scope) -> bool {
    let mut pending: TaskSet[Result[(), FileError]] of s = task.set(s)
    task.add(s, mut pending, task.async(s, fn() -> Result[(), FileError] {
      let guard = Joined(dir: d)
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
    if !problem { let _ = file.write_bytes(d, path.of("c"), b"queued") }
    problem
  })
  failure.check(problem, Arithmetic())
  failure.check(file.read_bytes(d, path.of("joined"))? == b"yes", Arithmetic())
  failure.check(file.read_bytes(d, path.of("c")) == Error(NotFound()), Arithmetic())
  test.log(t, "joined; queued suppressed")
  Ok(())
}
"#;

fn campaign(name: &str, source: &str, gate: FileReadScript) {
    with_compiler_stack(|| {
        let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        std::fs::create_dir_all(&scratch).unwrap();
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let context = default_context();
        let file = engine
            .sources
            .add(format!("{name}.hir"), source.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let mut entry = entry_path(&context);
        entry.0.name = "trial".into();
        let run = RunConfiguration {
            entry: entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let file_reads = BTreeMap::from([("a".into(), gate)]);
        let config = SimulationConfig {
            file_reads: file_reads.clone(),
            ..Default::default()
        };
        let execution = run_observed_snapshot(&mut engine, &run, config.clone()).unwrap();
        let reference = Observation::of_reference(execution.observation.as_ref().unwrap());
        assert_eq!(
            reference.blocked,
            name == "sim-file-blocked-29",
            "{reference:?}"
        );
        assert_eq!(
            reference.exit_code,
            if reference.blocked { None } else { Some(0) },
            "{reference:?}"
        );
        assert!(reference.failures.is_empty(), "{reference:?}");
        let events: Vec<_> = execution
            .observation
            .as_ref()
            .unwrap()
            .effects
            .iter()
            .filter_map(|effect| {
                if let hirsel_interp::observation::Effect::CapabilityConsumed(event) =
                    &effect.effect
                {
                    Some(event.clone())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            events.iter().any(|e| e.operation == "file.read.pending"),
            "read never parked"
        );
        let replay = RunConfiguration {
            inputs: events,
            ..run.clone()
        };
        let replayed = run_observed_snapshot(&mut engine, &replay, config.clone()).unwrap();
        assert_eq!(replayed.observation, execution.observation);
        let checked = run_observed_checked(&mut engine, &run, config).unwrap();
        assert_eq!(checked.observation, execution.observation);
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let options =
            Options::new(hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu());
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
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let tools = Toolchain::for_matrix(&scratch.join("runtime"), &matrix).unwrap();
        let fixture = Fixture {
            file_reads,
            ..Default::default()
        };
        for (configuration, outcome) in differential_in(
            &input, &entry, &reference, &matrix, &tools, &scratch, &fixture,
        ) {
            assert_eq!(outcome, Outcome::Agree, "{name}: {configuration}");
        }
        assert_eq!(
            real_io_replay_in(
                &input,
                &entry,
                &reference,
                &tools,
                &scratch,
                Default::default(),
                &fixture
            ),
            Outcome::Agree
        );
        let recording = hirsel_rt::launch::parse_recording(
            &hirsel_rt::launch::read_recording_text(&scratch.join("io-tape")).unwrap(),
        )
        .unwrap();
        assert!(
            recording.events.iter().any(|event| matches!(
                (&event.request, &event.completion),
                (
                    hirsel_rt::io::Request::FileRead { .. },
                    hirsel_rt::io::Completion::Blocked { committed: false }
                )
            )),
            "native read never parked"
        );
        if name == "sim-file-release-29" {
            assert!(
                recording
                    .events
                    .iter()
                    .any(|event| matches!(event.request, hirsel_rt::io::Request::Ready { .. })),
                "native read never released"
            );
        }
    });
}

#[test]
fn sim_file_pending_29_release_and_replay() {
    campaign(
        "sim-file-release-29",
        RELEASE,
        FileReadScript::AfterWrite {
            path: "release".into(),
        },
    );
}

#[test]
fn sim_file_pending_29_packtree_cancel_and_replay() {
    campaign("sim-file-packtree-29", CANCEL, FileReadScript::Pending);
}

#[test]
fn sim_file_pending_29_parhash_cancel_and_replay() {
    // parhash uses the same bounded TaskSet shape, with a typed worker error
    // rather than the short-body check that packtree performs after its read.
    let source = CANCEL.replace(
        "if n != 1u64 { Error(FileFault(7u32)) } else { Ok(()) }",
        "failure.check(n == 0u64, Arithmetic()); Error(FileFault(7u32))",
    );
    campaign("sim-file-parhash-29", &source, FileReadScript::Pending);
}

#[test]
fn sim_file_pending_29_gates_are_local_to_tree_and_path() {
    use hirsel_run_schema::FileReadiness;
    let mut state = FileReadiness::new(BTreeMap::from([
        (
            "a".into(),
            FileReadScript::AfterWrite {
                path: "release".into(),
            },
        ),
        ("b".into(), FileReadScript::Pending),
    ]));
    assert!(state.blocked(1, "a"));
    assert!(!state.blocked(1, "other"));
    assert!(!state.wrote(1, "wrong"));
    assert!(state.wrote(2, "release"));
    assert!(state.blocked(1, "a"));
    assert!(!state.blocked(2, "a"));
    assert!(state.wrote(1, "release"));
    assert!(!state.wrote(1, "release"));
    assert!(!state.blocked(1, "a"));
    assert!(state.blocked(1, "b"));
}

#[test]
fn sim_file_pending_29_never_changes_host_file_reads() {
    use hirsel_rt::{io::*, io_real::RealIo};
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sim-file-host-29");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a"), b"host").unwrap();
    let mut io = RealIo::grant(&root).unwrap();
    io.set_file_read_script(BTreeMap::from([("a".into(), FileReadScript::Pending)]));
    let Completion::Handle(Ok(directory)) = io
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap()
    else {
        panic!("directory")
    };
    let Completion::Handle(Ok(file)) = io
        .call(Request::Open {
            directory,
            path: "a".into(),
        })
        .unwrap()
    else {
        panic!("file")
    };
    io.enter(42);
    assert_eq!(
        io.call(Request::FileRead {
            directory,
            file,
            max: 4
        })
        .unwrap(),
        Completion::Bytes(Ok(b"host".to_vec()))
    );
    io.finish(42);
}

#[test]
fn sim_file_pending_29_without_release_blocks_and_replays() {
    campaign(
        "sim-file-blocked-29",
        r#"
pub fn trial(t: Test) -> Result[(), FileError] {
  let d = test.dir(t, [(path.of("a"), b"x")])
  let mut f = file.open(d, path.of("a"))?
  let mut bytes: List[u8] = []
  let _ = file.read(d, mut f, mut bytes, 1u64)?
  failure.check(false, Arithmetic())
  Ok(())
}
"#,
        FileReadScript::Pending,
    );
}
