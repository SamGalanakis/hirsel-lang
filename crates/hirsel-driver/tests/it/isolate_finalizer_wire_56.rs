//! Source finalizers enter the existing retained release cursor exactly once.
use hirsel_driver::native;

const SOURCE: &str = r#"
resource type Guard { output: Stdout, seed: u64 }
drop(x) { let _ = out.print(x.output, "drop\n") }
fn held(output: Stdout, seed: u64) -> u64 {
  let guard = box Guard(output: output, seed: seed)
  guard.seed
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let total = task.scope(tasks, fn(s: Scope) -> u64 {
    let mut set: TaskSet[Result[u64, ()]] of s = task.set(s)
    for seed in 1u64..5u64 {
      task.add(s, mut set, task.async(s, fn() -> Result[u64, ()] {
        Ok(held(output, seed))
      }))
    }
    let mut total = 0u64
    if let Ok(rows) = std@1/task.all(s, take set) { for row in rows { total = total + row } }
    total
  })
  failure.check(total == 10u64, Arithmetic())
  Ok(())
}
"#;

#[test]
fn source_finalizer_resolver_is_checked_at_both_native_entry_points() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_lir::emission::{EmitInputs, OptLevel};
        let program = super::runtime50_continuation::ready(SOURCE);
        let units: Vec<_> = program.units.iter().collect();
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let (body, ordinal, coverage) = plans
            .plans
            .iter()
            .find_map(|(id, plan)| {
                plan.cleanups.iter().enumerate().find_map(|(i, state)| {
                    state
                        .finalizers
                        .as_ref()
                        .map(|coverage| (*id, i, coverage.clone()))
                })
            })
            .unwrap_or_else(|| panic!("no source finalizer root: {:#?}", plans.plans));
        assert_eq!(coverage.finalizers.len(), 1);
        let finalizer = &coverage.finalizers[0];
        let emit = |proposed: &hirsel_lir::continuation::ContinuationPlans, accepted| {
            let empty = std::collections::BTreeSet::new();
            let inputs = EmitInputs {
                ahead: None,
                continuation: Some(proposed),
                alternate: false,
                index: None,
                input: &program.input,
                target: &program.target,
                layouts: &program.table,
                keys: &program.keys,
                manifest: &program.manifest,
                statics: &program.statics,
                inline_instances: &program.inline_instances,
                available_externally: &empty,
                callbacks: !program.callbacks.is_empty(),
                tracking: program.tracking(),
                foreign: &program.foreign_dispatch,
                opt: OptLevel::Default,
                measurement: false,
                fuel: program.fuel,
                verify_backend: true,
            };
            for (backend, result) in [
                (
                    "LLVM",
                    hirsel_llvm::emit_resume_object(&units, &inputs, proposed)
                        .map_err(|e| e.to_string()),
                ),
                (
                    "Cranelift",
                    hirsel_cranelift::emit_resume_object(&units, &inputs, proposed)
                        .map_err(|e| e.to_string()),
                ),
            ] {
                assert_eq!(result.is_ok(), accepted, "{backend}: {result:?}");
            }
        };
        emit(&plans, true);
        for mutation in 0..8 {
            let mut bad = plans.clone();
            let state = &mut bad.plans.get_mut(&body).unwrap().cleanups[ordinal];
            match mutation {
                0 => state.finalizers = None,
                1 => state.finalizers.as_mut().unwrap().finalizers.clear(),
                2 => {
                    state.finalizers.as_mut().unwrap().finalizers[0]
                        .nested
                        .offset = 0
                }
                3 => {
                    state.finalizers.as_mut().unwrap().finalizers[0]
                        .source
                        .borrowed_self = hirsel_lir::ir::ValueId(u32::MAX)
                }
                4 => {
                    state.finalizers.as_mut().unwrap().finalizers[0].source.body =
                        Default::default()
                }
                5 => bad.plans.get_mut(&body).unwrap().cleanups.clear(),
                6 => bad.plans.get_mut(&finalizer.source.body).unwrap().states[0]
                    .live
                    .clear(),
                7 => {
                    bad.plans
                        .get_mut(&finalizer.source.body)
                        .unwrap()
                        .cost
                        .frame_bytes = 8
                }
                _ => unreachable!(),
            }
            assert!(
                native::continuation::validate(&program, &bad).is_err(),
                "mutation {mutation}"
            );
            emit(&bad, false);
        }
    });
}

#[test]
fn source_finalizer_physically_saves_with_retained_cursor_and_strict_replay() {
    hirsel_engine::with_compiler_stack(|| {
        let traces = super::runtime50_resume::compact_runs_with(
            SOURCE,
            "source boxed finalizer",
            true,
            "tasks.free@1",
        );
        assert_eq!(traces.len(), 4);
        for (cell, trace) in traces.iter().enumerate() {
            let pending: Vec<_> = trace
                .iter()
                .enumerate()
                .filter(|(_, row)| row.0 == "finalizer" && row.1 != 0)
                .collect();
            assert!(
                !pending.is_empty(),
                "{cell}: no actual source finalizer Pending"
            );
            for (index, row) in pending {
                assert!(
                    trace[index + 1..]
                        .iter()
                        .any(|next| next.0 == "leave" && next.1 == row.1 && next.4),
                    "{cell}: finalizer Pending kept its stack"
                );
            }
            assert!(
                super::runtime50_resume::exact_compact_custody(trace, &format!("finalizer {cell}"))
                    > 0
            );
            assert!(
                trace
                    .iter()
                    .any(|(kind, task, _, _, pooled)| kind == "leave" && *task != 0 && *pooled)
            );
        }
    });
}
