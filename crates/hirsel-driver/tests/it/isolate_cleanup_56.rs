//! Cleanup coverage is separate from callback identity and physical admission.
use hirsel_lir::continuation::isolate::{CleanupOwner, IsolateProtocol};
use hirsel_lir::ir::{CallId, Callee, ValueId};

const PLAIN: &str = r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let value: Result[String, Failure] = task.isolate(tasks, fn() -> String { "held" })
  match value { Ok(text) => failure.check(text == "held", Arithmetic()), Error(_) => failure.check(false, Bounds()) }
  Ok(())
}
"#;

fn check(source: &str, mechanical: bool) {
    hirsel_engine::with_compiler_stack(|| {
        let program = super::runtime50_continuation::ready(source);
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
                let wrapper = program
                    .units
                    .iter()
                    .find(|u| Some(u.unit().header.instance) == symbol.instance)
                    .unwrap();
                let Some(code) = wrapper.unit().tables.calls.iter().find_map(|c| {
                    if let Callee::Bound { code, .. } = c.callee {
                        Some(code)
                    } else {
                        None
                    }
                }) else {
                    continue;
                };
                let target = &wrapper.unit().tables.symbols[code.index()];
                let callback = program
                    .units
                    .iter()
                    .find(|u| Some(u.unit().header.instance) == target.instance)
                    .unwrap();
                let proof = IsolateProtocol::derive(
                    caller,
                    CallId(at as u32),
                    wrapper,
                    callback,
                    program.ahead_environments[&callback.unit().header.instance],
                    &program.table,
                )
                .unwrap();
                proof
                    .validate(caller, wrapper, callback, &program.table)
                    .unwrap();
                let plan = proof.synchronous_cleanup(wrapper, &program.table);
                if !mechanical {
                    assert!(
                        plan.is_none(),
                        "fallible capture destruction must require its entered protocol"
                    );
                    checked += 1;
                    continue;
                }
                let plan = plan.unwrap_or_else(|| {
                    panic!("mechanical wrapper not recognized: {:#?}", wrapper.unit())
                });
                assert!(!plan.sites.is_empty());
                assert!(
                    plan.sites.iter().any(|s| s.environment.is_some()),
                    "actual closure cleanup is covered"
                );
                assert!(proof.check_synchronous_cleanup(
                    caller,
                    wrapper,
                    callback,
                    &program.table,
                    &plan
                ));
                let rejects = |bad| {
                    assert!(!proof.check_synchronous_cleanup(
                        caller,
                        wrapper,
                        callback,
                        &program.table,
                        bad
                    ))
                };
                let mut bad = plan.clone();
                bad.sites.pop();
                rejects(&bad);
                let mut bad = plan.clone();
                bad.sites.push(plan.sites[0].clone());
                rejects(&bad);
                let mut bad = plan.clone();
                bad.sites[0].at.instruction = u32::MAX;
                rejects(&bad);
                let mut bad = plan.clone();
                bad.sites[0].owner = CleanupOwner::Value(ValueId(u32::MAX));
                rejects(&bad);
                let mut bad = plan.clone();
                bad.sites[0].layout = hirsel_lir::layout_types::LayoutId(u32::MAX);
                rejects(&bad);
                let mut bad = plan.clone();
                bad.sites
                    .iter_mut()
                    .find(|s| s.environment.is_some())
                    .unwrap()
                    .environment = None;
                rejects(&bad);
                let mut bad = plan.clone();
                bad.digest = hirsel_syntax::ContentHash::default();
                rejects(&bad);
                checked += 1;
            }
        }
        assert!(checked > 0);
    });
}

#[test]
fn mechanical_isolate_cleanup_covers_failure_only_sites_and_rejects_forged_owners() {
    check(PLAIN, true);
}

#[test]
fn residual_finalizer_capture_cannot_claim_synchronous_isolate_cleanup() {
    check(
        r#"
resource type Guard { n: u64 }
drop(x) { failure.check(false, Bounds()) }
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let guard = Guard(n: 7u64)
  let value: Result[u64, Failure] = task.isolate(tasks, fn() -> u64 {
    if guard.n == 0u64 { let _ = take guard }
    7u64
  })
  match value { Ok(_) => (), Error(_) => () }
  Ok(())
}
"#,
        false,
    );
}
