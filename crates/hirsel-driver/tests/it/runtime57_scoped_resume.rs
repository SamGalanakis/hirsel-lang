//! Known scope callbacks retain an owned result across an entered exit.
use super::runtime50_continuation::ready;
use super::runtime50_resume::{agree_on, routing};
use hirsel_driver::native::{self, Backend, Fixture, Partition};
use hirsel_engine::with_compiler_stack;
use hirsel_lir::continuation::{Representation, ResumeAction, RuntimeOpKind};

const WITNESS: &str = r#"
fn witness(tasks:Tasks,c:MonotonicClock)->unique List[u64] {
  task.scope(tasks,fn(s:Scope)->unique List[u64] {
    let child=task.async(s,fn()->u64 {
      let now=match clock.monotonic(c) {Ok(now)=>now,Error(_)=>return 0u64}
      let at=match instant.checked_add(c,now,100u64) {Some(at)=>at,None()=>return 0u64}
      let _=clock.sleep_until(c,at)
      7u64
    })
    task.detach(s,take child)
    unique [41u64,42u64]
  })
}
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let c=io.monotonic_clock(rt)?
  let rows=task.scope(tasks,fn(outer:Scope)->List[u64] {
    let parent=task.async(outer,fn()->unique List[u64] {witness(tasks,c)})
    task.await(outer,take parent)
  })
  failure.check(rows[0]==41u64 && rows[1]==42u64,Arithmetic())
  let _=out.print(io.stdout(rt)?,f"{rows[0]} {rows[1]}\n")
  Ok(())
}
"#;

#[test]
fn runtime57_known_scope_has_owned_runtime_exit_and_canonical_intrinsic_results() {
    with_compiler_stack(|| {
        let program = ready(WITNESS);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let scopes: Vec<_> = plans
            .plans
            .values()
            .filter(|p| {
                p.states.iter().any(|s| {
                    s.runtime
                        .as_ref()
                        .is_some_and(|r| r.kind == RuntimeOpKind::ScopeExit)
                })
            })
            .collect();
        assert!(!scopes.is_empty(), "{plans:#?}");
        for plan in scopes {
            assert_eq!(plan.representation, Representation::Resumable);
            assert!(
                plan.states
                    .iter()
                    .any(|s| matches!(s.action, ResumeAction::ResumeNested(..)))
            );
            for state in &plan.states {
                if let Some(runtime) = &state.runtime {
                    assert_eq!(
                        state.action,
                        ResumeAction::ResumeRuntime(state.boundary.call)
                    );
                    assert!(runtime.scope.is_some());
                }
            }
        }
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let (_, entered) = routing(&program, backend, Partition::Whole);
            assert!(entered, "{backend:?}");
        }
    });
}

#[test]
fn runtime57_owned_scope_result_agrees_with_reference_heap_and_both_native_backends() {
    with_compiler_stack(|| {
        let fixture = Fixture {
            monotonic: Some(0),
            ..Fixture::default()
        };
        agree_on(
            WITNESS,
            &fixture,
            "scope owned no-await callback",
            true,
            &[false],
        );
    });
}

#[test]
fn runtime57_entered_scope_import_refuses_forged_kind_helper_storage_scope_and_successor() {
    use hirsel_lir::continuation::ContinuationError;
    use hirsel_lir::ir::{BlockId, CleanupAnchorId, ValueId};
    with_compiler_stack(|| {
        let program = ready(WITNESS);
        let plans = native::continuation::plans(&program);
        let (&body, plan) = plans
            .plans
            .iter()
            .find(|(_, p)| {
                p.states.iter().any(|s| {
                    s.runtime
                        .as_ref()
                        .is_some_and(|r| r.kind == RuntimeOpKind::ScopeEnter)
                })
            })
            .expect("certified scope installation");
        let enter = plan
            .states
            .iter()
            .position(|s| {
                s.runtime
                    .as_ref()
                    .is_some_and(|r| r.kind == RuntimeOpKind::ScopeEnter)
            })
            .unwrap();
        let exit = plan
            .states
            .iter()
            .position(|s| {
                s.runtime
                    .as_ref()
                    .is_some_and(|r| r.kind == RuntimeOpKind::ScopeExit)
                    && !s.storage.is_empty()
            })
            .expect("normal exit retains its owned callback result");
        let unit = program
            .units
            .iter()
            .find(|u| u.unit().header.instance == body)
            .unwrap()
            .unit();
        let callback = plan.states[exit]
            .runtime
            .as_ref()
            .unwrap()
            .scope
            .as_ref()
            .unwrap()
            .callback;
        let result = unit.tables.calls[callback.index()].result.unwrap();
        let held_layout = unit.types[unit.values[result.index()].ty.index()].layout;
        let held = plan.states[exit]
            .storage
            .iter()
            .position(|s| s.layout == held_layout)
            .expect("held owned list in frame storage");
        for (label, change) in [
            ("wrong intrinsic kind", 0),
            ("stale helper", 1),
            ("scope from another installation", 2),
            ("missing canonical output", 3),
            ("overlapping output", 4),
            ("missing held result storage", 5),
            ("retry an entered intrinsic", 6),
            ("premature completed action", 7),
            ("wrong successor", 8),
            ("unsupported cleanup", 9),
        ] {
            let mut bad = plans.clone();
            let state = &mut bad.plans.get_mut(&body).unwrap().states
                [if change == 5 { exit } else { enter }];
            match change {
                0 => state.runtime.as_mut().unwrap().kind = RuntimeOpKind::OrderedSleepUntil,
                1 => state.runtime.as_mut().unwrap().helper = "hirsel_rt_v2_task_scope_exit".into(),
                2 => {
                    let p = state.runtime.as_mut().unwrap().scope.as_mut().unwrap();
                    p.scope = ValueId(p.scope.0.wrapping_add(1));
                }
                3 => state.runtime.as_mut().unwrap().results.clear(),
                4 => state.runtime.as_mut().unwrap().results[0].offset = 0,
                5 => {
                    state.storage.remove(held);
                }
                6 => state.action = ResumeAction::RetryUnenteredCall(state.boundary.call),
                7 => {
                    state.action =
                        ResumeAction::AfterEnteredCall(state.boundary.call, state.target.edge)
                }
                8 => {
                    state.target.block =
                        BlockId((state.target.block.0 + 1) % unit.blocks.len() as u32)
                }
                9 => {
                    state.action = ResumeAction::ResumeCleanup {
                        block: state.cleanup.entry,
                        instruction: 0,
                        anchor: CleanupAnchorId(0),
                    }
                }
                _ => unreachable!(),
            }
            let error = native::continuation::validate(&program, &bad).expect_err(label);
            assert!(
                !matches!(
                    error,
                    ContinuationError::Certificate(_) | ContinuationError::Identity
                ),
                "{label}: {error:?}"
            );
            let units = program.units.iter().collect::<Vec<_>>();
            assert!(
                hirsel_lir::continuation::validate_runtime_operations(
                    &units,
                    &program.table,
                    &program.target,
                    &bad
                )
                .is_err(),
                "{label}: emission must reconstruct evidence"
            );
        }
    });
}
#[test]
fn runtime57_owned_scope_child_failure_after_callback_return_preserves_reference_chain() {
    with_compiler_stack(|| {
        let source = WITNESS.replace(
            "      7u64",
            "      failure.check(false,Arithmetic())\n      7u64",
        );
        agree_on(
            &source,
            &Fixture {
                monotonic: Some(0),
                ..Fixture::default()
            },
            "scope child fails after callback result",
            true,
            &[false],
        );
    });
}

const UNTIL: &str = r#"
fn witness(tasks:Tasks,c:MonotonicClock)->u64 {
  let now=match clock.monotonic(c) {Ok(now)=>now,Error(_)=>return 0u64}
  let at=match instant.checked_add(c,now,DEADLINEu64) {Some(at)=>at,None()=>return 0u64}
  let bounded=task.scope_until(tasks,Deadline(clock:c,at:at),fn(s:Scope)->List[u64] {
    let child=task.async(s,fn()->u64 {
      let _=match instant.checked_add(c,now,100u64) {
        Some(child_at)=>clock.sleep_until(c,child_at),
        None()=>clock.sleep_until(c,now),
      }
      7u64
    })
    task.detach(s,take child)
    [41u64,42u64]
  })
  match bounded {
    Finished(rows)=>{failure.check(EXPIRED==false,Arithmetic());rows[0]+rows[1]},
    Expired(rows)=>{failure.check(EXPIRED==true,Arithmetic());rows[0]+rows[1]},
  }
}
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let c=io.monotonic_clock(rt)?
  let answer=task.scope(tasks,fn(outer:Scope)->u64 {
    let parent=task.async(outer,fn()->u64 {witness(tasks,c)})
    task.await(outer,take parent)
  })
  failure.check(answer==83u64,Arithmetic())
  let _=out.print(io.stdout(rt)?,f"{answer}\n")
  Ok(())
}
"#;

fn check_scope_until(deadline: &str, expired: &str) {
    with_compiler_stack(|| {
        let source = UNTIL
            .replace("DEADLINE", deadline)
            .replace("EXPIRED", expired);
        let program = ready(&source);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        assert!(plans.plans.values().any(|p| p.states.iter().any(|s| {
            s.runtime
                .as_ref()
                .is_some_and(|r| r.kind == RuntimeOpKind::ScopeExitUntil)
        })));
        agree_on(
            &source,
            &Fixture {
                monotonic: Some(0),
                ..Fixture::default()
            },
            "owned deadline scope",
            true,
            &[false],
        );
    });
}

#[test]
fn runtime57_scope_until_preserves_owned_result_through_expiry_and_nonexpired_exit() {
    check_scope_until("0", "true");
}

#[test]
fn runtime57_scope_until_expires_during_exit_with_the_same_owned_result() {
    check_scope_until("50", "true");
}

#[test]
fn runtime57_scope_until_finishes_with_the_same_owned_result() {
    check_scope_until("300", "false");
}

#[test]
fn runtime57_unknown_scope_callback_retains_general_stackful_execution() {
    with_compiler_stack(|| {
        let source = r#"
pub fn main(rt:Io,args:Args)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let selected=if arg.len(args)>0u64 {
    fn(s:Scope)->u64 {41u64}
  } else {
    fn(s:Scope)->u64 {42u64}
  }
  let answer=task.scope(tasks,selected)
  let _=out.print(io.stdout(rt)?,f"{answer}\n")
  Ok(())
}
"#;
        let program = ready(source);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        for unit in &program.units {
            let u = unit.unit();
            if u.header.definition.is_core("task.scope") {
                assert!(matches!(
                    plans.get(u.header.instance).unwrap().representation,
                    Representation::Stackful(_)
                ));
            }
        }
        assert!(
            !plans
                .plans
                .values()
                .flat_map(|p| &p.states)
                .any(|s| s.runtime.is_some())
        );
    });
}
// A byte supplied only after the parent has physically left gates child completion.
const PHYSICAL: &str = r#"
fn witness(tasks:Tasks,input:Stdin)->unique List[u64] {
  task.scope(tasks,fn(s:Scope)->unique List[u64] {
    let child=task.async(s,fn()->u64 {
      let mut bytes:List[u8]=[]
      let read=stdin.read(input,mut bytes,1u64)
      failure.check(read==Ok(1u64),Arithmetic())
      7u64
    })
    task.detach(s,take child)
    unique [41u64,42u64]
  })
}
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let input=io.stdin(rt)?
  let rows=task.scope(tasks,fn(outer:Scope)->List[u64] {
    let parent=task.async(outer,fn()->unique List[u64] {witness(tasks,input)})
    let sibling=task.async(outer,fn()->u64 {0u64})
    let rows=task.await(outer,take parent)
    let _=task.await(outer,take sibling)
    rows
  })
  failure.check(rows[0]==41u64 && rows[1]==42u64,Arithmetic())
  let _=out.print(io.stdout(rt)?,f"{rows[0]} {rows[1]}\n")
  Ok(())
}
"#;

#[test]
fn runtime57_free_owned_scope_physically_leaves_its_pending_exit_and_replays() {
    use hirsel_driver::native::{OptLevel, TargetSpec, Toolchain};
    use hirsel_engine::{QueryEngine, check_progress, default_context};
    with_compiler_stack(|| {
        let mut engine: QueryEngine = crate::source_std::engine();
        let file = engine.sources.add("main.hir", PHYSICAL.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let sites = native::Sites::new(&input);
        let validated = engine.import_native_input(input.clone()).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let program = native::build(&validated, &native::Options::new(target.clone())).unwrap();
        let plans = native::continuation::plans(&program);
        let witness = program
            .units
            .iter()
            .find(|u| u.unit().header.definition.0.name == "witness")
            .unwrap();
        assert_eq!(
            plans
                .get(witness.unit().header.instance)
                .unwrap()
                .representation,
            Representation::Resumable,
            "owned parent must enter the scope through a complete retained call chain"
        );
        assert!(plans.plans.values().any(|p| p.states.iter().any(|s| {
            s.runtime
                .as_ref()
                .is_some_and(|r| r.kind == RuntimeOpKind::ScopeExit)
        })));
        let root = tempfile::Builder::new()
            .prefix("scope57-free-")
            .tempdir()
            .unwrap();
        let tools =
            Toolchain::in_tree_diagnostic(&root.path().join("runtime"), &[&target]).unwrap();
        let entry = hirsel_engine::entry_path(&default_context());
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let dir = root.path().join(format!("{backend:?}"));
            std::fs::create_dir_all(dir.join("root")).unwrap();
            let objects = native::emit_backend_objects(
                &program,
                backend,
                OptLevel::Default,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let exe =
                native::link_standalone_objects(&program, &entry, &objects, &tools, &dir).unwrap();
            let grants = dir.join("grants.json");
            std::fs::write(&grants,r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","stdin":"stdin.bytes@1","dirs":[]},"args":[],"placement":{"cores":4}}"#).unwrap();
            let tape = dir.join("run.tape");
            let mut expected = None;
            for replay in [false, true] {
                let observation = dir.join(format!("observation-{replay}"));
                let trace = dir.join(format!("compact-{replay}"));
                let mut command = std::process::Command::new(&exe);
                command
                    .env_clear()
                    .env("HIRSEL_RT_ROOT", dir.join("root"))
                    .env("HIRSEL_RT_DIAGNOSTIC", "1")
                    .env("HIRSEL_RT_OBSERVATION", &observation)
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
                let output = if replay {
                    command
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped());
                    finish_physical_run(command.spawn().unwrap())
                } else {
                    // The child cannot finish before this exact parent's X2 save.
                    // No clock delay, scheduler knob or output-only inference is used.
                    use std::io::Write;
                    command
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped());
                    let mut process = command.spawn().unwrap();
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                    loop {
                        let text = std::fs::read_to_string(&trace).unwrap_or_default();
                        if pending_exit_left(&text) {
                            process.stdin.take().unwrap().write_all(b"x").unwrap();
                            break;
                        }
                        if process.try_wait().unwrap().is_some() {
                            let output = process.wait_with_output().unwrap();
                            panic!("scope exited before its pending save: {text}, {output:?}");
                        }
                        if std::time::Instant::now() >= deadline {
                            process.kill().unwrap();
                            let output = process.wait_with_output().unwrap();
                            panic!("scope did not release its pending stack: {text}, {output:?}");
                        }
                        std::thread::yield_now();
                    }
                    finish_physical_run(process)
                };
                let launched = native::Launched {
                    audit_required: true,
                    exit: output.status.code(),
                    signal: None,
                    stdout: output.stdout,
                    stderr: output.stderr,
                    record: std::fs::read_to_string(&observation).unwrap(),
                };
                let actual = native::observe(&launched, &program, &sites).unwrap();
                assert_eq!(
                    actual.output,
                    b"41 42\n",
                    "{backend:?} replay={replay}: {actual:?}, {}",
                    String::from_utf8_lossy(&launched.stderr)
                );
                if let Some(expected) = &expected {
                    assert_eq!(&actual, expected);
                } else {
                    expected = Some(actual);
                }
                let text = std::fs::read_to_string(&trace).unwrap();
                let lines = text
                    .lines()
                    .map(|line| line.split_whitespace().collect::<Vec<_>>())
                    .collect::<Vec<_>>();
                let pending = lines
                    .iter()
                    .position(|w| {
                        w.first() == Some(&"runtime")
                            && w.get(4) == Some(&"18")
                            && w.get(5) == Some(&"settle")
                    })
                    .unwrap_or_else(|| panic!("known scope exit selected Pending at X2: {text}"));
                let task = lines[pending][1];
                assert!(
                    lines[pending + 1..]
                        .iter()
                        .any(|w| w.first() == Some(&"leave")
                            && w.get(1) == Some(&task)
                            && w.get(4) == Some(&"1")),
                    "scope parent released its stack: {text}"
                );
            }
        }
    });
}

// The diagnostic proves Pending at the entered exit and a subsequent physical
// pool return by that same task; unrelated compact leaves cannot satisfy it.
fn pending_exit_left(text: &str) -> bool {
    let lines = text
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    lines.iter().enumerate().any(|(index, words)| {
        words.first() == Some(&"runtime")
            && words.get(4) == Some(&"18")
            && words.get(5) == Some(&"settle")
            && lines[index + 1..].iter().any(|line| {
                line.first() == Some(&"leave")
                    && line.get(1) == words.get(1)
                    && line.get(4) == Some(&"1")
            })
    })
}

fn finish_physical_run(mut process: std::process::Child) -> std::process::Output {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if process.try_wait().unwrap().is_some() {
            return process.wait_with_output().unwrap();
        }
        if std::time::Instant::now() >= deadline {
            process.kill().unwrap();
            let output = process.wait_with_output().unwrap();
            panic!("scope did not complete after its child gate: {output:?}");
        }
        std::thread::yield_now();
    }
}
