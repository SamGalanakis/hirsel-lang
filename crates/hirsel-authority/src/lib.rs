//! Signature-only provenance, authority and capture checking (docs/spec/ownership.md,
//! brands.md, closures.md, capabilities.md and diagnostics.md).

#[cfg(test)]
hirsel_guard::install!();

mod closure_rows;
mod cursors;
pub use cursors::refine_cursor_plan;
mod facts;
mod identities;
mod source_rows;
mod type_bindings;
mod walk;
use hirsel_diag::Diagnostic;
use hirsel_intern::Symbol;
use hirsel_intern::work::{self, Counter};
use hirsel_span::Span;
use hirsel_syntax::{DefId, DefPath, ExprId, ItemSyntax, LexicalOwner, SyntaxTree};
use hirsel_types::core_roles::Intrinsic;
use hirsel_types::*;
use std::collections::BTreeMap;

pub use facts::brand_join;

pub type Intrinsics = BTreeMap<DefPath, Intrinsic>;

/// Binding identity includes its declaring scope; parameter ordinals alone are ambiguous.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedName {
    pub owner: NameSite,
    pub binding: ResolvedName,
    pub usage: UseKind,
    pub ty: TypeId,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureTable {
    pub expression: ExprId,
    pub context: CaptureContext,
    pub captures: Vec<CapturedName>,
    pub authority: AuthoritySummary,
}
#[derive(Clone, Debug)]
pub struct AuthorityOutput {
    pub body: CheckedBody,
    pub captures: Vec<CaptureTable>,
    /// Declared invocation allocation joined with each borrowed scoped callback.
    pub call_allocations: Vec<(ExprId, Allocation)>,
    /// Formal frame results in body.types, with body-origin parameter roots.
    /// Owners are Item, Statement (named functions), or Expression (closures).
    pub result_types: Vec<(NameSite, TypeId)>,
    /// Fresh binding identities at continuing source joins, authored by J.
    pub identity_joins: Vec<(NameSite, NameSite, Origin)>,
    pub identity_fields: Vec<(
        NameSite,
        NameSite,
        hirsel_types::Origin,
        Vec<(hirsel_types::FieldId, TypeId)>,
    )>,
    /// J-selected source identities retained for signature refinement after I.
    pub cursor_identities: BTreeMap<ExprId, Origin>,
    pub diagnostics: Vec<Diagnostic>,
    pub dependencies: Vec<SignatureDependency>,
    /// Every provider attempt, including failed reads, survives rejection.
    pub reads: Vec<hirsel_types::SharedRead>,
    /// Operational/unavailable provider failures prevent publication as accepted.
    pub failure: Option<QueryError>,
}

impl AuthorityOutput {
    /// Complete capture-at-creation facts shared by ownership and lowering.
    pub fn capture_modes(&self) -> Vec<CaptureMode> {
        self.captures
            .iter()
            .flat_map(|table| {
                table
                    .captures
                    .iter()
                    .enumerate()
                    .map(move |(ordinal, capture)| CaptureMode {
                        expression: table.expression,
                        ordinal: u32::try_from(ordinal)
                            .expect("capture ordinal exceeds semantic arena"),
                        owner: capture.owner,
                        usage: capture.usage,
                        context: table.context,
                    })
            })
            .collect()
    }
}

/// H's signature clauses are checked in distinct all-borrow frames.
pub fn check_file_with_clauses(
    t: &TypeInterner,
    m: &TypeMemo,
    syntax: &ItemSyntax,
    signature: &Signature,
    body: CheckedBody,
    provider: &mut dyn SignatureProvider,
    intrinsics: &Intrinsics,
    loops: &[CursorPlan],
    clauses: &[(ExprId, TypeId)],
) -> AuthorityOutput {
    let generics = body.resolution.generic_declarations.clone();
    let reader = hirsel_modes::reader::Reader::new(&mut *provider, signature);
    // A failed read proves nothing; the pass records its diagnostics.
    let mut lookup = |d: DefId| Ok(reader.borrow_mut().read(d).ok());
    let memo = ContextMemo::new(t, &generics, &mut lookup);
    walk::Checker::new(t, m, syntax, signature, body, &reader, memo, intrinsics)
        .with_cursor_loops(loops)
        .with_clauses(clauses)
        .run()
}

#[cfg(any(test, feature = "testing"))]
pub mod test_support;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod corpus;
