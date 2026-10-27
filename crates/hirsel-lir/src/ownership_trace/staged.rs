//! Exact private staging of an original return across a region exit.
use super::*;

pub(super) struct Staging {
    pub address: AddressId,
    pub original: ValueId,
    pub returned: ValueId,
    pub store: Point,
    pub take: Point,
    pub returning: BlockId,
}
pub(super) fn binding<S: Stage>(unit: &LirUnit<S>, observation: &Observation) -> Option<Staging> {
    let Point::StagedReturn { block, address } = observation.point else {
        return None;
    };
    let ObserveOwnership {
        kind: Kind::Move,
        token: Location::Value(original),
        owner: Location::Return,
        ..
    } = observation.marker
    else {
        return None;
    };
    let info = unit.addresses.get(address.index())?;
    let AddressBase::Slot(slot) = info.base else {
        return None;
    };
    let slot_info = unit.slots.get(slot.index())?;
    if info.projection.len != 0 || slot_info.kind != SlotKind::Temporary {
        return None;
    }
    let Terminator::Return(Some(returned)) = *unit.terminators.get(block.index())? else {
        return None;
    };
    if original == returned
        || unit.values.get(original.index())?.holding != Holding::Owned
        || unit.values.get(returned.index())?.holding != Holding::Owned
        || unit.values.get(original.index())?.ty != slot_info.ty
        || unit.values.get(returned.index())?.ty != slot_info.ty
    {
        return None;
    }
    let mut store = None;
    let mut take = None;
    let mut version = None;
    let mut reaching = None;
    for (index, body) in unit.blocks.iter().enumerate() {
        for (offset, instruction) in body
            .instructions
            .get(&unit.instructions)?
            .iter()
            .enumerate()
        {
            let point = Point::Instruction {
                block: BlockId(index as u32),
                index: offset as u32,
                moment: Moment::After,
            };
            match *instruction {
                Inst::Store {
                    address: target,
                    value,
                    version: output,
                } if target == address => {
                    if value != original || store.replace(point).is_some() {
                        return None;
                    }
                    version = Some(output);
                }
                Inst::Read {
                    address: target,
                    result,
                    reaching: input,
                    mode: ReadMode::Take,
                } if target == address => {
                    // Cleanup may lower a trivial failure drop to another read;
                    // only the read producing the actual returned SSA is selected.
                    if result == returned {
                        if take.replace(point).is_some() {
                            return None;
                        }
                        reaching = Some(input);
                    }
                }
                Inst::Store {
                    address: target, ..
                }
                | Inst::Read {
                    address: target, ..
                } if target != address && same_slot(unit, target, slot) => return None,
                _ => (),
            }
        }
    }
    if version != reaching {
        return None;
    }
    Some(Staging {
        address,
        original,
        returned,
        store: store?,
        take: take?,
        returning: block,
    })
}
fn same_slot<S>(unit: &LirUnit<S>, mut address: AddressId, slot: SlotId) -> bool {
    for _ in 0..unit.addresses.len() {
        let Some(info) = unit.addresses.get(address.index()) else {
            return false;
        };
        match info.base {
            AddressBase::Slot(found) => return found == slot,
            AddressBase::Address(parent) => address = parent,
            _ => return false,
        }
    }
    false
}
