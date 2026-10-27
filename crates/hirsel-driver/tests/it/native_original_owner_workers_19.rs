//! Retained-owner source vector from the removed-controller migration inventory.
use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::{
    store::{MemStore, ResultStore, VerifyStore},
    *,
};
use hirsel_intern::jobs::Jobs;
use hirsel_lir::ir::UnitOrigin;
use std::sync::Arc;

#[test]
fn original_retained_owners_match_at_one_two_eight_under_both_validation_policies() {
    let run = || {
        with_compiler_stack(|| {
            let mut transcript = String::new();
            let source = "fn step(x:u64)->u64{x+1u64}\nfn main()->u64 {let mut xs:List[u64]=[];list.push(mut xs,7u64);step(xs[0u64])}\n";
            for (backend, opt) in [
                (native::Backend::Cranelift, native::OptLevel::None),
                (native::Backend::Llvm, native::OptLevel::None),
                (native::Backend::Llvm, native::OptLevel::Default),
            ] {
                let mut baseline = None;
                for workers in [1, 2, 8] {
                    for validation in [native::Validation::Ready, native::Validation::EveryStage] {
                        let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
                        let mut engine =
                            QueryEngine::new(default_environment(), compiler_adapters())
                                .with_result_store(store.clone());
                        let file = engine
                            .sources
                            .add("owner-workers.hir", source.into())
                            .unwrap();
                        let checked =
                            check_progress(&mut engine, file, &default_context()).unwrap();
                        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
                        let input = engine.export_native_input().unwrap();
                        let _validated = engine.import_native_input(input).unwrap();
                        let mut options =
                            native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
                        options.validation = validation;
                        let actual = crate::native_object_products_19::compile_front(
                            crate::native_front_fixture::products(&mut engine, &[]).unwrap(),
                            store,
                            &[entry_path(&default_context())],
                            backend,
                            opt,
                            options,
                            false,
                            Jobs::new(workers),
                            if validation == native::Validation::EveryStage {
                                VerifyStore::Recompute
                            } else {
                                VerifyStore::Off
                            },
                            |_, _, _| (),
                            |_, _| (),
                        )
                        .0;
                        assert_eq!(actual.objects.len(), actual.summaries.len());
                        assert!(
                            actual
                                .summaries
                                .iter()
                                .any(|summary| summary.origin == UnitOrigin::Library)
                        );
                        for summary in &actual.summaries {
                            let symbol = hirsel_lir::emission::symbol_name(
                                &summary.definition,
                                Some(&summary.instance),
                            );
                            assert_eq!(
                                actual
                                    .objects
                                    .iter()
                                    .filter(|object| object
                                        .exports
                                        .iter()
                                        .any(|export| export.name == symbol))
                                    .count(),
                                1,
                                "{backend:?} {opt:?} {workers} {validation:?} {symbol}"
                            );
                        }
                        let actual = (actual.summaries, actual.objects);
                        if let Some(expected) = &baseline {
                            assert_eq!(
                                &actual, expected,
                                "{backend:?} {opt:?} {workers} {validation:?}"
                            );
                        } else {
                            baseline = Some(actual);
                        }
                    }
                }
                transcript.push_str(&format!("{backend:?} {opt:?} {:?}", baseline.unwrap()));
            }
            transcript
        })
    };
    if let Some(outputs) = hirsel_intern::under_salts(
        "native_original_owner_workers_19::original_retained_owners_match_at_one_two_eight_under_both_validation_policies",
        &[1, 2, 0x5eed],
        run,
    ) {
        assert_eq!(outputs.len(), 3);
        assert!(
            outputs.windows(2).all(|pair| pair[0] == pair[1]),
            "salt changed complete summaries or objects"
        );
    }
}
