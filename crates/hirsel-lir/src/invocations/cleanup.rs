//! Cleanup calls are reconstructed from typed finalizer declarations. Their
//! admissions are separate from the source calls and canonical library recipe.
use super::ReferenceInvocation;
use crate::{LayoutTable, ir::*, operation::OperationDeclarations, stage::Stage};

pub(super) fn targets<S: Stage>(
    unit: &LirUnit<S>,
    declarations: &dyn OperationDeclarations,
    layouts: &LayoutTable,
    body: u32,
) -> Result<Vec<Target>, String> {
    let mut references = Vec::new();
    for (block, row) in unit.blocks.iter().enumerate() {
        let Some(Terminator::Call { call, .. }) = unit.terminators.get(block) else {
            continue;
        };
        let site = unit
            .tables
            .calls
            .get(call.index())
            .ok_or("cleanup call outside table")?;
        let Callee::Direct(symbol) = site.callee else {
            continue;
        };
        let symbol = unit
            .tables
            .symbols
            .get(symbol.index())
            .ok_or("cleanup symbol outside table")?;
        let Some(instance) = symbol.instance else {
            continue;
        };
        let instructions = row
            .instructions
            .get(&unit.instructions)
            .ok_or("cleanup instruction range")?;
        let Some((address, anchor)) = instructions.iter().rev().find_map(|inst| match inst {
            Inst::FinalizeBegin { address, anchor } => Some((*address, *anchor)),
            _ => None,
        }) else {
            continue;
        };
        let address_info = unit
            .addresses
            .get(address.index())
            .ok_or("cleanup address outside table")?;
        let ty = unit
            .types
            .get(address_info.ty.index())
            .ok_or("cleanup type outside table")?;
        let layout = layouts
            .layout(ty.layout)
            .ok_or("cleanup layout outside table")?;
        let Some(finalizer) = &layout.finalizer else {
            continue;
        };
        if finalizer.definition != symbol.definition {
            continue;
        }
        // Only a typed cleanup target consumes independent body identity.
        // Ordinary direct calls must not record an absent peer body before
        // discovery later publishes that peer's source selection.
        let Some(selected) = declarations.invocation_body(&symbol.definition, instance) else {
            continue;
        };
        if finalizer.body != selected {
            continue;
        }
        let cleanup = unit
            .tables
            .cleanup_anchors
            .get(anchor.index())
            .ok_or("cleanup anchor outside table")?;
        let origin = unit
            .tables
            .source_anchors
            .get(site.anchor.index())
            .ok_or("cleanup origin outside table")?;
        if cleanup.body != body
            || origin.body != body
            || origin.block != u32::MAX
            || origin.statement != site.anchor.0
            || origin.site != cleanup.site
        {
            return Err("finalizer invocation differs from its cleanup origin".into());
        }
        let arguments = site
            .arguments
            .get(&unit.pools.call_arguments)
            .ok_or("cleanup argument range")?;
        let [argument] = arguments else {
            return Err("finalizer invocation has extra arguments".into());
        };
        let ArgumentValue::Value(value) = argument.value else {
            return Err("source finalizer invocation must borrow its resource".into());
        };
        if argument.passing != hirsel_core_contract::Passing::Borrow || !instructions.iter().any(|inst| {
            matches!(inst, Inst::Read { result, address: at, mode: ReadMode::Borrow, .. } if *result == value && *at == address)
        }) {
            return Err("finalizer invocation borrows another resource".into());
        }
        let resource = resource(unit, layouts, address)?;
        references.push(Target {
            address,
            resource,
            reference: ReferenceInvocation {
                body,
                block: u32::MAX,
                statement: site.anchor.0,
                restart: false,
                primitive: None,
                callee: Some(finalizer.definition.clone()),
            },
        });
    }
    Ok(references)
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct Resource {
    slot: SlotId,
    path: Vec<(u8, u64, u64)>,
}
#[derive(Clone, Debug)]
pub(super) struct Target {
    pub reference: ReferenceInvocation,
    pub address: AddressId,
    pub(super) resource: Resource,
}
impl Target {
    pub fn descendant_of(&self, parent: &Self) -> bool {
        self.resource.slot == parent.resource.slot
            && self.resource.path.len() > parent.resource.path.len()
            && self.resource.path.starts_with(&parent.resource.path)
    }
    pub fn identity(&self) -> (u32, u32, u32) {
        (
            self.reference.body,
            self.reference.block,
            self.reference.statement,
        )
    }
}

pub(super) fn references<S: Stage>(
    unit: &LirUnit<S>,
    declarations: &dyn OperationDeclarations,
    layouts: &LayoutTable,
    body: u32,
) -> Result<Vec<ReferenceInvocation>, String> {
    Ok(targets(unit, declarations, layouts, body)?
        .into_iter()
        .map(|target| target.reference)
        .collect())
}

/// Decode only owned inline containment, using the supplied layouts and actual
/// address projections. Neither offsets nor a producer's nesting claim proves
/// containment; counted payloads and borrowed fields are separate obligations.
fn resource<S: Stage>(
    unit: &LirUnit<S>,
    layouts: &LayoutTable,
    address: AddressId,
) -> Result<Resource, String> {
    use crate::layout_types::LayoutKind;
    let info = unit
        .addresses
        .get(address.index())
        .ok_or("cleanup resource address")?;
    let AddressBase::Slot(slot) = info.base else {
        return Err("cleanup resource is not slot storage".into());
    };
    let slot_info = unit
        .slots
        .get(slot.index())
        .ok_or("cleanup resource slot")?;
    let mut layout = unit
        .types
        .get(slot_info.ty.index())
        .ok_or("cleanup resource type")?
        .layout;
    let mut path = Vec::new();
    for step in info
        .projection
        .get(&unit.pools.projections)
        .ok_or("cleanup resource projection")?
    {
        let record = layouts.layout(layout).ok_or("cleanup resource layout")?;
        match *step {
            ProjectionStep::Field(ordinal) => {
                let field = record
                    .fields
                    .iter()
                    .find(|field| field.ordinal == ordinal)
                    .ok_or("cleanup resource field")?;
                if crate::layout::field_is_borrowed(layouts, record, field) {
                    return Err("cleanup resource field is borrowed".into());
                }
                path.push((0, u64::from(ordinal), 0));
                layout = field.layout;
            }
            ProjectionStep::VariantField { variant, field } => {
                let field_info = record
                    .tag
                    .as_ref()
                    .and_then(|tag| tag.variants.iter().find(|row| row.ordinal == variant))
                    .and_then(|row| row.fields.iter().find(|row| row.ordinal == field))
                    .ok_or("cleanup resource variant field")?;
                path.push((1, u64::from(variant), u64::from(field)));
                layout = field_info.layout;
            }
            ProjectionStep::Index(index) => {
                let LayoutKind::Inline { element, count } = record.kind else {
                    return Err("cleanup resource crosses dynamic or counted storage".into());
                };
                let constant = unit
                    .instructions
                    .iter()
                    .find_map(|inst| match *inst {
                        Inst::Const { result, constant } if result == index => {
                            unit.tables.constants.get(constant.index())
                        }
                        _ => None,
                    })
                    .and_then(|constant| match constant.value {
                        ConstantValue::Integer(value) => u64::try_from(value).ok(),
                        _ => None,
                    })
                    .filter(|&value| value < count)
                    .ok_or("cleanup resource inline index")?;
                path.push((2, constant, 0));
                layout = element;
            }
            _ => return Err("cleanup resource crosses non-owned inline storage".into()),
        }
    }
    if unit
        .types
        .get(info.ty.index())
        .is_none_or(|ty| ty.layout != layout)
    {
        return Err("cleanup resource projection layout differs".into());
    }
    Ok(Resource { slot, path })
}
