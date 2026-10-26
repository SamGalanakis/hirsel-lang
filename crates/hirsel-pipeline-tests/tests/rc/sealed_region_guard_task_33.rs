use hirsel_driver::native::queries::{
    INSTANCE_KIND,
    seal_demand::{GuardTasks, Input, Plan},
};
use hirsel_engine::{
    runtime::*,
    store::{Action, Address, Domain, MemStore, ResultStore},
};
use hirsel_lir::operation::{NativeType, Primitive};
use hirsel_rc::seal_demand::equations::LocalDemandEquations;
use hirsel_syntax::ContentHash;
use hirsel_types::TypeKind;
use std::{collections::BTreeMap, sync::Arc, task::Poll};

struct Context {
    inputs: BTreeMap<ContentHash, ProductRef>,
    pending: bool,
    changed: bool,
}
impl QueryContext for Context {
    fn read(&mut self, id: QueryId) -> Poll<Result<ProductRef, hirsel_engine::store::Reference>> {
        assert_eq!(id.kind, INSTANCE_KIND);
        if self.pending {
            return Poll::Pending;
        }
        let mut output = self.inputs[&id.address.0];
        if self.changed {
            output.action = Action(ContentHash([255; 32]));
        }
        Poll::Ready(Ok(output))
    }
}

#[test]
fn original_guard_task_waits_for_dependencies_and_keeps_its_existing_grant_with_output() {
    let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
    let domain = Domain {
        kind: INSTANCE_KIND,
        schema: 1,
    };
    let weight = MemoryWeight {
        scratch: 16 << 20,
        retained: 1 << 20,
        stack: 0,
    };
    let mut tasks = None;
    let mut selection = None;
    let mut retained = None;
    let mut guards = 0;
    let (h, _, _) = super::sealed_region_h_source_33::original_with_guard_observer(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-scalar-zero-run.hir"),
        TypeKind::Primitive(hirsel_types::Primitive::U64),
        NativeType::Primitive(Primitive::U64),
        |originals, entry, unit| {
            if tasks.is_none() {
                let mut locals = originals
                    .iter()
                    .map(LocalDemandEquations::extract)
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                locals.sort_by(|a, b| a.owner().cmp(b.owner()));
                let bytes = Plan::admission_bytes(&locals);
                let budget = ProcessBudget::new(bytes);
                let allowance = Arc::new(budget.reserve(bytes).unwrap());
                let mut context = Context {
                    inputs: BTreeMap::new(),
                    pending: true,
                    changed: false,
                };
                let inputs = locals
                    .into_iter()
                    .map(|equations| {
                        let encoded = serde_json::to_vec(&equations).unwrap();
                        let record = store.put_content(domain, &encoded).unwrap();
                        let address = Address(ContentHash::of_bytes(&encoded));
                        let action = Action(ContentHash::of_parts(
                            "guard-task-control",
                            [encoded.as_slice()],
                        ));
                        store.associate_product(address, action, record);
                        let product = ProductRef {
                            address,
                            action,
                            record,
                        };
                        context.inputs.insert(address.0, product);
                        Input { product, equations }
                    })
                    .collect();
                let root = (
                    originals[entry].header.definition.clone(),
                    originals[entry].header.instance,
                );
                let plan = Plan::new(
                    inputs,
                    &[root],
                    crate::rc_support::target().digest(),
                    ContentHash([1; 32]),
                )
                .unwrap();
                let clone = plan.clone();
                let clone_bytes = clone.retained_bytes();
                assert!(
                    GuardTasks::new(
                        store.clone(),
                        clone,
                        weight,
                        clone_bytes - 1,
                        vec![allowance.clone()]
                    )
                    .is_err()
                );
                assert!(
                    GuardTasks::new(
                        store.clone(),
                        plan.clone(),
                        MemoryWeight {
                            scratch: 0,
                            ..weight
                        },
                        bytes,
                        vec![allowance.clone()]
                    )
                    .is_err()
                );
                let owner =
                    GuardTasks::new(store.clone(), plan, weight, bytes, vec![allowance.clone()])
                        .unwrap();
                drop(allowance);
                let factory = owner.factory();
                let mut task = factory.create(owner.query_id()).unwrap();
                assert!(owner.selection().is_none());
                assert!(matches!(task.poll(&mut context), QueryPoll::Waiting(_)));
                assert!(owner.selection().is_none());
                context.pending = false;
                let QueryPoll::Complete(product) = task.poll(&mut context) else {
                    panic!("settled metadata query did not complete");
                };
                assert!(owner.accepts(product));
                selection = owner.selection();
                let mut rejected = factory.create(owner.query_id()).unwrap();
                context.changed = true;
                assert!(matches!(rejected.poll(&mut context), QueryPoll::Failed(_)));
                retained = Some((budget, bytes));
                tasks = Some(owner);
            }
            for region in &mut unit.tables.regions {
                region.seal_demand = false;
            }
            let owner = (unit.header.definition.clone(), unit.header.instance);
            selection
                .as_ref()
                .unwrap()
                .unit(&owner)
                .unwrap()
                .apply(unit)
                .unwrap();
            guards += unit.tables.regions.iter().filter(|r| r.seal_demand).count();
        },
    );
    assert_eq!(guards, 1);
    drop(h);
    drop(tasks);
    let (budget, bytes) = retained.unwrap();
    assert_eq!(
        budget.reserved(),
        bytes,
        "selection outlives producer under the same original charge"
    );
    drop(selection);
    assert_eq!(budget.reserved(), 0);
}
