//! Signature normalization of real source, from hirsel-signature's unit tests.
use hirsel_types::*;

#[test]
fn flat_storage_proves_actual_nominal_and_generic_fields() {
    use hirsel_driver::compiler_adapters;
    use hirsel_engine::{DefinitionQueries, QueryEngine, default_context, default_environment};
    for (prefix, payload, accepted) in [
        ("", "Dir", false),
        ("", "Mutex[u32]", false),
        ("type W { value:SlotId[u32] }", "W", false),
        ("type W[T] { value:T }", "W[W[u32]]", true),
        ("pub opaque type W:plain + flat uses {}", "(W, W)", true),
        ("", "List[Dir]", false),
        ("type W[T] { value:T }", "W[Dir]", false),
        ("type W { value:Dir }", "W", false),
        ("resource type W { value:u32 }", "W", false),
        ("pub opaque type W:plain uses {Dir}", "W", false),
        ("type W[T] { value:T }", "W[u32]", true),
        ("", "List[u32]", true),
        ("", "String", true),
        ("flat type W { value:String }", "W", true),
    ] {
        let source = format!("{prefix}\nflat type R {{ value:{payload} }}");
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("flat-owner.hir", source.clone())
            .unwrap();
        let parsed = engine.parse_file(file, &default_context()).unwrap();
        let result = engine.signature(&parsed.value.tree.items.last().unwrap().definition);
        if accepted {
            assert!(result.is_ok(), "{source}: {result:?}");
        } else {
            assert!(
                matches!(result, Err(QueryError::Diagnostics(ref ds)) if ds[0].rule.as_str() == "MEM-12"),
                "{source}: {result:?}"
            );
        }
    }
}

#[test]
fn represented_seals_prove_ceilings_and_exported_bounds() {
    use hirsel_driver::compiler_adapters;
    use hirsel_engine::{
        QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
    };
    with_compiler_stack(|| {
        for (declaration, expected) in [
            (
                "@capability(global) pub opaque type Seal: plain uses {Dir} { d: Dir }",
                None,
            ),
            (
                "@capability(global) pub opaque type Seal: plain uses {} { d: Dir }",
                Some("CAP-5"),
            ),
            (
                "@capability(global) pub opaque type Seal: plain { d: Dir }",
                Some("CAP-5"),
            ),
            (
                "@capability(global) pub opaque type Seal: () uses {Dir} { d: Dir, key: String }",
                None,
            ),
            (
                "@capability(global) pub opaque type Seal: plain uses {Dir} { d: Dir, key: String }",
                Some("GEN-1"),
            ),
            (
                "@capability(global) pub opaque type Seal: sendable uses {} { f: fn() -> () uses {} }",
                Some("MODE-4"),
            ),
            (
                "@capability(global) pub opaque type Seal: noalloc_drop uses {} { f: fn() -> () uses {} }",
                Some("GEN-1"),
            ),
            (
                "@capability(global) pub opaque type Seal: copyable uses {} { f: fn() -> () uses {} }",
                None,
            ),
            (
                "@capability(global) pub opaque type Seal: () uses {Dir} = Dir",
                None,
            ),
            (
                "@capability(global) pub opaque type Seal: plain uses {Dir} = Empty() | Full(Dir)",
                None,
            ),
            (
                "@capability(global) pub opaque type Seal: inert uses {} {}",
                Some("CAP-2"),
            ),
            (
                "@capability(global) pub opaque type Seal: data uses {} {}",
                Some("GEN-1"),
            ),
            (
                "@capability(global) pub opaque type Seal: key uses {} {}",
                Some("GEN-1"),
            ),
            (
                "@capability(global) pub opaque type Seal: flat uses {} {}",
                Some("GEN-1"),
            ),
            (
                "@capability(global) pub opaque type Seal: serializable uses {} {}",
                Some("SER-1"),
            ),
            (
                "@capability(global) pub opaque resource type Seal: () uses {Dir} { d: Dir }",
                None,
            ),
            (
                "@capability(global) pub opaque resource type Seal: plain uses {Dir} { d: Dir }",
                Some("GEN-1"),
            ),
            (
                "@capability(global) pub opaque resource type Seal: copyable uses {Dir} { d: Dir }",
                Some("GEN-1"),
            ),
            (
                "@capability(global) pub opaque resource type Seal: noalloc_drop uses {Dir} { d: Dir } @noalloc drop(self) {}",
                None,
            ),
            (
                "@capability(global) pub opaque resource type Seal: noalloc_drop uses {Dir} { d: Dir } drop(self) {}",
                Some("GEN-1"),
            ),
            (
                "@capability(global) pub opaque type Seal[T: plain + inert]: () uses {} { value: T }",
                None,
            ),
            (
                "@capability(global) pub opaque type Seal[T]: () uses {} { value: T }",
                Some("CAP-5"),
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("seal.hir", declaration.into()).unwrap();
            let result = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(
                result.diagnostics.first().map(|d| d.rule.as_str()),
                expected,
                "{declaration}: {:?}",
                result.diagnostics
            );
        }
    });
}
