//! Exact Option image lent by the reviewed task.await wrapper. This storage
//! certificate is separate from generic scalar ExclusiveSlot admission.
use super::*;

fn root(u: &LirUnit<Ready>, mut address: AddressId) -> Option<SlotId> {
    for _ in 0..=u.addresses.len() {
        match u.addresses.get(address.index())?.base {
            AddressBase::Slot(slot) => return Some(slot),
            AddressBase::Address(base) => address = base,
            AddressBase::Value(_) => return None,
        }
    }
    None
}

/// The fixed slot must already be in the caller's canonical frame storage.
/// The wrapper prefix is linear and performs no call before TaskJoin, so the
/// one reaching None and the handle Take cannot be repeated on resumption.
pub fn ordered_join_result(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    block: BlockId,
    frame: &[LiveStorage],
) -> Option<RuntimeResultSlot> {
    if !u.header.definition.is_core("task.await") {
        return None;
    }
    let Terminator::Call {
        call,
        normal: Some(_),
        ..
    } = *u.terminators.get(block.index())?
    else {
        return None;
    };
    let site = u.tables.calls.get(call.index())?;
    let Callee::Operation(op) = site.callee else {
        return None;
    };
    if core_name(&u.tables.operations.get(op.index())?.definition)?.as_str()
        != "$intrinsic.TaskJoin"
        || site.result.is_some()
        || site.pretransfer_failure.is_some()
    {
        return None;
    }
    let arguments = site.arguments.get(&u.pools.call_arguments)?;
    if arguments.len() != 4 {
        return None;
    }
    let argument = |parameter| {
        let mut matches = arguments.iter().filter(|a| a.parameter == parameter);
        let value = matches.next()?;
        matches.next().is_none().then_some(value)
    };
    let target = argument(2)?;
    let ArgumentValue::Address { address, .. } = target.value else {
        return None;
    };
    if target.passing != Passing::Mut {
        return None;
    }
    let info = u.addresses.get(address.index())?;
    let AddressBase::Slot(slot) = info.base else {
        return None;
    };
    if info.projection.len != 0 || !info.unchecked.is_empty() {
        return None;
    }
    if !matches!(
        u.slots.get(slot.index())?.kind,
        SlotKind::Temporary | SlotKind::Local
    ) {
        return None;
    }
    let layout = u.types.get(info.ty.index())?.layout;
    if u.types.get(u.slots.get(slot.index())?.ty.index())?.layout != layout {
        return None;
    }
    let abi = table.fn_abis.get(site.abi.0 as usize)?;
    let parameter = abi.parameters.iter().find(|p| p.source == 2)?;
    if parameter.passing != ParameterPassing::ExclusiveAddress
        || parameter.layout != layout
        || abi.result.is_some()
    {
        return None;
    }
    let record = table.layout(layout)?;
    let tag = record.tag.as_ref()?;
    if tag.variants.len() != 2
        || !tag
            .variants
            .iter()
            .any(|v| v.ordinal == 0 && v.fields.is_empty())
        || !tag
            .variants
            .iter()
            .any(|v| v.ordinal == 1 && v.fields.len() == 1)
    {
        return None;
    }
    let (bytes, align) = fixed(table, layout)?;
    let stored = frame.iter().find(|s| s.slot == slot)?;
    if stored.layout != layout
        || stored.repr != u.types[u.slots[slot.index()].ty.index()].repr
        || stored.offset < FRAME_HEADER_BYTES
        || stored
            .offset
            .checked_add(bytes)
            .is_none_or(|end| end > MAX_FRAME_BYTES)
        || stored.offset % align != 0
        || align > 16
    {
        return None;
    }
    if arguments.iter().any(|a| a.parameter != 2 && matches!(a.value, ArgumentValue::Address { address, .. } if root(u,address)==Some(slot))) { return None; }
    let ArgumentValue::Value(size) = argument(3)?.value else {
        return None;
    };
    let ArgumentValue::Value(handle) = argument(1)?.value else {
        return None;
    };
    if argument(1)?.passing != Passing::Take {
        return None;
    }
    // Collect only the prefix that actually reaches this call; branches and
    // preceding calls stay stackful rather than acquiring partial permission.
    let mut at = u.entry;
    let mut seen = BTreeSet::new();
    let mut prefix: Vec<Inst> = Vec::new();
    loop {
        if !seen.insert(at) {
            return None;
        }
        prefix.extend(
            u.blocks
                .get(at.index())?
                .instructions
                .get(&u.instructions)?,
        );
        if at == block {
            break;
        }
        let Terminator::Jump(edge) = *u.terminators.get(at.index())? else {
            return None;
        };
        at = u.tables.edges.get(edge.index())?.target;
    }
    let (slots, loans) = storage_at(u, block);
    if !slots.contains(&slot) {
        return None;
    }
    let reaching: Vec<_> = loans
        .iter()
        .copied()
        .filter(|l| root(u, u.tables.loans[l.index()].address) == Some(slot))
        .collect();
    let [loan] = reaching[..] else {
        return None;
    };
    let lent = u.tables.loans.get(loan.index())?;
    if lent.address != address || !lent.mutable {
        return None;
    }
    let initializers: Vec<_> = prefix
        .iter()
        .filter_map(|inst| match *inst {
            Inst::Store {
                address: a, value, ..
            } if root(u, a) == Some(slot) => Some((a, value)),
            _ => None,
        })
        .collect();
    let [(initialized_address, initial)] = initializers[..] else {
        return None;
    };
    if initialized_address != address {
        return None;
    }
    let mut none = false;
    let mut exact_size = false;
    let mut taken_handle = false;
    for inst in prefix {
        match inst {
            Inst::Construct {
                result, construct, ..
            } if result == initial => {
                let construct = u.tables.constructs.get(construct.index())?;
                let ConstructorKind::Variant {
                    definition,
                    variant: 0,
                } = construct.constructor
                else {
                    return None;
                };
                none = construct.layout == layout
                    && construct.fields.len == 0
                    && u.tables
                        .symbols
                        .get(definition.index())?
                        .definition
                        .is_core("Option");
            }
            Inst::Const { result, constant } if result == size => {
                exact_size = u.tables.constants.get(constant.index())?.value
                    == ConstantValue::Integer(u128::from(bytes));
            }
            Inst::Read {
                result,
                mode: ReadMode::Take,
                ..
            } if result == handle => taken_handle = true,
            Inst::Replace { address: a, .. } | Inst::Drop { address: a, .. }
                if root(u, a) == Some(slot) =>
            {
                return None;
            }
            Inst::Read {
                address: a,
                mode: ReadMode::Take,
                ..
            } if root(u, a) == Some(slot) => return None,
            Inst::StorageDead { slot: s, .. } if s == slot => return None,
            _ => {}
        }
    }
    if !none || !exact_size || !taken_handle {
        return None;
    }
    Some(RuntimeResultSlot {
        source: RuntimeResultSource::ArgumentSlot {
            parameter: 2,
            address,
            slot,
            loan,
            initial,
        },
        layout,
        offset: stored.offset,
        bytes,
        align,
    })
}

/// Independently check an imported join result descriptor. This does not
/// authorize the enclosing wrapper or its helper to leave a native stack.
pub fn check_ordered_join_result(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    block: BlockId,
    frame: &[LiveStorage],
    claimed: &RuntimeResultSlot,
) -> bool {
    let RuntimeResultSource::ArgumentSlot {
        parameter: 2,
        address,
        slot,
        loan,
        initial,
    } = claimed.source
    else {
        return false;
    };
    if !u.header.definition.is_core("task.await") {
        return false;
    }
    let Some(Terminator::Call {
        call,
        normal: Some(_),
        ..
    }) = u.terminators.get(block.index())
    else {
        return false;
    };
    let Some(site) = u.tables.calls.get(call.index()) else {
        return false;
    };
    let Callee::Operation(op) = site.callee else {
        return false;
    };
    let Some(operation) = u.tables.operations.get(op.index()) else {
        return false;
    };
    if core_name(&operation.definition).as_deref() != Some("$intrinsic.TaskJoin")
        || site.result.is_some()
        || site.pretransfer_failure.is_some()
    {
        return false;
    }
    let Some(arguments) = site.arguments.get(&u.pools.call_arguments) else {
        return false;
    };
    if arguments.len() != 4 {
        return false;
    }
    let Some(target) = arguments.iter().find(|a| a.parameter == 2) else {
        return false;
    };
    if target.passing != Passing::Mut
        || !matches!(target.value, ArgumentValue::Address { address: a, .. } if a==address)
    {
        return false;
    }
    let Some(info) = u.addresses.get(address.index()) else {
        return false;
    };
    if info.base != AddressBase::Slot(slot)
        || info.projection.len != 0
        || !info.unchecked.is_empty()
    {
        return false;
    }
    let Some(slot_info) = u.slots.get(slot.index()) else {
        return false;
    };
    if !matches!(slot_info.kind, SlotKind::Temporary | SlotKind::Local) {
        return false;
    }
    let Some(ty) = u.types.get(slot_info.ty.index()) else {
        return false;
    };
    if u.types
        .get(info.ty.index())
        .is_none_or(|address_ty| address_ty.layout != ty.layout)
    {
        return false;
    }
    let Some(record) = table.layout(ty.layout) else {
        return false;
    };
    let SizeSpec::Fixed(bytes) = record.size else {
        return false;
    };
    let align = u64::from(record.align.max(1));
    let Some(stored) = frame.iter().find(|s| s.slot == slot) else {
        return false;
    };
    if stored.layout != ty.layout
        || stored.repr != ty.repr
        || stored.offset < FRAME_HEADER_BYTES
        || stored.offset % align != 0
        || align > 16
        || stored
            .offset
            .checked_add(bytes)
            .is_none_or(|end| end > MAX_FRAME_BYTES)
        || claimed.layout != ty.layout
        || claimed.offset != stored.offset
        || claimed.bytes != bytes
        || claimed.align != align
    {
        return false;
    }
    let Some(tag) = &record.tag else {
        return false;
    };
    if tag.variants.len() != 2
        || !tag
            .variants
            .iter()
            .any(|v| v.ordinal == 0 && v.fields.is_empty())
        || !tag
            .variants
            .iter()
            .any(|v| v.ordinal == 1 && v.fields.len() == 1)
    {
        return false;
    }
    let Some(abi) = table.fn_abis.get(site.abi.0 as usize) else {
        return false;
    };
    if abi.result.is_some()
        || !abi.parameters.iter().any(|p| {
            p.source == 2
                && p.layout == ty.layout
                && p.passing == ParameterPassing::ExclusiveAddress
        })
    {
        return false;
    }
    if arguments.iter().any(|a| {
        a.parameter != 2
            && matches!(a.value, ArgumentValue::Address { address: a, .. } if root(u,a)==Some(slot))
    }) {
        return false;
    }
    let (live, loans) = storage_at(u, block);
    if !live.contains(&slot) {
        return false;
    }
    let reaching: Vec<_> = loans
        .iter()
        .copied()
        .filter(|l| root(u, u.tables.loans[l.index()].address) == Some(slot))
        .collect();
    if reaching != [loan]
        || !u.tables.loans[loan.index()].mutable
        || u.tables.loans[loan.index()].address != address
    {
        return false;
    }
    let Some(size) = arguments
        .iter()
        .find(|a| a.parameter == 3)
        .and_then(|a| match a.value {
            ArgumentValue::Value(v) => Some(v),
            _ => None,
        })
    else {
        return false;
    };
    let Some(handle) = arguments
        .iter()
        .find(|a| a.parameter == 1 && a.passing == Passing::Take)
        .and_then(|a| match a.value {
            ArgumentValue::Value(v) => Some(v),
            _ => None,
        })
    else {
        return false;
    };
    let mut position = u.entry;
    let mut visited = BTreeSet::new();
    let (mut stores, mut none, mut size_matches, mut taken) = (0, false, false, false);
    loop {
        if !visited.insert(position) {
            return false;
        }
        let Some(instructions) = u
            .blocks
            .get(position.index())
            .and_then(|b| b.instructions.get(&u.instructions))
        else {
            return false;
        };
        for inst in instructions {
            match *inst {
                Inst::Store {
                    address: a, value, ..
                } if root(u, a) == Some(slot) => {
                    if a != address || value != initial {
                        return false;
                    }
                    stores += 1;
                }
                Inst::Construct {
                    result, construct, ..
                } if result == initial => {
                    let Some(construction) = u.tables.constructs.get(construct.index()) else {
                        return false;
                    };
                    let ConstructorKind::Variant {
                        definition,
                        variant: 0,
                    } = construction.constructor
                    else {
                        return false;
                    };
                    none = construction.layout == ty.layout
                        && construction.fields.len == 0
                        && u.tables
                            .symbols
                            .get(definition.index())
                            .is_some_and(|s| s.definition.is_core("Option"));
                }
                Inst::Const { result, constant } if result == size => {
                    size_matches = u
                        .tables
                        .constants
                        .get(constant.index())
                        .is_some_and(|c| c.value == ConstantValue::Integer(u128::from(bytes)))
                }
                Inst::Read {
                    result,
                    mode: ReadMode::Take,
                    ..
                } if result == handle => taken = true,
                Inst::Read {
                    address: a,
                    mode: ReadMode::Take,
                    ..
                }
                | Inst::Replace { address: a, .. }
                | Inst::Drop { address: a, .. }
                    if root(u, a) == Some(slot) =>
                {
                    return false;
                }
                Inst::StorageDead { slot: s, .. } if s == slot => return false,
                _ => {}
            }
        }
        if position == block {
            break;
        }
        let Some(Terminator::Jump(edge)) = u.terminators.get(position.index()) else {
            return false;
        };
        let Some(next) = u.tables.edges.get(edge.index()) else {
            return false;
        };
        position = next.target;
    }
    stores == 1 && none && size_matches && taken
}
