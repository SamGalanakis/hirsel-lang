//! Private cursor protocols of hirsel-lower's lowering, checked on real source.

#[test]
fn cursor_preparation_lifetimes_close_on_exhaustion_break_continue_and_return() {
    for ending in ["", "break", "continue", "return"] {
        let source = format!(
            "fn example(s:String) {{ for x in string.scalars(s) {{ let _:char = x\n {ending} }} }}"
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

#[test]
fn cursor_retirement_after_initialization_cleans_the_new_owner_on_failure() {
    use hirsel_engine::DefinitionQueries;
    for initialize in [
        "for item in row.children {}",
        "let items = row.children\n let _ = items",
    ] {
        let source = format!(
            "type Row {{name:String,children:List[String]}}\n\
             fn example(xs:Row) {{\n\
               let row = xs\n\
               for c in string.scalars(row.name) {{}}\n\
               {initialize}\n\
             }}"
        );
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine.sources.add("cursor-cleanup.hir", source).unwrap();
        let diagnostics = hirsel_engine::check_snapshot(&mut engine, file, &context)
            .unwrap_or_else(|error| panic!("{initialize}: {error:?}"));
        assert!(diagnostics.is_empty(), "{initialize}: {diagnostics:?}");
        let path = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: context.package,
            modules: vec![],
            name: "example".into(),
        });
        engine
            .lower_to_tir(&path)
            .unwrap_or_else(|error| panic!("{initialize}: {error:?}"));
    }
}
