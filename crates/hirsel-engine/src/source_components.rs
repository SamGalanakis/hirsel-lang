//! Source-only call SCCs and independently addressed settled GEN-5 facets.
//! Membership uses hirsel-graph's iterative Tarjan walk, also used by the
//! scalar source oracle. Native instance/layout graphs never select roots.
use crate::store::{Action, Address, Domain, Reference, ResultStore};
use crate::{CheckedSummary, NativeRecordContext, NativeRecords, memo::Reads, runtime::ProductRef};
use hirsel_span::Span;
use hirsel_syntax::{ContentHash, DefId};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const FACT: Domain = Domain {
    kind: 0x4600_0020,
    schema: 1,
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

/// Compact accepted call/growth facts. Its output excludes unrelated body details.
pub struct SourceGrowthFact {
    owner: DefId,
    calls: Box<[DefId]>,
    summary: Arc<CheckedSummary>,
    product: ProductRef,
}

impl SourceGrowthFact {
    pub fn product(&self) -> ProductRef {
        self.product
    }

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
        let bytes = self.encode(&(owner, &calls, &summary.growth))?;
        let observation = self.encode(&(selected, reads.digest()))?;
        let address = Address(ContentHash::of_parts(
            "hirsel.source-growth.fact.address.v1",
            [self.namespace.0.0.as_slice(), definition.0.0.as_slice()],
        ));
        let action = Action(ContentHash::of_parts(
            "hirsel.source-growth.fact.action.v1",
            [
                address.0.0.as_slice(),
                observation.as_slice(),
                bytes.as_slice(),
            ],
        ));
        let product = self.publish(address, action, FACT, &bytes)?;
        Some(SourceGrowthFact {
            owner,
            calls,
            summary,
            product,
        })
    }

    /// Settle every source call SCC, observing member fact outputs.
    pub fn settle(&self, facts: &[SourceGrowthFact]) -> Option<SourceGrowthFacets> {
        // Account the temporary maps, adjacency lists, Tarjan frames, facts,
        // edge copies and canonical encoding before allocating the graph.
        let count = facts.iter().try_fold(facts.len() as u64, |count, fact| {
            count
                .checked_add(fact.calls.len() as u64)?
                .checked_add(fact.summary.growth.edges.len() as u64)
        })?;
        if count.checked_mul(512)? > self.scratch {
            return None;
        }
        let mut facts: Vec<_> = facts.iter().collect();
        facts.sort_by_key(|fact| (fact.owner, fact.product.address.0.0));
        let nodes: Vec<_> = facts
            .iter()
            .map(|fact| fact.owner)
            .chain(facts.iter().flat_map(|fact| fact.calls.iter().copied()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
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
                .map(|fact| fact.product.record)
                .collect();
            let action = Action(ContentHash::of_parts(
                "hirsel.source-growth.component.action.v1",
                [
                    member_bytes.as_slice(),
                    self.encode(&observations)?.as_slice(),
                ],
            ));
            let cached = self
                .store
                .lookup_product(address)
                .filter(|hit| {
                    hit.action == action
                        && hit.reference.domain == SETTLED
                        && hit.reference.len <= self.scratch
                })
                .and_then(|hit| {
                    let bytes = self.store.get_content(hit.reference)?;
                    let (selected, growing): (Vec<DefId>, Box<[Span]>) =
                        self.records.decode(self.source, &bytes)?;
                    (selected == members).then_some((hit.reference, growing))
                });
            let (record, growing) = if let Some(cached) = cached {
                cached
            } else {
                let selected: BTreeSet<_> = members.iter().copied().collect();
                let edges: Vec<_> = members
                    .iter()
                    .filter_map(|id| by_owner.get(id))
                    .flatten()
                    .flat_map(|fact| fact.summary.growth.edges.iter())
                    .filter(|edge| selected.contains(&edge.1.0))
                    .copied()
                    .collect();
                let growing: Box<[_]> = crate::growing_sites(&edges).into();
                let bytes = self.encode(&(&members, &growing))?;
                let record = self.store.put_content(SETTLED, &bytes)?;
                self.store.associate_product(address, action, record);
                (record, growing)
            };
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

#[cfg(test)]
mod tests;
