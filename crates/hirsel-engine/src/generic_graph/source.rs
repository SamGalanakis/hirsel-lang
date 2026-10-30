//! Source growth products are invalidated by the existing query graph. Only
//! pending source roots demand checked summaries. Components name canonical
//! parameter nodes and edges; their verdict references current position rows.
use crate::summary::{CheckedSummary, GenericParameterRef, GrowthEdge};
use hirsel_diag::Diagnostic;
use hirsel_span::Span;
use hirsel_syntax::{ContentHash, DefId, DefPath};
use hirsel_types::SharedSlice;
use std::collections::{BTreeMap, BTreeSet};

type Node = (DefId, GenericParameterRef);
type EdgeRef = (DefId, usize);

#[derive(Default)]
pub(crate) struct SourceGrowth {
    roots: BTreeSet<DefId>,
    pending: BTreeMap<DefId, bool>,
    products: BTreeMap<DefId, SharedSlice<GrowthEdge>>,
    table_calls: BTreeMap<DefId, BTreeSet<DefPath>>,
    region_calls: BTreeSet<DefId>,
    components: BTreeMap<ContentHash, Vec<EdgeRef>>,
    growing: Vec<EdgeRef>,
    topology_dirty: bool,
    generation: u64,
    table_result: Option<Vec<Diagnostic>>,
    unavailable: Option<Option<crate::QueryKind>>,
}
impl SourceGrowth {
    pub(crate) fn changed(&mut self, definition: DefId, present: bool) {
        if present {
            self.roots.insert(definition);
        } else {
            self.roots.remove(&definition);
        }
        self.pending.insert(definition, present);
        self.invalidate_table();
    }
    pub(crate) fn invalidate(&mut self, definition: DefId) {
        if self.roots.contains(&definition) {
            self.pending.insert(definition, true);
            self.invalidate_table();
        }
    }
    pub(crate) fn invalidate_table(&mut self) {
        self.generation += 1;
        self.table_result = None;
    }
    pub(crate) fn unavailable(&mut self, unavailable: Option<crate::QueryKind>) {
        if self.unavailable != Some(unavailable) {
            self.unavailable = Some(unavailable);
            for &root in &self.roots {
                self.pending.insert(root, true);
            }
            self.invalidate_table();
        }
    }
    pub(crate) fn pop(&mut self) -> Option<(DefId, bool)> {
        self.pending.pop_first()
    }
    pub(crate) fn retire(&mut self, definition: DefId) {
        self.roots.remove(&definition);
        self.pending.remove(&definition);
        self.table_calls.remove(&definition);
        self.region_calls.remove(&definition);
        self.topology_dirty |= self.products.remove(&definition).is_some();
    }
    pub(crate) fn publish(&mut self, definition: DefId, checked: CheckedSummary) {
        if checked
            .summary
            .calls
            .iter()
            .any(|call| call.callee.path().is_core("region.with"))
        {
            self.region_calls.insert(definition);
        } else {
            self.region_calls.remove(&definition);
        }
        let edges = checked.growth.edges;
        // Positions never decide semantic equality, but always replace the
        // current position product used to render a cached component verdict.
        self.topology_dirty |= self
            .products
            .get(&definition)
            .map_or(!edges.is_empty(), |old| {
                old.len() != edges.len()
                    || old
                        .iter()
                        .zip(edges.iter())
                        .any(|(a, b)| (a.0, a.1, a.2) != (b.0, b.1, b.2))
            });
        if edges.is_empty() {
            self.products.remove(&definition);
        } else {
            self.products.insert(definition, edges);
        }
        let calls: BTreeSet<_> = checked
            .summary
            .calls
            .iter()
            .filter_map(|call| {
                let path = call.callee.path();
                (!hirsel_types::core_roles::table_key_parameters(path).is_empty())
                    .then(|| path.clone())
            })
            .collect();
        if calls.is_empty() {
            self.table_calls.remove(&definition);
        } else {
            self.table_calls.insert(definition, calls);
        }
    }
    pub(crate) fn table_calls(&self) -> impl Iterator<Item = &DefPath> {
        self.table_calls.values().flat_map(|calls| calls.iter())
    }
    pub(crate) fn has_region_calls(&self) -> bool {
        !self.region_calls.is_empty()
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn table_result(&self) -> Option<Vec<Diagnostic>> {
        self.table_result.clone()
    }
    pub(crate) fn record_table_result(&mut self, generation: u64, result: Vec<Diagnostic>) {
        if generation == self.generation && self.pending.is_empty() {
            self.table_result = Some(result);
        }
    }
    pub(crate) fn edges(&self) -> impl Iterator<Item = &GrowthEdge> {
        self.products.values().flat_map(|edges| edges.iter())
    }
    pub(crate) fn growing_sites(
        &mut self,
        work: &mut crate::WorkStats,
        settled: bool,
    ) -> Vec<Span> {
        if !settled {
            self.components.clear();
            self.topology_dirty = true;
        }
        if self.topology_dirty {
            self.rebuild_components(work);
            self.topology_dirty = !settled;
        }
        if !settled {
            self.components.clear();
        }
        let mut sites = BTreeMap::new();
        for &(owner, edge) in &self.growing {
            let site = self.products[&owner][edge].3;
            sites.insert((site.anchor, site.start, site.end), site);
        }
        sites.into_values().collect()
    }
    fn rebuild_components(&mut self, work: &mut crate::WorkStats) {
        // Reuse hirsel-graph's iterative Tarjan implementation, as the previous
        // monolithic growth check did. Topology edits may split or merge SCCs;
        // membership is recomputed before any component verdict is admitted.
        let nodes: BTreeSet<Node> = self.edges().flat_map(|e| [e.0, e.1]).collect();
        let nodes: Vec<_> = nodes.into_iter().collect();
        let numbers: BTreeMap<_, _> = nodes.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let mut graph = vec![vec![]; nodes.len()];
        for edge in self.edges() {
            graph[numbers[&edge.0]].push(numbers[&edge.1]);
        }
        let mut retained = BTreeMap::new();
        let mut growing = Vec::new();
        let components = hirsel_graph::components(&graph);
        let mut component_of = vec![0; nodes.len()];
        for (component, members) in components.iter().enumerate() {
            for &member in members {
                component_of[member] = component;
            }
        }
        let mut edges = vec![vec![]; components.len()];
        for (&owner, rows) in &self.products {
            for (ordinal, edge) in rows.iter().enumerate() {
                let component = component_of[numbers[&edge.0]];
                // Include outgoing edges in the component action. An edit
                // that changes membership cannot reuse a previous verdict.
                edges[component].push((owner, ordinal, edge.0, edge.1, edge.2));
            }
        }
        for (component, members) in components.iter().enumerate() {
            let members: Vec<_> = members.iter().map(|&i| nodes[i]).collect();
            let bytes = bincode::serialize(&(&members, &edges[component])).expect("growth action");
            let key = ContentHash::of_parts("hirsel.source-growth.scc.v1", [bytes.as_slice()]);
            let verdict = if let Some(cached) = self.components.remove(&key) {
                work.growth_component_hits += 1;
                cached
            } else {
                work.growth_components += 1;
                edges[component]
                    .iter()
                    .filter_map(|&(owner, ordinal, _, to, grows)| {
                        (grows && component_of[numbers[&to]] == component)
                            .then_some((owner, ordinal))
                    })
                    .collect()
            };
            growing.extend(verdict.iter().copied());
            retained.insert(key, verdict);
        }
        self.components = retained;
        self.growing = growing;
    }
}
