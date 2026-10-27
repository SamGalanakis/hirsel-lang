//! Private snapshots use the ordinary affine holder and slot/loan proof.
//! Source-checked PayloadReceipt and taken entries supply holders. Metadata cannot enter
//! a language value, ordinary ABI, static, encoded result or task environment.
use super::{Cx, Report, Site, ViolationKind as K, structure};
use crate::ir::*;
use crate::layout_types::LayoutKind;
use crate::stage::Stage;

pub(super) fn check<S: Stage>(cx: &Cx<'_, S>, report: &mut Report) {
    let unit = cx.unit;
    if !cx
        .layouts
        .0
        .layouts
        .iter()
        .any(|layout| matches!(layout.kind, LayoutKind::PayloadReceipt { .. }))
    {
        return;
    }
    let metadata = |value: ValueId| cx.repr(value) == ValueRepr::PayloadReceipt;
    let mut reject = |site, text: &str| report.add(site, K::Sealed(text.into()));
    // Ordinary ABI ordinals are never hidden receipt ports. Entry and call
    // companions still require their own independently checked source contract.
    if unit
        .parameters
        .iter()
        .chain(&unit.captures)
        .any(|&v| metadata(v))
    {
        reject(
            Site::UNIT,
            "private receipt passed as an ordinary entry value",
        );
    }
    let abi = &cx.layouts.0.fn_abis[unit.header.abi.0 as usize];
    if abi
        .parameters
        .iter()
        .map(|p| p.layout)
        .chain(abi.result.iter().map(|r| r.layout))
        .any(|id| {
            matches!(
                cx.layouts.get(id).map(|l| &l.kind),
                Some(LayoutKind::PayloadReceipt { .. })
            )
        })
    {
        reject(Site::UNIT, "private receipt exposed in an ordinary ABI");
    }
    for slot in &unit.slots {
        if unit.types[slot.ty.index()].repr == ValueRepr::PayloadReceipt
            && !matches!(
                slot.kind,
                SlotKind::Local
                    | SlotKind::Temporary
                    | SlotKind::Displaced
                    | SlotKind::ReceiptParameter(_)
                    | SlotKind::ReceiptResult
            )
        {
            reject(Site::UNIT, "private receipt slot is not local custody");
        }
    }
    // A wrapper could otherwise hide metadata behind an Aggregate or Block
    // representation and bypass the value-level checks below.
    let mut pending: Vec<_> = unit.types.iter().map(|ty| ty.layout).collect();
    let mut visited = std::collections::BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        let layout = cx.layouts.get(id).expect("checked layout");
        pending.extend(layout.references());
        let contains = layout.references().into_iter().any(|id| {
            matches!(
                cx.layouts.get(id).map(|l| &l.kind),
                Some(LayoutKind::PayloadReceipt { .. })
            )
        });
        if contains {
            reject(
                Site::UNIT,
                "private receipt embedded in ordinary layout storage",
            );
        }
    }
    for (block, row) in unit.blocks.iter().enumerate() {
        let block = BlockId(block as u32);
        for (position, inst) in row
            .instructions
            .get(&unit.instructions)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            let site = Site::at(block, position as u32);
            match *inst {
                Inst::BeginLoan {
                    address,
                    mutable: true,
                    ..
                } if unit.types[unit.addresses[address.index()].ty.index()].repr
                    == ValueRepr::PayloadReceipt =>
                {
                    if !metadata_address(cx, address, false) {
                        reject(
                            site,
                            "receipt metadata cannot be mutably borrowed without writeback custody",
                        );
                    }
                }
                Inst::StorageLive { slot } | Inst::StorageDead { slot, .. }
                    if unit.slots[slot.index()].kind == SlotKind::ReceiptResult =>
                {
                    reject(site, "receipt output storage belongs to the caller");
                }
                Inst::StorageLive { slot } | Inst::StorageDead { slot, .. }
                    if matches!(unit.slots[slot.index()].kind, SlotKind::ReceiptParameter(_))
                        && (matches!(inst, Inst::StorageLive { .. })
                            || borrowed_parameter(cx, slot)) =>
                {
                    reject(
                        site,
                        "borrowed receipt entry storage cannot be retired or reinitialized",
                    );
                }
                Inst::PayloadReceipt { value, inputs, .. } => {
                    if metadata(value)
                        || inputs
                            .get(&unit.pools.receipt_inputs)
                            .unwrap_or(&[])
                            .iter()
                            .any(|i| metadata(i.original))
                    {
                        reject(site, "private receipt used as original source payload");
                    }
                }
                Inst::Read {
                    result, address, ..
                } if metadata(result) => {
                    if !metadata_address(cx, address, true) {
                        reject(site, "private receipt read is not a whole local slot");
                    }
                }
                Inst::Store { value, address, .. } | Inst::Replace { value, address, .. }
                    if metadata(value) =>
                {
                    if !metadata_address(cx, address, false) {
                        reject(site, "private receipt write is not a whole local slot");
                    }
                    if output_address(unit, address)
                        && (!matches!(inst, Inst::Store { .. })
                            || !matches!(unit.terminators[block.index()], Terminator::Return(_))
                            || row.instructions.get(&unit.instructions).unwrap_or(&[])
                                [position + 1..]
                                .iter()
                                .any(|inst| {
                                    !matches!(inst, Inst::StorageDead { .. } | Inst::EndLoan { .. })
                                }))
                    {
                        reject(
                            site,
                            "receipt output may only publish after cleanup immediately before normal return",
                        );
                    }
                }
                Inst::Release { value, .. } if metadata(value) => (),
                // No implicit copy, recast, freeze, forget or user operation
                // can manufacture or consume a metadata credit.
                _ => {
                    if inst
                        .defs()
                        .into_iter()
                        .chain(
                            structure::inst_operands(cx, inst)
                                .into_iter()
                                .map(|(v, _)| v),
                        )
                        .any(metadata)
                    {
                        reject(
                            site,
                            "private receipt used outside metadata custody operations",
                        );
                    }
                }
            }
        }
        if let Terminator::Return(Some(value)) = unit.terminators[block.index()]
            && metadata(value)
        {
            reject(
                Site::block(block),
                "private receipt returned as a language result",
            );
        }
    }
    for call in &unit.tables.calls {
        if call
            .arguments
            .get(&unit.pools.call_arguments)
            .unwrap_or(&[])
            .iter()
            .any(|a| match a.value {
                ArgumentValue::Value(value) => metadata(value),
                ArgumentValue::Address { address, version } => {
                    metadata(version)
                        || unit.types[unit.addresses[address.index()].ty.index()].repr
                            == ValueRepr::PayloadReceipt
                }
            })
            || call.result.is_some_and(metadata)
        {
            reject(
                Site::UNIT,
                "private receipt passed through an ordinary call",
            );
        }
    }
}

fn metadata_address<S>(cx: &Cx<'_, S>, mut address: AddressId, read: bool) -> bool {
    for _ in 0..cx.unit.addresses.len() {
        let row = &cx.unit.addresses[address.index()];
        if row.projection.len != 0 {
            return false;
        }
        match row.base {
            AddressBase::Address(parent) => address = parent,
            AddressBase::Slot(slot) => {
                return matches!(
                    cx.unit.slots[slot.index()].kind,
                    SlotKind::Local | SlotKind::Temporary | SlotKind::Displaced
                ) || !read && cx.unit.slots[slot.index()].kind == SlotKind::ReceiptResult
                    || read
                        && matches!(
                            cx.unit.slots[slot.index()].kind,
                            SlotKind::ReceiptParameter(_)
                        )
                    || matches!(
                        cx.unit.slots[slot.index()].kind,
                        SlotKind::ReceiptParameter(_)
                    ) && receipt_mode(cx, slot)
                        == Some(crate::sealed_payload_source::ReceiptMode::MutSlot);
            }
            AddressBase::Value(_) => return false,
        }
    }
    false
}

fn borrowed_parameter<S>(cx: &Cx<'_, S>, slot: SlotId) -> bool {
    receipt_mode(cx, slot) == Some(crate::sealed_payload_source::ReceiptMode::Borrow)
}
fn receipt_mode<S>(
    cx: &Cx<'_, S>,
    slot: SlotId,
) -> Option<crate::sealed_payload_source::ReceiptMode> {
    let SlotKind::ReceiptParameter(source) = cx.unit.slots[slot.index()].kind else {
        return None;
    };
    cx.layouts.0.fn_abis[cx.unit.header.abi.0 as usize]
        .receipts
        .as_ref()?
        .parameters
        .iter()
        .find(|p| p.source == source)
        .map(|p| p.mode)
}

fn output_address<S>(unit: &LirUnit<S>, mut address: AddressId) -> bool {
    for _ in 0..=unit.addresses.len() {
        match unit.addresses[address.index()].base {
            AddressBase::Slot(slot) => {
                return unit.slots[slot.index()].kind == SlotKind::ReceiptResult;
            }
            AddressBase::Address(parent) => address = parent,
            AddressBase::Value(_) => return false,
        }
    }
    false
}
