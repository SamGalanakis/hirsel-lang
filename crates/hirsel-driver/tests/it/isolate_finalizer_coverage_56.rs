//! Complete typed cleanup coverage precedes runtime resolver admission.
#[test]
fn finalizer_coverage_retains_nested_and_variant_descriptors_and_refuses_omissions() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native;
        use hirsel_lir::continuation::PlanInputs;
        use hirsel_lir::continuation::isolate::FinalizerCoverage;
        let program = super::runtime50_continuation::ready(
            r#"
resource type Leaf { output: Stdout }
drop(x) { let _ = out.print(x.output, "leaf\n") }
resource type Root { output: Stdout, child: Option[Leaf] }
drop(x) { let _ = out.print(x.output, "root\n") }
pub fn main(rt: Io) -> Result[(), GrantError] {
  let output = io.stdout(rt)?
  let held = Root(output: output, child: Some(Leaf(output: output)))
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
        let plans = native::continuation::plans(&program);
        let (&root, _) = program
            .finalizers
            .iter()
            .find(|(layout, _)| {
                program
                    .table
                    .layout(**layout)
                    .unwrap()
                    .finalizer
                    .as_ref()
                    .unwrap()
                    .definition
                    .to_string()
                    .contains("Root")
            })
            .unwrap();
        let coverage =
            FinalizerCoverage::propose(root, &program.finalizers, &units, &inputs, &plans, 33)
                .unwrap();
        assert_eq!(
            coverage.finalizers.len(),
            2,
            "nested leaf finalizer was omitted"
        );
        coverage.check(&units, &inputs, &plans, 33).unwrap();
        for mutation in 0..6 {
            let mut bad = coverage.clone();
            match mutation {
                0 => {
                    bad.layouts.pop();
                }
                1 => {
                    bad.finalizers.pop();
                }
                2 => bad.finalizers[1].nested.offset = bad.finalizers[0].nested.offset,
                3 => bad.end += 16,
                4 => {
                    let first = bad.layouts[0].clone();
                    bad.layouts.push(first);
                }
                5 => {
                    bad.layouts
                        .iter_mut()
                        .find(|(id, _)| *id == root)
                        .unwrap()
                        .1
                        .drop
                        .clear();
                }
                _ => unreachable!(),
            }
            assert!(
                bad.check(&units, &inputs, &plans, 33).is_err(),
                "mutation {mutation}"
            );
        }
        let mut missing = program.finalizers.clone();
        missing.remove(&coverage.finalizers[1].source.layout);
        assert!(FinalizerCoverage::propose(root, &missing, &units, &inputs, &plans, 33).is_err());
    });
}
