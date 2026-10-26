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
