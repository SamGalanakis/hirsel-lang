#[path = "../support/canonical_tir.rs"]
mod canonical_tir;
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::*;
#[test]
fn comparison_preserves_qualifiers_and_ignores_equal_row_sharing() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("canonical.hir", "fn t123(a:u8)->u8{a}".into())
            .unwrap();
        let parsed = engine.parse_file(file, &default_context()).unwrap();
        let path = &parsed.value.items[0].item.definition;
        let original = engine.lower_to_tir(path).unwrap().value;
        let mut duplicate = (*original).clone();
        let id = duplicate.bodies[0].locals[0].ty;
        let row = engine.types().get(id).unwrap().clone();
        let second = engine.types().intern(row.clone());
        duplicate.bodies[0].locals[0].ty = second;
        duplicate.types = hirsel_tir::definition_types(engine.types(), &duplicate);
        assert_eq!(
            canonical_tir::definition(engine.types(), &original),
            canonical_tir::definition(engine.types(), &duplicate)
        );
        let mut changed = row;
        changed.modes.multiplicity = Multiplicity::Once;
        let third = engine.types().intern(changed);
        duplicate.bodies[0].locals[0].ty = third;
        duplicate.types = hirsel_tir::definition_types(engine.types(), &duplicate);
        assert_ne!(
            canonical_tir::definition(engine.types(), &original),
            canonical_tir::definition(engine.types(), &duplicate)
        );
        let one = engine.types().intern_graph(1, |ids| {
            let mut row = engine.types().get(id).unwrap().clone();
            row.kind = TypeKind::Box(ids[0]);
            vec![row]
        });
        let two = engine.types().intern_graph(2, |ids| {
            ids.iter()
                .map(|_| {
                    let mut row = engine.types().get(id).unwrap().clone();
                    row.kind = TypeKind::Box(ids[1]);
                    row
                })
                .collect()
        });
        assert_eq!(
            canonical_tir::canonical_text(
                engine.types(),
                &one,
                "definition t123\nbody result t0\n"
            ),
            canonical_tir::canonical_text(
                engine.types(),
                &two,
                "definition t123\nbody result t1\n"
            )
        );
    });
}
