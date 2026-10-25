//! Resumable continuations run end to end (runtime-native-50): a task body
//! with a validated plan enters its compiled dispatcher through
//! `hirsel_rt_v2_resume_run`, and saving its compact frame at every boundary,
//! or moving the task to another worker at every suspension, changes no
//! observation. Under the shared dispatcher a saved compact frame leaves its
//! stack and activates on another worker's (resume-dispatch-51).
use hirsel_driver::native::{self, *};
use hirsel_engine::*;
use hirsel_lir::continuation::Representation;

use super::runtime50_continuation::{TWO_BOUNDARY, child, ready};

/// The two-boundary child canceled while it waits at its second send.
const CANCELED: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let (sender,receiver):(Sender[u64],Receiver[u64])=channel.open(tasks)
    let mut rx=take receiver
    let buffer:List[u64]=[3u64,5u64,7u64]
    let child=task.async(s,fn()->u64 {
      let mut tx=take sender
      let mut held=take buffer
      list.push(mut held,11u64)
      let _=channel.send(tasks,mut tx,held[0])
      let _=channel.send(tasks,mut tx,held[1])
      held[2]+held[3]
    })
    let mut sum=0u64
    match channel.receive(tasks,mut rx) {Ok(x)=>{sum=sum+x}, Error(_)=>{}}
    task.cancel(s,child)
    sum+task.await(s,take child)
  })
  let _=out.print(output,f"{total}\n")
  Ok(())
}
"#;

/// Several children, each crossing both boundaries, so migration moves
/// frames between workers while others are saved.
const FANOUT: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let (sender,receiver):(Sender[u64],Receiver[u64])=channel.open(tasks)
    let mut rx=take receiver
    let mut set:TaskSet[Result[u64,()]] of s=task.set(s)
    for seed in 1u64..5u64 {
      let dup=sender.dup(tasks,sender)
      task.add(s,mut set,task.async(s,fn()->Result[u64,()] {
        let mut tx=take dup
        let mut held:List[u64]=[seed,seed*3u64]
        list.push(mut held,seed*5u64)
        let _=channel.send(tasks,mut tx,held[0])
        let _=channel.send(tasks,mut tx,held[1])
        Ok(held[2])
      }))
    }
    let mut sum=0u64
    for _ in 0u64..8u64 {
      match channel.receive(tasks,mut rx) {Ok(x)=>{sum=sum*31u64+x}, Error(_)=>{}}
    }
    if let Ok(rows)=std@1/task.all(s,take set) {for x in rows {sum=sum+x}}
    sum
  })
  let _=out.print(output,f"{total}\n")
  Ok(())
}
"#;

/// The resumable symbols a backend's objects export, and whether a task body
/// enters through the runtime's resume loop.
fn routing(program: &ReadyProgram, backend: Backend, partition: Partition) -> (usize, bool) {
    let objects = native::emit_backend_objects(
        program,
        backend,
        hirsel_lir::emission::OptLevel::Default,
        partition,
        false,
        None,
    )
    .unwrap();
    let exported = objects
        .iter()
        .flat_map(|o| &o.exports)
        .filter(|s| s.name.ends_with("_resume"))
        .count();
    let entered = objects
        .iter()
        .flat_map(|o| &o.imports)
        .any(|s| s.name == "hirsel_rt_v2_resume_run");
    (exported, entered)
}

/// Every native backend agrees with the reference under `fixture`.
fn agree(source: &str, fixture: &Fixture, label: &str) {
    let mut engine = crate::source_std::engine();
    let file = engine.sources.add("main.hir", source.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let entry = entry_path(&default_context());
    let reference = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 50,
            inputs: vec![],
        },
        fixture.into(),
    )
    .unwrap();
    let expected = Observation::of_reference(reference.observation.as_ref().unwrap());
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    let configurations = [
        Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Default,
            },
        },
        Configuration {
            options: options.clone(),
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Default,
            },
        },
        Configuration {
            options,
            engine: Engine::Cranelift {
                partition: Partition::Singleton,
                runtime: Runtime::Default,
            },
        },
    ];
    let scratch = tempfile::Builder::new()
        .prefix("runtime50-resume-")
        .tempdir()
        .unwrap();
    let toolchain =
        Toolchain::for_matrix(&scratch.path().join("runtime"), &configurations).unwrap();
    for (configuration, outcome) in differential_in(
        &validated,
        &entry,
        &expected,
        &configurations,
        &toolchain,
        scratch.path(),
        fixture,
    ) {
        assert_eq!(
            outcome,
            Outcome::Agree,
            "{label} {configuration}: {outcome:?} (expected {expected:?})"
        );
    }
}

#[test]
fn runtime50_two_boundary_unique_region_continuation() {
    with_compiler_stack(|| {
        let program = ready(TWO_BOUNDARY);
        let plans = native::continuation::plans(&program);
        assert_eq!(child(&plans).representation, Representation::Resumable);
        for backend in [Backend::Llvm, Backend::Cranelift] {
            for partition in [Partition::Whole, Partition::Singleton] {
                let (exported, entered) = routing(&program, backend, partition);
                assert!(
                    exported >= 2 && entered,
                    "{backend:?} {partition:?}: {exported} {entered}"
                );
            }
        }
        let local = Fixture::default();
        let published = Fixture {
            resume_save: true,
            ..Fixture::default()
        };
        for (source, label) in [(TWO_BOUNDARY, "two-boundary"), (CANCELED, "canceled")] {
            agree(source, &local, &format!("{label} local"));
            agree(source, &published, &format!("{label} published"));
        }
    });
}

#[test]
fn runtime50_migrate_every_suspension_continuations() {
    with_compiler_stack(|| {
        let migrate = Fixture {
            migrate: true,
            ..Fixture::default()
        };
        for (source, label) in [(TWO_BOUNDARY, "two-boundary"), (FANOUT, "fanout")] {
            agree(source, &migrate, label);
        }
    });
}

/// One compact trace line: `leave` or `activate`, task row, state, worker, pooled.
type Line = (String, u32, u32, String, bool);

/// Live and replayed standalone runs of `source` on both backends, under
/// Ordered admission on four cores with forced migration, in the diagnostic
/// archive with its compact trace.
fn compact_runs(source: &str, label: &str) -> Vec<Vec<Line>> {
    let mut engine = crate::source_std::engine();
    let file = engine.sources.add("main.hir", source.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let entry = entry_path(&default_context());
    let reference = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 50,
            inputs: vec![],
        },
        (&Fixture::default()).into(),
    )
    .unwrap();
    let expected = Observation::of_reference(reference.observation.as_ref().unwrap());
    let input = engine.export_native_input().unwrap();
    let sites = native::Sites::new(&input);
    let validated = engine.import_native_input(input.clone()).unwrap();
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let ready = native::build(&validated, &Options::new(target.clone())).unwrap();
    let root = tempfile::Builder::new()
        .prefix("runtime50-compact-")
        .tempdir()
        .unwrap();
    let tools = Toolchain::in_tree_diagnostic(&root.path().join("runtime"), &[&target]).unwrap();
    let mut traces = Vec::new();
    for backend in [Backend::Llvm, Backend::Cranelift] {
        let dir = root.path().join(format!("{backend:?}"));
        std::fs::create_dir_all(dir.join("root")).unwrap();
        let objects = native::emit_backend_objects(
            &ready,
            backend,
            hirsel_lir::emission::OptLevel::Default,
            Partition::Whole,
            false,
            None,
        )
        .unwrap();
        let exe = native::link_standalone_objects(&ready, &entry, &objects, &tools, &dir).unwrap();
        let grants = dir.join("grants.json");
        std::fs::write(
            &grants,
            r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]},"args":[],"placement":{"cores":4}}"#,
        )
        .unwrap();
        let tape = dir.join("run.tape");
        for replay in [false, true] {
            let record = dir.join("observation");
            let trace = dir.join(format!("compact-{replay}"));
            let mut command = std::process::Command::new(&exe);
            command
                .env_clear()
                .env("HIRSEL_RT_ROOT", dir.join("root"))
                .env("HIRSEL_RT_DIAGNOSTIC", "1")
                .env("HIRSEL_RT_OBSERVATION", &record)
                .env("HIRSEL_RT_MIGRATE", "1")
                .env("HIRSEL_RT_COMPACT_TRACE", &trace)
                .stdin(std::process::Stdio::null());
            if replay {
                command.arg("--hirsel-replay").arg(&tape);
            } else {
                command
                    .arg("--hirsel-record")
                    .arg(&tape)
                    .arg("--hirsel-grants")
                    .arg(&grants);
            }
            let output = command.output().unwrap();
            let launched = native::Launched {
                audit_required: true,
                exit: output.status.code(),
                signal: None,
                stdout: output.stdout,
                stderr: output.stderr,
                record: std::fs::read_to_string(&record).unwrap(),
            };
            let cell = format!("{label} {backend:?} replay={replay}");
            assert_eq!(
                native::observe(&launched, &ready, &sites).unwrap(),
                expected,
                "{cell}: {}",
                String::from_utf8_lossy(&launched.stderr)
            );
            let lines = std::fs::read_to_string(&trace).unwrap();
            traces.push(
                lines
                    .lines()
                    .map(|line| {
                        let words: Vec<&str> = line.split(' ').collect();
                        (
                            words[0].to_owned(),
                            words[1].parse().unwrap(),
                            words[2].parse().unwrap(),
                            words[3].to_owned(),
                            words.get(4) == Some(&"1"),
                        )
                    })
                    .collect(),
            );
        }
    }
    traces
}

/// Each compact frame left its stack, which its worker pooled, and activated
/// once at exactly the state it saved, on another worker: never its entry,
/// never twice. Every task's last leave is followed by its activation.
fn exact_compact_custody(trace: &[Line], label: &str) -> usize {
    let mut saved: std::collections::BTreeMap<u32, (u32, String)> = Default::default();
    let mut activations = 0;
    for (kind, task, state, worker, pooled) in trace {
        match kind.as_str() {
            "leave" => {
                assert!(*pooled, "{label}: task {task} kept the stack it left");
                assert!(*state > 0, "{label}: task {task} saved at its entry");
                assert!(
                    saved.insert(*task, (*state, worker.clone())).is_none(),
                    "{label}: task {task} saved twice without an activation"
                );
            }
            "activate" => {
                let (left, from) = saved
                    .remove(task)
                    .unwrap_or_else(|| panic!("{label}: task {task} activated unsaved"));
                assert_eq!(*state, left, "{label}: task {task} resumed another state");
                assert_ne!(*worker, from, "{label}: task {task} did not migrate");
                activations += 1;
            }
            other => panic!("{label}: {other}"),
        }
    }
    assert!(
        saved.is_empty(),
        "{label}: frames never activated: {saved:?}"
    );
    activations
}

#[test]
fn runtime50_compact_frames_migrate_off_released_stacks_and_replay() {
    with_compiler_stack(|| {
        for (source, label, children) in [
            (TWO_BOUNDARY, "two-boundary", 1),
            (FANOUT, "fanout", 4),
            (CANCELED, "canceled", 1),
        ] {
            for (index, trace) in compact_runs(source, label).iter().enumerate() {
                let cell = format!("{label} run {index}");
                let activations = exact_compact_custody(trace, &cell);
                // Each child saves at its first send; one that is not canceled
                // first saves at its second too.
                assert!(activations >= children, "{cell}: {activations}");
            }
        }
    });
}
