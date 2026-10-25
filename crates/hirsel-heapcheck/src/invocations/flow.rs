//! H's symbolic invocation audit. An optional cleanup token is a predicate on
//! a private boolean cell. Joins retain that predicate rather than enumerate
//! combinations of entered ancestors. This code never calls C's scope walk.
use super::{Identity, cleanup};
use crate::violation::{Checked, ViolationKind as V, err};
use hirsel_lir::{LayoutTable, ir::*, stage::Stage};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum Key {
    Ordinary(Identity),
    Resource(SlotId, Vec<ProjectionStep<u64>>),
}
impl Key {
    fn below(&self, parent: &Self) -> bool {
        matches!((self, parent), (Self::Resource(child, path), Self::Resource(root, prefix))
            if child == root && path.len() > prefix.len() && path.starts_with(prefix))
    }
}
fn bad(message: &str) -> Box<crate::Violation> {
    err(V::FrameImbalance, message)
}

pub(super) struct Model {
    keys: BTreeMap<Identity, Key>,
    gates: BTreeMap<Identity, SlotId>,
    addresses: BTreeMap<AddressId, SlotId>,
    constants: BTreeMap<ValueId, bool>,
}
impl Model {
    pub fn new<S: Stage>(
        unit: &LirUnit<S>,
        targets: &[cleanup::Target],
        layouts: &LayoutTable,
    ) -> Checked<Self> {
        let constants: BTreeMap<_, _> = unit
            .instructions
            .iter()
            .filter_map(|inst| {
                let Inst::Const { result, constant } = inst else {
                    return None;
                };
                match unit.tables.constants.get(constant.index())?.value {
                    ConstantValue::Bool(value) => Some((*result, value)),
                    _ => None,
                }
            })
            .collect();
        let numbers: BTreeMap<_, _> = unit
            .instructions
            .iter()
            .filter_map(|inst| {
                let Inst::Const { result, constant } = inst else {
                    return None;
                };
                match unit.tables.constants.get(constant.index())?.value {
                    ConstantValue::Integer(value) => {
                        u64::try_from(value).ok().map(|value| (*result, value))
                    }
                    _ => None,
                }
            })
            .collect();
        let mut keys = BTreeMap::new();
        for target in targets {
            let address = unit
                .addresses
                .get(target.address.index())
                .ok_or_else(|| bad("cleanup address missing"))?;
            let AddressBase::Slot(slot) = address.base else {
                return Err(bad("cleanup must use slot storage"));
            };
            let mut layout = unit.types[unit.slots[slot.index()].ty.index()].layout;
            let mut path = Vec::new();
            for step in address
                .projection
                .get(&unit.pools.projections)
                .ok_or_else(|| bad("cleanup projection missing"))?
            {
                let record = layouts
                    .layout(layout)
                    .ok_or_else(|| bad("cleanup path layout missing"))?;
                match *step {
                    ProjectionStep::Field(ordinal) => {
                        let field = record
                            .fields
                            .iter()
                            .find(|field| field.ordinal == ordinal)
                            .ok_or_else(|| bad("cleanup field missing"))?;
                        if hirsel_lir::layout::field_is_borrowed(layouts, record, field) {
                            return Err(bad("cleanup descendant is borrowed"));
                        }
                        path.push(ProjectionStep::Field(ordinal));
                        layout = field.layout;
                    }
                    ProjectionStep::VariantField { variant, field } => {
                        let child = record
                            .tag
                            .as_ref()
                            .and_then(|tag| tag.variants.iter().find(|arm| arm.ordinal == variant))
                            .and_then(|arm| arm.fields.iter().find(|child| child.ordinal == field))
                            .ok_or_else(|| bad("cleanup variant field missing"))?;
                        path.push(ProjectionStep::VariantField { variant, field });
                        layout = child.layout;
                    }
                    ProjectionStep::Index(index) => {
                        let hirsel_lir::layout_types::LayoutKind::Inline { element, count } =
                            record.kind
                        else {
                            return Err(bad("cleanup crosses non-inline storage"));
                        };
                        let index = numbers
                            .get(&index)
                            .copied()
                            .filter(|index| *index < count)
                            .ok_or_else(|| bad("cleanup inline index is not constant"))?;
                        path.push(ProjectionStep::Index(index));
                        layout = element;
                    }
                    _ => return Err(bad("cleanup crosses non-owned storage")),
                }
            }
            let reference = &target.reference;
            keys.insert(
                (reference.body, reference.block, reference.statement),
                Key::Resource(slot, path),
            );
        }
        let mut addresses: BTreeMap<_, _> = unit
            .addresses
            .iter()
            .enumerate()
            .filter_map(|(i, address)| {
                let AddressBase::Slot(slot) = address.base else {
                    return None;
                };
                let info = &unit.slots[slot.index()];
                (address.projection.len == 0
                    && info.kind == SlotKind::Temporary
                    && unit.types[info.ty.index()].repr
                        == ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::Bool))
                .then_some((AddressId(i as u32), slot))
            })
            .collect();
        let mut excluded = BTreeSet::new();
        for address in &unit.addresses {
            if let AddressBase::Address(base) = address.base
                && let Some(slot) = addresses.get(&base)
            {
                excluded.insert(*slot);
            }
            if let AddressBase::Slot(slot) = address.base
                && address.projection.len != 0
            {
                excluded.insert(slot);
            }
        }
        for inst in &unit.instructions {
            if !matches!(
                inst,
                Inst::Address { .. }
                    | Inst::Store { .. }
                    | Inst::Read {
                        mode: ReadMode::Bits | ReadMode::Take,
                        ..
                    }
            ) {
                inst.visit(&mut |reference| {
                    if let Ref::Address(address) = reference
                        && let Some(slot) = addresses.get(&address)
                    {
                        excluded.insert(*slot);
                    }
                });
            }
        }
        for argument in &unit.pools.call_arguments {
            if let ArgumentValue::Address { address, .. } = argument.value
                && let Some(slot) = addresses.get(&address)
            {
                excluded.insert(*slot);
            }
        }
        for loan in &unit.tables.loans {
            if let Some(slot) = addresses.get(&loan.address) {
                excluded.insert(*slot);
            }
        }
        for address in &unit.pools.addresses {
            if let Some(slot) = addresses.get(address) {
                excluded.insert(*slot);
            }
        }
        addresses.retain(|_, slot| !excluded.contains(slot));
        let mut gates = BTreeMap::new();
        for block in &unit.blocks {
            let code = block
                .instructions
                .get(&unit.instructions)
                .ok_or_else(|| bad("guard block missing"))?;
            for (i, inst) in code.iter().enumerate() {
                let Inst::FrameEnter { failure } = inst else {
                    continue;
                };
                let origin = unit
                    .tables
                    .source_anchors
                    .get(unit.tables.failures[failure.index()].anchor.index())
                    .ok_or_else(|| bad("guard source missing"))?;
                let id = (origin.body, origin.block, origin.statement);
                if !keys.contains_key(&id) {
                    continue;
                }
                let store = |inst: &Inst| match inst {
                    Inst::Store { address, value, .. } => addresses
                        .get(address)
                        .zip(constants.get(value))
                        .map(|(&slot, &value)| (slot, value)),
                    _ => None,
                };
                if let (Some((slot, false)), Some((same, true))) = (
                    code[..i].iter().rev().find_map(store),
                    code[i + 1..].iter().find_map(store),
                ) && slot == same
                {
                    gates.insert(id, slot);
                }
            }
        }
        let used: BTreeSet<_> = gates.values().copied().collect();
        addresses.retain(|_, slot| used.contains(slot));
        Ok(Self {
            keys,
            gates,
            addresses,
            constants,
        })
    }
    pub fn key(&self, id: Identity) -> Key {
        self.keys.get(&id).cloned().unwrap_or(Key::Ordinary(id))
    }
    pub fn enter(&self, state: &mut State, id: Identity) -> Checked<()> {
        let key = self.key(id);
        if state.scopes.iter().any(|scope| !key.below(&scope.key)) {
            return Err(bad(
                "nested invocation is not an owned descendant finalizer",
            ));
        }
        state.scopes.push(Scope {
            key,
            gate: self.gates.get(&id).copied(),
            conditional: false,
            called: false,
        });
        Ok(())
    }
    pub fn update(&self, state: &mut State, inst: &Inst) -> Checked<()> {
        let write = match inst {
            Inst::Store { address, value, .. } => self
                .addresses
                .get(address)
                .map(|slot| (*slot, self.constants.get(value).copied())),
            Inst::Read {
                address,
                mode: ReadMode::Take,
                ..
            } => self.addresses.get(address).map(|slot| (*slot, None)),
            _ => None,
        };
        if let Some((slot, value)) = write {
            if state
                .scopes
                .iter()
                .any(|scope| scope.conditional && scope.gate == Some(slot))
            {
                return Err(bad("unresolved cleanup guard was overwritten"));
            }
            if let Some(value) = value {
                state.values.insert(slot, value);
            } else {
                state.values.remove(&slot);
            }
        }
        if let Inst::StorageLive { slot } | Inst::StorageDead { slot, .. } = inst {
            if state.scopes.iter().any(|scope| scope.gate == Some(*slot)) {
                return Err(bad("live cleanup guard storage ended"));
            }
            state.values.remove(slot);
        }
        Ok(())
    }
    pub fn split<S: Stage>(
        &self,
        unit: &LirUnit<S>,
        block: BlockId,
        state: &State,
    ) -> Checked<Option<Vec<(EdgeId, State)>>> {
        let Terminator::Branch { condition, yes, no } = unit.terminators[block.index()] else {
            return Ok(None);
        };
        let code = unit.blocks[block.index()]
            .instructions
            .get(&unit.instructions)
            .ok_or_else(|| bad("guard branch missing"))?;
        let Some(Inst::Read {
            result,
            address,
            mode: ReadMode::Bits,
            ..
        }) = code.last()
        else {
            return Ok(None);
        };
        let Some(&slot) = self.addresses.get(address).filter(|_| *result == condition) else {
            return Ok(None);
        };
        let mut paths = Vec::new();
        for (edge, truth) in [(yes, true), (no, false)] {
            if state.values.get(&slot).is_some_and(|known| *known != truth) {
                continue;
            }
            let mut next = state.clone();
            next.values.insert(slot, truth);
            for i in (0..next.scopes.len()).rev() {
                if next.scopes[i].gate != Some(slot) {
                    continue;
                }
                if !next.scopes[i].conditional && !truth {
                    return Err(bad("false guard hides an entered cleanup"));
                }
                if truth {
                    next.scopes[i].conditional = false;
                } else {
                    next.scopes.remove(i);
                }
            }
            paths.push((edge, next));
        }
        Ok(Some(paths))
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Scope {
    key: Key,
    gate: Option<SlotId>,
    conditional: bool,
    called: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct State {
    scopes: Vec<Scope>,
    values: BTreeMap<SlotId, bool>,
}
impl State {
    pub fn empty(&self) -> bool {
        self.scopes.is_empty()
    }
    pub fn current(&self) -> Option<&Key> {
        self.scopes
            .last()
            .filter(|scope| !scope.conditional)
            .map(|scope| &scope.key)
    }
    pub fn dispatch(&mut self) -> Checked<()> {
        let scope = self
            .scopes
            .last_mut()
            .filter(|scope| !scope.conditional && !scope.called)
            .ok_or_else(|| bad("callback has no distinct admission"))?;
        scope.called = true;
        Ok(())
    }
    pub fn leave(&mut self, require_call: bool) -> Checked<()> {
        let scope = self
            .scopes
            .pop()
            .filter(|scope| !scope.conditional)
            .ok_or_else(|| bad("retirement lacks admitted invocation"))?;
        if require_call && !scope.called {
            return Err(bad("callback retirement precedes dispatch"));
        }
        Ok(())
    }
    pub fn merge(&mut self, next: &Self) -> Checked<bool> {
        let mut keys: BTreeSet<_> = self
            .scopes
            .iter()
            .chain(&next.scopes)
            .map(|scope| scope.key.clone())
            .collect();
        let mut scopes = Vec::new();
        while let Some(key) = keys.pop_first() {
            let left = self.scopes.iter().find(|scope| scope.key == key);
            let right = next.scopes.iter().find(|scope| scope.key == key);
            let mut scope = left.or(right).unwrap().clone();
            match (left, right) {
                (Some(a), Some(b)) if a == b => (),
                (Some(a), Some(b)) if a.gate == b.gate && a.called == b.called => {
                    let gate = a
                        .gate
                        .ok_or_else(|| bad("ordinary invocation scopes disagree"))?;
                    if (!a.conditional && self.values.get(&gate) != Some(&true))
                        || (!b.conditional && next.values.get(&gate) != Some(&true))
                    {
                        return Err(bad("cleanup guard disagrees with scope"));
                    }
                    scope.conditional = a.conditional || b.conditional;
                }
                (Some(a), None) | (None, Some(a)) => {
                    let gate = a
                        .gate
                        .ok_or_else(|| bad("invocation scopes disagree at control-flow join"))?;
                    let (present, absent) = if left.is_some() {
                        (&*self, next)
                    } else {
                        (next, &*self)
                    };
                    if absent.values.get(&gate) != Some(&false)
                        || (!a.conditional && present.values.get(&gate) != Some(&true))
                    {
                        return Err(bad("joined cleanup lacks exact admission guard"));
                    }
                    scope.conditional = true;
                }
                _ => return Err(bad("invocation scopes disagree at control-flow join")),
            }
            scopes.push(scope);
        }
        // Resource paths order ancestors before children. An ordinary scope
        // can never share a stack with another scope.
        if scopes
            .windows(2)
            .any(|pair| !pair[1].key.below(&pair[0].key))
        {
            return Err(bad("joined scopes are not owned descendants"));
        }
        let values = self
            .values
            .iter()
            .filter(|(slot, truth)| next.values.get(slot) == Some(truth))
            .map(|(&slot, &truth)| (slot, truth))
            .collect();
        let merged = Self { scopes, values };
        let changed = *self != merged;
        *self = merged;
        Ok(changed)
    }
}
