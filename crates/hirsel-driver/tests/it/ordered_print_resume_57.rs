//! Print admission retains the original request and a checked result cell.
use hirsel_engine::with_compiler_stack;
use hirsel_lir::continuation::{Representation, ResumeAction, RuntimeOpKind};

pub(super) const CERTIFIED_SOURCE: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let value=task.scope(tasks,fn(s:Scope)->u64 {
    let joining=task.async(s,fn()->u64 {
      task.scope(tasks,fn(inner:Scope)->u64 {
        let child=task.async(inner,fn()->u64 {
          let _=out.print(output,"x")
          7u64
        })
        task.await(inner,take child)
      })
    })
    task.await(s,take joining)
  })
  failure.check(value==7u64,Arithmetic())
  Ok(())
}
"#;

#[test]
fn ordered_print_certificate_refuses_forged_cells_and_both_emitters_import_the_helper() {
    with_compiler_stack(|| {
        let program = super::runtime50_continuation::ready(CERTIFIED_SOURCE);
        let plans = hirsel_driver::native::continuation::plans(&program);
        hirsel_driver::native::continuation::validate(&program, &plans).unwrap();
        let units: Vec<_> = program.units.iter().collect();
        let check = |p: &hirsel_lir::continuation::ContinuationPlans| {
            hirsel_lir::continuation::validate_runtime_operations(
                &units,
                &program.table,
                &program.target,
                p,
            )
        };
        check(&plans).unwrap();
        let (&id, plan) = plans
            .plans
            .iter()
            .find(|(_, p)| {
                p.states.iter().any(|s| {
                    s.runtime
                        .as_ref()
                        .is_some_and(|r| r.kind == RuntimeOpKind::OrderedPrint)
                })
            })
            .unwrap_or_else(|| {
                let refused: Vec<_> = program
                    .units
                    .iter()
                    .filter_map(|unit| {
                        let u = unit.unit();
                        let calls: Vec<_> = u
                            .tables
                            .calls
                            .iter()
                            .filter_map(|call| match call.callee {
                                hirsel_lir::ir::Callee::Operation(op) => {
                                    hirsel_lir::continuation::core_name(
                                        &u.tables.operations[op.index()].definition,
                                    )
                                }
                                _ => None,
                            })
                            .collect();
                        calls
                            .iter()
                            .any(|c| c == "out.print")
                            .then(|| (&u.header.definition, calls, plans.get(u.header.instance)))
                    })
                    .collect();
                panic!("no entered print plan: {refused:#?}");
            });
        assert_eq!(plan.representation, Representation::Resumable);
        for field in 0..9 {
            let mut bad = plans.clone();
            let state = bad
                .plans
                .get_mut(&id)
                .unwrap()
                .states
                .iter_mut()
                .find(|s| {
                    s.runtime
                        .as_ref()
                        .is_some_and(|r| r.kind == RuntimeOpKind::OrderedPrint)
                })
                .unwrap();
            let runtime = state.runtime.as_mut().unwrap();
            match field {
                0 => runtime.helper = "hirsel_rt_v2_out_print_at_site".into(),
                1 => runtime.kind = RuntimeOpKind::OrderedSleepUntil,
                2 => runtime.results[0].offset = 0,
                3 => runtime.results[0].bytes += 1,
                4 => runtime.results[0].align *= 2,
                5 => {
                    runtime.results[0].source =
                        hirsel_lir::continuation::RuntimeResultSource::CallResult(
                            hirsel_lir::ir::ValueId(u32::MAX),
                        )
                }
                6 => runtime.results.clear(),
                7 => runtime.results.push(runtime.results[0].clone()),
                _ => state.action = ResumeAction::RetryUnenteredCall(state.boundary.call),
            }
            assert!(check(&bad).is_err(), "imported forged print field {field}");
        }
        for backend in [
            hirsel_driver::native::Backend::Llvm,
            hirsel_driver::native::Backend::Cranelift,
        ] {
            let objects = hirsel_driver::native::emit_backend_objects(
                &program,
                backend,
                hirsel_driver::native::OptLevel::Default,
                hirsel_driver::native::Partition::Whole,
                false,
                None,
            )
            .unwrap();
            assert!(
                objects
                    .iter()
                    .flat_map(|o| &o.imports)
                    .any(|s| s.name == "hirsel_rt_v2_ordered_print"),
                "{backend:?} omitted entered print"
            );
        }
    });
}

#[test]
fn ordered_certified_print_and_join_agree_with_checked_h_and_replay() {
    with_compiler_stack(|| {
        super::runtime50_resume::agree_on(
            CERTIFIED_SOURCE,
            &hirsel_driver::native::Fixture {
                migrate: true,
                ..Default::default()
            },
            "certified print and separate join",
            true,
            &[false],
        );
    });
}

#[test]
fn entered_print_and_join_preserve_canonical_charges_ordinals_and_first_stop() {
    use hirsel_driver::native::{self, *};
    use hirsel_engine::*;
    with_compiler_stack(|| {
        let source = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let value=task.scope(tasks,fn(s:Scope)->u64 {
    let child=task.async(s,fn()->u64 {
      let mut held:List[u64]=[3u64,4u64]
      let _=out.print(output,"x")
      list.push(mut held,11u64)
      held[0]+held[1]
    })
    task.await(s,take child)
  })
  failure.check(value==7u64,Arithmetic())
  let mut sum=0u64
  for i in 0u64..1000u64 {sum=int.wrapping_add(sum,i)}
  failure.check(sum==499500u64,Arithmetic())
  Ok(())
}
"#;
        let fixture = Fixture::default();
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
            (&fixture).into(),
        )
        .unwrap();
        let expected = Observation::of_reference(reference.observation.as_ref().unwrap());
        assert_eq!(expected.output, b"x");
        assert_eq!(expected.exit_code, Some(0));
        let input = engine.export_native_input().unwrap();
        let sites = Sites::new(&input);
        let validated = engine.import_native_input(input.clone()).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let mut program = native::build(&validated, &Options::new(target.clone())).unwrap();
        program.fuel = true;
        let root = tempfile::tempdir().unwrap();
        let configuration = Configuration {
            options: Options::new(target.clone()),
            engine: Engine::Heap { campaign: false },
        };
        let tools = Toolchain::for_matrix(
            &root.path().join("runtime"),
            std::slice::from_ref(&configuration),
        )
        .unwrap();
        for (_, outcome) in differential_in(
            &validated,
            &entry,
            &expected,
            &[configuration],
            &tools,
            root.path(),
            &fixture,
        ) {
            assert_eq!(outcome, Outcome::Agree);
        }
        let mut baselines = std::collections::BTreeMap::new();
        let mut stop_fuel = None;
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let objects = native::emit_backend_objects(
                &program,
                backend,
                OptLevel::Default,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let dir = root.path().join(format!("{backend:?}"));
            std::fs::create_dir_all(&dir).unwrap();
            let executable =
                native::link_standalone_objects(&program, &entry, &objects, &tools, &dir).unwrap();
            for width in [1, 4] {
                for stopped in [false, true] {
                    let mut grants = serde_json::json!({"args":[], "io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]},"placement":{"cores":width}});
                    grants["budgets"] = serde_json::json!({"provider":"budgets.limits@2","fuel":if stopped {stop_fuel.expect("successful trace defines the cut")} else {1000000},"memory_bytes":1048576});
                    let tape = dir.join(format!("{width}-{stopped}.tape"));
                    for mode in ["live", "record", "replay"] {
                        let trace = dir.join(format!("{width}-{stopped}-{mode}.jsonl"));
                        let observation = dir.join("observation");
                        let mut command = std::process::Command::new(&executable);
                        command
                            .env_clear()
                            .env("HIRSEL_RT_OBSERVATION", &observation)
                            .env("HIRSEL_RT_DIAGNOSTIC", "1")
                            .arg("--hirsel-trace-events")
                            .arg(&trace);
                        if mode == "replay" {
                            command.arg("--hirsel-replay").arg(&tape);
                        } else {
                            command.arg("--hirsel-grants").arg(grants.to_string());
                            if mode == "record" {
                                command.arg("--hirsel-record").arg(&tape);
                            }
                        }
                        let output = command.output().unwrap();
                        let launched = Launched {
                            audit_required: true,
                            exit: output.status.code(),
                            signal: None,
                            stdout: output.stdout,
                            stderr: output.stderr,
                            record: std::fs::read_to_string(&observation).unwrap(),
                        };
                        let observed =
                            native::observe(&launched, &program, &sites).unwrap_or_else(|error| {
                                panic!(
                                    "{backend:?}/{width}/{stopped}/{mode}: {error:?} {launched:?}"
                                )
                            });
                        let mut expected = expected.clone();
                        if stopped {
                            expected.exit_code = None;
                            expected.budget = Some(hirsel_run_schema::BudgetKind::Fuel);
                        }
                        assert_eq!(
                            observed, expected,
                            "{backend:?}/{width}/{stopped}/{mode}: {launched:?}"
                        );
                        let bytes = std::fs::read(&trace).unwrap();
                        let rows: Vec<serde_json::Value> = std::str::from_utf8(&bytes)
                            .unwrap()
                            .lines()
                            .map(|line| serde_json::from_str(line).unwrap())
                            .collect();
                        if stop_fuel.is_none() {
                            let mut boundaries = std::collections::BTreeSet::new();
                            let total = rows
                                .iter()
                                .filter(|r| {
                                    boundaries.insert((
                                        r["task"].to_string(),
                                        r["ordinal"].as_u64().unwrap(),
                                        r["retirement"].as_u64().unwrap(),
                                    ))
                                })
                                .flat_map(|r| r["before"].as_array().unwrap())
                                .map(|b| b["fuel"].as_u64().unwrap())
                                .sum::<u64>();
                            stop_fuel = Some(total.checked_sub(1).unwrap());
                        }
                        assert!(rows.iter().any(|r| r["op"] == "task.join"));
                        let output_rows: Vec<_> = rows
                            .iter()
                            .filter(|r| r["outcome"]["kind"] == "host_output")
                            .collect();
                        assert_eq!(output_rows.len(), 1);
                        assert_eq!(output_rows[0]["outcome"]["bytes"], "78");
                        assert!(
                            rows.iter()
                                .flat_map(|r| r["before"].as_array().unwrap())
                                .any(|b| b["fuel"].as_u64().unwrap() > 0)
                        );
                        assert!(
                            rows.iter()
                                .flat_map(|r| r["before"].as_array().unwrap())
                                .any(|b| b["net"].as_i64().unwrap() != 0)
                        );
                        assert_eq!(
                            rows.iter()
                                .filter(|r| r["item"]["kind"] == "terminal")
                                .count(),
                            usize::from(stopped)
                        );
                        if let Some(baseline) = baselines.get(&stopped) {
                            assert_eq!(
                                &bytes, baseline,
                                "canonical {backend:?}/{width}/{stopped}/{mode}"
                            );
                        } else {
                            baselines.insert(stopped, bytes);
                        }
                    }
                }
            }
        }
    });
}
