//! Body-derived evidence at same-module direct calls (MODE-2). These facts
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
}
impl UniqueSummary {
    pub fn is_empty(&self) -> bool {
        self.preserves_unique.is_empty() && self.results.is_empty()
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
#[derive(Clone, Debug, Default)]
pub struct UniqueInference {
    pub summary: UniqueSummary,
    pub failures: Vec<UniqueFailure>,
}
