//! Arena component discovery from immutable local equation descriptors. This
//! uses the existing iterative Tarjan implementation and the existing two
//! deletion strata; no Value collection or mutable global layout table enters.
use super::arena::{ArenaQueryCounts, EquationProduct, arena_query};
use super::*;
use hirsel_engine::store::ResultStore;
use hirsel_rc::arena_credit::equations::Function;
use std::collections::{BTreeMap, BTreeSet};

/// Callees are compact native-instance discovery metadata, extracted together
/// with `equations` on the instance miss. The equation reference binds their
/// exact predicates; component misses independently check the internal edges.
pub struct ArenaNode {
    pub equations: EquationProduct,
    pub callees: Vec<Function>,
}

/// Shared complete component addresses. An escaped key view retains the full
/// arena registry admission, including the backing of every component.
#[derive(Clone)]
pub struct ArenaComponentKeys(Arc<ComponentKeysData>);
#[derive(Clone)]
struct ComponentKeysData {
    values: Vec<ComponentKey>,
    owner: Option<Arc<hirsel_engine::runtime::MemoryReservation>>,
}
impl std::fmt::Debug for ArenaComponentKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.values.fmt(f)
    }
}
impl std::ops::Deref for ArenaComponentKeys {
    type Target = [ComponentKey];
    fn deref(&self) -> &Self::Target {
        &self.0.values
    }
}
impl ArenaComponentKeys {
    fn new(mut values: Vec<ComponentKey>) -> Self {
        values.shrink_to_fit();
        Self(Arc::new(ComponentKeysData {
            values,
            owner: None,
        }))
    }
    fn retained_bytes(&self) -> u64 {
        let mut w = hirsel_engine::NativeMetadataWeight::default();
        w.arc(&self.0);
        w.vec(
            &self.0.values,
            super::ownership_compilation::metadata::component,
        );
        w.bytes()
    }
    fn with_owner(&mut self, owner: Arc<hirsel_engine::runtime::MemoryReservation>) {
        Arc::make_mut(&mut self.0).owner = Some(owner);
    }
}

#[derive(Clone)]
pub struct SettledArenaGraph {
    /// Components in deterministic callee-first order in each stratum.
    pub candidates: Vec<Arc<ArenaSummary>>,
    pub chosen: Vec<Arc<ArenaSummary>>,
    /// Exact component addresses, including topology, facet and stratum.
    pub component_keys: ArenaComponentKeys,
    chosen_by_owner: BTreeMap<Function, Arc<ArenaSummary>>,
}

impl SettledArenaGraph {
    pub(super) fn retained_index_bytes(&mut self) -> u64 {
        let mut w = hirsel_engine::NativeMetadataWeight::default();
        w.flat_vec(&self.candidates);
        w.flat_vec(&self.chosen);
        if self.component_keys.0.owner.is_none() {
            w.add(self.component_keys.retained_bytes() as usize);
        }
        w.packed_map(&mut self.chosen_by_owner, |w, (path, _), _| w.path(path));
        w.bytes()
    }
    /// Only components read by one local Ready query: its own chosen claim and
    /// its direct callees' chosen claims. The descriptor index is built once
    /// during settlement, so a hit does not scan unrelated graph components.
    pub fn chosen_reads(
        &self,
        owner: &InstanceAddress,
        callees: &[Function],
    ) -> Result<Vec<Arc<ArenaSummary>>, String> {
        let mut reads = BTreeMap::new();
        let own = function(owner);
        for member in std::iter::once(&own).chain(callees) {
            let summary = self
                .chosen_by_owner
                .get(member)
                .ok_or("missing chosen arena dependency")?;
            reads.insert(summary.product.address.0, summary.clone());
        }
        Ok(reads.into_values().collect())
    }
}

fn function(address: &InstanceAddress) -> Function {
    (
        address.instance.definition().clone(),
        address.instance.digest(),
    )
}

/// Pure topology planning over actual instance descriptors. Both direct query
/// clients and runtime continuations use this one planner and the same solver.
/// No equation or Value body is opened while constructing the plan.
pub struct ArenaQueryPlan {
    pub component_keys: ArenaComponentKeys,
    members: Vec<Vec<EquationProduct>>,
    outgoing: Vec<Vec<InstanceAddress>>,
    by_owner: BTreeMap<Function, usize>,
    external: BTreeMap<Function, (InstanceAddress, Vec<u32>, ProductRef)>,
}
impl ArenaQueryPlan {
    pub fn new(
        nodes: &[ArenaNode],
        external: &[(InstanceAddress, Vec<u32>, ProductRef)],
        facet: u32,
    ) -> Result<Self, String> {
        if nodes.windows(2).any(|w| {
            !w[0]
                .equations
                .owner
                .canonical_cmp(&w[1].equations.owner)
                .is_lt()
        }) || external
            .windows(2)
            .any(|w| !w[0].0.canonical_cmp(&w[1].0).is_lt())
            || nodes
                .iter()
                .any(|n| n.callees.windows(2).any(|w| w[0] >= w[1]))
            || external
                .iter()
                .any(|(_, xs, _)| xs.windows(2).any(|w| w[0] >= w[1]))
        {
            return Err("noncanonical arena discovery descriptors".into());
        }
        let indices: BTreeMap<_, _> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (function(&n.equations.owner), i))
            .collect();
        let externals: BTreeMap<_, _> =
            external.iter().map(|row| (function(&row.0), row)).collect();
        if indices.len() != nodes.len()
            || externals.len() != external.len()
            || externals.keys().any(|f| indices.contains_key(f))
        {
            return Err("duplicate arena discovery owner".into());
        }
        let graph: Vec<Vec<usize>> = nodes
            .iter()
            .map(|n| {
                n.callees
                    .iter()
                    .filter_map(|f| indices.get(f).copied())
                    .collect()
            })
            .collect();
        if nodes
            .iter()
            .flat_map(|n| &n.callees)
            .any(|f| !indices.contains_key(f) && !externals.contains_key(f))
        {
            return Err("missing arena discovery dependency".into());
        }
        let components = hirsel_graph::components(&graph);

        let mut component_keys = Vec::new();
        let mut all_members = Vec::new();
        let mut all_outgoing = Vec::new();
        let mut by_owner = BTreeMap::new();
        for component in &components {
            let positions: BTreeMap<_, _> = component
                .iter()
                .enumerate()
                .map(|(i, &n)| (n, i as u32))
                .collect();
            let mut edges = Vec::new();
            for (i, &n) in component.iter().enumerate() {
                for callee in &graph[n] {
                    if let Some(&j) = positions.get(callee) {
                        edges.push((i as u32, j));
                    }
                }
            }
            edges.sort();
            edges.dedup();
            let key = ComponentKey {
                members: component
                    .iter()
                    .map(|&i| nodes[i].equations.owner.clone())
                    .collect(),
                edges: {
                    edges.shrink_to_fit();
                    edges
                },
                facet,
                stratum: ArenaStratum::Candidates,
            };
            key.address()?;
            let members: Vec<_> = component
                .iter()
                .map(|&i| EquationProduct {
                    owner: nodes[i].equations.owner.clone(),
                    reference: nodes[i].equations.reference,
                })
                .collect();
            let outgoing: BTreeSet<_> = component
                .iter()
                .flat_map(|&n| &nodes[n].callees)
                .filter(|f| indices.get(*f).is_none_or(|i| !positions.contains_key(i)))
                .collect();
            let mut outgoing = outgoing
                .into_iter()
                .map(|f| {
                    indices
                        .get(f)
                        .map(|&index| nodes[index].equations.owner.clone())
                        .or_else(|| externals.get(f).map(|row| row.0.clone()))
                        .ok_or_else(|| "missing outgoing arena discovery owner".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            outgoing.sort_by(InstanceAddress::canonical_cmp);
            for owner in &key.members {
                by_owner.insert(function(owner), component_keys.len());
            }
            component_keys.push(key);
            all_members.push(members);
            outgoing.shrink_to_fit();
            all_outgoing.push(outgoing);
        }
        let chosen = component_keys
            .iter()
            .cloned()
            .map(|mut key| {
                key.stratum = ArenaStratum::Chosen;
                key
            })
            .collect::<Vec<_>>();
        component_keys.extend(chosen);
        all_members.shrink_to_fit();
        all_outgoing.shrink_to_fit();
        Ok(Self {
            component_keys: ArenaComponentKeys::new(component_keys),
            members: all_members,
            outgoing: all_outgoing,
            by_owner,
            external: external
                .iter()
                .map(|row| (function(&row.0), row.clone()))
                .collect(),
        })
    }
    fn owner_query(&self, owner: &InstanceAddress, stratum: ArenaStratum) -> Option<usize> {
        self.by_owner.get(&function(owner)).map(|index| {
            index
                + if stratum == ArenaStratum::Chosen {
                    self.members.len()
                } else {
                    0
                }
        })
    }
    pub(super) fn retain_owner(&mut self, owner: Arc<hirsel_engine::runtime::MemoryReservation>) {
        self.component_keys.with_owner(owner);
    }
    pub(super) fn retained_storage_bytes(&mut self) -> u64 {
        use super::ownership_compilation::metadata::address;
        let mut w = hirsel_engine::NativeMetadataWeight::default();
        w.add(self.component_keys.retained_bytes() as usize);
        w.vec(&self.members, |w, members| {
            w.vec(members, |w, row| address(w, &row.owner))
        });
        w.vec(&self.outgoing, |w, outgoing| w.vec(outgoing, address));
        w.packed_map(&mut self.by_owner, |w, (path, _), _| w.path(path));
        w.packed_map(
            &mut self.external,
            |w, (path, _), (owner, candidates, _)| {
                w.path(path);
                address(w, owner);
                w.flat_vec(candidates);
            },
        );
        w.bytes()
    }
    /// Only same-stratum outgoing components and, in Chosen, own Candidates.
    pub fn dependencies(&self, index: usize) -> Result<Vec<usize>, String> {
        let key = self
            .component_keys
            .get(index)
            .ok_or("unknown arena component")?;
        let mut reads = BTreeSet::new();
        for owner in &self.outgoing[index % self.outgoing.len()] {
            if let Some(dependency) = self.owner_query(owner, key.stratum) {
                reads.insert(dependency);
            }
        }
        if key.stratum == ArenaStratum::Chosen {
            reads.insert(
                *self
                    .by_owner
                    .get(&function(&key.members[0]))
                    .ok_or("missing own Candidates component")?,
            );
        }
        Ok(reads.into_iter().collect())
    }
    pub fn solve(
        &self,
        store: &dyn ResultStore,
        index: usize,
        selected: &BTreeMap<usize, Arc<ArenaSummary>>,
        counts: &mut ArenaQueryCounts,
    ) -> Result<Arc<ArenaSummary>, String> {
        let key = self
            .component_keys
            .get(index)
            .ok_or("unknown arena component")?;
        for dependency in self.dependencies(index)? {
            let output = selected
                .get(&dependency)
                .ok_or("unsettled arena component dependency")?;
            if output.product.address != self.component_keys[dependency].address()? {
                return Err("arena component dependency has another address".into());
            }
        }
        let mut outgoing = Vec::new();
        for owner in &self.outgoing[index % self.outgoing.len()] {
            let row = if let Some(index) = self.owner_query(owner, key.stratum) {
                let output = selected
                    .get(&index)
                    .ok_or("unsettled outgoing arena dependency")?;
                let (_, candidates) = output
                    .candidates
                    .iter()
                    .find(|(key, _)| key == owner)
                    .ok_or("outgoing arena component omits selected owner")?;
                (owner.clone(), candidates.clone(), output.product)
            } else {
                self.external
                    .get(&function(owner))
                    .cloned()
                    .ok_or("missing external arena dependency")?
            };
            outgoing.push(row);
        }
        let own = if key.stratum == ArenaStratum::Chosen {
            let index = self
                .by_owner
                .get(&function(&key.members[0]))
                .ok_or("missing own Candidates")?;
            Some(
                selected
                    .get(index)
                    .ok_or("unsettled own Candidates")?
                    .as_ref(),
            )
        } else {
            None
        };
        arena_query(
            store,
            key,
            &self.members[index % self.members.len()],
            own,
            &outgoing,
            counts,
        )
    }
    pub fn finish(
        &self,
        selected: &BTreeMap<usize, Arc<ArenaSummary>>,
    ) -> Result<SettledArenaGraph, String> {
        if selected.len() != self.component_keys.len() {
            return Err("arena graph has unfinished components".into());
        }
        let mut candidates = Vec::new();
        let mut chosen = Vec::new();
        for (index, key) in self.component_keys.iter().enumerate() {
            let summary = selected
                .get(&index)
                .ok_or("missing arena component output")?;
            if summary.product.address != key.address()?
                || summary
                    .candidates
                    .iter()
                    .map(|(owner, _)| owner)
                    .ne(&key.members)
            {
                return Err("arena output differs from planned component".into());
            }
            match key.stratum {
                ArenaStratum::Candidates => candidates.push(summary.clone()),
                ArenaStratum::Chosen => chosen.push(summary.clone()),
            }
        }
        let chosen_by_owner = chosen
            .iter()
            .flat_map(|summary| {
                summary
                    .candidates
                    .iter()
                    .map(|(owner, _)| (function(owner), summary.clone()))
            })
            .collect();
        Ok(SettledArenaGraph {
            candidates,
            chosen,
            component_keys: self.component_keys.clone(),
            chosen_by_owner,
        })
    }
}

/// Direct clients and task clients share topology, both deletion strata, action
/// keys and independent equation checks. Provisional results are never exposed.
pub fn settle_arena_queries(
    store: &dyn ResultStore,
    nodes: &[ArenaNode],
    external: &[(InstanceAddress, Vec<u32>, ProductRef)],
    facet: u32,
    counts: &mut ArenaQueryCounts,
) -> Result<SettledArenaGraph, String> {
    let plan = ArenaQueryPlan::new(nodes, external, facet)?;
    let mut selected = BTreeMap::new();
    for index in 0..plan.component_keys.len() {
        let output = plan.solve(store, index, &selected, counts)?;
        selected.insert(index, output);
    }
    plan.finish(&selected)
}

#[cfg(test)]
mod retained_tests {
    use super::*;
    use hirsel_engine::store::Action;

    #[test]
    fn escaped_component_keys_and_summary_keep_the_complete_arena_grant() {
        let types = hirsel_types::TypeInterner::new(None);
        let owner = InstanceAddress {
            instance: hirsel_layout::instance::InstanceKey::new(
                &hirsel_syntax::DefPath::core("arena-owner"),
                0,
                &[],
                &types,
                None,
                None,
            )
            .unwrap(),
            target_abi: ContentHash::of_bytes(b"arena target"),
            policy: ContentHash::of_bytes(b"arena policy"),
        };
        let reference = hirsel_engine::store::Domain { kind: 1, schema: 1 }
            .reference(b"equations")
            .unwrap();
        let mut plan = ArenaQueryPlan::new(
            &[ArenaNode {
                equations: EquationProduct {
                    owner: owner.clone(),
                    reference,
                },
                callees: Vec::new(),
            }],
            &[],
            1,
        )
        .unwrap();
        assert!(plan.dependencies(0).unwrap().is_empty());
        assert_eq!(plan.dependencies(1).unwrap(), [0]);
        assert_ne!(
            plan.component_keys[0].address().unwrap(),
            plan.component_keys[1].address().unwrap()
        );
        let budget = hirsel_engine::runtime::ProcessBudget::new(1 << 20);
        let grant = Arc::new(budget.reserve(1 << 20).unwrap());
        plan.retain_owner(grant.clone());
        let keys = plan.component_keys.clone();
        let summary = Arc::new(ArenaSummary {
            product: ProductRef {
                address: owner.address(ARENA_CANDIDATES_KIND),
                action: Action(ContentHash::default()),
                record: reference,
            },
            candidates: vec![(owner, Vec::new())],
            retained_owner: Some(grant.clone()),
        });
        drop(plan);
        drop(grant);
        assert_eq!(budget.reserved(), 1 << 20);
        drop(keys);
        assert_eq!(budget.reserved(), 1 << 20);
        drop(summary);
        assert_eq!(budget.reserved(), 0);
    }
}
