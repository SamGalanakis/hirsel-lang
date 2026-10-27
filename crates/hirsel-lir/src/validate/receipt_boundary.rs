//! Data entry companions are source-bound snapshots with matching custody.
//! Captures and tasks remain closed here.
use super::{Cx, Inputs, Report, Site, ViolationKind as K};
use crate::{ir::*, sealed_payload_source::*, stage::Stage};

fn supported(contract: &ReceiptContract) -> Result<(), &'static str> {
    if !contract.captures.is_empty()
        || contract.inventory.iter().any(|grant| {
            !matches!(
                grant,
                crate::sealed_payload_source::ReceiptGrant::Parameter(_)
            )
        })
    {
        return Err(
            "receipt boundary requires data parameters and results without captures or non-parameter inventory",
        );
    }
    Ok(())
}

fn source(
    source: &ReceiptSource,
    definition: &hirsel_syntax::DefPath,
    instance: hirsel_syntax::ContentHash,
    abi: &crate::FnAbi,
) -> Result<(), &'static str> {
    source.check_structure()?;
    if source.original.definition != *definition
        || source.instance != instance
        || abi.receipts.as_ref() != Some(&source.contract)
    {
        return Err("receipt source differs from the selected boundary or complete ABI contract");
    }
    supported(&source.contract)
}

fn port_type(
    source: &ReceiptSource,
    port: ReceiptPort,
    metadata: crate::LayoutId,
    ordinary: Option<crate::LayoutId>,
    selected: (&hirsel_syntax::DefPath, hirsel_syntax::ContentHash),
    inputs: super::UnitInputs<'_>,
    operations: &dyn crate::operation::OperationDeclarations,
) -> Result<(), String> {
    let (definition, instance) = selected;
    let profile = source
        .profile(port)
        .ok_or("receipt boundary lacks its complete original parameter profile")?;
    if !profile.functions.is_empty() {
        return Err("receipt boundary callable profiles require environment custody".into());
    }
    if !matches!(inputs.layouts.layout(metadata).map(|r| &r.kind),
        Some(crate::layout_types::LayoutKind::PayloadReceipt { root }) if root == &profile.root.ty)
    {
        return Err("receipt boundary metadata has another original root type".into());
    }
    let descriptor = operations
        .seal_descriptor_for_source(
            definition,
            instance,
            &profile.root.ty,
            inputs.layouts,
            inputs.target,
        )
        .map_err(|e| format!("receipt boundary original layout: {e:?}"))?;
    descriptor.check(inputs.layouts)?;
    if descriptor.root_type != profile.root.ty
        || !ordinary.map_or_else(
            || {
                matches!(
                    inputs
                        .layouts
                        .layout(descriptor.root_layout)
                        .map(|r| &r.kind),
                    Some(crate::layout_types::LayoutKind::Unit)
                )
            },
            |ordinary| descriptor.root_layout == ordinary,
        )
    {
        return Err("receipt boundary ordinary layout differs from its original profile".into());
    }
    Ok(())
}

pub(super) fn entry<S: Stage>(unit: &LirUnit<S>, inputs: &Inputs<'_>) -> Result<(), String> {
    let abi = &inputs.layouts.fn_abis[unit.header.abi.0 as usize];
    let Some(entry) = &unit.tables.receipt_entry else {
        if abi.receipts.is_some()
            || unit.slots.iter().any(|slot| {
                matches!(
                    slot.kind,
                    SlotKind::ReceiptParameter(_)
                        | SlotKind::ReceiptCapture(_)
                        | SlotKind::ReceiptResult
                )
            })
        {
            return Err("receipt ABI transport: slots or ABI lack an entry contract".into());
        }
        return Ok(());
    };
    entry.check(abi, unit, inputs.layouts)?;
    let selected = inputs
        .operations
        .receipt_source(&unit.header.definition, unit.header.instance)
        .ok_or("receipt entry requires independent source and custody validation")?;
    if !unit.captures.is_empty()
        || entry.environment.is_some()
        || selected
            .profiles
            .iter()
            .any(|p| matches!(p.port, ReceiptPort::Capture(_)))
    {
        return Err("receipt entry captures require independent environment custody".into());
    }
    source(
        &selected,
        &unit.header.definition,
        unit.header.instance,
        abi,
    )?;
    if selected.implementation_revision != unit.header.implementation_revision {
        return Err("receipt entry implementation revision differs from original source".into());
    }
    for binding in &entry.parameters {
        let ordinary = abi
            .parameters
            .iter()
            .find(|p| p.source == binding.source)
            .ok_or("receipt entry lacks its original declared parameter")?;
        let metadata = unit.types[unit.slots[binding.slot.index()].ty.index()].layout;
        port_type(
            &selected,
            ReceiptPort::Parameter(binding.source),
            metadata,
            Some(ordinary.layout),
            (&unit.header.definition, unit.header.instance),
            inputs.into(),
            inputs.operations,
        )?;
    }
    if let Some(result) = entry.result {
        let metadata = unit.types[unit.slots[result.index()].ty.index()].layout;
        port_type(
            &selected,
            ReceiptPort::Result,
            metadata,
            abi.result.as_ref().map(|r| r.layout),
            (&unit.header.definition, unit.header.instance),
            inputs.into(),
            inputs.operations,
        )?;
    }
    Ok(())
}

pub(super) fn calls<S: Stage>(cx: &Cx<'_, S>, report: &mut Report) {
    let unit = cx.unit;
    let pairs = std::cell::OnceCell::new();
    for call in &unit.tables.calls {
        let abi = &cx.layouts.0.fn_abis[call.abi.0 as usize];
        let symbol = match call.callee {
            Callee::Direct(id) | Callee::Bound { code: id, .. } => {
                Some(&unit.tables.symbols[id.index()])
            }
            _ => None,
        };
        // An unavailable declaration is also an input. Erasing both a call's
        // contract and its metadata must not turn an aware target into an
        // ordinary target, or preserve a cached output after demand changes.
        let selected = symbol.and_then(|symbol| {
            symbol
                .instance
                .and_then(|instance| cx.operations.receipt_source(&symbol.definition, instance))
        });
        if abi.receipts.is_none() && selected.is_none() {
            continue;
        }
        let checked = (|| {
            let symbol =
                symbol.ok_or("receipt call requires an independently selected code target")?;
            let instance = symbol
                .instance
                .ok_or("receipt call lacks its selected instance")?;
            let selected = selected
                .as_ref()
                .ok_or("receipt call lacks its independent original source declaration")?;
            source(selected, &symbol.definition, instance, abi)?;
            let pairs = pairs.get_or_init(|| super::sealed_payload::pairing::Pairing::new(unit));
            let receipts = call
                .receipts
                .as_ref()
                .ok_or("receipt call lacks its carriers")?;
            for argument in &receipts.arguments {
                let (parameter, receipt) = match *argument {
                    ReceiptArgument::Borrow { parameter, receipt }
                    | ReceiptArgument::Take { parameter, receipt } => (parameter, receipt),
                    ReceiptArgument::Mut {
                        parameter,
                        address,
                        version,
                    } => {
                        let ordinary = call
                            .arguments
                            .get(&unit.pools.call_arguments)
                            .unwrap_or(&[])
                            .iter()
                            .find(|a| a.parameter == parameter)
                            .ok_or("receipt call lacks its ordinary mutable argument")?;
                        let ArgumentValue::Address {
                            address: payload, ..
                        } = ordinary.value
                        else {
                            return Err("receipt call lacks its ordinary mutable address".into());
                        };
                        if !whole_address(unit, payload) || !whole_address(unit, address) {
                            return Err("mutable receipt requires paired whole slots".into());
                        }
                        port_type(
                            selected,
                            ReceiptPort::Parameter(parameter),
                            cx.value_layout(version),
                            Some(unit.types[unit.addresses[payload.index()].ty.index()].layout),
                            (&symbol.definition, instance),
                            super::UnitInputs {
                                target: cx.target,
                                layouts: cx.layouts.0,
                                manifest: cx.manifest,
                            },
                            cx.operations,
                        )?;
                        continue;
                    }
                };
                let ordinary = call
                    .arguments
                    .get(&unit.pools.call_arguments)
                    .unwrap_or(&[])
                    .iter()
                    .find(|a| a.parameter == parameter)
                    .ok_or("receipt call lacks its ordinary argument")?;
                let ArgumentValue::Value(value) = ordinary.value else {
                    return Err("receipt input lacks its ordinary value".into());
                };
                if !pairs.paired(value, receipt) {
                    return Err(
                        "receipt call metadata is not paired with its ordinary argument".into(),
                    );
                }
                let inputs = super::UnitInputs {
                    target: cx.target,
                    layouts: cx.layouts.0,
                    manifest: cx.manifest,
                };
                port_type(
                    selected,
                    ReceiptPort::Parameter(parameter),
                    cx.value_layout(receipt),
                    Some(cx.value_layout(value)),
                    (&symbol.definition, instance),
                    inputs,
                    cx.operations,
                )?;
            }
            if let Some(result) = receipts.result {
                port_type(
                    selected,
                    ReceiptPort::Result,
                    cx.value_layout(result),
                    call.result.map(|v| cx.value_layout(v)),
                    (&symbol.definition, instance),
                    super::UnitInputs {
                        target: cx.target,
                        layouts: cx.layouts.0,
                        manifest: cx.manifest,
                    },
                    cx.operations,
                )?;
                if unit.values[result.index()].holding != Holding::Owned {
                    return Err("receipt result must publish one owned holder".into());
                }
            }
            Ok::<(), String>(())
        })();
        if let Err(error) = checked {
            report.add(Site::UNIT, K::Sealed(error));
        }
    }
}

fn whole_address<S>(unit: &LirUnit<S>, mut address: AddressId) -> bool {
    for _ in 0..=unit.addresses.len() {
        let row = &unit.addresses[address.index()];
        if row.projection.len != 0 {
            return false;
        }
        match row.base {
            AddressBase::Slot(_) => return true,
            AddressBase::Address(parent) => address = parent,
            AddressBase::Value(_) => return false,
        }
    }
    false
}
