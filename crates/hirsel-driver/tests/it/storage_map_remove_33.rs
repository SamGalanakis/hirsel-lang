//! A reviewed map removal preserves possible stored-value producers for a later sealed read.
use hirsel_driver::*;
use hirsel_engine::*;

fn check(remove: bool) {
    with_compiler_stack(|| {
        let removal = if remove {
            "let _ = map.remove(mut root, 1u64)"
        } else {
            ""
        };
        let source = format!(
            "pub fn main() {{
              let mut sealed = arena.with(fn(a: Arena) -> Region[Map[u64, Bytes]] {{
                let mut root: unique Map[u64, Bytes] in a = map.empty()
                map.insert(mut root, 1u64, unique b\"x\")
                map.insert(mut root, 2u64, unique b\"abc\")
                {removal}
                arena.seal(take a, take root)
              }})
              let size = region.with(mut sealed, fn(view: Map[u64, Bytes]) -> u64 {{
                match map.get(view, 2u64) {{
                  Some(value) => bytes.len(value),
                  None() => 0u64,
                }}
              }})
            }}"
        );
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("storage-map-remove.hir", source)
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(checked.completion, CheckCompletion::Complete);
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        assert!(!input.storage_profiles.is_empty());
        let validated = engine.import_native_input(input.clone()).unwrap();
        assert!(!validated.storage_proof_reads().bodies.is_empty());
        if remove {
            for mutation in 0..3 {
                let mut input = input.clone();
                let (_, signature) = input
                    .signatures
                    .iter_mut()
                    .find(|(path, _)| path.is_core("map.remove"))
                    .unwrap();
                let hirsel_types::SignatureKind::Function(function) =
                    &mut std::sync::Arc::make_mut(signature).kind
                else {
                    panic!("map.remove declaration")
                };
                match mutation {
                    0 => function.parameters[0].passing = hirsel_syntax::PassingMode::Borrow,
                    1 => function.allocation = hirsel_types::Allocation::NoAlloc,
                    2 => {
                        let mut result = input.types.row(function.result).clone();
                        result.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
                        function.result = input.types.intern(result);
                    }
                    _ => unreachable!(),
                }
                let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
                signatures.extend_signatures(input.signatures.values().map(|row| (**row).clone()));
                let checked = hirsel_validate::storage::validate_program(
                    &input.types,
                    &input.definitions.values().cloned().collect::<Vec<_>>(),
                    &signatures,
                    &input.constants,
                    &input.instance_bodies,
                    &input.storage_profiles,
                );
                assert!(
                    checked.violations.iter().any(|violation| matches!(
                        violation,
                        hirsel_validate::storage::StorageViolation::Malformed(_)
                            | hirsel_validate::storage::StorageViolation::ExecutableRead(_)
                    )),
                    "mutation {mutation}: {checked:?}"
                );
            }
        }
    });
}

#[test]
fn sealed_map_lookup_without_removal_has_complete_provenance() {
    check(false);
}

#[test]
fn sealed_map_lookup_after_removal_has_complete_provenance() {
    check(true);
}
