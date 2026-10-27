//! Demand-directed lineage checks consume the exact original logical path.
//! A projection cannot inherit the provenance of its whole storage root.
use crate::ir::*;
use crate::sealed_payload_source::{PayloadProducer, PayloadProjection};
use std::collections::{HashMap, VecDeque};

pub(super) struct Projections<'a, S> {
    unit: &'a LirUnit<S>,
    definitions: Vec<Option<&'a Inst>>,
    incoming: Vec<Vec<ValueId>>,
    concrete: Vec<bool>,
}
impl<'a, S> Projections<'a, S> {
    pub(super) fn new(unit: &'a LirUnit<S>) -> Self {
        let mut definitions = vec![None; unit.values.len()];
        let mut concrete = vec![false; unit.values.len()];
        for &value in &unit.parameters {
            concrete[value.index()] = true;
        }
        for call in &unit.tables.calls {
            if let Some(value) = call.result {
                concrete[value.index()] = true;
            }
        }
        for inst in &unit.instructions {
            for value in inst.defs() {
                definitions[value.index()] = Some(inst);
                concrete[value.index()] = matches!(
                    inst,
                    Inst::Const { .. }
                        | Inst::Construct { .. }
                        | Inst::Reuse { .. }
                        | Inst::Closure { .. }
                );
            }
        }
        let mut incoming = vec![vec![]; unit.values.len()];
        for edge in &unit.tables.edges {
            for (&to, &from) in unit.blocks[edge.target.index()]
                .parameters
                .get(&unit.pools.values)
                .unwrap_or(&[])
                .iter()
                .zip(edge.arguments.get(&unit.pools.values).unwrap_or(&[]))
            {
                incoming[to.index()].push(from);
            }
        }
        Self {
            unit,
            definitions,
            incoming,
            concrete,
        }
    }

    fn read_dependencies(
        &self,
        mut address: AddressId,
        reaching: ValueId,
        expected: &PayloadProducer,
    ) -> Option<Vec<(ValueId, PayloadProducer)>> {
        let mut ranges = vec![];
        let mut root = reaching;
        for _ in 0..=self.unit.addresses.len() {
            let row = &self.unit.addresses[address.index()];
            ranges.push(row.projection.get(&self.unit.pools.projections)?);
            match row.base {
                AddressBase::Address(parent) => address = parent,
                AddressBase::Slot(_) => break,
                AddressBase::Value(value) => {
                    root = value;
                    break;
                }
            }
            if ranges.len() > self.unit.addresses.len() {
                return None;
            }
        }
        let path: Vec<_> = ranges.into_iter().rev().flatten().collect();
        if path.is_empty() {
            return Some(vec![(root, expected.clone())]);
        }
        let PayloadProducer::Projected {
            root: original,
            path: wanted,
        } = expected
        else {
            return None;
        };
        let prefix = wanted.len().checked_sub(path.len())?;
        let mut dependencies = vec![];
        for (step, wanted) in path.iter().zip(&wanted[prefix..]) {
            match (step, wanted) {
                (ProjectionStep::Field(a), PayloadProjection::Field(b)) if a == b => (),
                (ProjectionStep::Payload, PayloadProjection::Field(0)) => (),
                (
                    ProjectionStep::VariantField {
                        variant: a,
                        field: b,
                    },
                    PayloadProjection::VariantField {
                        variant: c,
                        field: d,
                    },
                ) if a == c && b == d => (),
                (ProjectionStep::Index(index), PayloadProjection::Index(original)) => {
                    dependencies.push((*index, *original.clone()));
                }
                _ => return None,
            }
        }
        dependencies.push((
            root,
            if prefix == 0 {
                *original.clone()
            } else {
                PayloadProducer::Projected {
                    root: original.clone(),
                    path: wanted[..prefix].to_vec(),
                }
            },
        ));
        Some(dependencies)
    }

    pub(super) fn matches(
        &self,
        value: ValueId,
        expected: &PayloadProducer,
        whole: &[Vec<PayloadProducer>],
    ) -> bool {
        if !matches!(expected, PayloadProducer::Projected { .. }) {
            return whole[value.index()].as_slice() == [expected.clone()];
        }
        let initial = (value, expected.clone());
        let mut ids = HashMap::from([(initial.clone(), 0)]);
        let mut queries = vec![initial];
        let mut predecessors: Vec<Vec<usize>> = vec![vec![]];
        let mut seeds = VecDeque::new();
        let mut next = 0;
        while next < queries.len() {
            let (value, expected) = &queries[next];
            if self.concrete[value.index()] && whole[value.index()].as_slice() == [expected.clone()]
            {
                seeds.push_back(next);
            } else {
                let incoming = &self.incoming[value.index()];
                let dependencies = if !incoming.is_empty() {
                    Some(incoming.iter().map(|&v| (v, expected.clone())).collect())
                } else {
                    match self.definitions[value.index()] {
                        Some(
                            Inst::CopyValue { value, .. }
                            | Inst::Weaken { value, .. }
                            | Inst::Recast { value, .. },
                        ) => Some(vec![(*value, expected.clone())]),
                        Some(
                            Inst::Read {
                                reaching, address, ..
                            }
                            | Inst::Copy {
                                reaching, address, ..
                            },
                        ) => self.read_dependencies(*address, *reaching, expected),
                        Some(
                            Inst::Store { value, address, .. }
                            | Inst::Replace { value, address, .. },
                        ) if super::identity_address(self.unit, *address) => {
                            Some(vec![(*value, expected.clone())])
                        }
                        _ => None,
                    }
                };
                let Some(dependencies) = dependencies else {
                    return false;
                };
                if dependencies.is_empty() {
                    return false;
                }
                for query in dependencies {
                    let child = match ids.get(&query) {
                        Some(&id) => id,
                        None => {
                            let id = queries.len();
                            ids.insert(query.clone(), id);
                            queries.push(query);
                            predecessors.push(vec![]);
                            id
                        }
                    };
                    predecessors[child].push(next);
                }
            }
            next += 1;
        }
        let mut grounded = vec![false; queries.len()];
        while let Some(query) = seeds.pop_front() {
            if !std::mem::replace(&mut grounded[query], true) {
                seeds.extend(predecessors[query].iter().copied());
            }
        }
        grounded.into_iter().all(|grounded| grounded)
    }
}
