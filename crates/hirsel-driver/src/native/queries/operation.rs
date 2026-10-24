//! Descriptor-only operation-freedom component results over actual admitted
//! instance discoveries. Summary data grants neither V nor native authority.
use super::{demand::DemandInstances, *};
use hirsel_artifact_store::{Action, Domain};
use hirsel_engine::store::ResultStore;
use hirsel_lir::validate::NativeEnvironment;
use hirsel_rc::{
    arena_credit::equations::Function,
    closure_calls::{OperationFreedom, settle_operation_freedom},
};
use std::collections::{BTreeMap, BTreeSet};

pub const OPERATION_COMPONENT_KIND: u32 = 0x4e00_0026;
const COMPONENT: Domain = Domain {
    kind: OPERATION_COMPONENT_KIND,
    schema: 1,
};
#[derive(Default, Debug)]
pub struct OperationCounts {
    pub hits: u64,
    pub publications: u64,
    /// Each component is independently checked from compact equations, even
    /// on a store hit. This never restores a Value or trusts a stored boolean.
    pub evaluations: u64,
}
#[derive(Clone, Debug)]
pub struct OperationComponent {
    pub members: Vec<InstanceAddress>,
    pub product: ProductRef,
    pub reads: Vec<ProductRef>,
}
pub struct SettledOperations {
    pub components: Vec<OperationComponent>,
    values: BTreeMap<Function, bool>,
}
impl SettledOperations {
    pub fn get(&self, owner: &InstanceAddress) -> Option<bool> {
        self.values
            .get(&(owner.instance.definition().clone(), owner.instance.digest()))
            .copied()
    }
}
fn function(owner: &InstanceAddress) -> Function {
    (owner.instance.definition().clone(), owner.instance.digest())
}

/// The caller has completed actual discovery; admission rechecks the same C
/// output before consuming its ordinary local equations. No arbitrary summary
/// input is promoted to native acceptance, and no Value is opened here.
pub fn settle_operations(
    store: &dyn ResultStore,
    instances: &DemandInstances,
    environment: &NativeEnvironment<'_>,
    counts: &mut OperationCounts,
) -> Result<SettledOperations, String> {
    let completed = instances.completed_inputs()?;
    let mut rows = completed
        .iter()
        .map(|(owner, _)| {
            let (current, _) = instances.admitted(owner, environment)?;
            if !current.operation_freedom.canonical() {
                return Err("noncanonical local operation-freedom equations".to_string());
            }
            Ok((owner.clone(), current))
        })
        .collect::<Result<Vec<_>, String>>()?;
    rows.sort_by(|a, b| a.0.canonical_cmp(&b.0));
    let indices = rows
        .iter()
        .enumerate()
        .map(|(i, (owner, _))| (function(owner), i))
        .collect::<BTreeMap<_, _>>();
    if indices.len() != rows.len() {
        return Err("duplicate operation-freedom owner".into());
    }
    let edges = rows
        .iter()
        .map(|(_, output)| {
            output
                .operation_freedom
                .dependencies()
                .iter()
                .filter_map(|callee| indices.get(callee).copied())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut result = SettledOperations {
        components: Vec::new(),
        values: BTreeMap::new(),
    };
    let mut selected = BTreeMap::<Function, ProductRef>::new();
    // Reuse the same callee-first iterative Tarjan implementation as arena
    // planning. Missing direct targets retain the old conservative false rule.
    for mut component in hirsel_graph::components(&edges) {
        component.sort_unstable();
        let members = component
            .iter()
            .map(|&i| rows[i].0.clone())
            .collect::<Vec<_>>();
        let local = component
            .iter()
            .map(|&i| (function(&rows[i].0), rows[i].1.operation_freedom.clone()))
            .collect::<BTreeMap<Function, OperationFreedom>>();
        let internal = component
            .iter()
            .enumerate()
            .flat_map(|(from, &i)| {
                edges[i]
                    .iter()
                    .filter_map(|callee| {
                        component
                            .binary_search(callee)
                            .ok()
                            .map(|to| (from as u32, to as u32))
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let key = serde_json::to_vec(&(
            members
                .iter()
                .map(|owner| (owner.instance.encoding(), owner.target_abi, owner.policy))
                .collect::<Vec<_>>(),
            internal,
        ))
        .map_err(|error| error.to_string())?;
        let address = hirsel_artifact_store::Address(ContentHash::of_parts(
            "hirsel.native.operation-component-address.v1",
            [key.as_slice()],
        ));
        let mut outgoing = BTreeMap::new();
        let mut reads = BTreeMap::new();
        for facts in local.values() {
            for callee in facts.dependencies() {
                if local.contains_key(callee) {
                    continue;
                }
                if let Some(&value) = result.values.get(callee) {
                    let output = selected[callee];
                    let association = store
                        .lookup_product(output.address)
                        .ok_or("missing outgoing operation-freedom association")?;
                    if association.action != output.action
                        || association.reference != output.record
                        || store.get_content(output.record).is_none()
                    {
                        return Err("outgoing operation-freedom result is no longer settled".into());
                    }
                    outgoing.insert(callee.clone(), value);
                    reads.insert(output.address.0, output);
                } else if indices.contains_key(callee) {
                    return Err(
                        "operation-freedom component was not scheduled after its callee".into(),
                    );
                }
            }
        }
        let reads = reads.into_values().collect::<Vec<_>>();
        let action_bytes = serde_json::to_vec(&(
            crate::COMPILER_IDENTITY.as_slice(),
            &key,
            local.iter().collect::<Vec<_>>(),
            outgoing.iter().collect::<Vec<_>>(),
            reads.iter().map(|output| output.record).collect::<Vec<_>>(),
        ))
        .map_err(|error| error.to_string())?;
        let action = Action(ContentHash::of_parts(
            "hirsel.native.operation-component-action.v1",
            [action_bytes.as_slice()],
        ));
        // Recheck compact truth independently rather than letting a forged
        // ordinary store output manufacture a private compiler fast-path fact.
        counts.evaluations += 1;
        let values = settle_operation_freedom(&local, &outgoing)?;
        let bytes = serde_json::to_vec(&values.iter().collect::<Vec<_>>())
            .map_err(|error| error.to_string())?;
        let expected = COMPONENT
            .reference(&bytes)
            .ok_or("operation-freedom output exceeds record limit")?;
        let hit = store.lookup_product(address).is_some_and(|previous| {
            previous.action == action
                && previous.reference == expected
                && store
                    .get_content(expected)
                    .is_some_and(|actual| actual.as_ref() == bytes.as_slice())
        });
        if hit {
            counts.hits += 1;
        } else {
            let actual = store
                .put_content(COMPONENT, &bytes)
                .ok_or("operation-freedom publication failed")?;
            if actual != expected {
                return Err("operation-freedom publication identity differs".into());
            }
            store.associate_product(address, action, actual);
            counts.publications += 1;
        }
        let product = ProductRef {
            address,
            action,
            record: expected,
        };
        for (owner, value) in values {
            selected.insert(owner.clone(), product);
            result.values.insert(owner, value);
        }
        result.components.push(OperationComponent {
            members,
            product,
            reads,
        });
    }
    Ok(result)
}
