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

pub struct SettledArenaGraph {
    /// Components in deterministic callee-first order in each stratum.
    pub candidates: Vec<Arc<ArenaSummary>>,
    pub chosen: Vec<Arc<ArenaSummary>>,
}

fn function(address: &InstanceAddress) -> Function {
    (
        address.instance.definition().clone(),
        address.instance.digest(),
    )
}

/// Discover components from descriptors, then ask dependency-keyed component
/// queries. A hit reads only settled descriptor outputs. Each Chosen query
/// consumes its own Candidates and the outgoing Chosen products; it never
/// selects a replacement for a candidate removed by the second deletion pass.
pub fn settle_arena_queries(
    store: &dyn ResultStore,
    nodes: &[ArenaNode],
    external: &[(InstanceAddress, Vec<u32>, ProductRef)],
    facet: u32,
    counts: &mut ArenaQueryCounts,
) -> Result<SettledArenaGraph, String> {
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
    let externals: BTreeMap<_, _> = external.iter().map(|row| (function(&row.0), row)).collect();
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
    let mut candidates: Vec<Arc<ArenaSummary>> = Vec::new();
    let mut chosen = Vec::new();
    for stratum in [ArenaStratum::Candidates, ArenaStratum::Chosen] {
        let mut settled: BTreeMap<Function, (InstanceAddress, Vec<u32>, ProductRef)> = external
            .iter()
            .map(|row| (function(&row.0), row.clone()))
            .collect();
        for (component_index, component) in components.iter().enumerate() {
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
                    settled
                        .get(f)
                        .cloned()
                        .ok_or_else(|| "unsettled outgoing arena discovery dependency".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            outgoing.sort_by(|a, b| a.0.canonical_cmp(&b.0));
            let own =
                (stratum == ArenaStratum::Chosen).then(|| candidates[component_index].as_ref());
            let result = arena_query(store, &key, &members, own, &outgoing, counts)?;
            for (owner, xs) in &result.candidates {
                settled.insert(function(owner), (owner.clone(), xs.clone(), result.product));
            }
            match stratum {
                ArenaStratum::Candidates => candidates.push(result),
                ArenaStratum::Chosen => chosen.push(result),
            }
        }
    }
    Ok(SettledArenaGraph { candidates, chosen })
}
