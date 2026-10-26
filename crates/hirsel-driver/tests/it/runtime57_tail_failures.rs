//! The actual wrapper failure paths retain their own entered intrinsic site.
use super::runtime50_continuation::ready;
use super::runtime50_resume::compact_runs_with;
use hirsel_driver::native;
use hirsel_engine::with_compiler_stack;
use hirsel_lir::continuation::{ResumeAction, TailKind};

const KILL: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let mutex=mutex.create(tasks,0u64)
  let answer=task.scope(tasks,fn(s:Scope)->u64 {
    let child=task.async(s,fn()->u64 {
      mutex.with(tasks,mutex,fn(mut x:unique u64)->u64 {
        x=x+1u64
        let n:u8=255u8
        let overflow=n+1u8
        x
      })
    })
    task.await(s,take child)
  })
  Ok(())
}
"#;
const CAUSE: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let mutex=mutex.create(tasks,0u64)
  let poisoned:Result[(),Failure]=task.isolate(tasks,fn()->() {
    mutex.with(tasks,mutex,fn(mut x:unique u64)->() {
      x=x+1u64
      let n:u8=255u8
      let overflow=n+1u8
    })
  })
  let answer=task.scope(tasks,fn(s:Scope)->u64 {
    let child=task.async(s,fn()->u64 {
      mutex.with(tasks,mutex,fn(mut x:unique u64)->u64 { x=x+1u64; x })
    })
    task.await(s,take child)
  })
  Ok(())
}
"#;

fn check(source: &str, kind: TailKind, label: &str) {
    with_compiler_stack(|| {
        let program = ready(source);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let admitted = plans.plans.values().flat_map(|p| &p.states).any(|s| {
            s.tail.as_ref().is_some_and(|tail| tail.kind == kind)
                && matches!(s.action, ResumeAction::AfterEnteredCall(..))
        });
        assert!(admitted, "no entered {kind:?} plan");
        if kind == TailKind::Cause {
            let mut wrong = plans.clone();
            let state = wrong
                .plans
                .values_mut()
                .flat_map(|p| &mut p.states)
                .find(|s| s.tail.as_ref().is_some_and(|t| t.kind == TailKind::Cause))
                .unwrap();
            state.action = ResumeAction::RetryUnenteredCall(state.boundary.call);
            assert!(native::continuation::validate(&program, &wrong).is_err());
        }
        assert_eq!(
            compact_runs_with(source, label, true, "tasks.free@1").len(),
            4
        );
    });
}
#[test]
fn runtime57_kill_failure_resumes_the_original_wrapper_cleanup_on_both_backends() {
    check(KILL, TailKind::Kill, "entered kill failure cleanup");
}
#[test]
fn runtime57_cause_appends_the_original_dead_mutex_failure_on_both_backends() {
    check(CAUSE, TailKind::Cause, "entered dead-mutex cause cleanup");
}
