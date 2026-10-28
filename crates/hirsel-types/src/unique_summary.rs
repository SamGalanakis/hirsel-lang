//! Body-derived uniqueness and nonretention at same-module direct calls. These facts
//! never participate in a callable type or variance.
use crate::ParamId;

/// A result header row, without traversing arbitrary nominal representations.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash,
)]
pub enum UniqueResultRow {
    Root,
    Tuple(u32),
    Payload { variant: u32, field: u32 },
}

/// Inferred, independently re-proved evidence. Absence grants no guarantee.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct UniqueSummary {
    pub preserves_unique: Vec<ParamId>,
    pub results: Vec<UniqueResultRow>,
    /// Borrowed data parameters whose content never escapes the invocation.
    /// Private module inference proposes these; V and C check the bodies.
    #[serde(default)]
    pub nonretaining: Vec<ParamId>,
}
impl UniqueSummary {
    pub fn is_empty(&self) -> bool {
        self.preserves_unique.is_empty() && self.results.is_empty() && self.nonretaining.is_empty()
    }
}

/// Same defining module, including package version.
pub fn same_module(a: &hirsel_syntax::DefPath, b: &hirsel_syntax::DefPath) -> bool {
    a.0.package == b.0.package && a.0.modules == b.0.modules
}

/// Direct rows, with their declared types. Result uses the registered identity;
/// Option uses the canonical core declaration. Arbitrary nominal fields stay opaque.
pub fn result_rows(
    types: &crate::TypeInterner,
    root: crate::TypeId,
    roles: &crate::NominalRoles,
    lookup: &mut impl FnMut(hirsel_syntax::DefId) -> Option<crate::SignatureRef>,
) -> Vec<(UniqueResultRow, crate::TypeId)> {
    use crate::{SignatureKind, TypeKind};
    let mut rows = vec![(UniqueResultRow::Root, root)];
    match &types.row(root).kind {
        TypeKind::Tuple(parts) => rows.extend(
            parts
                .iter()
                .enumerate()
                .map(|(i, ty)| (UniqueResultRow::Tuple(i as u32), *ty)),
        ),
        TypeKind::Named {
            definition,
            arguments,
        } if roles.result_type.as_ref() == Some(definition.path())
            || definition.path() == &hirsel_syntax::DefPath::core("Option") =>
        {
            if let Some(signature) = lookup(*definition)
                && let SignatureKind::Type(declaration) = &signature.kind
                && let Some(representation) = declaration.representation
                && let Ok(representation) = crate::graph::expand(
                    types,
                    representation,
                    arguments,
                    &signature.metadata.roles,
                )
                && let TypeKind::Variant(variants) = &types.row(representation).kind
            {
                for (variant, arm) in variants.iter().enumerate() {
                    rows.extend(arm.fields.iter().enumerate().map(|(field, (_, ty))| {
                        (
                            UniqueResultRow::Payload {
                                variant: variant as u32,
                                field: field as u32,
                            },
                            *ty,
                        )
                    }));
                }
            }
        }
        _ => (),
    }
    rows
}

/// Apply direct-call result facts to the value receipt, leaving the callable
/// declaration unchanged. Nominal payloads refine only a direct bound member.
pub fn result_evidence(
    types: &crate::TypeInterner,
    root: crate::TypeId,
    facts: &[UniqueResultRow],
    roles: &crate::NominalRoles,
    lookup: &mut impl FnMut(hirsel_syntax::DefId) -> Option<crate::SignatureRef>,
) -> crate::TypeId {
    use crate::{GenericArgument, SignatureKind, TypeKind, TypeParameterRef, Uniqueness};
    let unique = |ty| {
        let mut row = types.row(ty).clone();
        row.modes.uniqueness = Uniqueness::Unique;
        types.intern(row)
    };
    if facts.is_empty() {
        return root;
    }
    let mut row = types.row(root).clone();
    for fact in facts {
        match (fact, &mut row.kind) {
            (UniqueResultRow::Root, _) => row.modes.uniqueness = Uniqueness::Unique,
            (UniqueResultRow::Tuple(field), TypeKind::Tuple(parts)) => {
                if let Some(part) = parts.get(*field as usize) {
                    let mut refined = parts.to_vec();
                    refined[*field as usize] = unique(*part);
                    *parts = refined.into();
                }
            }
            (
                UniqueResultRow::Payload { variant, field },
                TypeKind::Named {
                    definition,
                    arguments,
                },
            ) if roles.result_type.as_ref() == Some(definition.path())
                || definition.path() == &hirsel_syntax::DefPath::core("Option") =>
            {
                let Some(signature) = lookup(*definition) else {
                    continue;
                };
                let SignatureKind::Type(declaration) = &signature.kind else {
                    continue;
                };
                let Some(representation) = declaration.representation else {
                    continue;
                };
                let TypeKind::Variant(variants) = &types.row(representation).kind else {
                    continue;
                };
                let Some((_, ty)) = variants
                    .get(*variant as usize)
                    .and_then(|arm| arm.fields.get(*field as usize))
                else {
                    continue;
                };
                let TypeKind::Parameter(TypeParameterRef::Bound {
                    depth: 0,
                    parameter,
                }) = types.row(*ty).kind
                else {
                    continue;
                };
                let index = parameter.0 as usize;
                if let Some(GenericArgument::Type(part)) = arguments.get(index) {
                    let mut refined = arguments.to_vec();
                    refined[index] = GenericArgument::Type(unique(*part));
                    *arguments = refined.into();
                }
            }
            _ => (),
        }
    }
    types.intern(row)
}

/// The particular evidence a direct caller needed.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum UniqueClaim {
    Parameter(ParamId),
    Result(UniqueResultRow),
}
/// Diagnostic provenance of a failed body proof; not exported in a signature.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct UniqueFailure {
    pub claim: UniqueClaim,
    pub span: hirsel_span::Span,
    pub message: String,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct UniqueInference {
    pub summary: UniqueSummary,
    pub failures: Vec<UniqueFailure>,
}
