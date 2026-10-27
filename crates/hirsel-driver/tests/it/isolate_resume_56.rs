use hirsel_driver::native::*;
use hirsel_engine::*;

const RESIDUAL_CAPTURE: &str = r#"
resource type Guard { n: u64 }
drop(x) { failure.check(false, Bounds()) }

pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let guard = Guard(n: 7u64)
  let status: Result[u64, Failure] = task.isolate(tasks, fn() -> u64 {
    if guard.n == 0u64 { let _ = take guard }
    7u64
  })
  match status {
    Ok(n) => { let _ = out.print(output, f"ok {n}\n") },
    Error(f) => { let _ = out.print(output, f"caught {f.rule}\n") },
  }
  Ok(())
}
"#;

#[test]
fn isolate_resume_56_residual_capture_semantic_probe() {
    agrees(RESIDUAL_CAPTURE, b"caught MEM-14\n");
}

#[test]
fn isolate_resume_56_failed_body_keeps_primary_before_capture_drop() {
    let source = RESIDUAL_CAPTURE
        .replace("    7u64\n", "    failure.check(false, Arithmetic())\n    7u64\n")
        .replace(
            "f\"caught {f.rule}\\n\")",
            "f\"caught {f.rule}\\n\"); match f.cause { Some(cause) => { let _ = out.print(output, f\"cause {cause.rule}\\n\") }, None => {} }");
    agrees(&source, b"caught SEM-1\ncause MEM-14\n");
}

#[test]
fn isolate_resume_56_capture_drop_failure_discards_owned_result_before_catch() {
    let source = RESIDUAL_CAPTURE
        .replace(
            "pub fn main",
            "resource type Returned { n: u64 }\ndrop(x) { failure.check(false, Arithmetic()) }\n\npub fn main",
        )
        .replace("Result[u64, Failure]", "Result[Returned, Failure]")
        .replace("fn() -> u64", "fn() -> Returned")
        .replace("    7u64\n", "    Returned(n: 9u64)\n")
        .replace("ok {n}", "ok {n.n}")
        .replace(
            "f\"caught {f.rule}\\n\")",
            "f\"caught {f.rule}\\n\"); match f.cause { Some(cause) => { let _ = out.print(output, f\"cause {cause.rule}\\n\") }, None => {} }");
    agrees(&source, b"caught MEM-14\ncause SEM-1\n");
}

#[test]
fn isolate_resume_56_nested_cleanup_catches_preserve_the_active_prefix() {
    agrees(
        r#"
resource type Nested { tasks: Tasks }
drop(x) {
  let tasks = x.tasks
  let child: Result[u64, Failure] = task.isolate(tasks, fn() -> u64 {
    let grandchild: Result[u64, Failure] = task.isolate(tasks, fn() -> u64 {
      failure.check(false, Bounds())
      0u64
    })
    match grandchild { Error(f) => failure.check(f.rule == "MEM-14", Arithmetic()), Ok(_) => failure.check(false, Arithmetic()) }
    failure.check(false, Arithmetic())
    0u64
  })
  match child { Error(f) => failure.check(f.rule == "SEM-1" && f.cause == None(), Bounds()), Ok(_) => failure.check(false, Bounds()) }
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let status: Result[u64, Failure] = task.isolate(tasks, fn() -> u64 {
    let nested = Nested(tasks: tasks)
    failure.check(false, Arithmetic())
    0u64
  })
  match status {
    Error(f) => { failure.check(f.cause == None(), Bounds()); let _ = out.print(output, f"caught {f.rule}\n") },
    Ok(_) => failure.check(false, Bounds()),
  }
  Ok(())
}
"#,
        b"caught SEM-1\n",
    );
}

fn agrees(source: &str, output: &[u8]) {
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let file = engine.sources.add("main.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&default_context());
        let reference = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 56,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let expected = Observation::of_reference(reference.observation.as_ref().unwrap());
        assert_eq!(expected.exit_code, Some(0), "{expected:?}");
        assert_eq!(expected.output, output, "{expected:?}");
        assert!(expected.failures.is_empty(), "{expected:?}");
        eprintln!("ISOLATE56 reference: {expected:?}");
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let options =
            Options::new(hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu());
        let program = build(&validated, &options).unwrap();
        let plans = continuation::plans(&program);
        if let Err(error) = continuation::validate(&program, &plans) {
            for unit in &program.units {
                eprintln!(
                    "ISOLATE PLAN {:?} {} {:?}",
                    unit.unit().header.instance,
                    unit.unit().header.definition,
                    plans
                        .get(unit.unit().header.instance)
                        .map(|p| &p.representation)
                );
            }
            panic!("{error}");
        }
        let mut admitted = 0;
        for unit in &program.units {
            if unit.unit().header.definition.is_core("task.isolate")
                && unit.unit().tables.calls.iter().any(|c| {
                    matches!(
                        c.callee,
                        hirsel_lir::ir::Callee::Bound { .. } | hirsel_lir::ir::Callee::Direct(_)
                    )
                })
            {
                let plan = plans.get(unit.unit().header.instance).unwrap();
                if plan.isolate.is_none() {
                    for body in &program.units {
                        eprintln!(
                            "ISOLATE REFUSAL {:?} {} {:?}",
                            body.unit().header.instance,
                            body.unit().header.definition,
                            plans
                                .get(body.unit().header.instance)
                                .map(|p| &p.representation)
                        );
                    }
                }
                assert!(
                    plan.isolate.is_some(),
                    "source isolate cleanup stayed stackful: {:?}",
                    plan.representation
                );
                admitted += 1;
            }
        }
        assert!(admitted > 0);

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
        let scratch = tempfile::Builder::new()
            .prefix("isolate56-probe-")
            .tempdir()
            .unwrap();
        let toolchain = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
        let results = differential(
            &validated,
            &entry,
            &expected,
            &matrix,
            &toolchain,
            scratch.path(),
        );
        for (configuration, outcome) in &results {
            eprintln!("ISOLATE56 {configuration}: {outcome:?}");
        }
        assert_eq!(results.len(), 3);
        assert!(
            results
                .iter()
                .all(|(_, outcome)| matches!(outcome, Outcome::Agree)),
            "isolate engines disagree"
        );
    });
}
