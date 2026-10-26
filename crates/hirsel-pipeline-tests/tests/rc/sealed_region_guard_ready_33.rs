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
fn original_scalar_selected_guard_reaches_production_ready_and_independent_c() {
    let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
    let mut queries = ReadyQueries::new(store.clone());
    let context = store
        .put_content(EMISSION_CONTEXT_DOMAIN, b"original guarded Ready context")
        .unwrap();
    let mut selection = None;
    let mut guards = 0;
    let mut units = 0;
    let mut observation_checked = false;
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
                        let product = ProductRef {
                            address: Address(ContentHash::of_bytes(&bytes)),
                            action: Action(ContentHash([1; 32])),
                            record,
                        };
                        store.associate_product(product.address, product.action, record);
                        Input { product, equations }
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
            if expected == 1 && !observation_checked {
                // Keep the same physical guard through independent diagnostic
                // stage checks and the existing observation transport chain.
                let observed = queries.observe_from_value(&key, action, miss()).unwrap();
                assert_eq!(
                    observed
                        .unit
                        .unit()
                        .tables
                        .regions
                        .iter()
                        .filter(|r| r.seal_demand)
                        .count(),
                    1
                );
                observation_checked = true;
            }
        },
    );
    assert_eq!(guards, 1);
    assert!(units > 1);
    assert!(observation_checked);
}
