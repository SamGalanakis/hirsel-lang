//! Source-only call SCCs and independently addressed settled GEN-5 facets.
//! Membership uses hirsel-graph's iterative Tarjan walk, also used by the
//! scalar source oracle. Native instance/layout graphs never select roots.
use crate::store::{Action, Address, Domain, Reference, ResultStore};
use crate::{
    CheckedSummary, NativeRecordContext, NativeRecordSource, NativeRecords, memo::Reads,
    runtime::ProductRef,
};
use hirsel_span::Span;
use hirsel_syntax::{ContentHash, DefId};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const MEMBERSHIP: Domain = Domain {
    kind: 0x4600_0021,
    schema: 1,
};
const SETTLED: Domain = Domain {
    kind: 0x4600_0022,
    schema: 1,
};

/// Only the settled output is an observation for downstream consumers. The
/// action can change while this record remains equal.
#[derive(Clone, Debug)]
pub struct SourceGrowthComponent {
    pub product: ProductRef,
    pub members: Box<[DefId]>,
    pub growing: Box<[Span]>,
}

#[derive(Clone, Debug)]
pub struct SourceGrowthFacets {
    pub membership: ProductRef,
    pub components: Vec<SourceGrowthComponent>,
}

/// Accepted typed call/growth facts retained by the admitted source task.
/// Durable component publication encodes their complete observations together.
pub struct SourceGrowthFact {
    owner: DefId,
    calls: Box<[DefId]>,
    summary: Arc<CheckedSummary>,
    definition: Address,
    selected: Reference,
    observed: ContentHash,
}

impl SourceGrowthFact {
    /// Physical fact backing retained by an admitted source-growth task.
    pub fn input_bytes(&self) -> u64 {
        (std::mem::size_of::<Self>() + std::mem::size_of_val(&*self.calls)) as u64
    }
}

/// Bounded producer of source growth products. Supply the exact accepted source
/// context and an environment/root namespace. Every required root must contribute
/// a settled checked summary before `settle` is used as a program verdict.
pub struct SourceGrowthProducer<'a> {
    pub records: &'a NativeRecords,
    pub source: &'a NativeRecordContext,
    pub store: &'a dyn ResultStore,
    pub namespace: Address,
    pub scratch: u64,
}

impl SourceGrowthProducer<'_> {
    /// Called only with a completed checked-stage output, never with a
    /// provisional recursive summary. All observations remain with the root.
    pub fn fact(
        &self,
        definition: Address,
        summary: Arc<CheckedSummary>,
        reads: &Reads,
        selected: Reference,
    ) -> Option<SourceGrowthFact> {
        let read_bytes = reads.iter().try_fold(8u64, |bytes, (key, _)| {
            bytes
                .checked_add(key.encoded_bytes()? as u64)?
                .checked_add(32)
        })?;
        let indices = reads
            .len()
            .checked_mul(std::mem::size_of::<std::ops::Range<usize>>())?
            as u64;
        if read_bytes.checked_mul(2)?.checked_add(indices)? > self.scratch {
            return None;
        }
        self.fact_observed(
            definition,
            summary,
            selected,
            crate::store::payload_reads_hash(reads),
        )
    }

    /// P actions already bind the complete recorded reads through query_action;
    /// failed P records embed those reads. Their accepted witnesses avoid
    /// re-encoding a shared read universe for every source declaration.
    pub(crate) fn fact_observed(
        &self,
        definition: Address,
        summary: Arc<CheckedSummary>,
        selected: Reference,
        observed: ContentHash,
    ) -> Option<SourceGrowthFact> {
        let count = summary
            .summary
            .calls
            .len()
            .checked_add(summary.growth.edges.len())?
            .checked_add(1)?;
        if (count as u64).checked_mul(512)? > self.scratch {
            return None;
        }
        let owner = summary.summary.owner;
        let calls: Box<[_]> = summary
            .summary
            .calls
            .iter()
            .map(|call| call.callee)
            .chain(summary.growth.edges.iter().map(|edge| edge.1.0))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Some(SourceGrowthFact {
            owner,
            calls,
            summary,
            definition,
            selected,
            observed,
        })
    }

    /// Settle every source call SCC and publish only its coarse durable facet.
    pub fn settle(&self, facts: &[SourceGrowthFact]) -> Option<SourceGrowthFacets> {
        let mut facts: Vec<_> = facts.iter().collect();
        facts.sort_by_key(|fact| (fact.owner, fact.definition.0.0));
        let mut nodes = BTreeSet::new();
        for node in facts
            .iter()
            .map(|fact| fact.owner)
            .chain(facts.iter().flat_map(|fact| fact.calls.iter().copied()))
        {
            if !nodes.contains(&node) {
                let mut weight = crate::NativeMetadataWeight::default();
                weight.tree::<DefId, ()>(nodes.len().checked_add(1)?);
                weight.add(
                    facts
                        .len()
                        .checked_mul(std::mem::size_of::<&SourceGrowthFact>())?,
                );
                if weight.bytes() > self.scratch {
                    return None;
                }
                nodes.insert(node);
            }
        }
        let edges = facts
            .iter()
            .try_fold(0usize, |n, fact| n.checked_add(fact.calls.len()))?;
        if graph_bytes(nodes.len(), edges, facts.len())? > self.scratch {
            return None;
        }
        let nodes: Vec<_> = nodes.into_iter().collect();
        let numbers: BTreeMap<_, _> = nodes.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let mut graph = vec![vec![]; nodes.len()];
        for fact in &facts {
            graph[numbers[&fact.owner]].extend(fact.calls.iter().map(|id| numbers[id]));
        }
        // Tarjan emits dependency order. Persist path-sorted membership so
        // traversal and completion order cannot change addresses or outputs.
        let mut membership: Vec<Vec<DefId>> = hirsel_graph::components(&graph)
            .into_iter()
            .map(|members| members.into_iter().map(|i| nodes[i]).collect())
            .collect();
        membership.sort();
        drop(graph);
        drop(numbers);
        drop(nodes);
        let topology = self.encode(
            &facts
                .iter()
                .map(|fact| (fact.owner, &fact.calls))
                .collect::<Vec<_>>(),
        )?;
        let address = Address(ContentHash::of_parts(
            "hirsel.source-growth.membership.address.v1",
            [self.namespace.0.0.as_slice()],
        ));
        let action = Action(ContentHash::of_parts(
            "hirsel.source-growth.membership.action.v1",
            [topology.as_slice()],
        ));
        let membership_product =
            self.publish(address, action, MEMBERSHIP, &self.encode(&membership)?)?;
        drop(topology);
        let mut by_owner: BTreeMap<DefId, Vec<&SourceGrowthFact>> = BTreeMap::new();
        for fact in facts {
            by_owner.entry(fact.owner).or_default().push(fact);
        }
        let mut components = Vec::with_capacity(membership.len());
        for members in membership {
            let member_bytes = self.encode(&members)?;
            let address = Address(ContentHash::of_parts(
                "hirsel.source-growth.component.address.v1",
                [self.namespace.0.0.as_slice(), member_bytes.as_slice()],
            ));
            let observations: Vec<_> = members
                .iter()
                .filter_map(|id| by_owner.get(id))
                .flatten()
                .map(|fact| {
                    (
                        fact.definition.0,
                        fact.selected,
                        fact.observed,
                        fact.owner,
                        &fact.calls,
                        &fact.summary.growth,
                    )
                })
                .collect();
            let action = Action(ContentHash::of_parts(
                "hirsel.source-growth.component.action.v3",
                [
                    member_bytes.as_slice(),
                    self.encode(&observations)?.as_slice(),
                ],
            ));
            let selected: BTreeSet<_> = members.iter().copied().collect();
            let edges: Vec<_> = members
                .iter()
                .filter_map(|id| by_owner.get(id))
                .flatten()
                .flat_map(|fact| fact.summary.growth.edges.iter())
                .filter(|edge| selected.contains(&edge.1.0))
                .copied()
                .collect();
            // An ordinary store association supplies integrity, not source
            // validation authority. Settle current accepted equations before
            // publishing or backdating a verdict, including on a store hit.
            let mut growing = crate::growing_sites(&edges);
            growing.sort_by_cached_key(|site| {
                (self.source.record_owner(site.anchor), site.start, site.end)
            });
            let growing = growing.into_boxed_slice();
            let bytes = self.encode(&(&members, &growing))?;
            let record = self.publish(address, action, SETTLED, &bytes)?.record;
            components.push(SourceGrowthComponent {
                product: ProductRef {
                    address,
                    action,
                    record,
                },
                members: members.into(),
                growing,
            });
        }
        Some(SourceGrowthFacets {
            membership: membership_product,
            components,
        })
    }

    fn encode(&self, value: &impl serde::Serialize) -> Option<Vec<u8>> {
        let bytes = self.records.encode(self.source, value)?;
        (bytes.len() as u64 <= self.scratch).then_some(bytes)
    }

    fn publish(
        &self,
        address: Address,
        action: Action,
        domain: Domain,
        bytes: &[u8],
    ) -> Option<ProductRef> {
        let record = self.store.put_content(domain, bytes)?;
        self.store.associate_product(address, action, record);
        Some(ProductRef {
            address,
            action,
            record,
        })
    }
}

// Bounds the live containers of hirsel-graph's iterative Tarjan walk at
// lib.rs:170, including successor clones and Vec geometric spare capacity.
// Calls are edges, not separately allocated graph vertices. Equations stay
// in the front owner's existing reservation; graph backing is dropped before
// component solving and record encoding.
fn graph_bytes(nodes: usize, edges: usize, facts: usize) -> Option<u64> {
    let twice = nodes.checked_mul(2)?.checked_add(4)?;
    let members = nodes.checked_mul(6)?.checked_add(4)?;
    let adjacency = edges.checked_mul(2)?.checked_add(facts.checked_mul(4)?)?;
    let mut weight = crate::NativeMetadataWeight::default();
    weight.tree::<DefId, ()>(nodes);
    weight.tree::<DefId, usize>(nodes);
    weight.tree::<usize, (usize, usize)>(nodes);
    weight.tree::<usize, ()>(nodes);
    for (count, size) in [
        (nodes, std::mem::size_of::<DefId>()),
        (nodes, std::mem::size_of::<Vec<usize>>()),
        (adjacency, std::mem::size_of::<usize>()),
        (edges, std::mem::size_of::<usize>()),
        (facts, std::mem::size_of::<&SourceGrowthFact>()),
        (twice, std::mem::size_of::<(usize, Vec<usize>, usize)>()),
        (twice, std::mem::size_of::<usize>()),
        (nodes, std::mem::size_of::<usize>()),
        (twice, std::mem::size_of::<Vec<usize>>()),
        (members, std::mem::size_of::<usize>()),
        (nodes, std::mem::size_of::<Vec<DefId>>()),
        (
            nodes.checked_mul(8)?.checked_add(4)?,
            std::mem::size_of::<DefId>(),
        ),
    ] {
        weight.add(count.checked_mul(size)?);
    }
    Some(weight.bytes())
}

#[cfg(test)]
mod tests;
