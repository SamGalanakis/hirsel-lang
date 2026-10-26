//! Known mechanically cleaned isolates execute through generated nested frames.
use super::runtime50_resume::{compact_runs_with, exact_compact_custody};

const SOURCE: &str = r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let total = task.scope(tasks, fn(s: Scope) -> u64 {
    let mut set: TaskSet[Result[u64, ()]] of s = task.set(s)
    for seed in 1u64..17u64 {
      task.add(s, mut set, task.async(s, fn() -> Result[u64, ()] {
        let value: Result[String, Failure] = task.isolate(tasks, fn() -> String {
          let _ = out.print(output, "")
          "held"
        })
        let n = match value { Ok(text) => string.byte_len(text), Error(_) => 0u64 }
        Ok(seed + n)
      }))
    }
    let mut sum = 0u64
    if let Ok(rows) = std@1/task.all(s, take set) { for row in rows { sum = sum + row } }
    sum
  })
  failure.check(total == 200u64, Arithmetic())
  let _ = out.print(io.stdout(rt)?, f"{total}\n")
  Ok(())
}
"#;

#[test]
fn known_isolate_owned_result_survives_generated_begin_save_replay_and_migration() {
    hirsel_engine::with_compiler_stack(|| {
        let ready = super::runtime50_continuation::ready(SOURCE);
        let plans = hirsel_driver::native::continuation::plans(&ready);
        hirsel_driver::native::continuation::validate(&ready, &plans).unwrap();
        let wrappers: Vec<_> = plans
            .plans
            .values()
            .filter(|p| p.isolate.is_some())
            .collect();
        assert!(!wrappers.is_empty(), "no source isolate admitted");
        for wrapper in wrappers {
            let protocol = &wrapper.isolate.as_ref().unwrap().protocol;
            let callback = wrapper
                .states
                .iter()
                .find(|s| s.boundary.call == protocol.callback.call)
                .expect("the callback's scheduling boundary has a retained nested frame");
            assert!(matches!(
                callback.action,
                hirsel_lir::continuation::ResumeAction::ResumeNested(_, _)
            ));
            assert!(callback.loans.contains(&protocol.identity.loan));
            assert!(
                callback.live.iter().any(|slot| slot.value == protocol.mark),
                "catch-only begin mark must survive the nested callback"
            );
            let mut forged = plans.clone();
            let row = forged.plans.get_mut(&protocol.identity.wrapper).unwrap();
            let state = row
                .states
                .iter_mut()
                .find(|s| s.boundary.call == protocol.callback.call)
                .unwrap();
            state.live.retain(|slot| slot.value != protocol.mark);
            assert!(
                hirsel_driver::native::continuation::validate(&ready, &forged).is_err(),
                "independent validation accepted a missing catch-only begin mark"
            );
            assert!(
                callback
                    .storage
                    .iter()
                    .any(|s| s.slot == protocol.identity.closure)
            );
        }
        let traces = compact_runs_with(
            SOURCE,
            "known isolate retained begin",
            true,
            "tasks.cooperative@1",
        );
        assert_eq!(
            traces.len(),
            4,
            "LLVM and Cranelift, each live and replayed"
        );
        for (cell, trace) in traces.iter().enumerate() {
            let activations = exact_compact_custody(trace, &format!("known isolate {cell}"));
            assert!(
                activations > 0,
                "generated isolate callchain never physically resumed"
            );
            assert!(
                trace
                    .iter()
                    .any(|(kind, task, _, _, pooled)| kind == "leave" && *task != 0 && *pooled),
                "no child actually returned its native stack"
            );
        }
    });
}

#[test]
fn known_isolate_boundary_emits_without_a_task_scope() {
    hirsel_engine::with_compiler_stack(|| {
        let program = super::runtime50_continuation::ready(
            r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let value: Result[String, Failure] = task.isolate(tasks, fn() -> String {
    let _ = out.print(output, "")
    "held"
  })
  match value { Ok(text) => failure.check(text == "held", Arithmetic()), Error(_) => failure.check(false, Bounds()) }
  Ok(())
}
"#,
        );
        for backend in [
            hirsel_driver::native::Backend::Llvm,
            hirsel_driver::native::Backend::Cranelift,
        ] {
            let result = hirsel_driver::native::emit_backend_objects(
                &program,
                backend,
                hirsel_lir::emission::OptLevel::Default,
                hirsel_driver::native::Partition::Whole,
                false,
                None,
            );
            assert!(result.is_ok(), "{backend:?}: {result:?}");
        }
    });
}
