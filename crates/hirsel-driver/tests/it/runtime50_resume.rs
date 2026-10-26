//! Resumable continuations run end to end (runtime-native-50): a task body
//! with a validated plan enters its compiled dispatcher through
//! `hirsel_rt_v2_resume_run`, and saving its compact frame at every boundary,
//! or moving the task to another worker at every suspension, changes no
//! observation. Under the shared dispatcher a saved compact frame leaves its
//! stack and activates on another worker's (resume-dispatch-51).
use hirsel_driver::native::{self, *};
use hirsel_engine::*;
use hirsel_lir::continuation::{
    ContinuationError, ContinuationPlan, ContinuationPlans, RegionKind, RegionTokenLayout,
    Representation,
};

use hirsel_syntax::ContentHash;

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

/// A parameterized child: `relay`, entered with its declared parameters (a
/// borrowed sender, the child's unique buffer taken and a scalar seed), keeps
/// a list in its compiler-local region and the buffer across two sends.
const PARAMETERIZED: &str = r#"
fn relay(tasks:Tasks, owned:Sender[u64], take held:List[u64], seed:u64)->u64 {
  let mut tx=sender.dup(tasks,owned)
  let kept:List[u64]=[seed,seed+4u64,seed+6u64]
  let _=channel.send(tasks,mut tx,held[0]+kept[0])
  let _=channel.send(tasks,mut tx,held[1]+kept[1])
  held[2]+held[3]+kept[2]
}

pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let (sender,receiver):(Sender[u64],Receiver[u64])=channel.open(tasks)
    let mut rx=take receiver
    let buffer:List[u64]=[3u64,5u64,7u64]
    let child=task.async(s,fn()->u64 {
      let owned=take sender
      let mut held=take buffer
      list.push(mut held,11u64)
      relay(tasks,owned,take held,13u64)
    })
    let mut sum=0u64
    for _ in 0u64..RECEIVES {
      match channel.receive(tasks,mut rx) {Ok(x)=>{sum=sum+x}, Error(_)=>{}}
    }
    CANCEL
    sum+task.await(s,take child)
  })
  let _=out.print(output,f"{total}\n")
  Ok(())
}
"#;

/// The parameterized program, completed or canceled while `relay` waits at
/// its second send: its region exits and the buffer drops from its records.
fn parameterized_source(canceled: bool) -> String {
    let (receives, cancel) = if canceled {
        ("1u64", "task.cancel(s,child)")
    } else {
        ("2u64", "")
    };
    PARAMETERIZED
        .replace("RECEIVES", receives)
        .replace("CANCEL", cancel)
}

/// The resumable symbols a backend's objects export, and whether a task body
/// enters through the runtime's resume loop.
pub(crate) fn routing(
    program: &ReadyProgram,
    backend: Backend,
    partition: Partition,
) -> (usize, bool) {
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

/// Every native backend agrees with the reference under `fixture`, and with
/// `replay`, its recorded run replays to the same observation.
pub(crate) fn agree(source: &str, fixture: &Fixture, label: &str) {
    agree_replayed(source, fixture, label, false)
}

pub(crate) fn agree_replayed(source: &str, fixture: &Fixture, label: &str, replay: bool) {
    agree_on(source, fixture, label, replay, &[]);
}

/// As [`agree_replayed`], with the heap executor (checked H) once per entry
/// of `heap`, with fault injection where it is true, beside the native cells;
/// the reference's observation.
pub(crate) fn agree_on(
    source: &str,
    fixture: &Fixture,
    label: &str,
    replay: bool,
    heap: &[bool],
) -> Observation {
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
    let mut configurations = vec![
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
    configurations.extend(heap.iter().map(|&campaign| Configuration {
        options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        engine: Engine::Heap { campaign },
    }));
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
    if replay {
        let outcome = real_io_replay_in(
            &validated,
            &entry,
            &expected,
            &toolchain,
            scratch.path(),
            Default::default(),
            fixture,
        );
        assert_eq!(outcome, Outcome::Agree, "{label} replay: {outcome:?}");
    }
    expected
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
            migrate: true,
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

/// Checked H alone beside already qualified native observations.
pub(crate) fn checked_heap_agrees(source: &str) {
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
    let validated = engine.import_native_input(input).unwrap();
    let configurations = [Configuration {
        options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        engine: Engine::Heap { campaign: false },
    }];
    let scratch = tempfile::tempdir().unwrap();
    let tools = Toolchain::for_matrix(&scratch.path().join("runtime"), &configurations).unwrap();
    for (configuration, outcome) in differential_in(
        &validated,
        &entry,
        &expected,
        &configurations,
        &tools,
        scratch.path(),
        &Fixture::default(),
    ) {
        assert_eq!(outcome, Outcome::Agree, "{configuration}: {outcome:?}");
    }
}

/// One compact trace line: `leave` or `activate`, task row, state, worker,
/// pooled; or `hold` or `resume`, task row, 0, worker, false.
type Line = (String, u32, u32, String, bool);

/// Live and replayed standalone runs of `source` on both backends, under
/// Ordered admission on four cores with forced migration, in the diagnostic
/// archive with its compact trace.
pub(crate) fn compact_runs(source: &str, label: &str) -> Vec<Vec<Line>> {
    compact_runs_with(source, label, true, "tasks.cooperative@1")
}

/// As [`compact_runs`] under the `tasks` provider (`tasks.cooperative@1` for
/// Ordered, `tasks.free@1` for Free); without `migrate` no runtime knob is set
/// and frames save only where their tasks wait.
pub(crate) fn compact_runs_with(
    source: &str,
    label: &str,
    migrate: bool,
    tasks: &str,
) -> Vec<Vec<Line>> {
    compact_runs_recording(source, label, migrate, tasks, true)
}

/// As `compact_runs_with`, also allowing ordinary live runs without a tape.
/// Recording runs include their strict reference replay; unrecorded runs
/// still compare every observation to the reference interpreter.
pub(crate) fn compact_runs_recording(
    source: &str,
    label: &str,
    migrate: bool,
    tasks: &str,
    recording: bool,
) -> Vec<Vec<Line>> {
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
            format!(
                r#"{{"io":{{"stdout":"stdout.bytes@1","tasks":"{tasks}","dirs":[]}},"args":[],"placement":{{"cores":4}}}}"#
            ),
        )
        .unwrap();
        let tape = dir.join("run.tape");
        for replay in [false, true] {
            if replay && !recording {
                continue;
            }
            let record = dir.join("observation");
            let trace = dir.join(format!("compact-{replay}"));
            let mut command = std::process::Command::new(&exe);
            command
                .env_clear()
                .env("HIRSEL_RT_ROOT", dir.join("root"))
                .env("HIRSEL_RT_DIAGNOSTIC", "1")
                .env("HIRSEL_RT_OBSERVATION", &record)
                .env("HIRSEL_RT_COMPACT_TRACE", &trace)
                .stdin(std::process::Stdio::null());
            if migrate {
                command.env("HIRSEL_RT_MIGRATE", "1");
            }
            if replay {
                command.arg("--hirsel-replay").arg(&tape);
            } else {
                if recording {
                    command.arg("--hirsel-record").arg(&tape);
                }
                command.arg("--hirsel-grants").arg(&grants);
            }
            let output = command.output().unwrap();
            let cell = format!("{label} {tasks} {backend:?} replay={replay}");
            let observation = std::fs::read_to_string(&record).unwrap_or_else(|error| {
                panic!(
                    "{cell}: observation {record:?}: {error}; child status={:?}; stdout={}; stderr={}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                )
            });
            #[cfg(unix)]
            let signal = {
                use std::os::unix::process::ExitStatusExt;
                output.status.signal()
            };
            #[cfg(not(unix))]
            let signal = None;
            let launched = native::Launched {
                audit_required: true,
                exit: output.status.code(),
                signal,
                stdout: output.stdout,
                stderr: output.stderr,
                record: observation,
            };
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
                        if words[0] == "point" {
                            return (
                                words[0].to_owned(),
                                words[1].parse().unwrap(),
                                words[2].parse().unwrap(),
                                words[3].to_owned(),
                                words.get(4) == Some(&"1"),
                            );
                        }
                        if matches!(words[0], "hold" | "resume") {
                            return (
                                words[0].to_owned(),
                                words[1].parse().unwrap(),
                                0,
                                words[2].to_owned(),
                                false,
                            );
                        }
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
pub(crate) fn exact_compact_custody(trace: &[Line], label: &str) -> usize {
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
            "hold" | "resume" | "point" | "runtime" | "finalizer" => {}
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

/// `relay` (declared parameters, a region record, both sends) and the child
/// that nests its frame.
fn parameterized(plans: &ContinuationPlans) -> (&ContinuationPlan, &ContinuationPlan) {
    let summary = || {
        plans
            .plans
            .values()
            .map(|p| {
                format!(
                    "{:?} parameters {:?} records {} states {:?}",
                    p.representation,
                    p.parameters
                        .iter()
                        .map(|r| (r.passing, r.convention, r.slot.is_some()))
                        .collect::<Vec<_>>(),
                    p.records.len(),
                    p.states
                        .iter()
                        .map(|s| (s.boundary.operation.to_string(), s.callee.is_some()))
                        .collect::<Vec<_>>()
                )
            })
            .collect::<Vec<_>>()
    };
    let relay = plans
        .plans
        .values()
        .find(|p| {
            p.representation == Representation::Resumable
                && p.parameters.len() == 4
                && p.states
                    .iter()
                    .filter(|s| s.boundary.operation.to_string().ends_with("channel/send"))
                    .count()
                    == 2
        })
        .unwrap_or_else(|| panic!("resumable parameterized relay: {:#?}", summary()));
    let child = plans
        .plans
        .values()
        .find(|p| {
            p.states
                .iter()
                .any(|s| s.callee.is_some_and(|c| c.body == relay.body))
        })
        .unwrap_or_else(|| panic!("the relay's caller: {:#?}", summary()));
    (relay, child)
}

/// Rayon's join vectors (`join/test.rs:60-109`) as continuation forms: the
/// local form runs where it started, the migrated one moves at every
/// suspension, the canceled one propagates its failure through cleanup.
#[test]
fn runtime51_parameterized_region_continuation() {
    with_compiler_stack(|| {
        let program = ready(&parameterized_source(false));
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let (relay, child) = parameterized(&plans);
        assert_eq!(child.representation, Representation::Resumable);
        // Every declared parameter is a frame record under its ownership
        // mode, first after the header; the taken buffer's slot is its record.
        assert_eq!(
            relay.parameters[0].offset,
            hirsel_lir::continuation::FRAME_HEADER_BYTES
        );
        let held = &relay.parameters[2];
        assert_eq!(held.convention, hirsel_core_contract::Passing::Take);
        assert!(held.slot.is_some(), "{held:#?}");
        // The local region's record is in the frame, and both sends retain
        // its mark.
        assert_eq!(relay.records.len(), 1, "{relay:#?}");
        let record = relay.records[0];
        assert!(relay.parameters.iter().all(|p| record.offset > p.offset));
        let sends: Vec<_> = relay
            .states
            .iter()
            .filter(|s| s.boundary.operation.to_string().ends_with("channel/send"))
            .collect();
        for state in &sends {
            assert!(
                state.regions.iter().any(|r| r.kind == RegionKind::Mark),
                "{state:#?}"
            );
            assert!(state.storage.iter().all(|s| s.offset >= record.offset + 64));
        }
        assert!(relay.cost.frame_bytes <= hirsel_lir::continuation::MAX_FRAME_BYTES);
        for backend in [Backend::Llvm, Backend::Cranelift] {
            for partition in [Partition::Whole, Partition::Singleton] {
                let (exported, entered) = routing(&program, backend, partition);
                assert!(
                    exported >= 3 && entered,
                    "{backend:?} {partition:?}: {exported} {entered}"
                );
            }
        }

        // Forged parameter owners, region lifetimes and resume targets.
        let refused = |edit: &dyn Fn(&mut ContinuationPlan)| {
            let mut bad = plans.clone();
            edit(bad.plans.get_mut(&relay.body).unwrap());
            native::continuation::validate(&program, &bad).expect_err("a forged plan was imported")
        };
        let checks: Vec<(&str, ContinuationError, fn(&ContinuationError) -> bool)> = vec![
            (
                "parameter holding",
                refused(&|p| {
                    let r = &mut p.parameters[2];
                    r.holding = if r.holding == hirsel_lir::ir::Holding::Owned {
                        hirsel_lir::ir::Holding::Plain
                    } else {
                        hirsel_lir::ir::Holding::Owned
                    }
                }),
                |e| matches!(e, ContinuationError::Parameter(_, 2)),
            ),
            (
                "parameter ownership mode",
                refused(&|p| p.parameters[2].convention = hirsel_core_contract::Passing::Borrow),
                |e| matches!(e, ContinuationError::Parameter(_, 2)),
            ),
            (
                "parameter passed by the caller's storage",
                refused(&|p| {
                    p.parameters[1].passing =
                        hirsel_lir::abi_types::ParameterPassing::ExclusiveAddress
                }),
                |e| matches!(e, ContinuationError::Parameter(_, 1)),
            ),
            (
                "parameter value",
                refused(&|p| p.parameters.swap(1, 2)),
                |e| matches!(e, ContinuationError::Parameter(_, 1)),
            ),
            (
                "parameter record in the header",
                refused(&|p| p.parameters[0].offset = 0),
                |e| matches!(e, ContinuationError::Parameter(_, 0)),
            ),
            (
                "dropped parameter record",
                refused(&|p| {
                    p.parameters.pop();
                }),
                |e| matches!(e, ContinuationError::Parameter(_, _)),
            ),
            (
                "region record left on the stack",
                refused(&|p| {
                    p.records.clear();
                }),
                |e| matches!(e, ContinuationError::RegionRecord(_, _)),
            ),
            (
                "region record over a parameter",
                refused(&|p| p.records[0].offset = p.parameters[3].offset),
                |e| matches!(e, ContinuationError::RegionRecord(_, 0)),
            ),
            (
                "region token outlives its region",
                refused(&|p| {
                    let n = p.states.len() - 1;
                    let l = p.states[n].live[0].clone();
                    p.states[n].regions.push(RegionTokenLayout {
                        value: l.value,
                        layout: l.layout,
                        kind: RegionKind::Arena,
                    })
                }),
                |e| matches!(e, ContinuationError::RegionToken(..)),
            ),
            (
                "dropped region token",
                refused(&|p| {
                    let n = p.states.len() - 1;
                    p.states[n].regions.clear()
                }),
                |e| matches!(e, ContinuationError::RegionToken(..)),
            ),
            (
                "resume target",
                refused(&|p| {
                    let n = p.states.len() - 1;
                    p.states[n].target.block = p.states[0].target.block
                }),
                |e| matches!(e, ContinuationError::ResumeLabel(..)),
            ),
            (
                "certificate over the records",
                refused(&|p| p.certificate.identity = ContentHash::of_bytes(b"cached")),
                |e| matches!(e, ContinuationError::Certificate(_)),
            ),
        ];
        for (name, error, expected) in &checks {
            assert!(expected(error), "{name}: {error:?}");
        }
        let mut bad = plans.clone();
        bad.identity = ContentHash::of_bytes(b"other partition");
        assert_eq!(
            native::continuation::validate(&program, &bad),
            Err(ContinuationError::Identity)
        );

        let local = Fixture::default();
        let migrate = Fixture {
            migrate: true,
            ..Fixture::default()
        };
        let (done, canceled) = (parameterized_source(false), parameterized_source(true));
        agree(&done, &local, "parameterized local");
        agree_replayed(&done, &migrate, "parameterized migrate", true);
        agree(&canceled, &migrate, "parameterized canceled");
        // Under the shared dispatcher the saved frame, its parameter and
        // region records included, leaves its stack and activates on another
        // worker's, live and replayed.
        for (index, trace) in compact_runs(&done, "parameterized").iter().enumerate() {
            let cell = format!("parameterized run {index}");
            assert!(exact_compact_custody(trace, &cell) >= 1, "{cell}");
        }
    });
}

/// `CHILDREN` children (the bound is one past them), each sending twice on a
/// channel the root holds, whose results the root collects with `task.all`.
const DEFAULT_FANOUT: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let (sender,receiver):(Sender[u64],Receiver[u64])=channel.open(tasks)
    let mut rx=take receiver
    let mut set:TaskSet[Result[u64,()]] of s=task.set(s)
    for seed in 1u64..CHILDREN {
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
    for _ in 1u64..CHILDREN {
      for _ in 0u64..2u64 {
        match channel.receive(tasks,mut rx) {Ok(x)=>{sum=sum+x*x}, Error(_)=>{}}
      }
    }
    if let Ok(rows)=std@1/task.all(s,take set) {for x in rows {sum=sum+x}}
    sum
  })
  let _=out.print(output,f"{total}\n")
  Ok(())
}
"#;

/// The most stacks suspended children held at once: a child holds its stack
/// from a `hold` until its next `resume`. Task row 0 is the root.
pub(crate) fn peak_held(trace: &[Line]) -> usize {
    let mut held = std::collections::BTreeSet::new();
    let mut peak = 0;
    for (kind, task, ..) in trace {
        match kind.as_str() {
            "hold" if *task != 0 => {
                held.insert(*task);
                peak = peak.max(held.len());
            }
            "resume" | "activate" => {
                held.remove(task);
            }
            _ => {}
        }
    }
    peak
}

/// Go's `TestStackMem` (`runtime/stack_test.go:24-79`) as a native fanout:
/// with no runtime knob, N children whose ends hold their results until the
/// root's `task.all` hold at most one stack per worker (4 cores), not one
/// each, under Ordered and Free, on both backends, live and replayed. Each
/// child's end holds its result from its saved compact frame, and a Free
/// replay's send waits for its tape turn from it too (its library body
/// returns the suspension to the boundary call); every observation equals the
/// reference interpreter's (`compact_runs_with`).
#[test]
fn runtime52_parked_fanout_holds_worker_stacks() {
    with_compiler_stack(|| {
        for (children, tasks) in [8u64, 32]
            .into_iter()
            .flat_map(|n| [(n, "tasks.cooperative@1"), (n, "tasks.free@1")])
        {
            let source = DEFAULT_FANOUT.replace("CHILDREN", &format!("{}u64", children + 1));
            let label = format!("parked fanout {children} {tasks}");
            for (index, trace) in compact_runs_with(&source, &label, false, tasks)
                .iter()
                .enumerate()
            {
                let cell = format!("{label} run {index}");
                let leaves = trace.iter().filter(|line| line.0 == "leave").count();
                let peak = peak_held(trace);
                eprintln!("{cell}: leaves {leaves} peak held {peak}");
                assert!(leaves as u64 >= children, "{cell}: {leaves} leaves");
                assert!(peak <= 4, "{cell}: {peak} suspended children held stacks");
                let mut saved = std::collections::BTreeMap::new();
                for (kind, task, state, _, flag) in trace {
                    match kind.as_str() {
                        "leave" => {
                            assert!(*flag, "{cell}: task {task} kept the stack it left");
                            assert!(saved.insert(*task, *state).is_none(), "{cell}");
                        }
                        "activate" => {
                            assert_eq!(saved.remove(task), Some(*state), "{cell}: task {task}");
                        }
                        _ => {}
                    }
                }
                assert!(
                    saved.is_empty(),
                    "{cell}: frames never activated: {saved:?}"
                );
            }
        }
    });
}
