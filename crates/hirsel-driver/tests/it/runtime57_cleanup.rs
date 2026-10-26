//! A release resumes its exact instruction; no source owner is consumed twice.
use super::runtime50_continuation::ready;
use super::runtime50_resume::{compact_runs_with, routing};
use hirsel_driver::native::{self, *};
use hirsel_engine::with_compiler_stack;
use hirsel_lir::continuation::ResumeAction;
use hirsel_lir::ir::CallId;

const NESTED: &str = r#"
resource type Leaf { tasks:Tasks, done:Atomic[u64], id:u64 }
drop(x) { atomic.store(x.tasks,x.done,atomic.load(x.tasks,x.done)*10u64+x.id) }
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let done=atomic.create(tasks,0u64)
  let answer=task.scope(tasks,fn(s:Scope)->u64 {
    let child=task.async(s,fn()->u64 {
      let held=mutex.create(tasks,Leaf(tasks:tasks,done:copy done,id:7u64))
      41u64
    })
    task.await(s,take child)
  })
  let _=out.print(output,f"{answer}:{atomic.load(tasks,done)}\n")
  Ok(())
}
"#;

#[test]
fn runtime57_cleanup_good_mutex_root_drop_has_instruction_custody_on_both_backends() {
    with_compiler_stack(|| {
        let program = ready(NESTED);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let count = plans.plans.values().flat_map(|p| &p.cleanups).count();
        assert!(count > 0, "no instruction cleanup states: {plans:#?}");
        for state in plans.plans.values().flat_map(|p| &p.cleanups) {
            assert_eq!(
                state.action,
                ResumeAction::ResumeCleanup {
                    block: state.block,
                    instruction: state.instruction,
                    anchor: state.anchor
                }
            );
            assert!(state.live.iter().any(|v| v.value == state.value));
        }
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let (exported, entered) = routing(&program, backend, Partition::Whole);
            assert!(exported > 0 && entered);
        }
        let runs = compact_runs_with(NESTED, "root cleanup instruction", true, "tasks.free@1");
        assert_eq!(runs.len(), 4);
    });
}

#[test]
fn runtime57_cleanup_bad_instruction_call_confusion_wrong_suffix_and_missing_owner_are_refused() {
    with_compiler_stack(|| {
        let program = ready(NESTED);
        let original = native::continuation::plans(&program);
        let (&body, _) = original
            .plans
            .iter()
            .find(|(_, p)| !p.cleanups.is_empty())
            .expect("cleanup frame");
        for mutation in 0..7 {
            let mut plans = original.clone();
            let state = &mut plans.plans.get_mut(&body).unwrap().cleanups[0];
            match mutation {
                0 => state.instruction += 1,
                1 => state.anchor.0 += 1,
                2 => state.action = ResumeAction::RetryUnenteredCall(CallId(0)),
                3 => state.live.retain(|v| v.value != state.value),
                4 => state.failure.0 += 1,
                5 => state.state += 1,
                6 => state.bytes = state.bytes.saturating_sub(8),
                _ => unreachable!(),
            }
            assert!(
                native::continuation::validate(&program, &plans).is_err(),
                "mutation {mutation} accepted"
            );
        }
    });
}

const CHANNEL: &str = r#"
resource type Leaf { tasks:Tasks, done:Atomic[u64], id:u64 }
drop(x) { atomic.store(x.tasks,x.done,atomic.load(x.tasks,x.done)*10u64+x.id) }
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let done=atomic.create(tasks,0u64)
  let answer=task.scope(tasks,fn(s:Scope)->u64 {
    let child=task.async(s,fn()->u64 {
      let (inner_tx,inner_rx):(Sender[Leaf],Receiver[Leaf])=channel.open(tasks)
      let mut tx=take inner_tx
      let _=channel.send(tasks,mut tx,Leaf(tasks:tasks,done:copy done,id:1u64))
      let _=channel.send(tasks,mut tx,Leaf(tasks:tasks,done:copy done,id:2u64))
      let (outer_tx,outer_rx):(Sender[Sender[Leaf]],Receiver[Sender[Leaf]])=channel.open(tasks)
      let mut out_tx=take outer_tx
      let _=channel.send(tasks,mut out_tx,sender.dup(tasks,tx))
      41u64
    })
    task.await(s,take child)
  })
  let _=out.print(output,f"{answer}:{atomic.load(tasks,done)}\n")
  Ok(())
}
"#;

#[test]
fn runtime57_cleanup_sender_payload_runs_nested_close_on_both_backends() {
    with_compiler_stack(|| {
        let program = ready(CHANNEL);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        assert!(
            plans.plans.values().any(|p| !p.cleanups.is_empty()),
            "no instruction custody: {plans:#?}"
        );
        assert_eq!(
            compact_runs_with(CHANNEL, "nested sender cleanup", true, "tasks.free@1").len(),
            4
        );
    });
}
