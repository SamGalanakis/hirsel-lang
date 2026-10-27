//! Import original allocation obligations before placement or elimination.
//! The expected set comes from original Lower, never surviving constructors.
use super::{SourceError, SourceExpectation};
use hirsel_lir::ir::*;
use hirsel_lir::operation::OperationDeclarations;
use hirsel_lir::sealed_payload::*;
use hirsel_lir::stage::Value;
use hirsel_lir::{LayoutTable, TargetSpec};
use std::collections::{BTreeMap, HashMap};

/// Attach semantic allocation events and original operand bindings. Each event
/// remains an actual use of its payload through later dead-value elimination.
/// Growth/COW and held environments require their own original-source recipes;
/// they cannot be inferred from a surviving allocation's physical address.
pub fn emit(
    mut unit: LirUnit<Value>,
    expected: &SourceExpectation,
    declarations: &dyn OperationDeclarations,
    layouts: &LayoutTable,
    target: &TargetSpec,
) -> Result<LirUnit<Value>, SourceError> {
    let source = expected.source();
    source.check().map_err(SourceError::Malformed)?;
    if source.reference.definition != unit.header.definition
        || source.reference.instance != unit.header.instance
        || source.witness.implementation_revision != unit.header.implementation_revision
    {
        return Err(SourceError::Malformed(
            "inventory source belongs to another instance",
        ));
    }
    if unit
        .instructions
        .iter()
        .any(|i| matches!(i, Inst::ArenaInventory { .. }))
    {
        return Err(SourceError::Malformed("inventory was already imported"));
    }
    let mut constructors = HashMap::<SourceAnchor, Vec<(usize, ValueId, ConstructId)>>::new();
    for (index, inst) in unit.instructions.iter().enumerate() {
        if let Inst::Construct {
            result, construct, ..
        } = *inst
        {
            let site = unit
                .tables
                .constructs
                .get(construct.index())
                .ok_or(SourceError::Malformed("inventory construction site"))?;
            let anchor = unit
                .tables
                .source_anchors
                .get(site.anchor.index())
                .ok_or(SourceError::Malformed("inventory construction anchor"))?;
            constructors
                .entry(anchor.clone())
                .or_default()
                .push((index, result, construct));
        }
    }
    let parameter_slots: HashMap<_, _> = unit
        .slots
        .iter()
        .enumerate()
        .filter_map(|(index, slot)| {
            let SlotKind::Parameter(ordinal) = slot.kind else {
                return None;
            };
            Some((ordinal, SlotId(index as u32)))
        })
        .collect();
    let mut events = HashMap::<_, Vec<_>>::new();
    for event in &source.events {
        events.entry(&event.anchor).or_default().push(event);
    }
    let preparation = unit
        .blocks
        .get(unit.entry.index())
        .and_then(|block| {
            block
                .instructions
                .get(&unit.instructions)
                .map(|code| (block.instructions.start as usize, code))
        })
        .and_then(|(start, code)| {
            code.iter().enumerate().rev().find_map(|(offset, inst)| {
                if let Inst::Address { failure, .. } = inst {
                    Some((start + offset, *failure))
                } else {
                    None
                }
            })
        });
    let mut preparations = vec![];
    let mut after = BTreeMap::new();
    for payload in &source.payloads {
        let candidates =
            constructors
                .get(&payload.recipe.source)
                .ok_or(SourceError::Unsupported(
                    "original payload construction was eliminated before inventory import",
                ))?;
        let [(index, value, construct)] = candidates.as_slice() else {
            return Err(SourceError::Unsupported(
                "original payload needs one construction",
            ));
        };
        let descriptor = declarations
            .seal_descriptor_for_source(
                &unit.header.definition,
                unit.header.instance,
                &payload.root_type,
                layouts,
                target,
            )
            .map_err(|e| SourceError::Type(format!("{e:?}")))?;
        if descriptor.root_layout != unit.types[unit.values[value.index()].ty.index()].layout {
            return Err(SourceError::Malformed(
                "inventory payload layout differs from source",
            ));
        }
        let site = &unit.tables.constructs[construct.index()];
        let values = site
            .fields
            .get(&unit.pools.fields)
            .ok_or(SourceError::Malformed("inventory construction fields"))?
            .iter()
            .map(|f| f.value)
            .collect::<Vec<_>>();
        if values.len() != payload.recipe.inputs.len() {
            return Err(SourceError::Malformed(
                "inventory source operand arity differs",
            ));
        }
        let existing = unit
            .tables
            .seal_payloads
            .iter()
            .filter_map(|table| {
                table
                    .bindings
                    .iter()
                    .find(|binding| binding.recipe == payload.recipe)
                    .map(|binding| (table.descriptor, binding))
            })
            .collect::<Vec<_>>();
        let shape = match existing.as_slice() {
            [(shape, binding)]
                if binding.values == values
                    && unit.tables.seal_descriptors[shape.index()] == descriptor =>
            {
                *shape
            }
            [] => {
                let shape = SealDescriptorId(unit.tables.seal_descriptors.len() as u32);
                unit.tables.seal_descriptors.push(descriptor);
                unit.tables.seal_payloads.push(PayloadTable {
                    descriptor: shape,
                    bindings: vec![PayloadBinding {
                        recipe: payload.recipe.clone(),
                        values,
                    }],
                });
                shape
            }
            _ => {
                return Err(SourceError::Malformed(
                    "imported receipt differs from original inventory",
                ));
            }
        };
        let Some(events) = events.get(&payload.recipe.source) else {
            continue;
        };
        let [event] = events.as_slice() else {
            return Err(SourceError::Malformed(
                "ambiguous original allocation event",
            ));
        };
        if event.kind != SourceEventKind::Allocate || event.root_type != payload.root_type {
            return Err(SourceError::Unsupported(
                "inventory transition needs original mutation correspondence",
            ));
        }
        let PayloadOperation::Allocate {
            destination: PayloadDestination::Arena(arena),
        } = &payload.recipe.operation
        else {
            return Err(SourceError::Malformed(
                "allocation event has no original arena",
            ));
        };
        let PayloadProducer::Parameter(ordinal) = arena.producer else {
            return Err(SourceError::Unsupported(
                "inventory destination needs parameter or capture transport",
            ));
        };
        let reaching = *unit
            .parameters
            .get(ordinal as usize)
            .ok_or(SourceError::Malformed("inventory arena parameter"))?;
        let ty = unit.values[reaching.index()].ty;
        if unit.types[ty.index()].repr != ValueRepr::Arena {
            return Err(SourceError::Malformed(
                "inventory destination is not an Arena",
            ));
        }
        let slot = *parameter_slots.get(&ordinal).ok_or(SourceError::Malformed(
            "inventory arena has no parameter storage",
        ))?;
        let address = AddressId(unit.addresses.len() as u32);
        unit.addresses.push(AddressInfo {
            base: AddressBase::Slot(slot),
            source_place: None,
            unchecked: vec![],
            projection: PoolRange::EMPTY,
            ty,
        });
        let arena = ValueId(unit.values.len() as u32);
        unit.values.push(ValueInfo {
            ty,
            holding: Holding::Plain,
        });
        let (_, failure) = preparation.ok_or(SourceError::Unsupported(
            "inventory needs original parameter address preparation",
        ))?;
        let mut failure = unit.tables.failures[failure.index()].clone();
        let edge = unit.tables.edges[failure.cleanup.index()].clone();
        failure.cleanup = EdgeId(unit.tables.edges.len() as u32);
        unit.tables.edges.push(edge);
        let preparation_failure = FailureId(unit.tables.failures.len() as u32);
        unit.tables.failures.push(failure);
        preparations.push(Inst::Address {
            address,
            failure: preparation_failure,
        });
        let paired = unit
            .instructions
            .iter()
            .enumerate()
            .find_map(|(at, inst)| match *inst {
                Inst::PayloadReceipt {
                    result,
                    value: original,
                    shape: actual,
                    ..
                } if original == *value && actual == shape => Some((at, result)),
                _ => None,
            });
        after.insert(
            paired.map_or(*index, |(at, _)| at),
            vec![
                Inst::Read {
                    result: arena,
                    address,
                    reaching,
                    mode: ReadMode::Borrow,
                },
                Inst::ArenaInventory {
                    receipt: paired.map(|(_, result)| result).into(),
                    arena,
                    allocation: site.anchor,
                    value: *value,
                    shape,
                    previous: None,
                },
            ],
        );
    }
    if after.len() != source.events.len() {
        return Err(SourceError::Malformed(
            "original allocation event has no payload construction",
        ));
    }
    let mut instructions = Vec::with_capacity(unit.instructions.len() + after.len() * 3);
    for block in &mut unit.blocks {
        let start = block.instructions.start as usize;
        let code = block
            .instructions
            .get(&unit.instructions)
            .ok_or(SourceError::Malformed("inventory block instructions"))?;
        let begin = instructions.len() as u32;
        for (offset, instruction) in code.iter().enumerate() {
            instructions.push(instruction.clone());
            if preparation.is_some_and(|(at, _)| at == start + offset) {
                instructions.extend(preparations.iter().cloned());
            }
            if let Some(events) = after.get(&(start + offset)) {
                instructions.extend(events.iter().cloned());
            }
        }
        block.instructions = PoolRange::new(begin, instructions.len() as u32 - begin);
    }
    unit.instructions = instructions;
    Ok(unit)
}
