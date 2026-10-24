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
    pub(super) retained_allowance: Option<Arc<hirsel_engine::runtime::MemoryReservation>>,
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

struct PlannedComponent {
    members: Vec<InstanceAddress>,
    instances: Vec<ProductRef>,
    local: BTreeMap<Function, OperationFreedom>,
    key: Vec<u8>,
    address: hirsel_artifact_store::Address,
    outgoing: Vec<usize>,
}
pub(super) struct OperationResult {
    pub summary: OperationComponent,
    values: BTreeMap<Function, bool>,
}
/// Compact immutable topology and local equations. Both direct and runtime
/// clients use this planner and the same independently checked component query.
pub(super) struct OperationQueryPlan {
    components: Vec<PlannedComponent>,
}
impl OperationQueryPlan {
    pub fn new(
        instances: &DemandInstances,
        environment: &NativeEnvironment<'_>,
    ) -> Result<Self, String> {
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
        let mut components = hirsel_graph::components(&edges);
        for component in &mut components {
            component.sort_unstable();
        }
        let by_owner = components
            .iter()
            .enumerate()
            .flat_map(|(i, component)| component.iter().map(move |&member| (member, i)))
            .collect::<BTreeMap<_, _>>();
        let mut planned = Vec::new();
        for (at, component) in components.into_iter().enumerate() {
            let members = component
                .iter()
                .map(|&i| rows[i].0.clone())
                .collect::<Vec<_>>();
            let instances = component
                .iter()
                .map(|&i| rows[i].1.product.product)
                .collect();
            let local = component
                .iter()
                .map(|&i| (function(&rows[i].0), rows[i].1.operation_freedom.clone()))
                .collect();
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
            .map_err(|e| e.to_string())?;
            let address = hirsel_artifact_store::Address(ContentHash::of_parts(
                "hirsel.native.operation-component-address.v1",
                [key.as_slice()],
            ));
            let outgoing = component
                .iter()
                .flat_map(|&i| edges[i].iter().map(|callee| by_owner[callee]))
                .filter(|&i| i != at)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            if outgoing.iter().any(|&i| i >= at) {
                return Err("operation component order is not callee first".into());
            }
            planned.push(PlannedComponent {
                members,
                instances,
                local,
                key,
                address,
                outgoing,
            });
        }
        Ok(Self {
            components: planned,
        })
    }
    pub fn ids(&self) -> Vec<hirsel_engine::runtime::QueryId> {
        self.components
            .iter()
            .map(|component| hirsel_engine::runtime::QueryId {
                kind: OPERATION_COMPONENT_KIND,
                address: component.address,
            })
            .collect()
    }
    pub fn dependencies(&self, index: usize) -> Result<&[usize], String> {
        self.components
            .get(index)
            .map(|component| component.outgoing.as_slice())
            .ok_or("unregistered operation component".into())
    }
    pub fn instances(&self, index: usize) -> Result<&[ProductRef], String> {
        self.components
            .get(index)
            .map(|component| component.instances.as_slice())
            .ok_or("unregistered operation component".into())
    }
    pub fn retained_bytes(&self) -> Result<u64, String> {
        self.components
            .iter()
            .try_fold(4096u64, |total, component| {
                let facts = serde_json::to_vec(&component.local.iter().collect::<Vec<_>>())
                    .map_err(|e| e.to_string())?;
                Ok(total.saturating_add(
                    4096 + (facts.len()
                        + component.key.len()
                        + component.instances.len() * 256
                        + component.outgoing.len() * 128) as u64
                        * 8,
                ))
            })
    }
    pub fn solve(
        &self,
        store: &dyn ResultStore,
        index: usize,
        selected: &BTreeMap<usize, Arc<OperationResult>>,
        counts: &mut OperationCounts,
    ) -> Result<Arc<OperationResult>, String> {
        let component = self
            .components
            .get(index)
            .ok_or("unregistered operation component")?;
        if selected.keys().copied().collect::<Vec<_>>() != component.outgoing {
            return Err("operation component dependency selection differs".into());
        }
        let mut outgoing = BTreeMap::new();
        let mut reads = BTreeMap::new();
        for (&index, output) in selected {
            let expected = &self.components[index];
            let product = output.summary.product;
            let association = store
                .lookup_product(product.address)
                .ok_or("missing outgoing operation-freedom association")?;
            if product.address != expected.address
                || output.summary.members != expected.members
                || association.action != product.action
                || association.reference != product.record
                || store.get_content(product.record).is_none()
            {
                return Err("outgoing operation-freedom result is no longer settled".into());
            }
            for callee in component
                .local
                .values()
                .flat_map(OperationFreedom::dependencies)
            {
                if component.local.contains_key(callee) {
                    continue;
                }
                if let Some(&value) = output.values.get(callee) {
                    outgoing.insert(callee.clone(), value);
                }
            }
            reads.insert(product.address.0, product);
        }
        let reads = reads.into_values().collect::<Vec<_>>();
        let action_bytes = serde_json::to_vec(&(
            crate::COMPILER_IDENTITY.as_slice(),
            &component.key,
            component.local.iter().collect::<Vec<_>>(),
            outgoing.iter().collect::<Vec<_>>(),
            reads.iter().map(|output| output.record).collect::<Vec<_>>(),
        ))
        .map_err(|e| e.to_string())?;
        let action = Action(ContentHash::of_parts(
            "hirsel.native.operation-component-action.v1",
            [action_bytes.as_slice()],
        ));
        counts.evaluations += 1;
        let values = settle_operation_freedom(&component.local, &outgoing)?;
        let bytes =
            serde_json::to_vec(&values.iter().collect::<Vec<_>>()).map_err(|e| e.to_string())?;
        let expected = COMPONENT
            .reference(&bytes)
            .ok_or("operation-freedom output exceeds record limit")?;
        let hit = store
            .lookup_product(component.address)
            .is_some_and(|previous| {
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
            store.associate_product(component.address, action, actual);
            counts.publications += 1;
        }
        Ok(Arc::new(OperationResult {
            summary: OperationComponent {
                members: component.members.clone(),
                product: ProductRef {
                    address: component.address,
                    action,
                    record: expected,
                },
                reads,
            },
            values,
        }))
    }
    pub fn finish(
        &self,
        selected: &BTreeMap<usize, Arc<OperationResult>>,
    ) -> Result<SettledOperations, String> {
        if selected.len() != self.components.len() {
            return Err("operation graph has incomplete components".into());
        }
        let mut result = SettledOperations {
            components: Vec::new(),
            values: BTreeMap::new(),
            retained_allowance: None,
        };
        for (index, component) in self.components.iter().enumerate() {
            let output = selected.get(&index).ok_or("missing operation component")?;
            if output.summary.product.address != component.address
                || output.summary.members != component.members
            {
                return Err("operation component belongs to another graph".into());
            }
            result.components.push(output.summary.clone());
            result.values.extend(
                output
                    .values
                    .iter()
                    .map(|(key, value)| (key.clone(), *value)),
            );
        }
        Ok(result)
    }
}

/// Re-admit actual C output before constructing a metadata-only plan. No Value
/// is opened and every component boolean is rechecked from its exact equations.
pub fn settle_operations(
    store: &dyn ResultStore,
    instances: &DemandInstances,
    environment: &NativeEnvironment<'_>,
    counts: &mut OperationCounts,
) -> Result<SettledOperations, String> {
    let plan = OperationQueryPlan::new(instances, environment)?;
    let mut completed = BTreeMap::<usize, Arc<OperationResult>>::new();
    for index in 0..plan.components.len() {
        let selected = plan
            .dependencies(index)?
            .iter()
            .map(|key| (*key, completed[key].clone()))
            .collect();
        let output = plan.solve(store, index, &selected, counts)?;
        completed.insert(index, output);
    }
    plan.finish(&completed)
}
