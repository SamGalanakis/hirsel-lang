//! Free replay waits other than send leave their stacks (replay-leave-ops-54):
//! a task waiting for its tape turn in `mutex.with`'s take or in `sender.dup`
//! gives up its stack from its saved compact frame, and the wake reissues
//! the boundary call, as a send does. An op that is not its boundary
//! operation's first, such as one inside a `task.isolate` callback, keeps its
//! stack and nothing is reissued: every live and replayed observation on
//! both backends equals the reference interpreter's (`compact_runs_with`).
use super::runtime50_resume::{compact_runs_recording, compact_runs_with, peak_held};

/// `CHILDREN` children (the bound is one past them) each add their seed to
/// one shared mutex once.
const TAKERS: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let m:Mutex[u64]=mutex.create(tasks,0u64)
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let mut set:TaskSet[Result[u64,()]] of s=task.set(s)
    for seed in 1u64..CHILDREN {
      task.add(s,mut set,task.async(s,fn()->Result[u64,()] {
        mutex.with(tasks,m,fn(mut v:unique u64) { v=v+seed })
        Ok(seed)
      }))
    }
    let mut sum=0u64
    if let Ok(rows)=std@1/task.all(s,take set) {for x in rows {sum=sum+x}}
    sum
  })
  let last=mutex.with(tasks,m,fn(mut v:unique u64)->u64 { v })
  let _=out.print(output,f"{total} {last}\n")
  Ok(())
}
"#;

/// As [`TAKERS`], but each child takes the mutex inside a `task.isolate`
/// callback: the isolate's begin precedes the take, and its boundary cannot
/// reissue the callback, so the take's replay wait keeps its stack.
const ISOLATED_TAKERS: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let m:Mutex[u64]=mutex.create(tasks,0u64)
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let mut set:TaskSet[Result[u64,()]] of s=task.set(s)
    for seed in 1u64..CHILDREN {
      task.add(s,mut set,task.async(s,fn()->Result[u64,()] {
        match task.isolate(tasks,fn()->u64 {
          mutex.with(tasks,m,fn(mut v:unique u64) { v=v+seed })
          seed
        }) {Ok(x)=>Ok(x), Error(_)=>Ok(0u64)}
      }))
    }
    let mut sum=0u64
    if let Ok(rows)=std@1/task.all(s,take set) {for x in rows {sum=sum+x}}
    sum
  })
  let last=mutex.with(tasks,m,fn(mut v:unique u64)->u64 { v })
  let _=out.print(output,f"{total} {last}\n")
  Ok(())
}
"#;

/// `CHILDREN` children each duplicate a Sender of one shared channel and
/// send their seed on the duplicate.
const DUPS: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let (sender,receiver):(Sender[u64],Receiver[u64])=channel.open(tasks)
    let mut rx=take receiver
    let mut set:TaskSet[Result[u64,()]] of s=task.set(s)
    for seed in 1u64..CHILDREN {
      let mine=sender.dup(tasks,sender)
      task.add(s,mut set,task.async(s,fn()->Result[u64,()] {
        let tx=take mine
        let mut extra=sender.dup(tasks,tx)
        let _=channel.send(tasks,mut extra,seed)
        Ok(seed)
      }))
    }
    let mut sum=0u64
    for _ in 1u64..CHILDREN {
      match channel.receive(tasks,mut rx) {Ok(x)=>{sum=sum+x*x}, Error(_)=>{}}
    }
    if let Ok(rows)=std@1/task.all(s,take set) {for x in rows {sum=sum+x}}
    sum
  })
  let _=out.print(output,f"{total}\n")
  Ok(())
}
"#;

/// As [`DUPS`], but each child duplicates inside a `task.isolate`
/// callback, so the dup's replay wait keeps its stack.
const ISOLATED_DUPS: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let (sender,receiver):(Sender[u64],Receiver[u64])=channel.open(tasks)
    let mut rx=take receiver
    let mut set:TaskSet[Result[u64,()]] of s=task.set(s)
    for seed in 1u64..CHILDREN {
      let mine=sender.dup(tasks,sender)
      task.add(s,mut set,task.async(s,fn()->Result[u64,()] {
        let tx=take mine
        match task.isolate(tasks,fn()->u64 {
          let held=take tx
          let mut extra=sender.dup(tasks,held)
          let _=channel.send(tasks,mut extra,seed)
          seed
        }) {Ok(x)=>Ok(x), Error(_)=>Ok(0u64)}
      }))
    }
    let mut sum=0u64
    for _ in 1u64..CHILDREN {
      match channel.receive(tasks,mut rx) {Ok(x)=>{sum=sum+x*x}, Error(_)=>{}}
    }
    if let Ok(rows)=std@1/task.all(s,take set) {for x in rows {sum=sum+x}}
    sum
  })
  let _=out.print(output,f"{total}\n")
  Ok(())
}
"#;

/// Per replayed run: the leaves at each child's first boundary (its first
/// resume point's state: the `mutex.with` or `sender.dup` under test), and
/// the most stacks suspended children held at once. `compact_runs_with`
/// returns LLVM then Cranelift, each live then replayed.
fn replayed(source: &str, label: &str) -> Vec<(usize, usize)> {
    let mut cells = Vec::new();
    for (index, trace) in compact_runs_with(source, label, false, "tasks.free@1")
        .iter()
        .enumerate()
    {
        let mut first = std::collections::BTreeMap::new();
        let mut leaves = 0;
        for (kind, task, state, ..) in trace {
            match kind.as_str() {
                "point" if *task != 0 => {
                    first.entry(*task).or_insert(*state);
                }
                "leave" if first.get(task) == Some(state) => leaves += 1,
                _ => {}
            }
        }
        let peak = peak_held(trace);
        eprintln!("{label} run {index}: first-boundary leaves {leaves} peak held {peak}");
        if index % 2 == 1 {
            cells.push((leaves, peak));
        }
    }
    cells
}

/// N replayed mutex takers hold at most one stack per worker (4 cores): each
/// one waiting for its tape turn in `mutex.with`'s take leaves from its
/// saved frame, and its wake reissues `mutex.with`.
#[test]
fn runtime54_replayed_mutex_takers_hold_worker_stacks() {
    hirsel_engine::with_compiler_stack(|| {
        for children in [8u64, 16, 32] {
            let source = TAKERS.replace("CHILDREN", &format!("{}u64", children + 1));
            let label = format!("mutex takers {children}");
            let cells = replayed(&source, &label);
            for (leaves, peak) in &cells {
                assert!(*peak <= 4, "{label}: {peak} suspended takers held stacks");
                assert!(*leaves as u64 <= children, "{label}: {leaves} take leaves");
            }
            if children == 32 {
                let leaves: usize = cells.iter().map(|cell| cell.0).sum();
                assert!(leaves > 0, "{label}: no replayed take left its stack");
            }
        }
    });
}

/// Both ordinary and recorded live permit waits release their stacks, beside
/// the tape-turn waits covered above. Each observation (including strict
/// replay of the recorded live run) equals the reference interpreter's.
#[test]
fn runtime55_live_mutex_takers_hold_worker_stacks() {
    hirsel_engine::with_compiler_stack(|| {
        for children in [8u64, 16, 32] {
            let source = TAKERS.replace("CHILDREN", &format!("{}u64", children + 1));
            for recording in [false, true] {
                let label = format!("live mutex takers {children} recording={recording}");
                for (index, trace) in
                    compact_runs_recording(&source, &label, false, "tasks.free@1", recording)
                        .iter()
                        .enumerate()
                {
                    let peak = peak_held(trace);
                    eprintln!("{label} run {index}: peak held {peak}");
                    assert!(peak <= 4, "{label}: {peak} suspended takers held stacks");
                }
            }
        }
    });
}

#[test]
fn runtime55_live_mutex_takers_checked_heap_agrees() {
    hirsel_engine::with_compiler_stack(|| {
        for children in [8u64, 16, 32] {
            let source = TAKERS.replace("CHILDREN", &format!("{}u64", children + 1));
            super::runtime50_resume::checked_heap_agrees(&source);
        }
    });
}

/// A take inside a `task.isolate` callback is not its boundary's first op:
/// its replay wait keeps its stack and never reissues `task.isolate`.
#[test]
fn runtime54_a_take_after_an_effect_keeps_its_stack() {
    hirsel_engine::with_compiler_stack(|| {
        let source = ISOLATED_TAKERS.replace("CHILDREN", "17u64");
        for (leaves, _) in replayed(&source, "isolated takers") {
            assert_eq!(leaves, 0, "isolated takers: a take reissued task.isolate");
        }
    });
}

/// Replayed `sender.dup` waits leave their stacks: some child waiting for
/// its tape turn in its dup leaves from its saved frame.
#[test]
fn runtime54_replayed_sender_dups_leave_their_stacks() {
    hirsel_engine::with_compiler_stack(|| {
        let source = DUPS.replace("CHILDREN", "33u64");
        let leaves: usize = replayed(&source, "dups").iter().map(|cell| cell.0).sum();
        assert!(leaves > 0, "dups: no replayed dup left its stack");
    });
}

/// A dup inside a `task.isolate` callback is not its boundary's first op:
/// its replay wait keeps its stack and never reissues `task.isolate`.
#[test]
fn runtime54_a_dup_after_an_effect_keeps_its_stack() {
    hirsel_engine::with_compiler_stack(|| {
        let source = ISOLATED_DUPS.replace("CHILDREN", "17u64");
        for (leaves, _) in replayed(&source, "isolated dups") {
            assert_eq!(leaves, 0, "isolated dups: a dup reissued task.isolate");
        }
    });
}

/// A phi of different closure bodies remains opaque even when both branches
/// have the same environment layout and agree on these sixteen observations.
fn unknown_isolate(source: &str, alternate: &str) -> String {
    let before = "match task.isolate(tasks,fn()->u64 {";
    let after = "          seed\n        }) {Ok(x)=>Ok(x), Error(_)=>Ok(0u64)}";
    assert_eq!(source.matches(before).count(), 1);
    assert_eq!(source.matches(after).count(), 1);
    source
        .replace(before, "let callback: once fn() -> u64 = if seed % 2u64 == 0u64 { fn()->u64 {")
        .replace(after, &format!(
            "          seed\n        }} }} else {{ fn()->u64 {{\n{alternate}\n        }} }}\n        match task.isolate(tasks,take callback) {{Ok(x)=>Ok(x), Error(_)=>Ok(0u64)}}"
        ))
        .replace("CHILDREN", "17u64")
}

#[test]
fn runtime56_unknown_isolate_mutex_callbacks_never_restart() {
    hirsel_engine::with_compiler_stack(|| {
        let source = unknown_isolate(
            ISOLATED_TAKERS,
            r#"
          mutex.with(tasks,m,fn(mut v:unique u64) { v=v+seed })
          seed + seed / (seed + 1u64)
"#,
        );
        for (leaves, _) in replayed(&source, "unknown isolated takers") {
            assert_eq!(leaves, 0, "unknown isolate callback restarted its wrapper");
        }
    });
}

#[test]
fn runtime56_unknown_isolate_sender_callbacks_never_restart() {
    hirsel_engine::with_compiler_stack(|| {
        let source = unknown_isolate(
            ISOLATED_DUPS,
            r#"
          let held=take tx
          let mut extra=sender.dup(tasks,held)
          let _=channel.send(tasks,mut extra,seed)
          seed + seed / (seed + 1u64)
"#,
        );
        for (leaves, _) in replayed(&source, "unknown isolated dups") {
            assert_eq!(leaves, 0, "unknown isolate callback restarted its wrapper");
        }
    });
}
