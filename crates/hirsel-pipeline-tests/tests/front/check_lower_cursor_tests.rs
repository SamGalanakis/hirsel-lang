//! Private cursor protocols of hirsel-lower's lowering, checked on real source.

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
