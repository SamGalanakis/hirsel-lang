//! Explicit owned selection follows the same admitted source on all engines.
#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn source_owned_string_and_byte_leaves_keep_their_original_lifetimes() {
    for literal in ["\"equal\"", "b\"equal\""] {
        super::ownership_owned_call_49::source_owned_leaves(
            &format!(
                "pub fn main()->Result[(),()] {{ let first={literal}\nlet second={literal}\nlet moved=take first\nOk(()) }}"
            ),
            2,
        );
    }
}

#[test]
fn source_owned_and_resource_selections_keep_distinct_products_in_the_same_session() {
    use hirsel_driver::{COMPILER, native, requests::*, resident::ObservedSession};
    use hirsel_engine::{CompileConfig, runtime::ProcessBudget, store::MemStore};
    use hirsel_lir::ownership_trace::{Kind, Selection};
    use std::{collections::BTreeSet, sync::Arc};

    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        std::fs::write(&source, "pub fn main()->Result[(),()] { let first=\"equal\"\nlet second=b\"equal\"\nlet moved=take first\nOk(()) }").unwrap();
        let config = CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::ONE,
            store: Arc::new(MemStore::default()),
            hold_bytes: 0,
            ..Default::default()
        };
        let budget = ProcessBudget::new(1 << 30);
        let compiler = COMPILER.with_config(config.clone()).with_owned_engines();
        let selector = CompilerRequests::new(
            compiler.clone(),
            config.clone(),
            budget.clone(),
            RequestLimits {
                runtime_bytes: 256 << 20,
                snapshot_bytes: 64 << 20,
                transport_bytes: 32 << 20,
            },
        );
        let limits = selector.input_limits();
        let mut host = ObservedSession::new(source, compiler, config, selector, limits, 3);
        let mut selections = Vec::new();
        for selection in [Selection::Resources, Selection::Owned, Selection::Resources] {
            let output = host
                .request(CompileOperation::NativeOwnership(OwnershipRequest {
                    selection,
                    definition: None,
                    target: native::TargetSpec::x86_64_unknown_linux_gnu(),
                    validation: native::Validation::EveryStage,
                    launch: Default::default(),
                    fuel: false,
                }))
                .unwrap();
            let CompileResult::Ownership(execution) = &output.result else {
                panic!("missing selected ownership output");
            };
            let mut creates = 0;
            for product in execution.units() {
                execution
                    .with_unit(*product, |unit| {
                        assert_eq!(unit.initial.selection, selection);
                        assert_eq!(unit.plan.selection, selection);
                        creates += unit
                            .initial
                            .observations
                            .iter()
                            .filter(|row| row.marker.kind == Kind::Create)
                            .count();
                        Ok(())
                    })
                    .unwrap();
            }
            assert_eq!(creates, if selection == Selection::Owned { 2 } else { 0 });
            selections.push(execution.clone());
        }
        assert_eq!(selections[0].product, selections[2].product);
        assert_eq!(selections[0].units(), selections[2].units());
        assert_ne!(selections[0].product.address, selections[1].product.address);
        let resource_addresses = selections[0]
            .units()
            .iter()
            .map(|product| product.address)
            .collect::<BTreeSet<_>>();
        assert!(
            selections[1]
                .units()
                .iter()
                .all(|product| !resource_addresses.contains(&product.address))
        );
        for (owner, other) in [(0, 1), (1, 0)] {
            for product in selections[other].units() {
                assert!(selections[owner].with_unit(*product, |_| Ok(())).is_err());
            }
        }
        drop(host);
        assert!(budget.reserved() > 0);
        drop(selections);
        assert_eq!(budget.reserved(), 0);
    });
}
