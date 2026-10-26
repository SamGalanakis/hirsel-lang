//! The native dispatcher must allocate the whole checked runtime frame.
use hirsel_engine::with_compiler_stack;

#[test]
fn entered_runtime_import_refuses_a_forged_allocation_extent() {
    with_compiler_stack(|| {
        let program = super::runtime50_continuation::ready(
            r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let value=task.scope(tasks,fn(s:Scope)->u64 {7u64})
  failure.check(value==7u64,Arithmetic())
  Ok(())
}
"#,
        );
        let plans = hirsel_driver::native::continuation::plans(&program);
        let units: Vec<_> = program.units.iter().collect();
        hirsel_lir::continuation::validate_runtime_operations(
            &units,
            &program.table,
            &program.target,
            &plans,
        )
        .unwrap();
        let mut forged = plans.clone();
        let wrapper = forged
            .plans
            .values_mut()
            .find(|p| {
                p.states.iter().any(|s| {
                    s.runtime.as_ref().is_some_and(|r| {
                        r.kind == hirsel_lir::continuation::RuntimeOpKind::ScopeEnter
                    })
                })
            })
            .expect("entered scope plan");
        assert!(wrapper.cost.frame_bytes > 8);
        wrapper.cost.frame_bytes = 8;
        assert!(
            hirsel_lir::continuation::validate_runtime_operations(
                &units,
                &program.table,
                &program.target,
                &forged
            )
            .is_err(),
            "runtime importer trusted an undersized allocation"
        );
    });
}

#[test]
fn entered_runtime_import_keeps_valid_runtime_and_cleanup_frames() {
    with_compiler_stack(|| {
        let program = super::runtime50_continuation::ready(
            r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let held=task.scope(tasks,fn(s:Scope)->Sender[u64] {
    let (tx,rx):(Sender[u64],Receiver[u64])=channel.open(tasks)
    take tx
  })
  Ok(())
}
"#,
        );
        let plans = hirsel_driver::native::continuation::plans(&program);
        hirsel_driver::native::continuation::validate(&program, &plans).unwrap();
        assert!(
            plans
                .plans
                .values()
                .any(|p| p.states.iter().any(|s| s.runtime.is_some()) && !p.cleanups.is_empty()),
            "fixture needs runtime and cleanup states in one frame"
        );
        let units: Vec<_> = program.units.iter().collect();
        hirsel_lir::continuation::validate_runtime_operations(
            &units,
            &program.table,
            &program.target,
            &plans,
        )
        .unwrap();
    });
}
