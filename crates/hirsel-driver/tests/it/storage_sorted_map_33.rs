//! Ordered storage recipes execute actual comparator code and retain stored producers.
use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn sealed_sorted_map_lookup_has_complete_provenance() {
    with_compiler_stack(|| {
        let source = r#"// rule: MEM-4
// expect: run

fn fits(cap: u64) -> u64 {
  let outcome = arena.bounded(
    cap,
    fn(a: Arena) {
      let mut root: SortedMap[u64, Bytes] of u64.order in a = sorted_map.empty(
        u64.order
      )
      sorted_map.insert(u64.order, mut root, 7u64, copy_to(heap, b"abc"))
      arena.seal(take a, take root)
    }
  )
  match take outcome {
    Ok(r) => {
      let mut owned = take r
      region.with(
        mut owned,
        fn(view: SortedMap[u64, Bytes] of u64.order) -> u64 {
          match sorted_map.get(u64.order, view, 7u64) {
            Some(value) => bytes.len(value),
            None() => 0u64,
          }
        }
      )
    },
    Error(e) => 0u64,
  }
}

pub fn main() { let got = fits(19u64) }
"#;
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("storage-sorted-map.hir", source.to_owned())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(checked.completion, CheckCompletion::Complete);
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        assert!(!input.storage_profiles.is_empty());
        let validated = engine.import_native_input(input.clone()).unwrap();
        assert!(!validated.storage_proof_reads().bodies.is_empty());
        for mutation in 0..3 {
            let mut input = input.clone();
            let (_, signature) = input
                .signatures
                .iter_mut()
                .find(|(path, _)| path.is_core("sorted_map.get"))
                .unwrap();
            let hirsel_types::SignatureKind::Function(function) =
                &mut std::sync::Arc::make_mut(signature).kind
            else {
                panic!("sorted_map.get declaration")
            };
            match mutation {
                0 => function.parameters[1].passing = hirsel_syntax::PassingMode::Take,
                1 => function.allocation = hirsel_types::Allocation::NoAlloc,
                2 => function.parameters[0].passing = hirsel_syntax::PassingMode::Mut,
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
    });
}

#[test]
fn sealed_sorted_map_custom_comparison_preserves_read_effects() {
    with_compiler_stack(|| {
        for allocating in [false, true] {
            let effect = if allocating { "let _ = unique a;" } else { "" };
            let source = format!(
                r#"
fn compare(a: Bytes, b: Bytes) -> i32 {{ {effect} 0i32 }}
const BY: Order[Bytes] = Order(compare: compare)
pub fn main() {{
  let mut sealed = arena.with(fn(a: Arena) {{
    let mut root: SortedMap[Bytes, u64] of BY in a = sorted_map.empty(BY)
    sorted_map.insert(BY, mut root, unique b"key", 7u64)
    arena.seal(take a, take root)
  }})
  let observed = region.with(mut sealed, fn(view: SortedMap[Bytes, u64] of BY) -> u64 {{
    match sorted_map.get(BY, view, b"query") {{ Some(value) => value, None() => 0u64 }}
  }})
}}
"#
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("storage-custom-order.hir", source)
                .unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(checked.completion, CheckCompletion::Complete);
            if allocating {
                assert!(
                    checked
                        .diagnostics
                        .iter()
                        .any(|d| d.rule.to_string() == "MODE-1"),
                    "{:?}",
                    checked.diagnostics
                );
            } else {
                assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
                let input = engine.export_native_input().unwrap();
                assert!(!input.storage_profiles.is_empty());
                engine.import_native_input(input).unwrap();
            }
        }
    });
}

#[test]
fn sealed_sorted_map_removal_retains_possible_stored_values() {
    with_compiler_stack(|| {
        let source = r#"
pub fn main() {
  let mut sealed = arena.with(fn(a: Arena) {
    let mut root: unique SortedMap[u64, Bytes] of u64.order in a = sorted_map.empty(u64.order)
    sorted_map.insert(u64.order, mut root, 1u64, unique b"x")
    sorted_map.insert(u64.order, mut root, 2u64, unique b"abc")
    let _ = sorted_map.remove(u64.order, mut root, 1u64)
    arena.seal(take a, take root)
  })
  let size = region.with(mut sealed, fn(view: SortedMap[u64, Bytes] of u64.order) -> u64 {
    match sorted_map.get(u64.order, view, 2u64) { Some(value) => bytes.len(value), None() => 0u64 }
  })
}
"#;
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("storage-sorted-removal.hir", source.to_owned())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(checked.completion, CheckCompletion::Complete);
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        assert!(!input.storage_profiles.is_empty());
        engine.import_native_input(input.clone()).unwrap();
        for mutation in 0..3 {
            let mut input = input.clone();
            let (_, signature) = input
                .signatures
                .iter_mut()
                .find(|(path, _)| path.is_core("sorted_map.remove"))
                .unwrap();
            let hirsel_types::SignatureKind::Function(function) =
                &mut std::sync::Arc::make_mut(signature).kind
            else {
                panic!("sorted_map.remove declaration")
            };
            match mutation {
                0 => function.parameters[1].passing = hirsel_syntax::PassingMode::Borrow,
                1 => function.allocation = hirsel_types::Allocation::NoAlloc,
                2 => {
                    let mut row = input.types.row(function.result).clone();
                    row.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
                    function.result = input.types.intern(row);
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
    });
}
