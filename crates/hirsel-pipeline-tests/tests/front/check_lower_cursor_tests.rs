//! Private cursor protocols of hirsel-lower's lowering, checked on real source.

#[test]
fn cursor_preparation_lifetimes_close_on_exhaustion_break_continue_and_return() {
    for ending in ["", "break", "continue", "return"] {
        let source = format!(
            "fn example(s:String) {{ for x in string.scalars(s) {{ let _:u32 = x\n {ending} }} }}"
        );
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = engine
            .sources
            .add("cursor-preparation.hir", source)
            .unwrap();
        let diagnostics =
            hirsel_engine::check_snapshot(&mut engine, file, &hirsel_engine::default_context())
                .unwrap_or_else(|error| panic!("{ending:?}: {error:?}"));
        assert!(diagnostics.is_empty(), "{ending:?}: {diagnostics:?}");
    }
}

#[test]
fn borrowed_cursor_bindings_and_discards_do_not_leave_live_staging_slots() {
    for ending in ["", "break", "continue", "return"] {
        let source = format!(
            "fn example(xs:std@1/unicode.GraphemeView) {{ for x in xs {{ let alias = x\n let _ = alias\n {ending} }} }}"
        );
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../hirsel-driver/tests/fixtures/std_source/main.hir");
        let mut engine = hirsel_driver::load_program_source(&path, Some(""))
            .unwrap()
            .engine;
        let file = engine.sources.add("borrowed-cursor.hir", source).unwrap();
        let diagnostics =
            hirsel_engine::check_snapshot(&mut engine, file, &hirsel_engine::default_context())
                .unwrap_or_else(|error| panic!("{ending:?}: {error:?}"));
        assert!(diagnostics.is_empty(), "{ending:?}: {diagnostics:?}");
    }
}

#[test]
fn borrowed_aggregate_aliases_keep_the_permission_for_sibling_identity_proofs() {
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let file = engine.sources.add("borrowed-alias.hir", String::from(
        "type Log {dir:Dir,file:File of .dir}\nfn inspect(d:Dir,f:File of d) {}\nfn probe(log:Log) {let alias=log; inspect(alias.dir,alias.file)}",
    )).unwrap();
    let diagnostics =
        hirsel_engine::check_snapshot(&mut engine, file, &hirsel_engine::default_context())
            .unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}
