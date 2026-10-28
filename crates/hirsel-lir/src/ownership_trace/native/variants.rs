//! Static variant declaration paths and the possible absence of Resource children.
use super::*;

pub(super) fn constructed_field<S>(
    unit: &LirUnit<S>,
    location: NativeLocation,
) -> Result<Option<NativePath>, String> {
    let value = ValueId(location.index);
    let Some(site) = unit
        .instructions
        .iter()
        .find_map(|instruction| match instruction {
            Inst::Construct {
                result, construct, ..
            } if *result == value => unit.tables.constructs.get(construct.index()),
            _ => None,
        })
    else {
        return Ok(None);
    };
    let ConstructorKind::Variant { variant, .. } = site.constructor else {
        return Ok(None);
    };
    let field = location.kind - 8;
    if !site
        .fields
        .get(&unit.pools.fields)
        .is_some_and(|fields| fields.iter().any(|at| at.field == field))
    {
        return Err("ownership variant field has no checked constructor operand".into());
    }
    Ok(Some(NativePath {
        root: NativeLocation {
            kind: 0,
            index: value.0,
        },
        fields: vec![NativeProjection::VariantField { variant, field }],
    }))
}

pub(super) fn may_be_empty(table: &LayoutTable, root: LayoutId) -> bool {
    fn walk(table: &LayoutTable, layout: LayoutId, depth: usize) -> bool {
        if depth > table.layouts.len() {
            return false;
        }
        if !Selection::Resources.tracks_layout(table, layout) {
            return true;
        }
        let Some(record) = table.layout(layout) else {
            return false;
        };
        if record.finalizer.is_some() {
            return false;
        }
        match record.kind {
            LayoutKind::Aggregate => record
                .fields
                .iter()
                .all(|field| walk(table, field.layout, depth + 1)),
            LayoutKind::Variant => record.tag.as_ref().is_some_and(|tag| {
                tag.variants.iter().any(|arm| {
                    arm.fields
                        .iter()
                        .all(|field| walk(table, field.layout, depth + 1))
                })
            }),
            _ => false,
        }
    }
    table
        .layout(root)
        .is_some_and(|record| matches!(record.kind, LayoutKind::Aggregate | LayoutKind::Variant))
        && static_resource_tree(table, root, Selection::Resources, &mut BTreeSet::new())
        && walk(table, root, 0)
}

// Cleanup retires an empty active variant with Take/Forget. Prove its exact
// selecting edge and borrowed storage before omitting this Resource-free shell.
pub(super) fn empty_retirement<S: Stage>(
    unit: &LirUnit<S>,
    layouts: &LayoutTable,
    block: BlockId,
    index: usize,
    value: ValueId,
) -> bool {
    let proof = || -> Option<()> {
        let code = unit
            .blocks
            .get(block.index())?
            .instructions
            .get(&unit.instructions)?;
        let Inst::Read {
            result,
            address,
            mode: ReadMode::Take,
            ..
        } = *code.get(index.checked_sub(1)?)?
        else {
            return None;
        };
        if result != value
            || !code[..index - 1].iter().all(|inst| {
                matches!(
                    inst,
                    Inst::EndLoan {
                        if_active: false,
                        ..
                    }
                )
            })
        {
            return None;
        }
        let layout = unit
            .types
            .get(unit.values.get(value.index())?.ty.index())?
            .layout;
        let record = layouts.layout(layout)?;
        if !matches!(record.kind, LayoutKind::Variant) || record.finalizer.is_some() {
            return None;
        }
        // Count actual uses, including a shared EdgeId named twice. A single
        // table row does not prove a unique predecessor or selected variant.
        let mut used = Vec::new();
        for term in &unit.terminators {
            used.extend(term.normal_edges(&unit.pools));
            for failure in term.failure_edges(&unit.tables) {
                used.push(unit.tables.failures.get(failure.index())?.cleanup);
            }
        }
        for inst in &unit.instructions {
            if let Some(failure) = inst.failure() {
                used.push(unit.tables.failures.get(failure.index())?.cleanup);
            }
        }
        let mut incoming = used.into_iter().filter(|edge| {
            unit.tables
                .edges
                .get(edge.index())
                .is_some_and(|edge| edge.target == block)
        });
        let edge = incoming.next()?;
        if incoming.next().is_some() {
            return None;
        }
        let (predecessor, value, arms) =
            unit.terminators.iter().enumerate().find_map(|(at, term)| {
                let Terminator::Switch { value, arms, .. } = term else {
                    return None;
                };
                arms.get(&unit.pools.switch_arms)?
                    .iter()
                    .any(|arm| arm.edge == edge)
                    .then_some((at, *value, *arms))
            })?;
        let parent = unit
            .blocks
            .get(predecessor)?
            .instructions
            .get(&unit.instructions)?;
        let Inst::Read {
            result,
            address: borrowed,
            mode: ReadMode::Borrow,
            ..
        } = *parent.last()?
        else {
            return None;
        };
        if result != value
            || address_path(unit, layouts, borrowed).ok()?
                != address_path(unit, layouts, address).ok()?
        {
            return None;
        }
        let arm = arms
            .get(&unit.pools.switch_arms)?
            .iter()
            .find(|arm| arm.edge == edge)?;
        let variant = record
            .tag
            .as_ref()?
            .variants
            .iter()
            .find(|variant| variant.ordinal == arm.value)?;
        variant
            .fields
            .iter()
            .all(|field| !Selection::Resources.tracks_layout(layouts, field.layout))
            .then_some(())
    };
    proof().is_some()
}
