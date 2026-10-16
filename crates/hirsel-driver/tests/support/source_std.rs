// Tests select the same ordinary package graph as applications.
pub fn engine() -> hirsel_engine::QueryEngine {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/hirsel-driver/tests/fixtures/std_source/main.hir");
    let mut program = hirsel_driver::load_program_source(&path, Some("")).unwrap();
    // Publish the ordinary dependency check before discarding the package
    // coordinator. Later lowerings publish through the engine's cache handle.
    let output = program.check().unwrap();
    assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    program.engine
}
