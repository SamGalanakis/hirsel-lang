//! Runtime calls carry no Resource lineage until an explicit transport proves it.
use super::*;

// Scheduler handles are carrier identities, not Resource values. A Task with
// a Resource result is deliberately excluded. Channel endpoints are inert only
// with a self-bound schema, a plain receive payload and exact close retirement.
// Unknown endpoints and Resource-bearing receivers still require custody.
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

// Only resource selection uses this proof. Owned selection retains its
// original channel custody requirements.
pub(super) fn channel_handle(layouts: &LayoutTable, layout: LayoutId) -> bool {
    use crate::layout_types::{DropStep, HandleKind, ScalarKind, ScanStep};
    let Some(record) = layouts.layout(layout) else {
        return false;
    };
    if !matches!(record.kind, LayoutKind::Handle)
        || record.finalizer.is_some()
        || !matches!(record.fields.as_slice(), [field]
            if field.ordinal == 0 && field.offset == 0 && !field.unaligned
                && layouts.layout(field.layout).is_some_and(|word|
                    word.kind == LayoutKind::Scalar(ScalarKind::U64)
                        && plain(layouts, field.layout, 0)))
        || !matches!(
            record.drop.as_slice(),
            [DropStep::CloseHandle { offset: 0 }]
        )
    {
        return false;
    }
    match record.scan.as_slice() {
        [
            ScanStep::Handle {
                offset: 0,
                kind: HandleKind::Sender,
                ty: Some(schema),
                obtains: None,
            },
        ] => *schema == layout,
        [
            ScanStep::Handle {
                offset: 0,
                kind: HandleKind::Receiver,
                ty: Some(schema),
                obtains: Some(payload),
            },
        ] => *schema == layout && layouts.layout(*payload).is_some(),
        _ => false,
    }
}

pub(super) fn plain_channel_handle(layouts: &LayoutTable, layout: LayoutId) -> bool {
    channel_handle(layouts, layout)
        && layouts.layout(layout).is_some_and(|record| {
            record.scan.iter().all(|step| match step {
                crate::layout_types::ScanStep::Handle {
                    obtains: Some(payload),
                    ..
                } => plain(layouts, *payload, 0),
                _ => true,
            })
        })
}

pub(super) fn empty_option<S>(unit: &LirUnit<S>, layouts: &LayoutTable, mut value: ValueId) -> bool {
    let layout = unit.types[unit.values[value.index()].ty.index()].layout;
    let Some(record) = layouts.layout(layout) else {
        return false;
    };
    if record.kind != LayoutKind::Variant
        || record.finalizer.is_some()
        || !record.tag.as_ref().is_some_and(|tag| {
            tag.variants
                .iter()
                .any(|variant| variant.ordinal == 0 && variant.fields.is_empty())
        })
    {
        return false;
    }
    let mut seen = BTreeSet::new();
    while seen.insert(value) {
        if unit.types[unit.values[value.index()].ty.index()].layout != layout {
            return false;
        }
        let Some(inst) = unit
            .instructions
            .iter()
            .find(|inst| inst.defs().contains(&value))
        else {
            return false;
        };
        match *inst {
            Inst::Construct { construct, .. } => {
                let site = &unit.tables.constructs[construct.index()];
                return matches!(site.constructor, ConstructorKind::Variant { definition, variant: 0 }
                    if unit.tables.symbols[definition.index()].definition.is_core("Option"))
                    && site.layout == layout
                    && site
                        .fields
                        .get(&unit.pools.fields)
                        .is_some_and(|fields| fields.is_empty());
            }
            Inst::Read { reaching, .. } => value = reaching,
            Inst::Store { value: stored, .. } => value = stored,
            Inst::Recast { value: source, .. } => value = source,
            _ => return false,
        }
    }
    false
}

fn channel_none_argument<S>(
    unit: &LirUnit<S>,
    layouts: &LayoutTable,
    call: &CallSite,
    parameter: u32,
    value: ValueId,
) -> bool {
    parameter == 1
        && call.pretransfer_failure.is_none()
        && matches!(call.callee, Callee::Operation(operation)
            if unit.tables.operations[operation.index()].definition.is_core("$intrinsic.ChannelOpen"))
        && empty_option(unit, layouts, value)
}

#[derive(Clone, Copy)]
pub(super) struct ChannelCall {
    pub send: bool,
    pub address: AddressId,
    pub layout: LayoutId,
}

pub(super) fn channel_call<S>(
    unit: &LirUnit<S>,
    layouts: &LayoutTable,
    call: &CallSite,
) -> Option<ChannelCall> {
    use crate::layout_types::{HandleKind, ScalarKind, ScanStep};
    let Callee::Operation(operation) = call.callee else {
        return None;
    };
    let definition = &unit.tables.operations.get(operation.index())?.definition;
    let send = if definition.is_core("$intrinsic.ChannelSend") {
        true
    } else if definition.is_core("$intrinsic.ChannelReceive") {
        false
    } else {
        return None;
    };
    if call.pretransfer_failure.is_some() {
        return None;
    }
    let result = call.result?;
    let result_layout = unit.types[unit.values[result.index()].ty.index()].layout;
    if layouts.layout(result_layout)?.kind != LayoutKind::Scalar(ScalarKind::U32) {
        return None;
    }
    let arguments = call.arguments.get(&unit.pools.call_arguments)?;
    if arguments.len() != 2 {
        return None;
    }
    let address = |parameter| {
        let argument = arguments
            .iter()
            .find(|argument| argument.parameter == parameter)?;
        match argument.value {
            ArgumentValue::Address { address, .. }
                if argument.passing == hirsel_core_contract::Passing::Mut =>
            {
                Some(address)
            }
            _ => None,
        }
    };
    let endpoint = address(0)?;
    let message = address(1)?;
    let endpoint = unit.types[unit.addresses[endpoint.index()].ty.index()].layout;
    let layout = unit.types[unit.addresses[message.index()].ty.index()].layout;
    if !channel_handle(layouts, endpoint)
        || !static_resource_tree(layouts, layout, Selection::Resources, &mut BTreeSet::new())
    {
        return None;
    }
    let record = layouts.layout(layout)?;
    let tag = record.tag.as_ref()?;
    if record.kind != LayoutKind::Variant || record.finalizer.is_some() || tag.variants.len() != 2 {
        return None;
    }
    let none = tag.variants.iter().find(|variant| variant.ordinal == 0)?;
    let some = tag.variants.iter().find(|variant| variant.ordinal == 1)?;
    let [field] = some.fields.as_slice() else {
        return None;
    };
    if !none.fields.is_empty() || field.ordinal != 0 {
        return None;
    }
    let [ScanStep::Handle { kind, obtains, .. }] = layouts.layout(endpoint)?.scan.as_slice() else {
        return None;
    };
    if (send && (*kind != HandleKind::Sender || obtains.is_some()))
        || (!send && (*kind != HandleKind::Receiver || *obtains != Some(field.layout)))
    {
        return None;
    }
    Some(ChannelCall {
        send,
        address: message,
        layout,
    })
}

pub(super) fn channel_alias<S>(
    unit: &LirUnit<S>,
    layouts: &LayoutTable,
    observation: &Observation,
) -> Option<ChannelCall> {
    if observation.marker.kind != Kind::Alias {
        return None;
    }
    channel_alias_at(
        unit,
        layouts,
        observation.point,
        observation.marker.token,
        observation.marker.owner,
    )
}

pub(super) fn channel_alias_at<S>(
    unit: &LirUnit<S>,
    layouts: &LayoutTable,
    point: Point,
    token: Location,
    owner: Location,
) -> Option<ChannelCall> {
    if owner != Location::Parameter(1) {
        return None;
    }
    let Point::CallEntered(block) = point else {
        return None;
    };
    let Terminator::Call { call, .. } = *unit.terminators.get(block.index())? else {
        return None;
    };
    let call = channel_call(unit, layouts, unit.tables.calls.get(call.index())?)?;
    (token == Location::Address(call.address)).then_some(call)
}

// Only a complete program request can additionally prove the callback's code
// and empty Resource environment. The ordinary callable ABI still checks every
// data argument; this predicate cannot authorize any Resource transport.
pub(super) fn data_callback<S>(
    unit: &LirUnit<S>,
    layouts: &LayoutTable,
    selection: Selection,
    call: &CallSite,
) -> bool {
    if selection != Selection::Resources
        || call.callee.value().is_none()
        || call.pretransfer_failure.is_some()
        || !layouts
            .fn_abis
            .get(call.abi.0 as usize)
            .is_some_and(|abi| abi.multiplicity != crate::abi_types::CallMultiplicity::Once)
    {
        return false;
    }
    let inert = |layout| {
        !selection.tracks_layout(layouts, layout)
            || plain(layouts, layout, 0)
            || resource_free_handle(layouts, layout)
    };
    call.result
        .is_none_or(|value| inert(unit.types[unit.values[value.index()].ty.index()].layout))
        && call
            .arguments
            .get(&unit.pools.call_arguments)
            .is_some_and(|arguments| {
                arguments.iter().all(|argument| {
                    inert(match argument.value {
                        ArgumentValue::Value(value) => {
                            unit.types[unit.values[value.index()].ty.index()].layout
                        }
                        ArgumentValue::Address { address, .. } => {
                            unit.types[unit.addresses[address.index()].ty.index()].layout
                        }
                    })
                })
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
    selection: Selection,
    call: &CallSite,
    codes: Option<&[calls::Code]>,
    empty: Option<&[bool]>,
) -> Result<(), String> {
    let carried = |value: ValueId| {
        let info = &unit.values[value.index()];
        info.holding == Holding::Owned
            && selection.tracks_layout(layouts, unit.types[info.ty.index()].layout)
            && !resource_free_handle(layouts, unit.types[info.ty.index()].layout)
    };
    let channel = (selection == Selection::Resources)
        .then(|| channel_call(unit, layouts, call))
        .flatten();
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
                if argument.passing == hirsel_core_contract::Passing::Borrow
                    && channel_none_argument(unit, layouts, call, argument.parameter, value)
                {
                    continue;
                }
                let layout = unit.types[unit.values[value.index()].ty.index()].layout;
                let inert = !selection.tracks_layout(layouts, layout)
                    || plain(layouts, layout, 0)
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
                if selection.tracks_layout(
                    layouts,
                    unit.types[unit.addresses[address.index()].ty.index()].layout,
                ) && !resource_free_handle(
                    layouts,
                    unit.types[unit.addresses[address.index()].ty.index()].layout,
                ) && !plain(
                    layouts,
                    unit.types[unit.addresses[address.index()].ty.index()].layout,
                    0,
                ) =>
            {
                if channel.is_some_and(|call| call.address == address && argument.parameter == 1) {
                    continue;
                }
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
    fn channel_creation_empty_image_requires_the_exact_none_origin_through_memory_ssa() {
        use crate::{abi_types::FnAbiId, objects::UnitKey, testing::UnitBuilder};
        let hash = ContentHash::of_bytes(b"channel-none-origin");
        let mut b = UnitBuilder::new(LirUnit::<Ready>::new(UnitHeader {
            source_count: 0,
            track_caller: false,
            stage: crate::stage::StageKind::Ready,
            lir_version: crate::LIR_VERSION,
            origin: UnitOrigin::Library,
            key: UnitKey(hash),
            definition: hirsel_syntax::DefPath::core("channel_none_origin"),
            implementation_revision: hash,
            instance: hash,
            target: hash,
            abi: FnAbiId(0),
        }));
        let mut table = LayoutTable::new(&TargetSpec::x86_64_unknown_linux_gnu());
        table.layouts = vec![record(LayoutKind::Resource), record(LayoutKind::Variant)];
        table.layouts[1].tag = Some(TagEncoding {
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
        let block = b.block(BlockKind::Body, &[]);
        let symbol = b.symbol(SymbolKind::Type, hirsel_syntax::DefPath::core("Option"));
        let none = b.owned(ValueRepr::Aggregate, LayoutId(1));
        let construct = b.construct(
            ConstructorKind::Variant {
                definition: symbol,
                variant: 0,
            },
            LayoutId(1),
            &[],
            Placement::Fixed(StorageOwner::Heap(TaskDomainId(0))),
        );
        b.push(
            block,
            Inst::Construct {
                result: none,
                construct,
                failure: FailureId(0),
            },
        );
        let slot = b.slot(ValueRepr::Aggregate, LayoutId(1), SlotKind::Temporary);
        let address = b.address(
            AddressBase::Slot(slot),
            &[],
            ValueRepr::Aggregate,
            LayoutId(1),
        );
        let version = b.plain(ValueRepr::Aggregate, LayoutId(1));
        b.push(
            block,
            Inst::Store {
                address,
                value: none,
                version,
            },
        );
        let borrowed = b.plain(ValueRepr::Aggregate, LayoutId(1));
        b.push(
            block,
            Inst::Read {
                result: borrowed,
                address,
                reaching: version,
                mode: ReadMode::Borrow,
            },
        );
        b.ret(block, None);
        let unit = b.finish();
        assert!(empty_option(&unit, &table, none));
        assert!(empty_option(&unit, &table, borrowed));
        let mut some = unit.clone();
        some.tables.constructs[construct.index()].constructor = ConstructorKind::Variant {
            definition: symbol,
            variant: 1,
        };
        assert!(!empty_option(&some, &table, borrowed));
        let mut other = unit.clone();
        other.tables.symbols[symbol.index()].definition = hirsel_syntax::DefPath::core("Other");
        assert!(!empty_option(&other, &table, borrowed));
        let mut cycle = unit;
        if let Inst::Read { reaching, .. } = &mut cycle.instructions[2] {
            *reaching = borrowed;
        }
        assert!(!empty_option(&cycle, &table, borrowed));
    }

    #[test]
    fn resource_selection_separates_typed_endpoints_from_their_queue_payload() {
        let mut table = LayoutTable::new(&TargetSpec::x86_64_unknown_linux_gnu());
        table.layouts = vec![
            record(LayoutKind::Resource),
            record(LayoutKind::Scalar(ScalarKind::U64)),
            record(LayoutKind::Handle),
        ];
        let endpoint = LayoutId(2);
        table.layouts[2].fields = vec![FieldLayout {
            ordinal: 0,
            offset: 0,
            layout: LayoutId(1),
            unaligned: false,
        }];
        table.layouts[2].drop = vec![DropStep::CloseHandle { offset: 0 }];
        for kind in [HandleKind::Sender, HandleKind::Receiver] {
            table.layouts[2].scan = vec![ScanStep::Handle {
                offset: 0,
                kind,
                ty: Some(endpoint),
                obtains: (kind == HandleKind::Receiver).then_some(LayoutId(0)),
            }];
            assert!(channel_handle(&table, endpoint));
            assert!(!Selection::Resources.tracks_layout(&table, endpoint));
            assert!(Selection::Owned.tracks_layout(&table, endpoint));
            assert!(Selection::Resources.tracks_layout(&table, LayoutId(0)));
            assert_eq!(
                plain_channel_handle(&table, endpoint),
                kind == HandleKind::Sender
            );
            if let ScanStep::Handle { ty, .. } = &mut table.layouts[2].scan[0] {
                *ty = None;
            }
            assert!(!channel_handle(&table, endpoint));
            assert!(Selection::Resources.tracks_layout(&table, endpoint));
        }
    }

    #[test]
    fn resource_selection_does_not_follow_flattened_queue_obtains_as_inline_storage() {
        let mut table = LayoutTable::new(&TargetSpec::x86_64_unknown_linux_gnu());
        table.layouts = vec![
            record(LayoutKind::Resource),
            record(LayoutKind::Scalar(ScalarKind::U64)),
            record(LayoutKind::Handle),
            record(LayoutKind::Aggregate),
            record(LayoutKind::Resource),
        ];
        table.layouts[2].fields = vec![FieldLayout {
            ordinal: 0,
            offset: 0,
            layout: LayoutId(1),
            unaligned: false,
        }];
        table.layouts[2].drop = vec![DropStep::CloseHandle { offset: 0 }];
        table.layouts[2].scan = vec![ScanStep::Handle {
            offset: 0,
            kind: HandleKind::Receiver,
            ty: Some(LayoutId(2)),
            obtains: Some(LayoutId(0)),
        }];
        table.layouts[3].fields = vec![FieldLayout {
            ordinal: 0,
            offset: 0,
            layout: LayoutId(2),
            unaligned: false,
        }];
        table.layouts[3].scan = table.layouts[2].scan.clone();
        assert!(!Selection::Resources.tracks_layout(&table, LayoutId(3)));
        table.layouts[3].fields.push(FieldLayout {
            ordinal: 1,
            offset: 8,
            layout: LayoutId(0),
            unaligned: false,
        });
        assert!(
            Selection::Resources.tracks_layout(&table, LayoutId(3)),
            "an inline Resource also referenced by the queue must remain tracked"
        );
        table.layouts[3].fields.pop();
        if let ScanStep::Handle { obtains, .. } = &mut table.layouts[3].scan[0] {
            *obtains = Some(LayoutId(4));
        }
        assert!(
            Selection::Resources.tracks_layout(&table, LayoutId(3)),
            "a forged queue schema must not hide a Resource"
        );
    }

    #[test]
    fn plain_channel_endpoints_require_their_own_schema_and_exact_retirement() {
        let mut table = LayoutTable::new(&TargetSpec::x86_64_unknown_linux_gnu());
        table.layouts = vec![
            record(LayoutKind::Unit),
            record(LayoutKind::Resource),
            record(LayoutKind::Handle),
            record(LayoutKind::Scalar(ScalarKind::U64)),
        ];
        let channel = LayoutId(2);
        for kind in [HandleKind::Sender, HandleKind::Receiver] {
            for corruption in [
                "none",
                "foreign type",
                "unknown type",
                "resource",
                "recursive",
                "extra scan",
                "drop",
                "missing word",
                "owned word",
                "misplaced word",
            ] {
                let record = &mut table.layouts[2];
                record.fields = vec![FieldLayout {
                    ordinal: 0,
                    offset: 0,
                    layout: LayoutId(3),
                    unaligned: false,
                }];
                record.drop = vec![DropStep::CloseHandle { offset: 0 }];
                record.scan = vec![ScanStep::Handle {
                    offset: 0,
                    kind,
                    ty: Some(channel),
                    obtains: (kind == HandleKind::Receiver).then_some(LayoutId(0)),
                }];
                match corruption {
                    "none" => (),
                    "foreign type" => {
                        if let ScanStep::Handle { ty, .. } = &mut record.scan[0] {
                            *ty = Some(LayoutId(0));
                        }
                    }
                    "unknown type" => {
                        if let ScanStep::Handle { ty, .. } = &mut record.scan[0] {
                            *ty = None;
                        }
                    }
                    "resource" => {
                        if let ScanStep::Handle { obtains, .. } = &mut record.scan[0] {
                            *obtains = Some(LayoutId(1));
                        }
                    }
                    "recursive" => {
                        if let ScanStep::Handle { obtains, .. } = &mut record.scan[0] {
                            *obtains = Some(channel);
                        }
                    }
                    "extra scan" => record.scan.push(record.scan[0].clone()),
                    "drop" => record.drop.clear(),
                    "missing word" => record.fields.clear(),
                    "owned word" => record.fields[0].layout = LayoutId(1),
                    "misplaced word" => record.fields[0].offset = 8,
                    _ => unreachable!(),
                }
                assert_eq!(
                    plain_channel_handle(&table, channel),
                    corruption == "none",
                    "{kind:?}/{corruption}"
                );
            }
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

    #[test]
    fn resource_selection_checks_recursive_mutex_payloads_and_keeps_owned_and_channel_custody() {
        let mut table = LayoutTable::new(&TargetSpec::x86_64_unknown_linux_gnu());
        table.layouts = vec![
            record(LayoutKind::Unit),
            record(LayoutKind::Resource),
            record(LayoutKind::Variant),
            record(LayoutKind::Handle),
            record(LayoutKind::Shared {
                payload: LayoutId(3),
            }),
            record(LayoutKind::Block {
                payload: LayoutId(0),
            }),
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
                        layout: LayoutId(5),
                        unaligned: false,
                    }],
                },
            ],
        });
        table.layouts[3].scan = vec![ScanStep::Handle {
            offset: 0,
            kind: HandleKind::Mutex,
            ty: Some(LayoutId(4)),
            obtains: Some(LayoutId(2)),
        }];
        assert!(!Selection::Resources.tracks_layout(&table, LayoutId(4)));
        assert!(Selection::Owned.tracks_layout(&table, LayoutId(4)));
        assert!(!resource_free_handle(&table, LayoutId(3)));

        // A Resource hidden behind the block, or in any variant arm, is tracked.
        table.layouts[5].kind = LayoutKind::Block {
            payload: LayoutId(1),
        };
        assert!(Selection::Resources.tracks_layout(&table, LayoutId(4)));
        table.layouts[5].kind = LayoutKind::Block {
            payload: LayoutId(0),
        };
        table.layouts[2].tag.as_mut().unwrap().variants[1].fields[0].layout = LayoutId(1);
        assert!(Selection::Resources.tracks_layout(&table, LayoutId(4)));
        table.layouts[2].tag.as_mut().unwrap().variants[1].fields[0].layout = LayoutId(5);

        for kind in [
            HandleKind::Sender,
            HandleKind::Receiver,
            HandleKind::Unknown,
        ] {
            table.layouts[3].scan = vec![ScanStep::Handle {
                offset: 0,
                kind,
                ty: Some(LayoutId(4)),
                obtains: Some(LayoutId(2)),
            }];
            assert!(Selection::Resources.tracks_layout(&table, LayoutId(4)));
        }
    }
    #[test]
    fn empty_variant_retirement_requires_one_incoming_occurrence_even_when_an_edge_is_reused() {
        use crate::{abi_types::FnAbiId, objects::UnitKey, testing::UnitBuilder};
        for alternate in ["none", "otherwise", "predecessor", "failure"] {
            let hash = ContentHash::of_bytes(b"empty-variant-retirement");
            let mut b = UnitBuilder::new(LirUnit::<Ready>::new(UnitHeader {
                source_count: 0,
                track_caller: false,
                stage: crate::stage::StageKind::Ready,
                lir_version: crate::LIR_VERSION,
                origin: UnitOrigin::Source,
                key: UnitKey(hash),
                definition: hirsel_syntax::DefPath::core("empty_variant_retirement"),
                implementation_revision: hash,
                instance: hash,
                target: hash,
                abi: FnAbiId(0),
            }));
            let entry = b.block(BlockKind::Body, &[]);
            let empty = b.block(BlockKind::Cleanup, &[]);
            let other = b.block(BlockKind::Body, &[]);
            let slot = b.slot(ValueRepr::Aggregate, LayoutId(0), SlotKind::Temporary);
            let address = b.address(
                AddressBase::Slot(slot),
                &[],
                ValueRepr::Aggregate,
                LayoutId(0),
            );
            let borrowed = b.plain(ValueRepr::Aggregate, LayoutId(0));
            b.push(
                entry,
                Inst::Read {
                    result: borrowed,
                    address,
                    reaching: borrowed,
                    mode: ReadMode::Borrow,
                },
            );
            let value = b.owned(ValueRepr::Aggregate, LayoutId(0));
            b.push(
                empty,
                Inst::Read {
                    result: value,
                    address,
                    reaching: borrowed,
                    mode: ReadMode::Take,
                },
            );
            b.push(empty, Inst::Forget { value });
            let selected = b.edge(empty, &[]);
            let otherwise = b.edge(other, &[]);
            let arms = PoolRange::push(
                &mut b.unit.pools.switch_arms,
                [SwitchArm {
                    value: 0,
                    edge: selected,
                }],
            );
            b.term(
                entry,
                Terminator::Switch {
                    value: borrowed,
                    arms,
                    otherwise: if alternate == "otherwise" {
                        selected
                    } else {
                        otherwise
                    },
                },
            );
            b.ret(empty, None);
            if alternate == "predecessor" {
                b.term(other, Terminator::Jump(selected));
            } else {
                if alternate == "failure" {
                    let failure = b.failure(selected, None, &[]);
                    b.push(other, Inst::Address { address, failure });
                }
                b.ret(other, None);
            }
            let unit = b.finish();
            let mut table = LayoutTable::new(&TargetSpec::x86_64_unknown_linux_gnu());
            let mut variant = record(LayoutKind::Variant);
            variant.tag = Some(TagEncoding {
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
                            layout: LayoutId(1),
                            unaligned: false,
                        }],
                    },
                ],
            });
            table.layouts = vec![variant, record(LayoutKind::Resource)];
            assert_eq!(
                super::super::variants::empty_retirement(&unit, &table, empty, 1, value),
                alternate == "none",
                "{alternate}"
            );
        }
    }
}

/// Prove resource-free callable environments from checked capture equations.
/// Physical callable layout equality never identifies the captured code.
pub(super) fn empty_callables(
    units: &[&ValidatedUnit<Ready>],
    layouts: &LayoutTable,
    selection: Selection,
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
        selection: Selection,
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
        // Lowering can retain a data-only aggregate after eliding a lexical
        // environment. The complete checked layout graph proves its selection
        // without inventing capture slots on the environment-free callee.
        if !selection.tracks_layout(layouts, environment) {
            return true;
        }
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
            !selection.tracks_layout(layouts, layout)
                || plain(layouts, layout, 0)
                || resource_free_handle(layouts, layout)
                || (body.types[body.values[value.index()].ty.index()].repr == ValueRepr::Callable
                    && codes[index][value.index()]
                        .targets()
                        .is_some_and(|targets| {
                            targets.iter().all(|target| {
                                empty(target, units, layouts, selection, codes, symbols, active)
                            })
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
                                selection,
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
