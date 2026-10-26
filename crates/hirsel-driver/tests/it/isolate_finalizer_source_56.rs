//! Source finalizer identity precedes any entered cleanup admission.
#[test]
fn finalizer_source_binds_actual_resource_borrow_and_rejects_forged_resolvers() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_lir::continuation::isolate::FinalizerSource;
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
            source.check(&units, &program.table).unwrap();
            for mutation in 0..5 {
                let mut bad = source.clone();
                match mutation {
                    0 => bad.layout = hirsel_lir::layout_types::LayoutId(u32::MAX),
                    1 => bad.body = hirsel_syntax::ContentHash::default(),
                    2 => bad.digest = hirsel_syntax::ContentHash::default(),
                    3 => bad.abi = hirsel_lir::abi_types::FnAbiId(u32::MAX),
                    4 => bad.borrowed_self = hirsel_lir::ir::ValueId(u32::MAX),
                    _ => unreachable!(),
                }
                assert!(
                    bad.check(&units, &program.table).is_err(),
                    "mutation {mutation}"
                );
            }
            let main = program
                .units
                .iter()
                .find(|u| u.unit().header.definition.0.name == "main")
                .unwrap();
            assert!(
                FinalizerSource::derive(
                    &units,
                    &program.table,
                    layout,
                    main.unit().header.instance
                )
                .is_err()
            );
            checked += 1;
        }
        assert!(checked > 0);
    });
}
