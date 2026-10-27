//! Same-snapshot correspondence, independently recovered from SSA producers.
//! Type, source recipe, dominance and holder custody are checked separately.
//! Joins compare pairs on each edge; two equal sets of possible origins do not
//! prove that an ordinary value and receipt came from the same branch.
use crate::ir::*;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Copy)]
enum Origin {
    Unknown,
    Alias(ValueId),
    Receipt(ValueId),
    Parameter(BlockId, usize),
}

pub(in crate::validate) struct Pairing<'a, S> {
    unit: &'a LirUnit<S>,
    origins: Vec<Origin>,
    incoming: Vec<Vec<EdgeId>>,
}

impl<'a, S> Pairing<'a, S> {
    /// Called only after the common reference-bounds check. These facts never
    /// turn an unknown entry/capture/call receipt into a proved producer.
    pub(in crate::validate) fn new(unit: &'a LirUnit<S>) -> Self {
        let mut origins = vec![Origin::Unknown; unit.values.len()];
        // The boundary checker has bound these selectors and complete profiles
        // to independent source. Versions name the incoming slot snapshot.
        if let Some(entry) = &unit.tables.receipt_entry {
            for binding in &entry.parameters {
                if let Some(&ordinary) = unit.parameters.get(binding.source as usize) {
                    origins[binding.version.index()] = Origin::Receipt(ordinary);
                }
            }
        }
        for (block, row) in unit.blocks.iter().enumerate() {
            for (ordinal, value) in row
                .parameters
                .get(&unit.pools.values)
                .unwrap_or(&[])
                .iter()
                .enumerate()
            {
                origins[value.index()] = Origin::Parameter(BlockId(block as u32), ordinal);
            }
        }
        for instruction in &unit.instructions {
            let (result, origin) = match *instruction {
                Inst::PayloadReceipt { result, value, .. } => (result, Origin::Receipt(value)),
                Inst::CopyValue { result, value } | Inst::Weaken { result, value } => {
                    (result, Origin::Alias(value))
                }
                Inst::Read {
                    result,
                    reaching,
                    address,
                    ..
                }
                | Inst::Copy {
                    result,
                    reaching,
                    address,
                    ..
                } if super::identity_address(unit, address) => (result, Origin::Alias(reaching)),
                Inst::Store {
                    version,
                    value,
                    address,
                }
                | Inst::Replace {
                    version,
                    value,
                    address,
                    ..
                } if super::identity_address(unit, address) => (version, Origin::Alias(value)),
                // Deep copies, projections, recasts and mutations need their
                // own independently checked semantic producer, not an alias.
                _ => continue,
            };
            origins[result.index()] = origin;
        }
        for call in &unit.tables.calls {
            if let Some(receipts) = &call.receipts {
                if let (Some(value), Some(receipt)) = (call.result, receipts.result) {
                    origins[receipt.index()] = Origin::Receipt(value);
                }
                for argument in &receipts.arguments {
                    if let ReceiptArgument::Mut {
                        parameter, version, ..
                    } = *argument
                        && let Some(ArgumentValue::Address {
                            version: ordinary, ..
                        }) = call
                            .arguments
                            .get(&unit.pools.call_arguments)
                            .unwrap_or(&[])
                            .iter()
                            .find(|a| a.parameter == parameter)
                            .map(|a| a.value)
                    {
                        origins[version.index()] = Origin::Receipt(ordinary);
                    }
                }
            }
        }
        let mut incoming = vec![vec![]; unit.blocks.len()];
        for (id, edge) in unit.tables.edges.iter().enumerate() {
            incoming[edge.target.index()].push(EdgeId(id as u32));
        }
        Self {
            unit,
            origins,
            incoming,
        }
    }

    fn canonical(&self, mut value: ValueId) -> Option<ValueId> {
        for _ in 0..=self.origins.len() {
            match self.origins[value.index()] {
                Origin::Alias(parent) => value = parent,
                _ => return Some(value),
            }
        }
        None
    }

    /// A heap seal borrows the companion still held in its original Take entry
    /// slot. Resolve only whole-value aliases, matching the physical carrier.
    pub(in crate::validate) fn seal_entry(&self, root: ValueId) -> Option<&'a ReceiptEntrySlot> {
        let original = self.canonical(root)?;
        self.unit
            .tables
            .receipt_entry
            .as_ref()?
            .parameters
            .iter()
            .find(|binding| self.unit.parameters.get(binding.source as usize) == Some(&original))
    }

    fn argument(&self, edge: EdgeId, ordinal: usize) -> ValueId {
        self.unit.tables.edges[edge.index()]
            .arguments
            .get(&self.unit.pools.values)
            .expect("checked edge arguments")[ordinal]
    }

    fn dependencies(&self, value: ValueId, receipt: ValueId) -> Option<Vec<(ValueId, ValueId)>> {
        match (self.origins[value.index()], self.origins[receipt.index()]) {
            (Origin::Parameter(a, i), Origin::Parameter(b, j)) if a == b => Some(
                self.incoming[a.index()]
                    .iter()
                    .map(|&edge| (self.argument(edge, i), self.argument(edge, j)))
                    .collect(),
            ),
            (Origin::Parameter(block, i), _) => Some(
                self.incoming[block.index()]
                    .iter()
                    .map(|&edge| (self.argument(edge, i), receipt))
                    .collect(),
            ),
            (_, Origin::Parameter(block, i)) => Some(
                self.incoming[block.index()]
                    .iter()
                    .map(|&edge| (value, self.argument(edge, i)))
                    .collect(),
            ),
            _ => None,
        }
    }

    pub(in crate::validate) fn paired(&self, value: ValueId, receipt: ValueId) -> bool {
        // Build just the demanded relational obligations, following the same
        // iterative worklist pattern as the other SSA validator facts. No native
        // recursion and no all-values Cartesian product are needed.
        let mut ids = BTreeMap::from([((value, receipt), 0)]);
        let mut pairs = vec![(value, receipt)];
        let mut predecessors: Vec<Vec<usize>> = vec![vec![]];
        let mut seeds = Vec::new();
        let mut next = 0;
        while next < pairs.len() {
            let (value, receipt) = pairs[next];
            let (Some(value), Some(receipt)) = (self.canonical(value), self.canonical(receipt))
            else {
                return false;
            };
            if let Origin::Receipt(original) = self.origins[receipt.index()]
                && self.canonical(original) == Some(value)
            {
                seeds.push(next);
            } else {
                let Some(required) = self.dependencies(value, receipt) else {
                    return false;
                };
                if required.is_empty() {
                    return false;
                }
                for pair in required {
                    let child = match ids.get(&pair) {
                        Some(&id) => id,
                        None => {
                            let id = pairs.len();
                            ids.insert(pair, id);
                            pairs.push(pair);
                            predecessors.push(vec![]);
                            id
                        }
                    };
                    predecessors[child].push(next);
                }
            }
            next += 1;
        }
        // Every reachable obligation is conjunctive. A loop may preserve a
        // pair proved on entry, but a self-supporting cycle without any actual
        // producer is not evidence. Reverse reachability from concrete seeds
        // requires every demanded node to have that grounding.
        let mut grounded = vec![false; pairs.len()];
        let mut queue: VecDeque<_> = seeds.into();
        while let Some(node) = queue.pop_front() {
            if std::mem::replace(&mut grounded[node], true) {
                continue;
            }
            queue.extend(predecessors[node].iter().copied());
        }
        grounded.into_iter().all(|known| known)
    }
}

#[cfg(test)]
mod tests;
