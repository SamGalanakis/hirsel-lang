//! A reviewed integer comparison is a bodyless primitive, not a missing source body.
use hirsel_driver::*;
use hirsel_engine::*;

fn check(primitive: bool, kind: &str) {
    with_compiler_stack(|| {
        let call = if primitive {
            format!("by.compare(root, 0{kind})")
        } else {
            "compare(root, 0u64)".to_owned()
        };
        let source = format!(
            "fn compare(a: u64, b: u64) -> i32 {{
               if a < b {{ -1i32 }} else {{ if a > b {{ 1i32 }} else {{ 0i32 }} }}
             }}
             pub fn main() {{
               let by = {kind}.order
               let mut sealed = arena.with(fn(a: Arena) -> Region[{kind}] {{
                 let root = 7{kind}
                 arena.seal(take a, take root)
               }})
               let rank = region.with(mut sealed, fn(root: {kind}) -> i32 {{ {call} }})
             }}"
        );
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("storage-core-callable.hir", source)
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(checked.completion, CheckCompletion::Complete);
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        assert!(!input.storage_profiles.is_empty());
        let validated = engine.import_native_input(input.clone()).unwrap();
        assert!(!validated.storage_proof_reads().bodies.is_empty());
        if primitive && kind == "u64" {
            for mutation in 0..3 {
                let mut input = input.clone();
                let (_, signature) = input
                    .signatures
                    .iter_mut()
                    .find(|(path, _)| path.is_core("$intrinsic.OrderCompare_u64"))
                    .unwrap();
                let signature = std::sync::Arc::make_mut(signature);
                let hirsel_types::SignatureKind::Function(function) = &mut signature.kind else {
                    panic!("integer comparator")
                };
                match mutation {
                    0 => function.allocation = hirsel_types::Allocation::MayAllocate,
                    1 => function.parameters[0].passing = hirsel_syntax::PassingMode::Take,
                    2 => signature.metadata.visibility = hirsel_syntax::Visibility::Public,
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
                        hirsel_validate::storage::StorageViolation::ExecutableRead(_)
                    )),
                    "mutation {mutation}: {checked:?}"
                );
            }
        }
    });
}

#[test]
fn original_scalar_comparison_has_complete_read_provenance() {
    check(false, "u64");
}

#[test]
fn reviewed_integer_order_has_complete_read_provenance() {
    for kind in [
        "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128",
    ] {
        check(true, kind);
    }
}
