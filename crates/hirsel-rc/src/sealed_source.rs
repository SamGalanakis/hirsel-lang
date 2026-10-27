//! Semantic constructor recipes read from validated source, before LIR and
//! physical representation selection. This module never reads candidate LIR.
//! A missing/ambiguous producer is a typed refusal, not fabricated provenance.
use hirsel_layout::operation::collect_arguments;
use hirsel_lir::ir::{SiteRange, SourceAnchor};
use hirsel_lir::operation::{NativeType, OperationArgument};
use hirsel_lir::sealed_payload_source::*;
use hirsel_span::Anchor;
use hirsel_tir::{Body, BodyId, InstructionKind, PlaceId, Rvalue, Storage, Terminator};
use hirsel_types::{Destination, GenericArgument, TypeId, TypeInterner};
use std::collections::BTreeSet;

pub mod contract;
pub mod demand;
pub mod inventory;
pub mod profile;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceError {
    Malformed(&'static str),
    Unsupported(&'static str),
    Type(String),
}

/// Parameter positions are the instance's mapping from original formal ordinal
/// to ABI ordinal; they are independent declaration input, never candidate IDs.
pub struct Context<'a> {
    pub body: &'a Body,
    pub body_id: BodyId,
    pub types: &'a TypeInterner,
    pub source_anchors: &'a [Anchor],
    pub parameters: &'a [u32],
}

#[derive(Clone)]
enum Definition {
    Parameter(u32),
    Capture(u32),
    Value(usize, SourceAnchor),
    Call(SourceAnchor),
    Unknown,
}

impl Context<'_> {
    fn anchor(&self, block: usize, statement: usize, span: hirsel_span::Span) -> SourceAnchor {
        SourceAnchor {
            body: self.body_id.0,
            block: block as u32,
            statement: statement as u32,
            site: SiteRange {
                source: self
                    .source_anchors
                    .iter()
                    .position(|&a| a == span.anchor)
                    .map_or(u32::MAX, |i| i as u32),
                start: span.start,
                end: span.end,
            },
        }
    }
    fn place(&self, place: PlaceId) -> Result<&hirsel_tir::Place, SourceError> {
        self.body
            .places
            .get(place.0 as usize)
            .ok_or(SourceError::Malformed("source place"))
    }
    fn local(&self, place: PlaceId) -> Result<usize, SourceError> {
        let place = self.place(place)?;
        if !place.projection.is_empty() {
            return Err(SourceError::Unsupported(
                "projected payload producer needs source path transport",
            ));
        }
        let local = place.root.0 as usize;
        if local >= self.body.locals.len() {
            return Err(SourceError::Malformed("source local"));
        }
        Ok(local)
    }
    fn destination(
        &self,
        destination: &Destination,
        definitions: &[Vec<Definition>],
        substitute: &mut impl FnMut(TypeId) -> Result<TypeId, SourceError>,
    ) -> Result<PayloadDestination, SourceError> {
        Ok(match destination {
            Destination::Heap | Destination::Inferred => PayloadDestination::Heap,
            Destination::Arena(origin) => {
                let mut matches =
                    self.body.places.iter().enumerate().filter(|(_, p)| {
                        p.projection.is_empty() && p.binding.as_ref() == Some(origin)
                    });
                let Some((index, first)) = matches.next() else {
                    return Err(SourceError::Malformed(
                        "arena destination lacks its source place",
                    ));
                };
                if matches.any(|(_, place)| place.root != first.root) {
                    return Err(SourceError::Malformed(
                        "ambiguous arena destination source place",
                    ));
                }
                PayloadDestination::Arena(PayloadOperand {
                    place: index as u32,
                    ty: native(self.types, substitute(first.ty)?)?,
                    producer: producer(self, definitions, PlaceId(index as u32))?,
                    capture_slot: None,
                })
            }
            Destination::Caller(parameters) => PayloadDestination::Caller(
                parameters
                    .iter()
                    .map(|parameter| {
                        self.parameters
                            .get(parameter.parameter().0 as usize)
                            .copied()
                            .ok_or(SourceError::Malformed(
                                "caller destination parameter mapping",
                            ))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Destination::Region(_) => {
                return Err(SourceError::Unsupported(
                    "compiler-local region payload destination",
                ));
            }
        })
    }
}

fn native(types: &TypeInterner, ty: TypeId) -> Result<NativeType, SourceError> {
    let mut arguments = collect_arguments(types, &[GenericArgument::Type(ty)])
        .map_err(|e| SourceError::Type(format!("{e:?}")))?;
    match arguments.pop() {
        Some(OperationArgument::Type(ty)) => Ok(ty),
        _ => Err(SourceError::Malformed("source operand type")),
    }
}

/// Preserve the full original rooted type identity beside the native erased
/// axes. The completed source query supplies substituted original types; this
/// helper reads neither candidate LIR nor a physical callable signature.
/// A returned hash identifies data, never independently proves a profile.
pub fn closed_storage_type(
    types: &TypeInterner,
    ty: TypeId,
) -> Result<ClosedStorageType, SourceError> {
    let native = native(types, ty)?;
    let source_type = hirsel_types::encoding::closed_storage_source_type(types, ty)
        .map_err(|error| SourceError::Type(format!("{error:?}")))?;
    Ok(ClosedStorageType {
        ty: native,
        source_type,
    })
}
fn literal(value: &hirsel_tir::Constant) -> PayloadLiteral {
    match value {
        hirsel_tir::Constant::Unit => PayloadLiteral::Unit,
        hirsel_tir::Constant::Bool(v) => PayloadLiteral::Bool(*v),
        hirsel_tir::Constant::Integer(v) => PayloadLiteral::Integer(*v),
        hirsel_tir::Constant::F32(v) => PayloadLiteral::F32(*v),
        hirsel_tir::Constant::F64(v) => PayloadLiteral::F64(*v),
        hirsel_tir::Constant::String(v) => PayloadLiteral::String(v.clone()),
        hirsel_tir::Constant::Bytes(v) => PayloadLiteral::Bytes(v.clone()),
    }
}

fn producer(
    context: &Context<'_>,
    definitions: &[Vec<Definition>],
    place: PlaceId,
) -> Result<PayloadProducer, SourceError> {
    let mut local = context.local(place)?;
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(local) {
            return Err(SourceError::Unsupported("cyclic source value transport"));
        }
        let [definition] = definitions[local].as_slice() else {
            return Err(SourceError::Unsupported(
                "payload operand needs reaching source alternatives",
            ));
        };
        return Ok(match definition {
            Definition::Parameter(ordinal) => PayloadProducer::Parameter(*ordinal),
            Definition::Capture(slot) => PayloadProducer::Capture(*slot),
            Definition::Call(source) => PayloadProducer::Call(source.clone()),
            Definition::Value(index, source) => match &context.body.rvalues[*index] {
                Rvalue::Constant(value) => PayloadProducer::Literal(literal(value)),
                Rvalue::Construct { .. } => PayloadProducer::Construct(source.clone()),
                Rvalue::Closure { .. } => PayloadProducer::Closure(source.clone()),
                Rvalue::Use(operand) => {
                    local = context.local(operand.place)?;
                    continue;
                }
                _ => {
                    return Err(SourceError::Unsupported(
                        "payload operand source expression",
                    ));
                }
            },
            Definition::Unknown => {
                return Err(SourceError::Unsupported("mutated payload source value"));
            }
        });
    }
}

/// Derive original typed constructor/capture leaves. No receiving lifetime can
/// retag Constant String/Bytes as an allocation: constants have no Destination.
/// Mutable CFG joins and source projections remain explicit missing cells until
/// their independently checked reaching/path representations are linked.
pub fn payloads(
    context: &Context<'_>,
    substitute: impl FnMut(TypeId) -> Result<TypeId, SourceError>,
) -> Result<Vec<SourcePayload>, SourceError> {
    payloads_selected(context, substitute, |_, _| true)
}

/// Select obligations from original source demand before resolving operands.
/// Unselected heap values do not belong to a sealed root or retained inventory.
fn payloads_selected(
    context: &Context<'_>,
    mut substitute: impl FnMut(TypeId) -> Result<TypeId, SourceError>,
    mut include: impl FnMut(u32, &Rvalue) -> bool,
) -> Result<Vec<SourcePayload>, SourceError> {
    let body = context.body;
    if context.parameters.len() != body.parameters.len() {
        return Err(SourceError::Malformed("source parameter mapping arity"));
    }
    let mut definitions = vec![vec![]; body.locals.len()];
    for (source, local) in body.parameters.iter().enumerate() {
        let row = definitions
            .get_mut(local.0 as usize)
            .ok_or(SourceError::Malformed("source parameter local"))?;
        row.push(Definition::Parameter(context.parameters[source]));
    }
    for (slot, (local, _)) in body
        .locals
        .iter()
        .enumerate()
        .filter(|(_, l)| l.storage == Storage::Capture)
        .enumerate()
    {
        definitions[local].push(Definition::Capture(slot as u32));
    }
    let mut constructions = vec![];
    for (block_id, block) in body.blocks.iter().enumerate() {
        for (statement, instruction) in block.instructions.iter().enumerate() {
            let source = context.anchor(block_id, statement, instruction.span);
            match instruction.kind {
                InstructionKind::Assign { destination, value } => {
                    let index = value.0 as usize;
                    let rvalue = body
                        .rvalues
                        .get(index)
                        .ok_or(SourceError::Malformed("source rvalue"))?;
                    let local = context.local(destination)?;
                    definitions[local].push(Definition::Value(index, source.clone()));
                    if matches!(rvalue, Rvalue::Construct { .. } | Rvalue::Closure { .. })
                        && include(local as u32, rvalue)
                    {
                        constructions.push((destination, rvalue, source));
                    }
                }
                InstructionKind::Replace {
                    destination,
                    displaced,
                    ..
                } => {
                    definitions[context.place(destination)?.root.0 as usize]
                        .push(Definition::Unknown);
                    definitions[context.place(displaced)?.root.0 as usize]
                        .push(Definition::Unknown);
                }
                _ => (),
            }
        }
        if let Terminator::Call {
            callee,
            arguments,
            returns,
            ..
        } = &block.terminator
        {
            // A mut argument can replace a whole value or a projected field.
            // Until reaching versions are transported, no old producer of its
            // storage root can authorize a semantic payload. Include indirect
            // callable mutation and diverging calls (which have no result).
            let callable = match callee {
                hirsel_tir::Callee::Indirect { value, .. } => Some(value),
                hirsel_tir::Callee::Definition { .. } => None,
            };
            for operand in arguments.iter().map(|a| &a.value).chain(callable) {
                if operand.usage == hirsel_types::UseKind::Mut {
                    let local = context.place(operand.place)?.root.0 as usize;
                    definitions
                        .get_mut(local)
                        .ok_or(SourceError::Malformed("mutated source local"))?
                        .push(Definition::Unknown);
                }
            }
            if let Some(returned) = returns {
                let local = context.local(returned.destination)?;
                definitions[local].push(Definition::Call(context.anchor(
                    block_id,
                    block.instructions.len(),
                    block.span,
                )));
            }
        }
    }
    let mut result = vec![];
    for (place, rvalue, source) in constructions {
        let (destination, operands) = match rvalue {
            Rvalue::Construct {
                constructor,
                fields,
                destination,
            } => {
                let mut operands: Vec<_> = fields.iter().map(|f| (&f.value, None)).collect();
                if let hirsel_tir::Constructor::SortedMap { by } = constructor {
                    operands.insert(0, (by, None));
                }
                (destination, operands)
            }
            Rvalue::Closure {
                captures,
                destination,
                ..
            } => (
                destination,
                captures
                    .iter()
                    .enumerate()
                    .map(|(slot, value)| (value, Some(slot as u32)))
                    .collect(),
            ),
            _ => unreachable!(),
        };
        let inputs = operands
            .into_iter()
            .map(|(operand, capture_slot)| {
                Ok(PayloadOperand {
                    place: operand.place.0,
                    ty: native(context.types, substitute(context.place(operand.place)?.ty)?)?,
                    producer: producer(context, &definitions, operand.place)?,
                    capture_slot,
                })
            })
            .collect::<Result<Vec<_>, SourceError>>()?;
        result.push(SourcePayload {
            root_type: native(context.types, substitute(context.place(place)?.ty)?)?,
            recipe: PayloadRecipe {
                source,
                operation: PayloadOperation::Allocate {
                    destination: context.destination(destination, &definitions, &mut substitute)?,
                },
                inputs,
            },
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests;

mod witness;
pub use witness::{
    SourceCallableEntry, SourceCapture, SourceDeclarations, SourceDemandProjection,
    SourceEntryKind, SourceEnvironment, SourceExpectation, SourceInput, SourceParameter,
};
