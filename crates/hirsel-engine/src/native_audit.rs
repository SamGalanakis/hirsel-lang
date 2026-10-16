//! R9: per-definition completion of the source allocation audit, with the
//! promises it enforced. V is not a body-effect verifier, so native passes check
//! what they introduce against these records; import recomputes each record
//! from current fully checked source rather than trusting a claimed bit.
use crate::*;
use hirsel_syntax::ExprId;
use hirsel_types::{
    Allocation, DropAllocation, ParamId, PassingMode, Signature, SignatureKind, TypeId, TypeKind,
    TypeStore,
};

/// Storage whose destruction the audited body relies on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CleanupSite {
    /// A taken parameter the body may drop.
    Parameter(ParamId),
    /// The returned value, dropped by the caller.
    Result,
    /// A finalizer's remaining fields, dropped after its body.
    Fields,
}
/// The audit of one selected executable definition completed without errors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceAudit {
    pub definition: DefPath,
    pub implementation_revision: ContentHash,
    /// The declaration the promises below were read from.
    pub signature: ContentHash,
    /// The entry body (function body or finalizer) promises not to allocate.
    pub noalloc: bool,
    /// Declared destruction promises, in declaration order.
    pub cleanup: Vec<(CleanupSite, DropAllocation)>,
    /// The declared invocation promise of each closure literal, in source order.
    pub closures: Vec<(ExprId, Allocation)>,
    /// The invocation promise the audit read for each call, direct or through a
    /// callable value, in source order.
    pub invocations: Vec<(ExprId, Allocation)>,
}
fn drop_allocation(types: &TypeStore, ty: TypeId) -> DropAllocation {
    types
        .get(ty)
        .map(|row| row.drop_allocation.clone())
        .unwrap_or_default()
}
impl SourceAudit {
    /// Only an error-free analysis has a completed audit; diagnostics stay the
    /// caller's to report.
    pub(crate) fn completed(
        signature: &Signature,
        implementation_revision: ContentHash,
        analyzed: &AnalyzedBody,
        representation: Option<&RepresentationRecord>,
    ) -> Self {
        let types = &signature.types;
        let (noalloc, cleanup) = match &signature.kind {
            SignatureKind::Function(function) => (
                function.allocation == Allocation::NoAlloc,
                function
                    .parameters
                    .iter()
                    .enumerate()
                    .filter(|(_, parameter)| parameter.passing == PassingMode::Take)
                    .map(|(ordinal, parameter)| {
                        (
                            CleanupSite::Parameter(ParamId(ordinal as u32)),
                            drop_allocation(types, parameter.ty),
                        )
                    })
                    .chain([(CleanupSite::Result, drop_allocation(types, function.result))])
                    .collect(),
            ),
            SignatureKind::Type(declaration) => (
                declaration.destructor_allocation == Allocation::NoAlloc,
                representation
                    .map(|record| drop_allocation(&record.types, record.private_shape))
                    .or_else(|| {
                        declaration
                            .representation
                            .map(|shape| drop_allocation(types, shape))
                    })
                    .map(|fields| (CleanupSite::Fields, fields))
                    .into_iter()
                    .collect(),
            ),
            SignatureKind::Const(_) => (false, vec![]),
        };
        let checked = &analyzed.typed.checked;
        let closures = checked
            .closures
            .iter()
            .map(|closure| {
                let allocation = match checked.types.get(closure.ty).map(|row| &row.kind) {
                    Some(TypeKind::Function(function)) => function.allocation.clone(),
                    _ => Allocation::MayAllocate,
                };
                (closure.expression, allocation)
            })
            .collect();
        Self {
            definition: signature.definition.clone(),
            implementation_revision,
            signature: signature.fingerprint,
            noalloc,
            cleanup,
            closures,
            invocations: analyzed.call_allocations.clone(),
        }
    }
}
