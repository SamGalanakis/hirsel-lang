//! Entered intrinsic actions bind to the wrapper's exact post-callback state.
use super::runtime50_continuation::ready;
use super::runtime50_resume::{compact_runs_with, routing};
use hirsel_driver::native::{self, *};
use hirsel_engine::with_compiler_stack;
use hirsel_lir::continuation::{Representation, ResumeAction};

const PUT: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let m=mutex.create(tasks,0u64)
  let n=task.scope(tasks,fn(s:Scope)->u64 {
    let child=task.async(s,fn()->u64 {
      mutex.with(tasks,m,fn(mut x:unique u64)->u64 { x=x+1u64; x })
    })
    task.await(s,take child)
  })
  let after=mutex.with(tasks,m,fn(mut x:unique u64)->u64 { x })
  let _=out.print(output,f"{n}:{after}\n")
  Ok(())
}
"#;

#[test]
fn runtime57_good_owned_mutex_tail_certifies_exact_intrinsics_and_emits_on_both_backends() {
    with_compiler_stack(|| {
        let program = ready(PUT);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let wrappers: Vec<_> = plans
            .plans
            .values()
            .filter(|p| p.states.iter().any(|s| s.tail.is_some()))
            .collect();
        assert!(!wrappers.is_empty(), "no owned mutex wrapper: {plans:#?}");
        for p in wrappers {
            assert_eq!(p.representation, Representation::Resumable);
            for state in &p.states {
                if let Some(tail) = &state.tail {
                    assert_eq!(
                        state.action,
                        ResumeAction::AfterEnteredCall(state.boundary.call, tail.normal)
                    );
                    assert_eq!(state.cleanup.failure, tail.failure);
                }
            }
        }
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let (exported, entered) = routing(&program, backend, Partition::Whole);
            assert!(exported > 0 && entered);
        }
        let runs = compact_runs_with(PUT, "owned mutex tails", true, "tasks.free@1");
        assert_eq!(runs.len(), 4);
    });
}

#[test]
fn runtime57_bad_owned_mutex_tail_import_rejects_reissue_skipped_effect_and_forged_custody() {
    with_compiler_stack(|| {
        let program = ready(PUT);
        let original = native::continuation::plans(&program);
        let (&id, p) = original
            .plans
            .iter()
            .find(|(_, p)| p.states.iter().any(|s| s.tail.is_some()))
            .expect("owned wrapper");
        let at = p
            .states
            .iter()
            .position(|s| s.tail.as_ref().is_some_and(|t| t.carrier.is_some()))
            .expect("put carrier");
        for mutation in 0..5 {
            let mut plans = original.clone();
            let state = &mut plans.plans.get_mut(&id).unwrap().states[at];
            match mutation {
                0 => state.action = ResumeAction::RetryUnenteredCall(state.boundary.call),
                1 => state.tail.as_mut().unwrap().normal.0 += 1,
                2 => state.tail.as_mut().unwrap().failure.0 += 1,
                3 => {
                    state
                        .tail
                        .as_mut()
                        .unwrap()
                        .carrier
                        .as_mut()
                        .unwrap()
                        .offset += 8
                }
                4 => state.tail = None,
                _ => unreachable!(),
            }
            assert!(
                native::continuation::validate(&program, &plans).is_err(),
                "mutation {mutation} accepted"
            );
        }
    });
}
