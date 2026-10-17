use hirsel_driver::*;
use hirsel_engine::*;
fn path(name: &str) -> DefPath {
    let mut p = entry_path(&default_context());
    p.0.name = name.into();
    p
}
fn check(engine: &mut QueryEngine, source: &str) -> CheckOutput {
    let file = engine
        .sources
        .add("invalidation.hir", source.into())
        .unwrap();
    check_progress(engine, file, &default_context()).unwrap()
}
fn counts(engine: &QueryEngine, name: &str) -> [usize; 5] {
    [
        QueryKind::Resolve,
        QueryKind::Signature,
        QueryKind::Check,
        QueryKind::Modes,
        QueryKind::Lower,
    ]
    .map(|kind| engine.recompute_count(&path(name), kind))
}
#[test]
fn unrelated_definition_membership_does_not_recompute_semantic_queries() {
    with_compiler_stack(|| {
        let source = "fn a() -> u8 { 1u8 }\nfn b() -> u8 { a() }\nfn c() {}\n";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        assert!(check(&mut engine, source).diagnostics.is_empty());
        let before = [counts(&engine, "a"), counts(&engine, "b")];
        for extra in ["fn zz_bad() { zz_missing }\n", "fn zz_other() {}\n", ""] {
            check(&mut engine, &format!("{source}{extra}"));
            for name in ["a", "b"] {
                engine.lower_to_tir(&path(name)).unwrap();
            }
            assert_eq!(
                [counts(&engine, "a"), counts(&engine, "b")],
                before,
                "{extra}"
            );
        }
    });
}
#[test]
fn local_reservation_reads_track_only_the_declaring_module() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        assert!(check(&mut engine, "fn f(x:u8) {}\n").diagnostics.is_empty());
        let before = counts(&engine, "f");
        let file = engine
            .sources
            .add("other.hir", "fn x() {}\n".into())
            .unwrap();
        let mut context = default_context();
        context.modules.push("other".into());
        engine.parse_file(file, &context).unwrap();
        engine.check_definition(&path("f")).unwrap();
        assert_eq!(
            counts(&engine, "f"),
            before,
            "a qualified-only value must not invalidate the local binding"
        );
        let file = engine
            .sources
            .add("same.hir", "fn x() {}\n".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let Err(QueryError::Diagnostics(ds)) = engine.check_definition(&path("f")) else {
            panic!("a same-module value must still reserve x")
        };
        assert_eq!(ds[0].rule.as_str(), "LANG-4");
    });
}

#[test]
fn changing_a_type_to_a_value_invalidates_namespace_reads() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        assert!(
            check(&mut engine, "type x { value:u8 }\nfn f(x:u8) {}\n")
                .diagnostics
                .is_empty()
        );
        let output = check(&mut engine, "fn x() {}\nfn f(x:u8) {}\n");
        assert_eq!(output.diagnostics[0].rule.as_str(), "LANG-4");
    });
}
#[test]
fn materializations_track_executed_bodies_without_invalidating_unrelated_lowerings() {
    with_compiler_stack(|| {
        let source = "fn helper() -> u32 { 3u32 }\nconst FIRST:u32 = comptime helper()\nconst SECOND:u32 = comptime (FIRST + 4)\nfn user() -> u32 { SECOND }\nfn other() -> u32 { 0u32 }\n";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        assert!(check(&mut engine, source).diagnostics.is_empty());
        let before = counts(&engine, "user");
        let value = engine.materialize_constant(&path("SECOND")).unwrap();
        for edited in [
            source.to_string(),
            source.replace("0u32", "1u32"),
            format!("{source}fn added() {{}}\n"),
        ] {
            assert!(check(&mut engine, &edited).diagnostics.is_empty());
            assert_eq!(counts(&engine, "user"), before);
            assert_eq!(engine.materialize_constant(&path("SECOND")).unwrap(), value);
        }
        let edited = source.replace("3u32", "5u32");
        assert!(check(&mut engine, &edited).diagnostics.is_empty());
        assert_ne!(engine.materialize_constant(&path("SECOND")).unwrap(), value);
        assert_eq!(counts(&engine, "user")[0..4], before[0..4]);
        assert_eq!(counts(&engine, "user")[4], before[4] + 1);
        let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
        check(&mut fresh, &edited);
        assert_eq!(
            hirsel_lower::print::definition(&engine.lower_to_tir(&path("user")).unwrap().value),
            hirsel_lower::print::definition(&fresh.lower_to_tir(&path("user")).unwrap().value)
        );
    });
}

#[test]
fn cached_materializations_validate_type_signatures_and_survive_owner_moves() {
    with_compiler_stack(|| {
        let original = "fn prefix() {}\ntype Word = u8\nconst VALUE:Word = comptime 3\nfn user() -> Word { VALUE }\n";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        assert!(check(&mut engine, original).diagnostics.is_empty());
        let value = engine.materialize_constant(&path("VALUE")).unwrap();
        for edited in [
            original.replace("type Word = u8", "type Word = u16"),
            original.replace("fn prefix() {}", "fn prefix() { let x:u8 = 1u8; }"),
        ] {
            assert!(check(&mut engine, &edited).diagnostics.is_empty());
            let actual = engine.materialize_constant(&path("VALUE")).unwrap();
            let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
            check(&mut fresh, &edited);
            assert_eq!(actual, fresh.materialize_constant(&path("VALUE")).unwrap());
        }
        check(
            &mut engine,
            &original.replace("const VALUE:Word = comptime 3\n", ""),
        );
        let restored = original.replace("fn prefix() {}", "fn prefix() { let x:u8 = 2u8; }");
        assert!(check(&mut engine, &restored).diagnostics.is_empty());
        assert_eq!(engine.materialize_constant(&path("VALUE")).unwrap(), value);
    });
}

#[test]
fn shadowed_root_values_do_not_enter_the_semantic_revision() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let root = default_context();
        let mut nested = root.clone();
        nested.modules.push("repl".into());
        let file = engine
            .sources
            .add("root-shadow.hir", "pub fn input() -> u64 { 1u64 }".into())
            .unwrap();
        engine.parse_file(file, &root).unwrap();
        let file = engine
            .sources
            .add(
                "nested-shadow.hir",
                "pub fn main() -> Result[(), ()] { let input: u64 = 0; Ok(()) }".into(),
            )
            .unwrap();
        engine.parse_file(file, &nested).unwrap();
        let entry = entry_path(&nested);
        let first = engine.semantic_program_revision(&entry).unwrap();
        let file = engine
            .sources
            .add("root-shadow.hir", "pub fn input() -> u64 { 2u64 }".into())
            .unwrap();
        engine.parse_file(file, &root).unwrap();
        assert_eq!(first, engine.semantic_program_revision(&entry).unwrap());
    });
}
