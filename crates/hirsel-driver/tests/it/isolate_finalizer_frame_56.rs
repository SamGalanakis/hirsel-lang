//! A source finalizer's compact extent never substitutes for root custody.
#[test]
fn finalizer_frame_checks_self_and_disjoint_bounded_nested_storage() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native;
        use hirsel_lir::continuation::isolate::{FinalizerFrame, FinalizerSource};
        use hirsel_lir::continuation::{MAX_FRAME_BYTES, PlanInputs};
        let program = super::runtime50_continuation::ready(
            r#"
resource type Guard { output: Stdout }
drop(x) { let _ = out.print(x.output, "drop\n") }
pub fn main(rt: Io) -> Result[(), GrantError] {
  let held = Guard(output: io.stdout(rt)?)
  Ok(())
}
"#,
        );
        let units: Vec<_> = program.units.iter().collect();
        let plans = native::continuation::plans(&program);
        let inputs = PlanInputs {
            table: &program.table,
            target: &program.target,
            environments: &program.ahead_environments,
            runtime: native::continuation::runtime_identity(),
            classify: &native::continuation::classify,
        };
        let mut checked = 0;
        for (&layout, &body) in &program.finalizers {
            let record = program.table.layout(layout).unwrap();
            if !record
                .finalizer
                .as_ref()
                .unwrap()
                .definition
                .to_string()
                .contains("Guard")
            {
                continue;
            }
            let source = FinalizerSource::derive(&units, &program.table, layout, body).unwrap();
            let frame = FinalizerFrame::propose(&source, &units, &inputs, &plans, 33).unwrap();
            assert_eq!(frame.nested.offset, 48);
            frame.check(&units, &inputs, &plans, 33).unwrap();
            for mutation in 0..7 {
                let mut bad = frame.clone();
                match mutation {
                    0 => bad.nested.offset = 32,
                    1 => bad.nested.offset += 1,
                    2 => bad.nested.bytes = 8,
                    3 => bad.nested.body = hirsel_syntax::ContentHash::default(),
                    4 => bad.self_slot = hirsel_lir::ir::SlotId(u32::MAX),
                    5 => bad.end += 16,
                    6 => bad.source.borrowed_self = hirsel_lir::ir::ValueId(u32::MAX),
                    _ => unreachable!(),
                }
                assert!(
                    bad.check(&units, &inputs, &plans, 33).is_err(),
                    "mutation {mutation}"
                );
            }
            for after in [0, MAX_FRAME_BYTES, u64::MAX] {
                assert!(FinalizerFrame::propose(&source, &units, &inputs, &plans, after).is_err());
            }
            checked += 1;
        }
        assert!(checked > 0);
    });
}
