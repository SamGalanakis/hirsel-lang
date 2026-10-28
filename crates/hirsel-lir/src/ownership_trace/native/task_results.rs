//! Checked task result carriers. A task handle contains no inline result;
//! only the actual join can move the child's retained Option image.
use super::*;
use crate::layout_types::{HandleKind, ScalarKind, ScanStep};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskResultDirection {
    Hold,
    Join,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskResultCall {
    pub direction: TaskResultDirection,
    pub address: AddressId,
    pub layout: LayoutId,
}

/// Match the self-bound typed task word, not an aggregate containing a task.
/// Its obtains edge describes runtime custody and supplies no Resource credit.
pub fn task_result_payload(layouts: &LayoutTable, layout: LayoutId) -> Option<LayoutId> {
    let record = layouts.layout(layout)?;
    if record.kind != LayoutKind::Handle
        || record.finalizer.is_some()
        || !record.drop.is_empty()
        || !matches!(record.fields.as_slice(), [field]
            if field.ordinal == 0 && field.offset == 0 && !field.unaligned
                && layouts.layout(field.layout).is_some_and(|word|
                    word.kind == LayoutKind::Scalar(ScalarKind::U64)
                        && plain(layouts, field.layout, 0)))
    {
        return None;
    }
    let [ScanStep::Handle {
        offset: 0,
        kind: HandleKind::Task,
        ty: Some(schema),
        obtains: Some(payload),
    }] = record.scan.as_slice()
    else {
        return None;
    };
    (*schema == layout && layouts.layout(*payload).is_some()).then_some(*payload)
}

/// Structural matcher for the exact intrinsic and formal Option image. This
/// does not authorize transport; a native result receipt must also bind the
/// issuing and receiving tasks and their separately checked marker intents.
pub fn task_result_call<S>(
    unit: &LirUnit<S>,
    layouts: &LayoutTable,
    call: &CallSite,
) -> Option<TaskResultCall> {
    let Callee::Operation(operation) = call.callee else {
        return None;
    };
    if call.pretransfer_failure.is_some() || call.result.is_some() {
        return None;
    }
    let definition = &unit.tables.operations.get(operation.index())?.definition;
    let direction = if definition.is_core("$intrinsic.TaskFinish") {
        TaskResultDirection::Hold
    } else if definition.is_core("$intrinsic.TaskJoin") {
        TaskResultDirection::Join
    } else {
        return None;
    };
    let arguments = call.arguments.get(&unit.pools.call_arguments)?;
    if arguments.len() != 4
        || (0..4).any(|parameter| arguments.iter().filter(|arg| arg.parameter == parameter).count() != 1)
    {
        return None;
    }
    let parameter = match direction {
        TaskResultDirection::Hold => 0,
        TaskResultDirection::Join => 2,
    };
    let argument = arguments.iter().find(|arg| arg.parameter == parameter)?;
    let ArgumentValue::Address { address, .. } = argument.value else {
        return None;
    };
    if argument.passing != hirsel_core_contract::Passing::Mut {
        return None;
    }
    let layout = unit.types.get(unit.addresses.get(address.index())?.ty.index())?.layout;
    let record = layouts.layout(layout)?;
    let tag = record.tag.as_ref()?;
    if record.kind != LayoutKind::Variant
        || record.finalizer.is_some()
        || tag.variants.len() != 2
        || !static_resource_tree(layouts, layout, Selection::Resources, &mut BTreeSet::new())
    {
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
    match direction {
        TaskResultDirection::Hold => {
            let none = arguments.iter().find(|arg| arg.parameter == 1)?;
            let ArgumentValue::Value(value) = none.value else {
                return None;
            };
            if none.passing != hirsel_core_contract::Passing::Borrow
                || unit.types.get(unit.values.get(value.index())?.ty.index())?.layout != layout
                || !runtime_calls::empty_option(unit, layouts, value)
            {
                return None;
            }
        }
        TaskResultDirection::Join => {
            let task_argument = arguments.iter().find(|arg| arg.parameter == 1)?;
            let ArgumentValue::Value(value) = task_argument.value else {
                return None;
            };
            let task = unit.types.get(unit.values.get(value.index())?.ty.index())?.layout;
            if task_argument.passing != hirsel_core_contract::Passing::Take
                || task_result_payload(layouts, task) != Some(field.layout)
            {
                return None;
            }
        }
    }
    Some(TaskResultCall { direction, address, layout })
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
            abi: AbiInputs { scalars: vec![], unaligned: false },
        }
    }

    #[test]
    fn typed_task_result_requires_its_own_exact_word_schema() {
        let mut table = LayoutTable::new(&TargetSpec::x86_64_unknown_linux_gnu());
        table.layouts = vec![
            record(LayoutKind::Resource),
            record(LayoutKind::Scalar(ScalarKind::U64)),
            record(LayoutKind::Handle),
        ];
        table.layouts[2].fields.push(FieldLayout {
            ordinal: 0, offset: 0, layout: LayoutId(1), unaligned: false,
        });
        table.layouts[2].scan.push(ScanStep::Handle {
            offset: 0, kind: HandleKind::Task, ty: Some(LayoutId(2)), obtains: Some(LayoutId(0)),
        });
        assert_eq!(task_result_payload(&table, LayoutId(2)), Some(LayoutId(0)));
        for mutation in 0..7 {
            let mut forged = table.clone();
            match mutation {
                0 => forged.layouts[2].kind = LayoutKind::Aggregate,
                1 => forged.layouts[2].fields[0].offset = 8,
                2 => forged.layouts[2].fields[0].layout = LayoutId(0),
                3 => forged.layouts[2].drop.push(DropStep::CloseHandle { offset: 0 }),
                4 => if let ScanStep::Handle { ty, .. } = &mut forged.layouts[2].scan[0] {
                    *ty = Some(LayoutId(1));
                },
                5 => if let ScanStep::Handle { kind, .. } = &mut forged.layouts[2].scan[0] {
                    *kind = HandleKind::TaskSet;
                },
                6 => if let ScanStep::Handle { obtains, .. } = &mut forged.layouts[2].scan[0] {
                    *obtains = Some(LayoutId(99));
                },
                _ => unreachable!(),
            }
            assert_eq!(task_result_payload(&forged, LayoutId(2)), None, "mutation {mutation}");
        }
    }
}
