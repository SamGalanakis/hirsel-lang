//! A nested callee borrowing one scalar slot of its caller's compact frame
//! (resume-exclusive-slot-55): `wait_mut(t, s, mut n)` with `n` a whole local
//! of the child suspends compactly, its frame records the slot's address, and
//! every mutation lands in the canonical slot across saves, migration, replay,
//! cancellation, entered failure and a terminal stop. Gathered SoA places,
//! underaligned packed fields, pointer/length views and owning `mut`
//! parameters keep the stackful form; a forged call-edge proof is refused.
use hirsel_driver::native::{self, *};
use hirsel_engine::*;
use hirsel_lir::abi_types::ParameterPassing;
use hirsel_lir::continuation::{
    ContinuationError, ContinuationPlan, ContinuationPlans, ExclusiveSlot, Representation,
};
use hirsel_lir::ir::{EdgeId, FailureId, LoanId, SlotId};
use hirsel_syntax::ContentHash;

use super::runtime50_continuation::ready;
use super::runtime50_resume::{
    agree, agree_replayed, compact_runs, exact_compact_custody, routing,
};

/// `wait_mut` writes its exclusive parameter around two sends; FINAL ends it.
const WAIT_MUT: &str = r#"
fn wait_mut(t:Tasks, s:Sender[u64], mut n:u64) {
  let mut tx=sender.dup(t,s)
  n=7u64
  let _=channel.send(t,mut tx,n)
  n=n+2u64
  let _=channel.send(t,mut tx,n)
  FINAL
}
"#;

/// The root receives RECEIVES values, runs PARENT, then awaits the child,
/// whose body is CHILD; the printed total encodes every value in order.
const ROOT: &str = r#"
DECLS
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let (sender,receiver):(Sender[u64],Receiver[u64])=channel.open(tasks)
    let mut rx=take receiver
    let child=task.async(s,fn()->u64 {
      let owned=take sender
      CHILD
    })
    let mut sum=0u64
    for _ in 0u64..RECEIVES {
      match channel.receive(tasks,mut rx) {Ok(x)=>{sum=sum*100u64+x}, Error(_)=>{}}
    }
    PARENT
    sum*10000u64+task.await(s,take child)
  })
  let _=out.print(output,f"{total}\n")
  Ok(())
}
"#;

/// The proved shape: the child's whole local `n`, read after the call.
const SLOT: &str = "let mut n=1u64\n      wait_mut(tasks,owned,mut n)\n      n+1000u64";

struct Shape<'a> {
    decls: &'a str,
    child: &'a str,
    receives: u32,
    parent: &'a str,
    last: &'a str,
}

const DONE: Shape<'static> = Shape {
    decls: "",
    child: SLOT,
    receives: 2,
    parent: "",
    last: "n=n*10u64",
};

fn source(shape: &Shape<'_>) -> String {
    let decls = format!("{}{}", WAIT_MUT.replace("FINAL", shape.last), shape.decls);
    ROOT.replace("DECLS", &decls)
        .replace("CHILD", shape.child)
        .replace("RECEIVES", &format!("{}u64", shape.receives))
        .replace("PARENT", shape.parent)
}

/// The resumable `wait_mut` plan: its exclusive parameter is a pointer-sized
/// address record bound to the parameter's slot.
fn wait_mut(plans: &ContinuationPlans) -> &ContinuationPlan {
    plans
        .plans
        .values()
        .find(|p| {
            p.parameters
                .iter()
                .any(|r| r.passing == ParameterPassing::ExclusiveAddress)
        })
        .unwrap_or_else(|| panic!("no plan holds an exclusive record: {:#?}", summary(plans)))
}

fn summary(plans: &ContinuationPlans) -> Vec<String> {
    plans
        .plans
        .values()
        .map(|p| {
            format!(
                "{:?} {:?} {:?}",
                p.representation,
                p.parameters.iter().map(|r| r.passing).collect::<Vec<_>>(),
                p.states
                    .iter()
                    .map(|s| s.exclusive.clone())
                    .collect::<Vec<_>>()
            )
        })
        .collect()
}

/// Plans of every body that calls a definition named `name`.
fn callers<'p>(
    program: &ReadyProgram,
    plans: &'p ContinuationPlans,
    name: &str,
) -> Vec<&'p ContinuationPlan> {
    program
        .units
        .iter()
        .map(|u| u.unit())
        .filter(|u| {
            u.tables.symbols.iter().any(|s| {
                s.definition.0.name == name && s.definition.0.package.package_is("standalone", "0")
            })
        })
        .filter_map(|u| plans.get(u.header.instance))
        .collect()
}

/// The child's state at the nested `wait_mut` call and its proof.
fn proved<'p>(plans: &'p ContinuationPlans, callee: ContentHash) -> (&'p ContinuationPlan, usize) {
    plans
        .plans
        .values()
        .find_map(|p| {
            p.states
                .iter()
                .position(|s| s.callee.is_some_and(|c| c.body == callee))
                .map(|i| (p, i))
        })
        .unwrap_or_else(|| panic!("no compact caller of wait_mut: {:#?}", summary(plans)))
}

#[test]
fn runtime55_wait_mut_slot_plans_and_routing() {
    with_compiler_stack(|| {
        let done = source(&DONE);
        let program = ready(&done);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let callee = wait_mut(&plans);
        assert_eq!(callee.representation, Representation::Resumable);
        let record = callee
            .parameters
            .iter()
            .find(|r| r.passing == ParameterPassing::ExclusiveAddress)
            .unwrap();
        assert!(
            record.slot.is_some() && record.offset % 8 == 0,
            "{record:#?}"
        );
        let (child, at) = proved(&plans, callee.body);
        assert_eq!(child.representation, Representation::Resumable);
        let state = &child.states[at];
        let proof = state.exclusive.as_ref().expect("the nested edge's proof");
        assert_eq!(proof.callee, callee.body);
        assert_eq!(proof.parameter, record.source);
        assert_eq!((proof.size, proof.align), (8, 8));
        // The slot is frame storage at the boundary, before the nested frame,
        // and the boundary's loans include its one mutable loan.
        let stored = state.storage.iter().find(|s| s.slot == proof.slot).unwrap();
        assert_eq!(stored.offset, proof.offset);
        assert!(proof.offset + proof.size <= state.callee.unwrap().offset);
        assert!(state.loans.contains(&proof.loan));
        for backend in [Backend::Llvm, Backend::Cranelift] {
            for partition in [Partition::Whole, Partition::Singleton] {
                let (exported, entered) = routing(&program, backend, partition);
                assert!(
                    exported >= 2 && entered,
                    "{backend:?} {partition:?}: {exported} {entered}"
                );
            }
        }
    });
}

#[test]
fn runtime55_wait_mut_slot_local() {
    with_compiler_stack(|| {
        let done = source(&DONE);
        agree(&done, &Fixture::default(), "slot local");
    });
}

#[test]
fn runtime55_wait_mut_slot_migrate_replay() {
    with_compiler_stack(|| {
        let done = source(&DONE);
        let migrate = Fixture {
            migrate: true,
            ..Fixture::default()
        };
        agree_replayed(&done, &migrate, "slot migrate", true);
    });
}

#[test]
fn runtime55_wait_mut_slot_unread() {
    with_compiler_stack(|| {
        let migrate = Fixture {
            migrate: true,
            ..Fixture::default()
        };
        // A caller local used only through the borrow keeps its storage.
        let unread = source(&Shape {
            child: "let mut n=1u64\n      wait_mut(tasks,owned,mut n)\n      5u64",
            ..DONE
        });
        let unread_program = ready(&unread);
        let unread_plans = native::continuation::plans(&unread_program);
        let (child, at) = proved(&unread_plans, wait_mut(&unread_plans).body);
        let proof = child.states[at].exclusive.clone().unwrap();
        assert!(
            child.states[at]
                .storage
                .iter()
                .any(|s| s.slot == proof.slot)
        );
        agree(&unread, &migrate, "slot unread");
    });
}

#[test]
fn runtime55_wait_mut_slot_compact_custody() {
    with_compiler_stack(|| {
        let done = source(&DONE);
        // Two waits under the shared dispatcher: each save leaves its stack
        // and activates once at its state on another worker, live and replayed.
        for (index, trace) in compact_runs(&done, "slot").iter().enumerate() {
            let cell = format!("slot run {index}");
            assert!(exact_compact_custody(trace, &cell) >= 2, "{cell}");
        }
    });
}

/// The proved shape under one stop: plans validate with the edge proved, and
/// every engine agrees locally and under forced migration.
fn stopped_agrees(text: &str, label: &str) {
    let program = ready(text);
    let plans = native::continuation::plans(&program);
    native::continuation::validate(&program, &plans).unwrap();
    let (child, at) = proved(&plans, wait_mut(&plans).body);
    assert!(child.states[at].exclusive.is_some(), "{label}");
    agree(text, &Fixture::default(), &format!("{label} local"));
    let migrate = Fixture {
        migrate: true,
        ..Fixture::default()
    };
    agree(text, &migrate, &format!("{label} migrate"));
}

/// Canceled while `wait_mut` waits at its second send.
#[test]
fn runtime55_wait_mut_slot_canceled() {
    with_compiler_stack(|| {
        let canceled = source(&Shape {
            receives: 1,
            parent: "task.cancel(s,child)",
            ..DONE
        });
        stopped_agrees(&canceled, "slot canceled");
        for (index, trace) in compact_runs(&canceled, "slot canceled").iter().enumerate() {
            let cell = format!("slot canceled run {index}");
            assert!(exact_compact_custody(trace, &cell) >= 1, "{cell}");
        }
    });
}

/// Entered failure after both writes: the slot holds 9 when the child's
/// cleanup runs, and the failure reaches the root.
#[test]
fn runtime55_wait_mut_slot_entered_failure() {
    with_compiler_stack(|| {
        let failed = source(&Shape {
            last: "let _=1u8/0u8",
            ..DONE
        });
        stopped_agrees(&failed, "slot entered failure");
    });
}

/// The root fails while the child is parked at its second send: the saved
/// frame is disposed without resuming its callee.
#[test]
fn runtime55_wait_mut_slot_terminal_stop() {
    with_compiler_stack(|| {
        let stopped = source(&Shape {
            receives: 1,
            parent: "let _=1u8/0u8",
            ..DONE
        });
        stopped_agrees(&stopped, "slot terminal stop");
    });
}

/// The callee's plan refusal, or None when the callee itself is compact.
type Refusal = Option<&'static str>;

/// A place the successor does not prove keeps the stackful form with its
/// current refusal: the callee is refused by `callee_refusal`, or stays
/// compact (proved on other edges) while every caller of it here is refused
/// for `caller_refusal`; the program agrees on every engine under migration.
fn stays_stackful(text: &str, callee: &str, callee_refusal: Refusal, caller_refusal: &str) {
    let program = ready(text);
    let plans = native::continuation::plans(&program);
    native::continuation::validate(&program, &plans).unwrap();
    let callee_plan = program
        .units
        .iter()
        .map(|u| u.unit())
        .find(|u| u.header.definition.0.name == callee)
        .and_then(|u| plans.get(u.header.instance))
        .unwrap_or_else(|| panic!("no plan for {callee}"));
    let expected = match callee_refusal {
        Some(why) => Representation::Stackful(why.into()),
        None => Representation::Resumable,
    };
    assert_eq!(callee_plan.representation, expected, "{callee}");
    let refused = callers(&program, &plans, callee);
    assert!(!refused.is_empty(), "{callee}: no caller plans");
    for p in refused {
        assert!(
            matches!(&p.representation, Representation::Stackful(why) if why.starts_with(caller_refusal)),
            "{callee}: {:?}",
            p.representation
        );
    }
    let migrate = Fixture {
        migrate: true,
        ..Fixture::default()
    };
    agree(text, &migrate, callee);
}

fn root_with(decls: &str, child: &str, receives: &str, parent: &str) -> String {
    ROOT.replace("DECLS", decls)
        .replace("CHILD", child)
        .replace("RECEIVES", receives)
        .replace("PARENT", parent)
}

/// `wait_mut(t, s, mut packed.value)`: an underaligned field. `wait_mut`
/// itself stays compact, but this edge gains nothing from that.
#[test]
fn runtime55_packed_field_stays_stackful() {
    with_compiler_stack(|| {
        let packed = root_with(
            &format!(
                "{}@packed\ntype Wire {{\n  tag: u8,\n  value: u64,\n}}\n",
                WAIT_MUT.replace("FINAL", "n=n*10u64")
            ),
            "let mut w=Wire(tag:1u8, value:1u64)\n      wait_mut(tasks,owned,mut w.value)\n      w.value+1000u64",
            "2u64",
            "",
        );
        stays_stackful(&packed, "wait_mut", None, "exclusive argument");
    });
}

/// `edit(t, s, mut rows[i])`: a gathered SoA row.
#[test]
fn runtime55_soa_row_stays_stackful() {
    with_compiler_stack(|| {
        let soa = root_with(
            "@soa\ntype Row {\n  small: u8,\n  x: u64,\n}\n\
             fn edit(t:Tasks, s:Sender[u64], mut r:Row) {\n  let mut tx=sender.dup(t,s)\n  r.x=7u64\n  let _=channel.send(t,mut tx,r.x)\n  r.x=r.x+2u64\n  let _=channel.send(t,mut tx,r.x)\n}\n",
            "let mut rows:List[Row]=[Row(small:1u8,x:1u64),Row(small:2u8,x:2u64)]\n      edit(tasks,owned,mut rows[1])\n      rows[1].x+1000u64",
            "2u64",
            "",
        );
        stays_stackful(
            &soa,
            "edit",
            Some("exclusive parameter"),
            "suspends inside stackful callee",
        );
    });
}

/// A suspending `inspect(..., f: F)`: a pointer/length view.
#[test]
fn runtime55_flat_view_stays_stackful() {
    with_compiler_stack(|| {
        let flat = root_with(
            "flat type F {\n  bytes: Bytes,\n}\n\
             fn inspect(t:Tasks, s:Sender[u64], f:F) {\n  let mut tx=sender.dup(t,s)\n  let _=channel.send(t,mut tx,7u64)\n  let _=channel.send(t,mut tx,bytes.len(f.bytes))\n}\n",
            "let mut data:List[u8]=[65u8,66u8]\n      let f=F(bytes:bytes.from_list(take data))\n      inspect(tasks,owned,f)\n      1000u64",
            "2u64",
            "",
        );
        stays_stackful(
            &flat,
            "inspect",
            Some("pointer/length parameter"),
            "suspends inside stackful callee",
        );
    });
}

/// An owning `mut` parameter replaced before the wait, then canceled (the
/// drop and assignment vector): resource writeback stays excluded.
#[test]
fn runtime55_owning_replacement_stays_stackful() {
    with_compiler_stack(|| {
        let owning = root_with(
            "fn wait_list(t:Tasks, s:Sender[u64], mut r:List[u64], take next:List[u64]) {\n  let mut tx=sender.dup(t,s)\n  r=take next\n  let _=channel.send(t,mut tx,r[0])\n  list.push(mut r,9u64)\n  let _=channel.send(t,mut tx,r[1])\n}\n",
            "let mut r:List[u64]=[1u64]\n      let next:List[u64]=[7u64]\n      wait_list(tasks,owned,mut r,take next)\n      r[0]+r[1]+1000u64",
            "1u64",
            "task.cancel(s,child)",
        );
        stays_stackful(
            &owning,
            "wait_list",
            Some("exclusive parameter"),
            "suspends inside stackful callee",
        );
    });
}

/// A closure capturing a `mut` parameter would outlive its loan: refused.
#[test]
fn runtime55_escaping_exclusive_capture_is_refused() {
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let file = engine
            .sources
            .add(
                "main.hir",
                "fn escape(mut xs:List[u64])->fn()->u64 { fn()->u64 { list.len(xs) } }\n\
                 pub fn main(rt:Io)->Result[(),GrantError] {\n  let mut xs:List[u64]=[1u64]\n  let f=escape(mut xs)\n  let output=io.stdout(rt)?\n  let _=out.print(output,f\"{f()}\\n\")\n  Ok(())\n}\n"
                    .into(),
            )
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            checked.diagnostics.len() == 1
                && checked.diagnostics[0].rule.as_str() == "CLO-4"
                && checked.diagnostics[0].message.contains("`xs` is mutable"),
            "an escaping borrowed capture was not refused by CLO-4: {:?}",
            checked.diagnostics
        );
    });
}

/// Every forged or stale call-edge claim is refused on import.
#[test]
fn runtime55_forged_exclusive_slot_claims_are_refused() {
    with_compiler_stack(|| {
        let program = ready(&source(&DONE));
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let callee = wait_mut(&plans).body;
        let (child, at) = proved(&plans, callee);
        let child = child.body;
        let other_slot = plans.plans[&child].states[at]
            .storage
            .iter()
            .map(|s| s.slot)
            .find(|s| {
                *s != plans.plans[&child].states[at]
                    .exclusive
                    .as_ref()
                    .unwrap()
                    .slot
            })
            .unwrap_or(SlotId(0));
        let refused = |body: ContentHash, edit: &dyn Fn(&mut ContinuationPlan)| {
            let mut bad = plans.clone();
            edit(bad.plans.get_mut(&body).unwrap());
            native::continuation::validate(&program, &bad)
                .expect_err("a forged exclusive claim was imported")
        };
        let proof = |edit: fn(&mut ExclusiveSlot)| {
            refused(child, &move |p: &mut ContinuationPlan| {
                edit(p.states[at].exclusive.as_mut().unwrap())
            })
        };
        let exclusive = |e: &ContinuationError| matches!(e, ContinuationError::Exclusive(..));
        let mut checks: Vec<(&str, ContinuationError, &dyn Fn(&ContinuationError) -> bool)> = vec![
            ("slot offset", proof(|e| e.offset += 8), &exclusive),
            ("slot", proof(|e| e.slot = SlotId(e.slot.0 + 1)), &exclusive),
            ("loan", proof(|e| e.loan = LoanId(e.loan.0 ^ 1)), &exclusive),
            (
                "actual address",
                proof(|e| e.address = hirsel_lir::ir::AddressId(e.address.0 + 1)),
                &exclusive,
            ),
            ("callee parameter", proof(|e| e.parameter ^= 1), &exclusive),
            (
                "callee identity",
                proof(|e| e.callee = ContentHash::of_bytes(b"another callee")),
                &exclusive,
            ),
            ("alignment", proof(|e| e.align = 1), &exclusive),
            ("extent", proof(|e| e.size = 16), &exclusive),
            ("layout", proof(|e| e.layout.0 += 1), &exclusive),
            (
                "normal target",
                proof(|e| e.normal = EdgeId(e.normal.0 + 1)),
                &exclusive,
            ),
            (
                "cleanup target",
                proof(|e| e.failure = FailureId(e.failure.0 + 1)),
                &exclusive,
            ),
            (
                "omitted proof",
                refused(child, &|p| p.states[at].exclusive = None),
                &exclusive,
            ),
            (
                "omitted retention",
                refused(child, &|p| {
                    let slot = p.states[at].exclusive.as_ref().unwrap().slot;
                    p.storage.retain(|s| s.slot != slot);
                    p.states[at].storage.retain(|s| s.slot != slot);
                }),
                &|e| {
                    matches!(
                        e,
                        ContinuationError::Storage(..) | ContinuationError::Exclusive(..)
                    )
                },
            ),
            (
                "callee record held by value",
                refused(callee, &|p| {
                    for r in &mut p.parameters {
                        if r.passing == ParameterPassing::ExclusiveAddress {
                            r.passing = ParameterPassing::ByValue;
                        }
                    }
                }),
                &|e| matches!(e, ContinuationError::Parameter(..)),
            ),
            (
                "callee record misaligned",
                refused(callee, &|p| {
                    for r in &mut p.parameters {
                        if r.passing == ParameterPassing::ExclusiveAddress {
                            r.offset += 4;
                        }
                    }
                }),
                &|e| matches!(e, ContinuationError::Parameter(..)),
            ),
            (
                "stale certificate",
                refused(child, &|p| {
                    p.certificate.identity = ContentHash::of_bytes(b"cached")
                }),
                &|e| matches!(e, ContinuationError::Certificate(_)),
            ),
        ];
        if other_slot != SlotId(0) {
            checks.push((
                "another frame slot",
                refused(child, &move |p| {
                    let stored = p.states[at]
                        .storage
                        .iter()
                        .find(|s| s.slot == other_slot)
                        .unwrap()
                        .clone();
                    let e = p.states[at].exclusive.as_mut().unwrap();
                    e.slot = stored.slot;
                    e.offset = stored.offset;
                }),
                &exclusive,
            ));
        }
        for (name, error, expected) in &checks {
            assert!(expected(error), "{name}: {error:?}");
        }
        eprintln!("runtime55 forged claims refused: {}", checks.len());
    });
}
