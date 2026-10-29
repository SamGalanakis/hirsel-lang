//! Admission coverage reconstructed from types and control flow, without tail claims.
use crate::ReadDefinition;
use crate::{
    SignatureLookup,
    input::Reader,
    tail::{verify_constructor_hole, verify_self_tail},
    typing,
};
use hirsel_syntax::DefId;
use hirsel_tir::*;
use hirsel_types::{TypeInterner, UseKind};

/// (body, call block, source statement ordinal, eligible self restart).
/// The definition and any primitive declaration identity come from the independently
/// supplied declaration universe, never an asserted lowering event count.
pub fn invocation_coverage(
    types: &TypeInterner,
    definition: &Definition,
    signatures: &dyn SignatureLookup,
) -> Result<Vec<(u32, u32, u32, bool)>, &'static str> {
    coverage(
        types,
        &ReadDefinition::live(definition),
        &Reader::new(types, signatures),
    )
}

pub(crate) fn coverage(
    types: &TypeInterner,
    definition: &ReadDefinition<'_>,
    reader: &Reader<'_>,
) -> Result<Vec<(u32, u32, u32, bool)>, &'static str> {
    let declarations = typing::declaration_tables(types, definition, reader)
        .ok_or("invalid invocation type declarations")?;
    coverage_with_declarations(types, definition, reader, &declarations)
}

/// Full V already derived this exact immutable definition's lexical binders.
/// Standalone coverage still obtains its own independently checked declarations.
pub(crate) fn coverage_with_declarations(
    types: &TypeInterner,
    definition: &ReadDefinition<'_>,
    reader: &Reader<'_>,
    declarations: &[hirsel_types::GenericDeclarations],
) -> Result<Vec<(u32, u32, u32, bool)>, &'static str> {
    let mut result = Vec::new();
    for (body_id, body) in definition.bodies.iter().enumerate() {
        let property = |ty, counted| {
            let mut lookup = |id: DefId| {
                Ok::<_, hirsel_types::QueryError>(reader.get(id).cloned().map(std::sync::Arc::new))
            };
            if counted {
                hirsel_types::structural_equality(
                    types,
                    ty,
                    hirsel_types::Bound::Data,
                    &declarations[body_id],
                    &mut lookup,
                )
                .unwrap_or(false)
            } else {
                hirsel_types::structural_property(
                    types,
                    ty,
                    hirsel_types::StructuralProperty::Plain,
                    &declarations[body_id],
                    &mut lookup,
                )
                .unwrap_or(false)
            }
        };
        for (block_id, block) in body.blocks.iter().enumerate() {
            let Terminator::Call {
                callee, arguments, ..
            } = &block.terminator
            else {
                continue;
            };
            // Typing independently checks all possible callable producers of
            // this flag against the definition's generated contract bodies.
            if matches!(
                callee,
                Callee::Indirect {
                    forward_caller: true,
                    ..
                }
            ) {
                continue;
            }
            let owned_loop_arguments = arguments
                .iter()
                .filter_map(|argument| {
                    let place = body.places.get(argument.value.place.0 as usize)?;
                    let local = body.locals.get(place.root.0 as usize)?;
                    (argument.value.usage == UseKind::Borrow
                        && place.projection.is_empty()
                        && local.storage == Storage::Temporary
                        && !property(place.ty, false)
                        && property(place.ty, true))
                    .then_some((argument.parameter, argument.value.place))
                })
                .collect();
            let candidate = TailSite {
                owner_body: BodyId(body_id as u32),
                call_block: BlockId(block_id as u32),
                kind: TailKind::SelfTail,
                owned_loop_arguments,
            };
            let ordinary =
                |p: &hirsel_syntax::DefPath| crate::tail::ordinary_constructor(reader, p);
            let restart = verify_self_tail(
                types,
                definition,
                BodyId(body_id as u32),
                &candidate,
                &property,
            ) || crate::tail::constructor_path(
                types,
                definition,
                BodyId(body_id as u32),
                BlockId(block_id as u32),
                &property,
                &ordinary,
            )
            .is_some_and(|path| {
                verify_constructor_hole(
                    types,
                    definition,
                    BodyId(body_id as u32),
                    &TailSite {
                        kind: TailKind::ConstructorHole(path),
                        ..candidate.clone()
                    },
                    &property,
                    &ordinary,
                )
            });
            result.push((
                body_id as u32,
                block_id as u32,
                block.instructions.len() as u32,
                restart,
            ));
        }
    }
    Ok(result)
}
