//! Runtime calls carry no Resource lineage until an explicit transport proves it.
use super::*;

// Scheduler handles are carrier identities, not Resource values. A Task with
// a Resource result is deliberately excluded; channel and unknown handles need
// their own transport proof even when their immediate scan has no payload.
pub(super) fn resource_free_handle(layouts: &LayoutTable, layout: LayoutId) -> bool {
    use crate::layout_types::{HandleKind, ScanStep};
    let Some(record) = layouts.layout(layout) else {
        return false;
    };
    if !matches!(record.kind, LayoutKind::Handle)
        || record.finalizer.is_some()
        || record.scan.is_empty()
    {
        return false;
    }
    record.scan.iter().all(|step| match step {
        ScanStep::Handle {
            kind: HandleKind::Scope,
            obtains: None,
            ..
        } => true,
        ScanStep::Handle {
            kind: HandleKind::Task,
            obtains: Some(payload),
            ..
        } => plain(layouts, *payload, 0),
        _ => false,
    })
}

pub(super) fn spawn_parameter<S>(unit: &LirUnit<S>, call: &CallSite) -> Option<ValueId> {
    let Callee::Operation(operation) = call.callee else {
        return None;
    };
    if !unit
        .tables
        .operations
        .get(operation.index())?
        .definition
        .is_core("$intrinsic.TaskSpawn")
    {
        return None;
    }
    let argument = call
        .arguments
        .get(&unit.pools.call_arguments)?
        .iter()
        .find(|argument| argument.parameter == 1)?;
    match argument.value {
        ArgumentValue::Value(value)
            if argument.passing == hirsel_core_contract::Passing::Take
                && unit.types[unit.values[value.index()].ty.index()].repr
                    == ValueRepr::Callable =>
        {
            Some(value)
        }
        _ => None,
    }
}

pub(super) fn runtime_call<S>(
    unit: &LirUnit<S>,
    layouts: &LayoutTable,
    call: &CallSite,
    codes: Option<&[calls::Code]>,
    empty: Option<&[bool]>,
) -> Result<(), String> {
    let carried = |value: ValueId| {
        let info = &unit.values[value.index()];
        info.holding == Holding::Owned
            && !resource_free_handle(layouts, unit.types[info.ty.index()].layout)
    };
    if call.pretransfer_failure.is_some() || call.result.is_some_and(carried) {
        return Err(format!(
            "native ownership tracing: runtime result transport in {}",
            unit.header.definition
        ));
    }
    for argument in call
        .arguments
        .get(&unit.pools.call_arguments)
        .ok_or("ownership runtime call arguments are absent")?
    {
        match argument.value {
            ArgumentValue::Value(value)
                if unit.types[unit.values[value.index()].ty.index()].repr
                    == ValueRepr::Callable =>
            {
                // Source capture keeps this tentative operation, but emission
                // must independently bind its complete checked code universe.
                if let Some(codes) = codes {
                    if codes
                        .get(value.index())
                        .and_then(calls::Code::targets)
                        .is_none()
                    {
                        return Err("native ownership tracing: unresolved runtime callable".into());
                    }
                    if !empty.is_some_and(|empty| empty[value.index()])
                        && !(spawn_parameter(unit, call) == Some(value)
                            && codes[value.index()]
                                .targets()
                                .is_some_and(|targets| targets.len() == 1))
                    {
                        return Err(
                            "native ownership tracing: owned runtime callback environment".into(),
                        );
                    }
                }
            }
            ArgumentValue::Value(value) => {
                let layout = unit.types[unit.values[value.index()].ty.index()].layout;
                let inert = plain(layouts, layout, 0)
                    || resource_free_handle(layouts, layout)
                    || layouts.layout(layout).is_some_and(|record| {
                        matches!(record.kind, LayoutKind::HostCapability { .. })
                    });
                if carried(value) || !inert {
                    return Err(format!(
                        "native ownership tracing: runtime argument transport in {} parameter {}",
                        unit.header.definition, argument.parameter
                    ));
                }
            }
            ArgumentValue::Address { address, .. }
                if !resource_free_handle(
                    layouts,
                    unit.types[unit.addresses[address.index()].ty.index()].layout,
                ) && !plain(
                    layouts,
                    unit.types[unit.addresses[address.index()].ty.index()].layout,
                    0,
                ) =>
            {
                return Err(format!(
                    "native ownership tracing: runtime mutable argument transport in {} parameter {}",
                    unit.header.definition, argument.parameter
                ));
            }
            _ => (),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_types::*;

    fn record(kind: LayoutKind) -> LayoutRecord {
        LayoutRecord {
            key: LayoutIdentity {
                owner_revision: None,
                instance: None,
                target: TargetSpec::x86_64_unknown_linux_gnu().digest(),
                layout_version: crate::LAYOUT_VERSION,
            },
            kind,
            size: SizeSpec::Fixed(8),
            align: 8,
            fields: vec![],
            tag: None,
            initialized: InitializedBytes::Fields,
            scan: vec![],
            drop: vec![],
            copy: CopyRecipe::Bits,
            finalizer: None,
            storage: vec![],
            abi: AbiInputs {
                scalars: vec![],
                unaligned: false,
            },
        }
    }

    #[test]
    fn resource_free_task_results_check_every_variant_and_refuse_channel_custody() {
        let mut table = LayoutTable::new(&TargetSpec::x86_64_unknown_linux_gnu());
        table.layouts = vec![
            record(LayoutKind::Unit),
            record(LayoutKind::Resource),
            record(LayoutKind::Variant),
            record(LayoutKind::Handle),
        ];
        table.layouts[2].tag = Some(TagEncoding {
            offset: 0,
            form: TagForm::Ordinal,
            variants: vec![
                VariantLayout {
                    ordinal: 0,
                    fields: vec![],
                },
                VariantLayout {
                    ordinal: 1,
                    fields: vec![FieldLayout {
                        ordinal: 0,
                        offset: 0,
                        layout: LayoutId(0),
                        unaligned: false,
                    }],
                },
            ],
        });
        table.layouts[3].scan = vec![ScanStep::Handle {
            offset: 0,
            kind: HandleKind::Task,
            ty: None,
            obtains: Some(LayoutId(2)),
        }];
        assert!(resource_free_handle(&table, LayoutId(3)));
        table.layouts[2].tag.as_mut().unwrap().variants[1].fields[0].layout = LayoutId(1);
        assert!(!resource_free_handle(&table, LayoutId(3)));
        table.layouts[2].tag.as_mut().unwrap().variants[1].fields[0].layout = LayoutId(2);
        assert!(!resource_free_handle(&table, LayoutId(3)));
        table.layouts[3].scan = vec![ScanStep::Handle {
            offset: 0,
            kind: HandleKind::Receiver,
            ty: None,
            obtains: Some(LayoutId(0)),
        }];
        assert!(!resource_free_handle(&table, LayoutId(3)));
        table.layouts[3].scan = vec![ScanStep::Handle {
            offset: 0,
            kind: HandleKind::Unknown,
            ty: None,
            obtains: None,
        }];
        assert!(!resource_free_handle(&table, LayoutId(3)));
    }
}

/// Prove resource-free callable environments from checked capture equations.
/// Physical callable layout equality never identifies the captured code.
pub(super) fn empty_callables(
    units: &[&ValidatedUnit<Ready>],
    layouts: &LayoutTable,
    codes: &[Vec<calls::Code>],
) -> Vec<Vec<bool>> {
    let symbols = units
        .iter()
        .enumerate()
        .map(|(index, unit)| {
            let header = &unit.unit().header;
            ((&header.definition, header.instance), index)
        })
        .collect::<BTreeMap<_, _>>();
    fn empty(
        target: &calls::Target,
        units: &[&ValidatedUnit<Ready>],
        layouts: &LayoutTable,
        codes: &[Vec<calls::Code>],
        symbols: &BTreeMap<(&hirsel_syntax::DefPath, ContentHash), usize>,
        active: &mut BTreeSet<usize>,
    ) -> bool {
        let Some(environment) = target.environment else {
            return true;
        };
        let Some(record) = layouts.layout(environment) else {
            return false;
        };
        if record.finalizer.is_some() {
            return false;
        }
        if record.drop.is_empty() {
            return true;
        }
        let Some(index) = target
            .symbol
            .instance
            .and_then(|instance| symbols.get(&(&target.symbol.definition, instance)))
            .copied()
        else {
            return false;
        };
        let body = units[index].unit();
        if !matches!(record.kind, LayoutKind::Environment { .. })
            || body.tables.environment() != Some(environment)
            || !active.insert(index)
        {
            return false;
        }
        let result = record.fields.iter().all(|field| {
            let Some(&value) = body.captures.get(field.ordinal as usize) else {
                return false;
            };
            let layout = body.types[body.values[value.index()].ty.index()].layout;
            if layout != field.layout {
                return false;
            }
            plain(layouts, layout, 0)
                || resource_free_handle(layouts, layout)
                || (body.types[body.values[value.index()].ty.index()].repr == ValueRepr::Callable
                    && codes[index][value.index()]
                        .targets()
                        .is_some_and(|targets| {
                            targets
                                .iter()
                                .all(|target| empty(target, units, layouts, codes, symbols, active))
                        }))
        });
        active.remove(&index);
        result
    }
    codes
        .iter()
        .map(|row| {
            row.iter()
                .map(|code| {
                    code.targets().is_some_and(|targets| {
                        targets.iter().all(|target| {
                            empty(
                                target,
                                units,
                                layouts,
                                codes,
                                &symbols,
                                &mut BTreeSet::new(),
                            )
                        })
                    })
                })
                .collect()
        })
        .collect()
}
