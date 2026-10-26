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
