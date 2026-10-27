//! Physical proposals remain independent of ordinary isolate admission.
use hirsel_driver::native;
use hirsel_lir::continuation::isolate::IsolateProtocol;
use hirsel_lir::continuation::{PlanInputs, Representation, ResumeAction};
use hirsel_lir::ir::{CallId, Callee};

#[test]
fn isolate_frame_retains_entered_mark_and_refuses_forged_physical_custody() {
    hirsel_engine::with_compiler_stack(|| {
        let program = super::runtime50_continuation::ready(
            r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let value: Result[String, Failure] = task.isolate(tasks, fn() -> String { "held" })
  match value { Ok(text) => failure.check(text == "held", Arithmetic()), Error(_) => failure.check(false, Bounds()) }
  Ok(())
}
"#,
        );
        let units: Vec<_> = program.units.iter().collect();
        let inputs = PlanInputs {
            table: &program.table,
            target: &program.target,
            environments: &program.ahead_environments,
            runtime: native::continuation::runtime_identity(),
            classify: &native::continuation::classify,
        };
        let ordinary = native::continuation::plans(&program);
        let mut checked = 0;
        for caller in &program.units {
            for (at, call) in caller.unit().tables.calls.iter().enumerate() {
                let Callee::Direct(symbol) = call.callee else {
                    continue;
                };
                let symbol = &caller.unit().tables.symbols[symbol.index()];
                if !symbol.definition.is_core("task.isolate") {
                    continue;
                }
                let wrapper = units
                    .iter()
                    .copied()
                    .find(|u| Some(u.unit().header.instance) == symbol.instance)
                    .unwrap();
                let Some(target) = wrapper.unit().tables.calls.iter().find_map(|c| {
                    if let Callee::Bound { code, .. } = c.callee {
                        wrapper.unit().tables.symbols[code.index()].instance
                    } else {
                        None
                    }
                }) else {
                    continue;
                };
                let callback = units
                    .iter()
                    .copied()
                    .find(|u| u.unit().header.instance == target)
                    .unwrap();
                let protocol = IsolateProtocol::derive(
                    caller,
                    CallId(at as u32),
                    wrapper,
                    callback,
                    program.ahead_environments[&target],
                    &program.table,
                )
                .unwrap();
                let plan = protocol.frame(&units, &inputs, &ordinary).unwrap();
                plan.check(&units, &inputs, &ordinary).unwrap();
                assert!(
                    matches!(
                        ordinary
                            .get(protocol.identity.wrapper)
                            .unwrap()
                            .representation,
                        Representation::Resumable
                    ),
                    "only independently checked source custody enables admission"
                );
                let admitted = ordinary.get(protocol.identity.wrapper).unwrap();
                assert!(
                    !admitted
                        .isolate
                        .as_ref()
                        .unwrap()
                        .callback_bodies
                        .is_empty()
                );
                let emit = |proposed: &hirsel_lir::continuation::ContinuationPlans,
                            accepted: bool| {
                    use hirsel_lir::emission::{EmitInputs, OptLevel};
                    let empty = std::collections::BTreeSet::new();
                    let inputs = EmitInputs {
                        ahead: None,
                        continuation: Some(proposed),
                        alternate: false,
                        index: None,
                        input: &program.input,
                        target: &program.target,
                        layouts: &program.table,
                        keys: &program.keys,
                        manifest: &program.manifest,
                        statics: &program.statics,
                        inline_instances: &program.inline_instances,
                        available_externally: &empty,
                        callbacks: !program.callbacks.is_empty(),
                        tracking: program.tracking(),
                        foreign: &program.foreign_dispatch,
                        opt: OptLevel::Default,
                        measurement: false,
                        fuel: program.fuel,
                        verify_backend: true,
                    };
                    for (backend, result) in [
                        (
                            "LLVM",
                            hirsel_llvm::emit_resume_object(&units, &inputs, proposed)
                                .map_err(|e| e.to_string()),
                        ),
                        (
                            "Cranelift",
                            hirsel_cranelift::emit_resume_object(&units, &inputs, proposed)
                                .map_err(|e| e.to_string()),
                        ),
                    ] {
                        assert_eq!(result.is_ok(), accepted, "{backend}: {result:?}");
                    }
                };
                emit(&ordinary, true);
                for change in 0..6 {
                    let mut forged = ordinary.clone();
                    let row = forged.plans.get_mut(&protocol.identity.wrapper).unwrap();
                    let begin = row
                        .states
                        .iter()
                        .position(|s| s.boundary.call == protocol.begin.call)
                        .unwrap();
                    match change {
                        0 => row.isolate = None,
                        1 => row.isolate.as_mut().unwrap().callback_bodies.clear(),
                        2 => {
                            row.isolate.as_mut().unwrap().callback_bodies[0].digest =
                                hirsel_syntax::ContentHash::default()
                        }
                        3 => row.states[begin].runtime = None,
                        4 => row.states[begin].runtime.as_mut().unwrap().results[0].offset = 0,
                        5 => row.cost.frame_bytes = 8,
                        _ => unreachable!(),
                    }
                    assert!(native::continuation::validate(&program, &forged).is_err());
                    emit(&forged, false);
                }
                let begin = plan
                    .frame
                    .states
                    .iter()
                    .position(|s| s.boundary.call == protocol.begin.call)
                    .unwrap();
                assert_eq!(
                    plan.frame.states[begin].action,
                    ResumeAction::ResumeRuntime(protocol.begin.call)
                );
                let rejects = |bad: &hirsel_lir::continuation::isolate::IsolatePlan| {
                    assert!(bad.check(&units, &inputs, &ordinary).is_err());
                };
                let mut bad = plan.clone();
                bad.frame.states[begin].action =
                    ResumeAction::RetryUnenteredCall(protocol.begin.call);
                rejects(&bad);
                let mut bad = plan.clone();
                bad.frame.states[begin].runtime.as_mut().unwrap().results[0].offset = 0;
                rejects(&bad);
                let mut bad = plan.clone();
                bad.frame.cost.frame_bytes = 8;
                rejects(&bad);
                let mut bad = plan.clone();
                bad.frame.states[begin].runtime = None;
                rejects(&bad);
                let mut bad = plan.clone();
                bad.cleanup.sites.pop();
                rejects(&bad);
                let mut bad = plan.clone();
                bad.frame.states[begin].live.pop();
                rejects(&bad);
                checked += 1;
            }
        }
        assert!(checked > 0);
    });
}

#[test]
fn a_callback_local_finalizer_cannot_hide_behind_a_mechanical_isolate_environment() {
    hirsel_engine::with_compiler_stack(|| {
        let mut program = super::runtime50_continuation::ready(
            r#"
resource type Guard { n: u64 }
drop(x) { failure.check(false, Bounds()) }
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let value: Result[u64, Failure] = task.isolate(tasks, fn() -> u64 {
    let held = Guard(n: 7u64)
    held.n
  })
  match value { Ok(_) => (), Error(_) => () }
  Ok(())
}
"#,
        );
        let proved = native::continuation::plans(&program);
        native::continuation::validate(&program, &proved).unwrap();
        assert!(
            proved.plans.values().any(|p| p.isolate.is_some()),
            "complete source finalizer proof was not admitted"
        );
        let finalizers: std::collections::BTreeSet<_> =
            program.finalizers.values().copied().collect();
        assert!(!finalizers.is_empty());
        program
            .units
            .retain(|u| !finalizers.contains(&u.unit().header.instance));
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let mut found = false;
        for unit in &program.units {
            if unit.unit().header.definition.is_core("task.isolate")
                && unit
                    .unit()
                    .tables
                    .calls
                    .iter()
                    .any(|c| matches!(c.callee, Callee::Bound { .. }))
            {
                let plan = plans.get(unit.unit().header.instance).unwrap();
                assert!(
                    matches!(plan.representation, Representation::Stackful(_)),
                    "{plan:#?}"
                );
                assert!(plan.isolate.is_none());
                found = true;
            }
        }
        assert!(found);
    });
}
