//! Resume coverage for `arena.bounded` (resume-bounded-callback-54): a task
//! suspending inside a known `arena.bounded` callback plans Resumable. The
//! specialized library body's frame holds the bounded arena's region record
//! and token across the callback's suspension, so a charge after the resume
//! still sees the same budget, and its `OutOfSpace` still unwinds to the
//! same recovery, once.
use hirsel_driver::native::{self, *};
use hirsel_engine::*;
use hirsel_lir::continuation::{
    ContinuationError, ContinuationPlan, ContinuationPlans, RegionKind, Representation,
};
use hirsel_syntax::ContentHash;

use super::runtime50_continuation::ready;
use super::runtime50_resume::{agree_on, compact_runs, exact_compact_custody, routing};

/// A child that sends twice inside an `arena.bounded` callback, charging its
/// arena before the first send, right after it and after the second. The
/// overwritten and consumed list before the first send (rustc's
/// drop-and-assign) is dead at every suspension, so no frame holds it.
const BOUNDED: &str = r#"
fn consume(take xs: List[u64]) -> u64 { list.len(xs) }

pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let (sender,receiver):(Sender[u64],Receiver[u64])=channel.open(tasks)
    let mut rx=take receiver
    let child=task.async(s,fn()->u64 {
      let owned=take sender
      let outcome:Result[u64,OutOfSpace]=arena.bounded(CAP,fn(a:Arena)->u64 {
        let mut tx=sender.dup(tasks,owned)
        let mut gone:List[u64]=[1u64]
        gone=[2u64,4u64]
        let dropped=consume(take gone)
        let mut xs:List[u64] in a=[]
        list.push(mut xs,3u64)
        let _=channel.send(tasks,mut tx,3u64+dropped)
        list.push(mut xs,5u64)
        let _=channel.send(tasks,mut tx,5u64)
        list.push(mut xs,7u64)
        list.len(xs)*100u64+xs[2]
      })
      match outcome {Ok(x)=>x, Error(_)=>77u64}
    })
    let mut sum=0u64
    for _ in 0u64..RECEIVES {
      match channel.receive(tasks,mut rx) {Ok(x)=>{sum=sum*31u64+x}, Error(_)=>{sum=sum+1000u64}}
    }
    CANCEL
    sum+task.await(s,take child)
  })
  let _=out.print(output,f"{total}\n")
  Ok(())
}
"#;

/// Where the bounded arena runs out, in charges of one element each.
#[derive(Clone, Copy, Debug)]
enum Cap {
    /// Every charge fits.
    Fits,
    /// The first charge, before the first send, does not fit.
    BeforeSave,
    /// The charge right after the first send's resume does not fit.
    AfterSave,
    /// The charge after the second send's resume does not fit.
    AfterSecondSave,
}

fn bounded_source(cap: Cap, canceled: bool) -> String {
    let cap = match cap {
        Cap::Fits => "64u64",
        Cap::BeforeSave => "0u64",
        Cap::AfterSave => "8u64",
        Cap::AfterSecondSave => "16u64",
    };
    let (receives, cancel) = if canceled {
        ("1u64", "task.cancel(s,child)")
    } else {
        ("2u64", "")
    };
    BOUNDED
        .replace("CAP", cap)
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

/// The callback (both sends), the specialized `arena.bounded` that nests its
/// frame, and the child that nests the arena's.
fn bounded_plans(
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
        .unwrap_or_else(|| panic!("the bounded callback: {:#?}", summary(plans)));
    let arena = nesting(callback.body);
    let child = nesting(arena.body);
    assert!(
        child
            .states
            .iter()
            .any(|s| s.boundary.operation.to_string().ends_with("arena/bounded")),
        "{:#?}",
        summary(plans)
    );
    (callback, arena, child)
}

#[test]
fn runtime54_bounded_callback_continuation() {
    with_compiler_stack(|| {
        let program = ready(&bounded_source(Cap::Fits, false));
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let (callback, arena, child) = bounded_plans(&plans);
        for plan in [callback, arena, child] {
            assert_eq!(
                plan.representation,
                Representation::Resumable,
                "{:#?}",
                summary(&plans)
            );
        }
        // The bounded arena's region record is in the specialized body's
        // frame, and its token survives the callback's suspension there.
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

        // Forged bounded frames: the nested callback, its size, the arena's
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
                "bounded arena record left on the stack",
                refused(arena.body, &|p| p.records.clear()),
                |e| matches!(e, ContinuationError::RegionRecord(_, _)),
            ),
            (
                "dropped bounded arena token",
                refused(arena.body, &|p| p.states[0].regions.clear()),
                |e| matches!(e, ContinuationError::RegionToken(..)),
            ),
            (
                "bounded arena token as a mark",
                refused(arena.body, &|p| {
                    for r in &mut p.states[0].regions {
                        r.kind = RegionKind::Mark
                    }
                }),
                |e| matches!(e, ContinuationError::RegionToken(..)),
            ),
            // Refused at whichever body the validator reaches first: the
            // callback's stackful plan keeping its states, or the arena's
            // frame nesting a stackful callee.
            (
                "stackful callback",
                refused(callback.body, &|p| {
                    p.representation = Representation::Stackful("indirect call".into())
                }),
                |e| {
                    matches!(
                        e,
                        ContinuationError::Representation(_) | ContinuationError::States(_)
                    )
                },
            ),
            (
                "certificate over the bounded frame",
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

/// `cap` agrees with the reference and checked H on both native backends:
/// locally, migrated at every suspension (which saves the frame at every
/// boundary) and replayed, and, with `cancel`, canceled at the second send.
/// The reference prints `printed`: sends 5 and 5 (sum 160), then 307; a
/// closed channel adds 1000 per receive, and a recovered `OutOfSpace` is 77.
/// So an exhaustion recovers once, at the same charge, and the callback's
/// cleanup runs once, on every path.
fn bounded_agrees(cap: Cap, printed: &str, cancel: bool) {
    with_compiler_stack(|| {
        let local = Fixture::default();
        let migrate = Fixture {
            migrate: true,
            ..Fixture::default()
        };
        let done = bounded_source(cap, false);
        let label = format!("{cap:?}");
        let seen = agree_on(&done, &local, &format!("{label} local"), false, &[true]);
        assert_eq!(String::from_utf8_lossy(&seen.output), printed, "{label}");
        agree_on(&done, &migrate, &format!("{label} migrate"), true, &[false]);
        if cancel {
            let canceled = bounded_source(cap, true);
            agree_on(
                &canceled,
                &migrate,
                &format!("{label} canceled"),
                false,
                &[false],
            );
        }
    });
}

#[test]
fn runtime54_bounded_callback_fits_agrees() {
    bounded_agrees(Cap::Fits, "467\n", true);
}

#[test]
fn runtime54_bounded_callback_exhausted_before_save_agrees() {
    bounded_agrees(Cap::BeforeSave, "2077\n", false);
}

#[test]
fn runtime54_bounded_callback_exhausted_after_save_agrees() {
    bounded_agrees(Cap::AfterSave, "1082\n", true);
}

#[test]
fn runtime54_bounded_callback_exhausted_after_second_save_agrees() {
    bounded_agrees(Cap::AfterSecondSave, "237\n", false);
}

/// Under the shared dispatcher the saved child, with the bounded arena's
/// frame and the callback's nested in it, leaves its stack and activates on
/// another worker's, live and replayed, whether its budget lasts or runs out
/// right after a resume.
#[test]
fn runtime54_bounded_callback_leaves_its_stack() {
    with_compiler_stack(|| {
        for cap in [Cap::Fits, Cap::AfterSave] {
            let source = bounded_source(cap, false);
            for (index, trace) in compact_runs(&source, "bounded").iter().enumerate() {
                let cell = format!("bounded {cap:?} run {index}");
                assert!(exact_compact_custody(trace, &cell) >= 1, "{cell}");
            }
        }
    });
}
