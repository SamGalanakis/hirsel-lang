use hirsel_driver::native::{
    Options, Validation,
    queries::{
        InstanceAddress,
        ready::{EMISSION_CONTEXT_DOMAIN, ReadyMiss, ReadyQueries, ReadySourceFacts},
        seal_demand::{Input, Plan},
    },
};
use hirsel_engine::{
    runtime::ProductRef,
    store::{Action, Address, Domain, MemStore, ResultStore},
};
use hirsel_lir::operation::{NativeType, Primitive};
use hirsel_rc::seal_demand::equations::LocalDemandEquations;
use hirsel_syntax::ContentHash;
use hirsel_types::TypeKind;
use std::sync::Arc;

#[test]
fn original_scalar_selected_guard_reaches_every_production_ready_unit() {
    let budget = hirsel_engine::runtime::ProcessBudget::new(64 << 20);
    let store: Arc<dyn ResultStore> = Arc::new(hirsel_engine::store::RequestStore::new(
        Arc::new(MemStore::default()),
        budget.reserve(64 << 20).unwrap(),
    ));
    let mut queries = ReadyQueries::new(store.clone());
    let context = store
        .put_content(EMISSION_CONTEXT_DOMAIN, b"original guarded Ready context")
        .unwrap();
    let mut selection = None;
    let mut guards = 0;
    let mut units = 0;
    let _ = super::sealed_region_h_source_33::original_with_ready_observer(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-scalar-zero-run.hir"),
        TypeKind::Primitive(hirsel_types::Primitive::U64),
        NativeType::Primitive(Primitive::U64),
        |originals, entry, placed, inputs, instances, origin, key| {
            if selection.is_none() {
                let mut locals = originals
                    .iter()
                    .map(LocalDemandEquations::extract)
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                locals.sort_by(|a, b| a.owner().cmp(b.owner()));
                let domain = Domain {
                    kind: 0x4e00_0001,
                    schema: 1,
                };
                let rows = locals
                    .into_iter()
                    .map(|equations| {
                        let bytes = serde_json::to_vec(&equations).unwrap();
                        let record = store.put_content(domain, &bytes).unwrap();
                        let product = crate::imported_fixture(
                            store.as_ref(),
                            Address(ContentHash::of_bytes(&bytes)),
                            Action(ContentHash([1; 32])),
                            record,
                        );
                        store.associate_product(product.address, product.action, record);
                        Input {
                            query: hirsel_engine::runtime::QueryId {
                                kind: 0x4e00_0001,
                                address: product.address,
                            },
                            product,
                            equations,
                        }
                    })
                    .collect();
                let root = (
                    originals[entry].header.definition.clone(),
                    originals[entry].header.instance,
                );
                selection = Some(
                    Plan::new(rows, &[root], inputs.target.digest(), ContentHash([1; 32]))
                        .unwrap()
                        .execute(store.as_ref())
                        .unwrap(),
                );
            }
            let original = originals
                .iter()
                .find(|unit| {
                    unit.header.definition == placed.header.definition
                        && unit.header.instance == placed.header.instance
                })
                .unwrap();
            let guard = selection
                .as_ref()
                .unwrap()
                .unit(&(original.header.definition.clone(), original.header.instance))
                .unwrap();
            let source = ReadySourceFacts::for_instance(instances, origin);
            let finalizers = hirsel_rc::cleanup::Finalizers::planned(instances, inputs.layouts);
            let mut options = Options::new(inputs.target.clone());
            options.validation = Validation::EveryStage;
            let key = InstanceAddress {
                instance: key.clone(),
                target_abi: inputs.target.digest(),
                policy: ContentHash([1; 32]),
            };
            let action = Action(ContentHash::of_parts(
                "original-guarded-ready",
                [key.instance.encoding()],
            ));
            let miss = || ReadyMiss {
                value: original.clone(),
                options: &options,
                source: &source,
                guard: Some(guard),
                finalizers: &finalizers,
                inputs,
                additional_abis: &[],
                statics: &[],
                emission_context: context,
                debug: false,
                optimizer_reads: vec![],
            };
            let (ready, _) = queries.publish_from_value(&key, action, miss()).unwrap();
            let expected = placed
                .tables
                .regions
                .iter()
                .filter(|r| r.seal_demand)
                .count();
            assert_eq!(
                ready
                    .unit()
                    .tables
                    .regions
                    .iter()
                    .filter(|r| r.seal_demand)
                    .count(),
                expected
            );
            guards += expected;
            units += 1;
        },
    );
    assert_eq!(guards, 1);
    assert!(units > 1);
}

#[test]
fn original_scalar_ready_refuses_foreign_or_malformed_guard_inputs_before_lowering() {
    let budget = hirsel_engine::runtime::ProcessBudget::new(64 << 20);
    let store: Arc<dyn ResultStore> = Arc::new(hirsel_engine::store::RequestStore::new(
        Arc::new(MemStore::default()),
        budget.reserve(64 << 20).unwrap(),
    ));
    let mut queries = ReadyQueries::new(store.clone());
    let context = store
        .put_content(EMISSION_CONTEXT_DOMAIN, b"guard refusal context")
        .unwrap();
    let mut checked = false;
    let _ = super::sealed_region_h_source_33::original_with_ready_observer(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-scalar-zero-run.hir"),
        TypeKind::Primitive(hirsel_types::Primitive::U64),
        NativeType::Primitive(Primitive::U64),
        |originals, entry, placed, inputs, instances, origin, key| {
            if checked {
                return;
            }
            let original = originals
                .iter()
                .find(|unit| {
                    unit.header.definition == placed.header.definition
                        && unit.header.instance == placed.header.instance
                })
                .unwrap();
            let equations = LocalDemandEquations::extract(original).unwrap();
            let guard = hirsel_rc::seal_demand::equations::solve(&[equations.clone()], &[0])
                .unwrap()
                .for_unit(0, &equations)
                .unwrap();
            let source = ReadySourceFacts::for_instance(instances, origin);
            let finalizers = hirsel_rc::cleanup::Finalizers::planned(instances, inputs.layouts);
            let options = Options::new(inputs.target.clone());
            let key = InstanceAddress {
                instance: key.clone(),
                target_abi: inputs.target.digest(),
                policy: ContentHash([1; 32]),
            };
            let action = Action(ContentHash::of_bytes(b"guard refusal action"));
            for mutation in 0..4 {
                let mut encoded = serde_json::to_value(&guard).unwrap();
                match mutation {
                    0 => {
                        encoded["owner"] =
                            serde_json::to_value((guard.owner().0.clone(), ContentHash([255; 32])))
                                .unwrap()
                    }
                    1 => encoded["original_values"] = serde_json::json!(original.values.len() + 1),
                    2 => encoded["values"] = serde_json::json!([original.values.len()]),
                    3 => encoded["values"] = serde_json::json!([0, 0]),
                    _ => unreachable!(),
                }
                let forged = serde_json::from_value(encoded).unwrap();
                let before = (queries.counts.transformations, queries.counts.validations);
                let error = queries
                    .publish_from_value(
                        &key,
                        action,
                        ReadyMiss {
                            value: original.clone(),
                            options: &options,
                            source: &source,
                            guard: Some(&forged),
                            finalizers: &finalizers,
                            inputs,
                            additional_abis: &[],
                            statics: &[],
                            emission_context: context,
                            debug: false,
                            optimizer_reads: vec![],
                        },
                    )
                    .err()
                    .expect("foreign guard metadata must refuse");
                assert!(
                    error.contains("guard selection") || error.contains("seal-demand selection"),
                    "{error}"
                );
                assert_eq!(queries.counts.transformations, before.0);
                assert_eq!(queries.counts.validations, before.1);
            }
            assert!(entry < originals.len());
            checked = true;
        },
    );
    assert!(checked);
}
