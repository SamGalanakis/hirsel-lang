//! A resumed state re-runs an address preparation whose failure edge names a
//! cleanup block the state never emits (resume-coverage-52). Cranelift traps
//! there as LLVM marks it unreachable: the check passed before the save.
use hirsel_driver::native::{self, *};
use hirsel_engine::*;

use super::runtime50_continuation::ready;
use super::runtime50_resume::{agree, routing};

/// `conformance/CONC/CONC-2-detached-result-cleanup-run.hir`.
const DETACHED: &str = r#"resource type Signal {
  tasks: Tasks,
  done: Atomic[u32],
}
drop(x) {
  atomic.store(x.tasks, x.done, 1u32)
}

fn check_detach(tasks: Tasks, early: bool) {
  let done: Atomic[u32] = atomic.create(tasks, 0u32)
  task.scope(
    tasks,
    fn(s: Scope) {
      let (sender, receiver): (Sender[u8], Receiver[u8]) = channel.open(tasks)
      let child = task.async(
        s,
        fn() -> Signal {
          let mut rx = take receiver
          if early {
            let _ = channel.receive(tasks, mut rx)
          }
          Signal(tasks: tasks, done: copy done)
        }
      )
      task.detach(s, take child)
      let mut tx = take sender
      if early {
        let _ = channel.send(tasks, mut tx, 1u8)
      }
      let barrier = task.async(s, fn() {})
      task.await(s, take barrier)
      failure.check(atomic.load(tasks, done) == 1u32, Arithmetic())
    }
  )
}

pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  check_detach(tasks, false)
  check_detach(tasks, true)
  Ok(())
}
"#;

/// Both backends plan, emit and agree on the detached result, in place and
/// saved at every boundary.
#[test]
fn runtime52_resumed_preparation_cleanup_emits_on_both_backends() {
    with_compiler_stack(|| {
        let program = ready(DETACHED);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        assert!(plans.resumable());
        for backend in [Backend::Llvm, Backend::Cranelift] {
            for partition in [Partition::Whole, Partition::Singleton] {
                let (exported, entered) = routing(&program, backend, partition);
                assert!(exported >= 1 && entered, "{backend:?} {partition:?}");
            }
        }
        let published = Fixture {
            resume_save: true,
            ..Fixture::default()
        };
        agree(DETACHED, &Fixture::default(), "detached result local");
        agree(DETACHED, &published, "detached result published");
    });
}
