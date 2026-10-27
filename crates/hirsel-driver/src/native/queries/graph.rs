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

#[derive(Clone)]
pub struct SettledArenaGraph {
    /// Components in deterministic callee-first order in each stratum.
    pub candidates: Vec<Arc<ArenaSummary>>,
    pub chosen: Vec<Arc<ArenaSummary>>,
    /// Exact component addresses, including topology, facet and stratum.
    pub component_keys: Vec<ComponentKey>,
    chosen_by_owner: BTreeMap<Function, Arc<ArenaSummary>>,
}

impl SettledArenaGraph {
    pub(super) fn retained_index_bytes(&mut self) -> u64 {
        let mut w = hirsel_engine::NativeMetadataWeight::default();
        w.flat_vec(&self.candidates);
        w.flat_vec(&self.chosen);
        w.vec(
            &self.component_keys,
            super::ownership_compilation::metadata::component,
        );
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
    pub component_keys: Vec<ComponentKey>,
    members: Vec<Vec<EquationProduct>>,
    outgoing: Vec<Vec<InstanceAddress>>,
    by_owner: BTreeMap<(bool, Function), usize>,
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
        for stratum in [ArenaStratum::Candidates, ArenaStratum::Chosen] {
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
                    edges,
                    facet,
                    stratum,
                };
                key.address()?;
                let members = component
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
                    by_owner.insert(
                        (stratum == ArenaStratum::Chosen, function(owner)),
                        component_keys.len(),
                    );
                }
                component_keys.push(key);
                all_members.push(members);
                all_outgoing.push(outgoing);
            }
        }
        Ok(Self {
            component_keys,
            members: all_members,
            outgoing: all_outgoing,
            by_owner,
            external: external
                .iter()
                .map(|row| (function(&row.0), row.clone()))
                .collect(),
        })
    }
    /// Only same-stratum outgoing components and, in Chosen, own Candidates.
    pub fn dependencies(&self, index: usize) -> Result<Vec<usize>, String> {
        let key = self
            .component_keys
            .get(index)
            .ok_or("unknown arena component")?;
        let mut reads = BTreeSet::new();
        for owner in &self.outgoing[index] {
            if let Some(&dependency) = self
                .by_owner
                .get(&(key.stratum == ArenaStratum::Chosen, function(owner)))
            {
                reads.insert(dependency);
            }
        }
        if key.stratum == ArenaStratum::Chosen {
            reads.insert(
                *self
                    .by_owner
                    .get(&(false, function(&key.members[0])))
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
        for owner in &self.outgoing[index] {
            let row = if let Some(index) = self
                .by_owner
                .get(&(key.stratum == ArenaStratum::Chosen, function(owner)))
            {
                let output = selected
                    .get(index)
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
                .get(&(false, function(&key.members[0])))
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
        arena_query(store, key, &self.members[index], own, &outgoing, counts)
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
