//! Reconstruct optional cleanup activations from actual private bool stores.
//! A guard is admitted only for slot-local bits that never escape. Joins prove
//! it is true exactly on paths with that activation; branches refine that fact.
use super::cleanup::Target;
use crate::{ir::*, stage::Stage};
use std::collections::{BTreeMap, BTreeSet};

type Identity = (u32, u32, u32);

pub(super) struct Guards {
    slots: BTreeMap<AddressId, SlotId>,
    constants: BTreeMap<ValueId, bool>,
    entries: BTreeMap<Identity, SlotId>,
    canonical: BTreeMap<Identity, Identity>,
}
impl Guards {
    pub fn new<S: Stage>(unit: &LirUnit<S>, targets: &BTreeMap<Identity, &Target>) -> Self {
        let mut eligible: BTreeSet<_> = unit
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                (slot.kind == SlotKind::Temporary
                    && unit.types[slot.ty.index()].repr
                        == ValueRepr::Scalar(crate::layout_types::ScalarKind::Bool))
                .then_some(SlotId(i as u32))
            })
            .collect();
        let mut slots = BTreeMap::new();
        for (i, address) in unit.addresses.iter().enumerate() {
            if let AddressBase::Slot(slot) = address.base {
                if address.projection.len == 0 {
                    slots.insert(AddressId(i as u32), slot);
                } else {
                    eligible.remove(&slot);
                }
            }
        }
        for address in &unit.addresses {
            if let AddressBase::Address(base) = address.base
                && let Some(slot) = slots.get(&base)
            {
                eligible.remove(slot);
            }
        }
        let mut constants = BTreeMap::new();
        for inst in &unit.instructions {
            if let Inst::Const { result, constant } = *inst
                && let Some(ConstantData {
                    value: ConstantValue::Bool(value),
                    ..
                }) = unit.tables.constants.get(constant.index())
            {
                constants.insert(result, *value);
            }
            let allowed = matches!(
                inst,
                Inst::Address { .. }
                    | Inst::Read {
                        mode: ReadMode::Bits | ReadMode::Take,
                        ..
                    }
                    | Inst::Store { .. }
            );
            if !allowed {
                inst.visit(&mut |reference| {
                    if let Ref::Address(address) = reference
                        && let Some(slot) = slots.get(&address)
                    {
                        eligible.remove(slot);
                    }
                });
            }
        }
        // No call or side-table loan may mutate or retain a private guard.
        for argument in &unit.pools.call_arguments {
            if let ArgumentValue::Address { address, .. } = argument.value
                && let Some(slot) = slots.get(&address)
            {
                eligible.remove(slot);
            }
        }
        for loan in &unit.tables.loans {
            if let Some(slot) = slots.get(&loan.address) {
                eligible.remove(slot);
            }
        }
        for address in &unit.pools.addresses {
            if let Some(slot) = slots.get(address) {
                eligible.remove(slot);
            }
        }
        slots.retain(|_, slot| eligible.contains(slot));
        let mut entries = BTreeMap::new();
        for block in &unit.blocks {
            let Some(code) = block.instructions.get(&unit.instructions) else {
                continue;
            };
            for (i, inst) in code.iter().enumerate() {
                let Inst::FrameEnter { failure } = inst else {
                    continue;
                };
                let Some(failure) = unit.tables.failures.get(failure.index()) else {
                    continue;
                };
                let Some(anchor) = unit.tables.source_anchors.get(failure.anchor.index()) else {
                    continue;
                };
                let id = (anchor.body, anchor.block, anchor.statement);
                if !targets.contains_key(&id) {
                    continue;
                }
                let before = code[..i].iter().rev().find_map(|inst| match inst {
                    Inst::Store { address, value, .. } => slots
                        .get(address)
                        .zip(constants.get(value))
                        .map(|(&slot, &value)| (slot, value)),
                    _ => None,
                });
                let after = code[i + 1..].iter().find_map(|inst| match inst {
                    Inst::Store { address, value, .. } => slots
                        .get(address)
                        .zip(constants.get(value))
                        .map(|(&slot, &value)| (slot, value)),
                    _ => None,
                });
                if let (Some((left, false)), Some((right, true))) = (before, after)
                    && left == right
                {
                    entries.insert(id, left);
                }
            }
        }
        let mut representatives = BTreeMap::new();
        let used: BTreeSet<_> = entries.values().copied().collect();
        slots.retain(|_, slot| used.contains(slot));
        let canonical = targets
            .iter()
            .map(|(&id, target)| {
                let first = *representatives.entry(target.resource.clone()).or_insert(id);
                (id, first)
            })
            .collect();
        Self {
            slots,
            constants,
            entries,
            canonical,
        }
    }
    pub fn identity(&self, id: Identity) -> Identity {
        self.canonical.get(&id).copied().unwrap_or(id)
    }
    pub fn enter(&self, id: Identity) -> Frame {
        Frame {
            identity: self.identity(id),
            guard: self.entries.get(&id).copied(),
            optional: false,
            dispatched: false,
        }
    }
    pub fn branch<S: Stage>(&self, unit: &LirUnit<S>, block: BlockId) -> Option<SlotId> {
        let Terminator::Branch { condition, .. } = unit.terminators[block.index()] else {
            return None;
        };
        let code = unit.blocks[block.index()]
            .instructions
            .get(&unit.instructions)?;
        // Read the current guard just before branching. An older SSA snapshot
        // cannot justify retiring a token after a later guard write.
        match code.last()? {
            Inst::Read {
                result,
                address,
                mode: ReadMode::Bits,
                ..
            } if *result == condition => self.slots.get(address).copied(),
            _ => None,
        }
    }
    pub fn instruction(&self, state: &mut State, inst: &Inst) -> Result<(), String> {
        match inst {
            Inst::Read {
                address,
                mode: ReadMode::Take,
                ..
            } => {
                if let Some(&slot) = self.slots.get(address) {
                    if state
                        .active
                        .iter()
                        .any(|frame| frame.optional && frame.guard == Some(slot))
                    {
                        return Err("take of an unresolved cleanup admission guard".into());
                    }
                    state.flags.remove(&slot);
                }
            }
            Inst::Store { address, value, .. } => {
                if let Some(&slot) = self.slots.get(address) {
                    if state
                        .active
                        .iter()
                        .any(|frame| frame.optional && frame.guard == Some(slot))
                    {
                        return Err("write to an unresolved cleanup admission guard".into());
                    }
                    if let Some(&value) = self.constants.get(value) {
                        state.flags.insert(slot, value);
                    } else {
                        state.flags.remove(&slot);
                    }
                }
            }
            Inst::StorageLive { slot } | Inst::StorageDead { slot, .. } => {
                if state.active.iter().any(|frame| frame.guard == Some(*slot)) {
                    return Err("cleanup admission guard ends while activation is live".into());
                }
                state.flags.remove(slot);
            }
            _ => (),
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Frame {
    pub identity: Identity,
    guard: Option<SlotId>,
    optional: bool,
    pub dispatched: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct State {
    pub active: Vec<Frame>,
    flags: BTreeMap<SlotId, bool>,
}
impl State {
    pub fn current(&self) -> Option<(u32, u32, u32)> {
        self.active
            .last()
            .filter(|frame| !frame.optional)
            .map(|frame| frame.identity)
    }
    pub fn leave(&mut self) -> Result<Frame, String> {
        let frame = self
            .active
            .pop()
            .ok_or("invocation retirement without admission")?;
        if frame.optional {
            return Err("cleanup retirement lacks a proved admission guard".into());
        }
        Ok(frame)
    }
    pub fn refine(&self, slot: SlotId, value: bool) -> Result<Option<Self>, String> {
        if self.flags.get(&slot).is_some_and(|known| *known != value) {
            return Ok(None);
        }
        let mut out = self.clone();
        out.flags.insert(slot, value);
        for i in (0..out.active.len()).rev() {
            if out.active[i].guard != Some(slot) {
                continue;
            }
            if out.active[i].optional {
                if value {
                    out.active[i].optional = false;
                } else {
                    out.active.remove(i);
                }
            } else if !value {
                return Err("false guard hides a live cleanup invocation".into());
            }
        }
        Ok(Some(out))
    }
    pub fn join(
        &mut self,
        other: &Self,
        targets: &BTreeMap<Identity, &Target>,
    ) -> Result<bool, String> {
        let mut active = self.active.clone();
        for frame in &other.active {
            if active.iter().any(|old| old.identity == frame.identity) {
                continue;
            }
            let target = targets
                .get(&frame.identity)
                .ok_or("ordinary invocation identity differs at control-flow join")?;
            let i = active
                .iter()
                .position(|old| {
                    targets
                        .get(&old.identity)
                        .is_some_and(|old| old.descendant_of(target))
                })
                .unwrap_or(active.len());
            active.insert(i, frame.clone());
        }
        for pair in active.windows(2) {
            match (
                targets.get(&pair[0].identity),
                targets.get(&pair[1].identity),
            ) {
                (Some(parent), Some(child)) if child.descendant_of(parent) => (),
                _ => return Err("invocation identity differs at control-flow join".into()),
            }
        }
        for frame in &mut active {
            let left = self
                .active
                .iter()
                .find(|old| old.identity == frame.identity);
            let right = other
                .active
                .iter()
                .find(|old| old.identity == frame.identity);
            match (left, right) {
                (Some(left), Some(right)) if left == right => (),
                (Some(left), Some(right))
                    if left.guard == right.guard && left.dispatched == right.dispatched =>
                {
                    let slot = left
                        .guard
                        .ok_or("invocation identity differs at control-flow join")?;
                    if (!left.optional && self.flags.get(&slot) != Some(&true))
                        || (!right.optional && other.flags.get(&slot) != Some(&true))
                    {
                        return Err("cleanup guard differs from its admitted invocation".into());
                    }
                    frame.optional = left.optional || right.optional;
                }
                (Some(present), None) | (None, Some(present)) => {
                    let slot = present
                        .guard
                        .ok_or("invocation identity differs at control-flow join")?;
                    let (there, absent) = if left.is_some() {
                        (&*self, other)
                    } else {
                        (other, &*self)
                    };
                    if absent.flags.get(&slot) != Some(&false)
                        || (!present.optional && there.flags.get(&slot) != Some(&true))
                    {
                        return Err("cleanup guard differs from its admitted invocation".into());
                    }
                    frame.optional = true;
                }
                _ => return Err("invocation identity differs at control-flow join".into()),
            }
        }
        let flags = self
            .flags
            .iter()
            .filter(|(slot, value)| other.flags.get(slot) == Some(value))
            .map(|(&slot, &value)| (slot, value))
            .collect();
        let next = Self { active, flags };
        let changed = *self != next;
        *self = next;
        Ok(changed)
    }
}
