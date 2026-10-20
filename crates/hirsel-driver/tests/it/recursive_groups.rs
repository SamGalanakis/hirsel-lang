use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::ContentHash;
fn path(name: &str) -> DefPath {
    let mut p = entry_path(&default_context());
    p.0.name = name.into();
    p
}
const GROUP: &str = "type Tree = Leaf | Node(box Entry)\n\
                     type Entry { left: Tree, key: u64, right: Tree }\n";
/// Both members' signatures, the members queried in `order` on a fresh engine.
fn signatures(
    engine: &mut QueryEngine,
    source: &str,
    order: [&str; 2],
) -> Vec<hirsel_types::SignatureRef> {
    let file = engine.sources.add("group.hir", source.into()).unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    for name in order {
        engine
            .signature(hirsel_syntax::DefId::intern(&path(name)))
            .unwrap();
    }
    ["Tree", "Entry"]
        .map(|name| {
            engine
                .signature(hirsel_syntax::DefId::intern(&path(name)))
                .unwrap()
        })
        .to_vec()
}
/// What a signature means: its fingerprint covers its content and group key.
fn identity(signatures: &[hirsel_types::SignatureRef]) -> Vec<ContentHash> {
    signatures.iter().map(|s| s.fingerprint).collect()
}
fn fresh() -> QueryEngine {
    QueryEngine::new(default_environment(), compiler_adapters())
}
#[test]
fn recursive_type_groups_normalize_alike_in_any_order_and_move_together() {
    with_compiler_stack(|| {
        let first = signatures(&mut fresh(), GROUP, ["Tree", "Entry"]);
        let second = signatures(&mut fresh(), GROUP, ["Entry", "Tree"]);
        assert_eq!(identity(&first), identity(&second));
        let keys = first.iter().map(|s| s.metadata.recursive_group);
        let keys: Vec<_> = keys.collect();
        assert!(keys.iter().all(Option::is_some), "{keys:?}");
        assert_ne!(keys[0], keys[1]);
        // Neither member lists the other: the group key stands for it.
        for (signature, other) in first.iter().zip(["Entry", "Tree"]) {
            let mut engine = fresh();
            signatures(&mut engine, GROUP, ["Tree", "Entry"]);
            let covered = engine
                .signature_coverage(signature.definition.path())
                .unwrap();
            assert!(covered.iter().all(|d| d.definition != path(other)));
        }
        // Editing one member moves both, also through a session's caches.
        let edited = GROUP.replace("key: u64", "key: u32");
        let mut engine = fresh();
        signatures(&mut engine, GROUP, ["Tree", "Entry"]);
        let moved = signatures(&mut engine, &edited, ["Tree", "Entry"]);
        let cold = signatures(&mut fresh(), &edited, ["Entry", "Tree"]);
        assert_eq!(identity(&moved), identity(&cold));
        for (before, after) in first.iter().zip(&moved) {
            assert_ne!(
                before.fingerprint, after.fingerprint,
                "{}",
                before.definition
            );
        }
        // A type that only names itself is not a group.
        let lone = signatures(
            &mut fresh(),
            "type Tree = Leaf | Node(box Tree)\ntype Entry { key: u64 }\n",
            ["Tree", "Entry"],
        );
        assert!(lone.iter().all(|s| s.metadata.recursive_group.is_none()));
    });
}
