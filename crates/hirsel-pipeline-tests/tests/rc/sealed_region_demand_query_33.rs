use hirsel_driver::native::queries::seal_demand::{Input, Plan};
use hirsel_engine::{
    runtime::ProductRef,
    store::{Action, Address, Domain, MemStore, ResultStore},
};
use hirsel_lir::operation::{NativeType, Primitive};
use hirsel_rc::seal_demand::equations::LocalDemandEquations;
use hirsel_syntax::ContentHash;
use hirsel_types::TypeKind;

#[test]
fn guard_query_refuses_missing_roots_and_repairs_malformed_cached_selections() {
    use hirsel_lir::{
        FnAbiId,
        ir::*,
        objects::UnitKey,
        stage::{StageKind, Value},
    };
    let hash = ContentHash([7; 32]);
    let unit = hirsel_lir::LirUnit::<Value>::new(UnitHeader {
        source_count: 0,
        key: UnitKey(hash),
        track_caller: false,
        stage: StageKind::Value,
        lir_version: hirsel_lir::LIR_VERSION,
        origin: UnitOrigin::Source,
        definition: hirsel_syntax::DefPath::core("ordinary_guard_query_control"),
        implementation_revision: hash,
        instance: hash,
        target: hash,
        abi: FnAbiId(0),
    });
    let equations = LocalDemandEquations::extract(&unit).unwrap();
    let owner = equations.owner().clone();
    let store = MemStore::default();
    let domain = Domain {
        kind: 0x4e00_0001,
        schema: 1,
    };
    let record = store
        .put_content(domain, b"ordinary admitted instance control")
        .unwrap();
    let input = Input {
        product: ProductRef {
            address: Address(hash),
            action: Action(hash),
            record,
        },
        equations,
    };
    store.associate_product(input.product.address, input.product.action, record);
    assert!(
        Plan::new(
            vec![input.clone(), input.clone()],
            std::slice::from_ref(&owner),
            hash,
            hash
        )
        .is_err()
    );
    assert!(
        Plan::new(
            vec![input.clone()],
            &[owner.clone(), owner.clone()],
            hash,
            hash
        )
        .is_err()
    );
    let missing = (hirsel_syntax::DefPath::core("missing_guard_owner"), hash);
    assert!(Plan::new(vec![input.clone()], &[missing], hash, hash).is_err());
    let query = Plan::new(vec![input], &[owner], hash, hash).unwrap();
    let correct = query.execute(&store).unwrap();
    let bytes = store.get_content(correct.product.record).unwrap();
    for mutant in 0..4 {
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        match mutant {
            0 => value[0]["values"] = serde_json::json!([0]),
            1 => value[0]["original_values"] = serde_json::json!(1),
            2 => value = serde_json::json!([]),
            _ => value[0]["values"] = serde_json::json!([0, 0]),
        }
        let forged = store
            .put_content(
                correct.product.record.domain,
                &serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
        store.associate_product(correct.product.address, correct.product.action, forged);
        assert_eq!(
            query.execute(&store).unwrap().product,
            correct.product,
            "cache mutant {mutant}"
        );
    }
}

#[test]
fn original_scalar_guard_query_uses_only_metadata_and_rechecks_dependency_outputs() {
    let store = MemStore::default();
    let domain = Domain {
        kind: 0x4e00_0001,
        schema: 1,
    };
    let mut records = vec![];
    let mut query = None;
    let mut selected = None;
    let mut guards = 0;
    let (h, _, _) = super::sealed_region_h_source_33::original_with_guard_observer(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-scalar-zero-run.hir"),
        TypeKind::Primitive(hirsel_types::Primitive::U64),
        NativeType::Primitive(Primitive::U64),
        |originals, entry, unit| {
            if query.is_none() {
                let mut equations = originals
                    .iter()
                    .map(LocalDemandEquations::extract)
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                equations.sort_by(|a, b| a.owner().cmp(b.owner()));
                let root = (
                    originals[entry].header.definition.clone(),
                    originals[entry].header.instance,
                );
                let inputs = equations
                    .into_iter()
                    .map(|equations| {
                        let bytes = serde_json::to_vec(&equations).unwrap();
                        let record = store.put_content(domain, &bytes).unwrap();
                        let address = Address(ContentHash::of_bytes(&bytes));
                        let action = Action(ContentHash::of_parts(
                            "original-demand-test",
                            [bytes.as_slice()],
                        ));
                        store.associate_product(address, action, record);
                        let product = ProductRef {
                            address,
                            action,
                            record,
                        };
                        records.push(product);
                        Input { product, equations }
                    })
                    .collect();
                let plan = Plan::new(
                    inputs,
                    &[root],
                    crate::rc_support::target().digest(),
                    ContentHash([1; 32]),
                )
                .unwrap();
                assert_eq!(plan.dependencies().count(), records.len());
                selected = Some(plan.execute(&store).unwrap());
                query = Some(plan);
            }
            // Apply the portable selection at its actual Value-to-Placed seam.
            // The original builder's independently computed hints are the twin.
            let expected: Vec<_> = unit.tables.regions.iter().map(|r| r.seal_demand).collect();
            for region in &mut unit.tables.regions {
                region.seal_demand = false;
            }
            let owner = (unit.header.definition.clone(), unit.header.instance);
            let selection = selected.as_ref().unwrap().unit(&owner).unwrap();
            selection.apply(unit).unwrap();
            assert_eq!(
                unit.tables
                    .regions
                    .iter()
                    .map(|r| r.seal_demand)
                    .collect::<Vec<_>>(),
                expected
            );
            guards += unit.tables.regions.iter().filter(|r| r.seal_demand).count();
            let mut wrong = unit.clone();
            wrong.header.instance = ContentHash([255; 32]);
            assert!(selection.apply(&mut wrong).is_err());
        },
    );
    let query = query.unwrap();
    let selected = selected.unwrap();
    assert_eq!(query.execute(&store).unwrap().product, selected.product);
    assert_eq!(
        guards, 1,
        "only the original consuming arena guard is demanded"
    );
    drop(h);
    assert_eq!(query.execute(&store).unwrap().product, selected.product);
    let changed = store
        .put_content(domain, b"changed ordinary descriptor")
        .unwrap();
    let first = records[0];
    store.associate_product(first.address, first.action, changed);
    assert!(
        query
            .execute(&store)
            .err()
            .unwrap()
            .contains("dependency changed")
    );
}
