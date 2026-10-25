//! Resume coverage (resume-coverage-52): a task suspending inside an
//! `arena.with` callback plans Resumable. The callback is known at the call,
//! so `arena.with`'s library body is specialized on it as an ordinary
//! instance (rustc's collector, `visit_fn_use`), its callback call becomes
//! direct and its frame nests the callback's.
use hirsel_driver::native::{self, *};
use hirsel_engine::*;
use hirsel_lir::continuation::{
    ContinuationError, ContinuationPlan, ContinuationPlans, RegionKind, Representation,
};
use hirsel_syntax::ContentHash;

use super::runtime50_continuation::ready;
use super::runtime50_resume::{
    agree, agree_replayed, compact_runs, exact_compact_custody, routing,
};

/// A child that sends twice inside an `arena.with` callback, holding a sender
/// it duplicated and a list across both sends; completed, or canceled while
/// it waits at its second send, so the arena closes through cleanup.
const ARENA: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let (sender,receiver):(Sender[u64],Receiver[u64])=channel.open(tasks)
    let mut rx=take receiver
    let child=task.async(s,fn()->u64 {
      let owned=take sender
      arena.with(fn(a:Arena)->u64 {
        let mut tx=sender.dup(tasks,owned)
        let kept:List[u64]=[3u64,5u64,7u64]
        let _=channel.send(tasks,mut tx,kept[0])
        let _=channel.send(tasks,mut tx,kept[1])
        kept[2]
      })
    })
    let mut sum=0u64
    for _ in 0u64..RECEIVES {
      match channel.receive(tasks,mut rx) {Ok(x)=>{sum=sum*31u64+x}, Error(_)=>{}}
    }
    CANCEL
    sum+task.await(s,take child)
  })
  let _=out.print(output,f"{total}\n")
  Ok(())
}
"#;

fn arena_source(canceled: bool) -> String {
    let (receives, cancel) = if canceled {
        ("1u64", "task.cancel(s,child)")
    } else {
        ("2u64", "")
    };
    ARENA
        .replace("RECEIVES", receives)
        .replace("CANCEL", cancel)
}

fn summary(plans: &ContinuationPlans) -> Vec<String> {
    plans
        .plans
        .values()
        .map(|p| {
            format!(
                "{:?} records {} states {:?}",
                p.representation,
                p.records.len(),
                p.states
                    .iter()
                    .map(|s| (
                        s.boundary.operation.to_string(),
                        s.callee.is_some(),
                        s.regions.iter().map(|r| r.kind).collect::<Vec<_>>()
                    ))
                    .collect::<Vec<_>>()
            )
        })
        .collect()
}

/// The callback (both sends), the specialized `arena.with` that nests its
/// frame, and the child that nests the arena's.
fn arena_plans(
    plans: &ContinuationPlans,
) -> (&ContinuationPlan, &ContinuationPlan, &ContinuationPlan) {
    let nesting = |body: ContentHash| {
        plans
            .plans
            .values()
            .find(|p| {
                p.states
                    .iter()
                    .any(|s| s.callee.is_some_and(|c| c.body == body))
            })
            .unwrap_or_else(|| panic!("a caller nesting {body:?}: {:#?}", summary(plans)))
    };
    let callback = plans
        .plans
        .values()
        .find(|p| {
            p.states
                .iter()
                .filter(|s| s.boundary.operation.to_string().ends_with("channel/send"))
                .count()
                == 2
        })
        .unwrap_or_else(|| panic!("the arena callback: {:#?}", summary(plans)));
    let arena = nesting(callback.body);
    let child = nesting(arena.body);
    assert!(
        child
            .states
            .iter()
            .any(|s| s.boundary.operation.to_string().ends_with("arena/with")),
        "{:#?}",
        summary(plans)
    );
    (callback, arena, child)
}

#[test]
fn runtime52_arena_callback_continuation() {
    with_compiler_stack(|| {
        let program = ready(&arena_source(false));
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let (callback, arena, child) = arena_plans(&plans);
        for plan in [callback, arena, child] {
            assert_eq!(
                plan.representation,
                Representation::Resumable,
                "{:#?}",
                summary(&plans)
            );
        }
        // The arena's region record is in the specialized body's frame, and
        // its token survives the callback's suspension in that frame.
        assert_eq!(arena.states.len(), 1, "{:#?}", summary(&plans));
        assert!(!arena.records.is_empty(), "{arena:#?}");
        assert!(
            arena.states[0]
                .regions
                .iter()
                .any(|r| r.kind == RegionKind::Arena),
            "{arena:#?}"
        );
        let nested = arena.states[0].callee.unwrap();
        assert_eq!(nested.bytes, callback.cost.frame_bytes);
        assert!(arena.cost.frame_bytes <= hirsel_lir::continuation::MAX_FRAME_BYTES);
        for backend in [Backend::Llvm, Backend::Cranelift] {
            for partition in [Partition::Whole, Partition::Singleton] {
                let (exported, entered) = routing(&program, backend, partition);
                assert!(
                    exported >= 4 && entered,
                    "{backend:?} {partition:?}: {exported} {entered}"
                );
            }
        }

        // Forged arena frames: the nested callback, its size, the arena's
        // record and token, a stackful callback claimed by the arena, and the
        // certificate.
        let refused = |body: ContentHash, edit: &dyn Fn(&mut ContinuationPlan)| {
            let mut bad = plans.clone();
            edit(bad.plans.get_mut(&body).unwrap());
            native::continuation::validate(&program, &bad).expect_err("a forged plan was imported")
        };
        let other = child.body;
        let checks: Vec<(&str, ContinuationError, fn(&ContinuationError) -> bool)> = vec![
            (
                "nested frame of another body",
                refused(arena.body, &|p| {
                    p.states[0].callee.as_mut().unwrap().body = other
                }),
                |e| matches!(e, ContinuationError::Frame(_)),
            ),
            (
                "nested frame smaller than the callback's",
                refused(arena.body, &|p| {
                    p.states[0].callee.as_mut().unwrap().bytes -= 8
                }),
                |e| matches!(e, ContinuationError::Frame(_)),
            ),
            (
                "nested frame over the arena token",
                refused(arena.body, &|p| {
                    p.states[0].callee.as_mut().unwrap().offset = 8
                }),
                |e| matches!(e, ContinuationError::Frame(_)),
            ),
            (
                "arena record left on the stack",
                refused(arena.body, &|p| p.records.clear()),
                |e| matches!(e, ContinuationError::RegionRecord(_, _)),
            ),
            (
                "dropped arena token",
                refused(arena.body, &|p| p.states[0].regions.clear()),
                |e| matches!(e, ContinuationError::RegionToken(..)),
            ),
            (
                "arena token as a mark",
                refused(arena.body, &|p| {
                    for r in &mut p.states[0].regions {
                        r.kind = RegionKind::Mark
                    }
                }),
                |e| matches!(e, ContinuationError::RegionToken(..)),
            ),
            (
                "stackful callback",
                refused(callback.body, &|p| {
                    p.representation = Representation::Stackful("indirect call".into())
                }),
                |e| matches!(e, ContinuationError::Representation(_)),
            ),
            (
                "certificate over the arena frame",
                refused(arena.body, &|p| {
                    p.certificate.identity = ContentHash::of_bytes(b"cached")
                }),
                |e| matches!(e, ContinuationError::Certificate(_)),
            ),
        ];
        for (name, error, expected) in &checks {
            assert!(expected(error), "{name}: {error:?}");
        }
    });
}

/// The arena child agrees with the reference locally, saved at every
/// boundary (and replayed), migrated at every suspension and canceled.
#[test]
fn runtime52_arena_callback_agrees() {
    with_compiler_stack(|| {
        let local = Fixture::default();
        let published = Fixture {
            resume_save: true,
            ..Fixture::default()
        };
        let migrate = Fixture {
            migrate: true,
            ..Fixture::default()
        };
        let (done, canceled) = (arena_source(false), arena_source(true));
        agree(&done, &local, "arena local");
        agree_replayed(&done, &published, "arena published", true);
        agree(&done, &migrate, "arena migrate");
        agree(&canceled, &published, "arena canceled");
    });
}

#[test]
fn runtime52_arena_callback_leaves_its_stack() {
    with_compiler_stack(|| {
        let done = arena_source(false);
        // Under the shared dispatcher the saved child, with the arena's frame
        // and the callback's nested in it, leaves its stack and activates on
        // another worker's, live and replayed.
        for (index, trace) in compact_runs(&done, "arena").iter().enumerate() {
            let cell = format!("arena run {index}");
            assert!(exact_compact_custody(trace, &cell) >= 1, "{cell}");
        }
    });
}
