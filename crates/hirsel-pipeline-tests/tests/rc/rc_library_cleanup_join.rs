//! The originally failing payload join through all 42 execution configurations.
#[test]
#[ignore = "native: full matrix"]
fn a_match_that_moves_only_the_ok_payload_agrees_in_all_configurations() {
    use hirsel_driver::native::{Observation, Outcome, Toolchain, differential, full_matrix};
    use hirsel_driver::*;
    use hirsel_engine::*;
    with_compiler_stack(|| {
        let source = r#"pub fn main(rt: Io, args: Args) -> Result[(), LaunchError] {
  let text: String = match string.from_bytes(b"ok") { Ok(s) => take s, Error(_) => "bad" }
  out.print(io.stdout(rt)? Grant, text) ? Output
  Ok(())
}
pub type LaunchError = Grant(GrantError) | Output(OutputError)
"#;
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let context = default_context();
        let file = engine
            .sources
            .add("payload-join.hir", source.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&context);
        let config = RunConfiguration {
            entry: entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let execution =
            run_observed_snapshot(&mut engine, &config, SimulationConfig::default()).unwrap();
        let reference = Observation::of_reference(execution.observation.as_ref().unwrap());
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let configurations = full_matrix();
        assert_eq!(configurations.len(), 50);
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cleanup-native");
        std::fs::create_dir_all(&dir).unwrap();
        let toolchain = Toolchain::for_matrix(&dir.join("runtime"), &configurations).unwrap();
        for (name, outcome) in differential(
            &validated,
            &entry,
            &reference,
            &configurations,
            &toolchain,
            &dir,
        ) {
            assert!(matches!(outcome, Outcome::Agree), "{name}: {outcome:?}");
        }
    });
}
