//! Validated continuation plans (runtime-native-50).
use hirsel_driver::native;
use hirsel_engine::{QueryEngine, check_progress, default_context, with_compiler_stack};
use hirsel_lir::continuation::{
    ContinuationError, ContinuationPlan, ContinuationPlans, RegionKind, RegionTokenLayout,
    Representation, ResumeAction,
};
use hirsel_lir::emission::OptLevel;
use hirsel_lir::ir::{BlockId, Holding};
use hirsel_syntax::ContentHash;

/// A child retaining a unique buffer across two channel boundaries.
pub(crate) const TWO_BOUNDARY: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let (sender,receiver):(Sender[u64],Receiver[u64])=channel.open(tasks)
    let mut rx=take receiver
    let buffer:List[u64]=[3u64,5u64,7u64]
    let child=task.async(s,fn()->u64 {
      let mut tx=take sender
      let mut held=take buffer
      list.push(mut held,11u64)
      let _=channel.send(tasks,mut tx,held[0])
      let _=channel.send(tasks,mut tx,held[1])
      held[2]+held[3]
    })
    let mut sum=0u64
    for _ in 0u64..2u64 {
      match channel.receive(tasks,mut rx) {Ok(x)=>{sum=sum+x}, Error(_)=>{}}
    }
    sum+task.await(s,take child)
  })
  let _=out.print(output,f"{total}\n")
  Ok(())
}
"#;

const SCALAR: &str = r#"
fn work(seed:u64)->u64 uses {} {
  let mut x=seed
  for i in 0u64..64u64 {
    x=int.wrapping_add(int.wrapping_mul(x ^ (x >> 7),0x9e3779b97f4a7c15u64),i)
  }
  x
}
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let sum=task.scope(tasks,fn(s:Scope)->u64 {
    let mut set:TaskSet[Result[u64,()]] of s=task.set(s)
    for seed in 1u64..5u64 {
      task.add(s,mut set,task.async(s,fn()->Result[u64,()] {Ok(work(seed))}))
    }
    let mut sum=0u64
    if let Ok(rows)=std@1/task.all(s,take set) {for x in rows {sum=sum ^ x}}
    sum
  })
  let _=out.print(output,f"{sum}\n")
  Ok(())
}
"#;

pub(crate) fn ready(source: &str) -> native::ReadyProgram {
    let mut engine: QueryEngine = crate::source_std::engine();
    let file = engine.sources.add("main.hir", source.to_owned()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let snapshot = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(snapshot).unwrap();
    native::build(
        &validated,
        &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
    )
    .unwrap()
}

/// The child body: the resumable plan whose boundaries are both sends.
pub(crate) fn child(plans: &ContinuationPlans) -> &ContinuationPlan {
    let mut children = plans.plans.values().filter(|p| {
        p.representation == Representation::Resumable
            && p.states.len() == 2
            && p.states
                .iter()
                .all(|s| s.boundary.operation.to_string().ends_with("channel/send"))
    });
    let child = children.next().expect("resumable two-boundary child");
    assert!(children.next().is_none());
    child
}

fn refused(
    program: &native::ReadyProgram,
    plans: &ContinuationPlans,
    body: ContentHash,
    edit: impl FnOnce(&mut ContinuationPlan),
) -> ContinuationError {
    let mut bad = plans.clone();
    edit(bad.plans.get_mut(&body).unwrap());
    native::continuation::validate(program, &bad).expect_err("a mutated plan was imported")
}

#[test]
fn runtime50_resume_import_mutations_refused() {
    with_compiler_stack(|| {
        let program = ready(TWO_BOUNDARY);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        assert_eq!(
            plans,
            native::continuation::plans(&program),
            "plans are deterministic"
        );
        let plan = child(&plans);
        let id = plan.body;
        // Both states retain the unique buffer and the sender as owners, at
        // distinct labels after each send, with cleanup from each send's edge.
        for state in &plan.states {
            // The buffer and sender are `mut` bindings: body storage.
            let owners: Vec<_> = state.storage.iter().map(|s| s.repr).collect();
            assert!(
                owners.contains(&hirsel_lir::ir::ValueRepr::Block)
                    && owners.contains(&hirsel_lir::ir::ValueRepr::Handle),
                "{state:#?}"
            );
            assert!(!state.cleanup.anchors.is_empty());
        }
        assert_ne!(plan.states[0].target.block, plan.states[1].target.block);
        assert_eq!(plan.states.len(), 2, "the two original send boundaries");
        // Entered instruction cleanup adds its own labels without changing
        // either send or its import-mutation assertions below.
        assert_eq!(plan.cost.boundaries, 2 + plan.cleanups.len() as u32);
        assert!(plan.cost.frame_bytes > 8 && plan.cost.frame_bytes <= 4096);
        // The child's specialized task wrapper nests the child's frame at its
        // first boundary and finishes the task at its second.
        let wrapper = plans
            .plans
            .values()
            .find(|p| {
                p.states
                    .first()
                    .and_then(|s| s.callee)
                    .is_some_and(|c| c.body == id)
            })
            .expect("resumable wrapper nesting the child");
        assert_eq!(wrapper.representation, Representation::Resumable);
        let nested = wrapper.states[0].callee.unwrap();
        assert_eq!(nested.bytes, plan.cost.frame_bytes);
        assert!(wrapper.cost.frame_bytes >= nested.offset + nested.bytes);
        assert!(
            wrapper.states[1]
                .boundary
                .operation
                .to_string()
                .ends_with("TaskFinish")
        );
        // Unknown calls keep the general stackful form.
        assert!(plans.plans.values().any(|p| matches!(
            &p.representation,
            Representation::Stackful(why) if why.contains("indirect call")
        )));
        let buffer = plan.states[0]
            .storage
            .iter()
            .position(|s| s.repr == hirsel_lir::ir::ValueRepr::Block)
            .unwrap();
        let checks: Vec<(&str, ContinuationError, fn(&ContinuationError) -> bool)> = vec![
            (
                "live owner representation",
                refused(&program, &plans, id, |p| {
                    p.states[0].storage[buffer].repr = hirsel_lir::ir::ValueRepr::Uncounted
                }),
                |e| matches!(e, ContinuationError::Storage(_, 1)),
            ),
            (
                "dropped live owner",
                refused(&program, &plans, id, |p| {
                    p.states[1]
                        .storage
                        .retain(|s| s.repr != hirsel_lir::ir::ValueRepr::Block)
                }),
                |e| matches!(e, ContinuationError::Storage(_, 2)),
            ),
            (
                "invented live value",
                refused(&program, &plans, id, |p| {
                    let mut extra = p.states[0].live[0].clone();
                    extra.value = hirsel_lir::ir::ValueId(u32::MAX - 1);
                    p.states[0].live.push(extra)
                }),
                |e| matches!(e, ContinuationError::LiveOwner(_, 1)),
            ),
            (
                "live value holding",
                refused(&program, &plans, id, |p| {
                    let l = &mut p.states[1].live[0];
                    l.holding = if l.holding == Holding::Owned {
                        Holding::Plain
                    } else {
                        Holding::Owned
                    }
                }),
                |e| matches!(e, ContinuationError::LiveOwner(_, 2)),
            ),
            (
                "region token",
                refused(&program, &plans, id, |p| {
                    let l = p.states[0].live[0].clone();
                    p.states[0].regions.push(RegionTokenLayout {
                        value: l.value,
                        layout: l.layout,
                        kind: RegionKind::Mark,
                    })
                }),
                |e| matches!(e, ContinuationError::RegionToken(_, 1)),
            ),
            (
                "cleanup entry",
                refused(&program, &plans, id, |p| {
                    p.states[1].cleanup.entry = p.states[0].cleanup.entry
                }),
                |e| matches!(e, ContinuationError::CleanupAnchor(_, 2)),
            ),
            (
                "cleanup anchors",
                refused(&program, &plans, id, |p| {
                    p.states[0].cleanup.anchors.pop();
                }),
                |e| matches!(e, ContinuationError::CleanupAnchor(_, 1)),
            ),
            (
                "resume label",
                refused(&program, &plans, id, |p| {
                    p.states[0].target.block = p.states[1].target.block
                }),
                |e| matches!(e, ContinuationError::ResumeLabel(_, 1)),
            ),
            (
                "action retries another call",
                refused(&program, &plans, id, |p| {
                    p.states[0].action = ResumeAction::RetryUnenteredCall(p.states[1].boundary.call)
                }),
                |e| matches!(e, ContinuationError::ResumeLabel(_, 1)),
            ),
            (
                "entered runtime action without its independently checked operation proof",
                refused(&program, &plans, id, |p| {
                    p.states[0].action = ResumeAction::ResumeRuntime(p.states[0].boundary.call)
                }),
                |e| matches!(e, ContinuationError::ResumeLabel(_, 1)),
            ),
            (
                "runtime helper evidence attached to an ordinary retry",
                refused(&program, &plans, id, |p| {
                    p.states[0].runtime = Some(hirsel_lir::continuation::RuntimeOpPlan {
                        kind: hirsel_lir::continuation::RuntimeOpKind::OrderedPrint,
                        helper: "hirsel_rt_v2_task_scope_exit".into(),
                        results: Vec::new(),
                        scope: None,
                    });
                }),
                |e| matches!(e, ContinuationError::RuntimeOperation(_, 1)),
            ),
            (
                "completed call claimed without initialized entered state",
                refused(&program, &plans, id, |p| {
                    p.states[0].action = ResumeAction::AfterEnteredCall(
                        p.states[0].boundary.call,
                        p.states[0].target.edge,
                    )
                }),
                |e| matches!(e, ContinuationError::ResumeLabel(_, 1)),
            ),
            (
                "unproved cleanup action",
                refused(&program, &plans, id, |p| {
                    p.states[0].action = ResumeAction::ResumeCleanup {
                        block: p.states[0].cleanup.entry,
                        instruction: 0,
                        anchor: p.states[0].cleanup.anchors[0],
                    }
                }),
                |e| matches!(e, ContinuationError::ResumeLabel(_, 1)),
            ),
            (
                "resume state ordinal",
                refused(&program, &plans, id, |p| p.states.swap(0, 1)),
                |e| matches!(e, ContinuationError::Boundary(_, 1)),
            ),
            (
                "frame offset",
                refused(&program, &plans, id, |p| p.states[0].live[0].offset = 0),
                |e| matches!(e, ContinuationError::Frame(_)),
            ),
            (
                "restart label",
                refused(&program, &plans, id, |p| {
                    p.states[1].target.block = BlockId(0)
                }),
                |e| matches!(e, ContinuationError::ResumeLabel(_, 2)),
            ),
            (
                "frame storage offset",
                refused(&program, &plans, id, |p| p.storage[0].offset += 8),
                |e| matches!(e, ContinuationError::Storage(_, 0)),
            ),
            (
                "representation",
                refused(&program, &plans, id, |p| {
                    p.representation = Representation::Direct
                }),
                |e| matches!(e, ContinuationError::Representation(_)),
            ),
            (
                "body digest",
                refused(&program, &plans, id, |p| {
                    p.digest = ContentHash::of_bytes(b"stale body")
                }),
                |e| matches!(e, ContinuationError::Body(_)),
            ),
            (
                "certificate",
                refused(&program, &plans, id, |p| {
                    p.certificate.identity = ContentHash::of_bytes(b"stale runtime")
                }),
                |e| matches!(e, ContinuationError::Certificate(_)),
            ),
        ];
        for (name, error, expected) in &checks {
            assert!(expected(error), "{name}: {error:?}");
        }
        // Cache binding: a plan set missing a body, or one bound to another set.
        let mut bad = plans.clone();
        bad.plans.remove(&id);
        assert!(matches!(
            native::continuation::validate(&program, &bad),
            Err(ContinuationError::Coverage(_))
        ));
        let mut bad = plans.clone();
        bad.identity = ContentHash::of_bytes(b"other partition");
        assert_eq!(
            native::continuation::validate(&program, &bad),
            Err(ContinuationError::Identity)
        );

        // Final-frame certificates of the ahead roots, in both backends.
        let program = ready(SCALAR);
        let plan = native::ahead_plan(&program);
        let plans = native::continuation::plans(&program);
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let objects = native::emit_backend_objects(
                &program,
                backend,
                OptLevel::Default,
                native::Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let evidence = objects.iter().find_map(|o| o.ahead.clone()).unwrap();
            let certificates = native::continuation::ahead_certificates(&program, &objects);
            assert_eq!(certificates.len(), 1, "{backend:?}");
            let certificate = &certificates[0];
            certificate.check(&plan, &plans, &evidence).unwrap();
            assert_ne!(certificate.ordinary_symbol, certificate.ahead_symbol);
            assert_eq!(certificate.caller.spines, None);
            for mutation in 0..7 {
                let mut bad = certificate.clone();
                match mutation {
                    0 => bad.ahead_symbol = bad.ordinary_symbol.clone(),
                    1 => bad.ordinary_symbol.push('x'),
                    2 => bad.final_frames.nodes[0].1 += 16,
                    3 => bad
                        .final_frames
                        .nodes
                        .iter_mut()
                        .find(|(_, _, calls)| !calls.is_empty())
                        .expect("the scalar root has a nested work call")
                        .2
                        .clear(),
                    4 => bad.max_stack_bytes -= 8,
                    5 => bad.caller.spines = Some(ContentHash::of_bytes(b"caller")),
                    _ => bad.result.reads.push((
                        bad.root,
                        hirsel_lir::ir::CallId(0),
                        hirsel_lir::layout_types::LayoutId(0),
                        hirsel_lir::layout_types::LayoutId(0),
                        1,
                    )),
                }
                assert!(
                    bad.check(&plan, &plans, &evidence).is_err(),
                    "{backend:?}: final-frame mutation {mutation} accepted"
                );
            }
        }
    })
}
