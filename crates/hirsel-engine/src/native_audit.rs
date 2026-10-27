//! R9: per-definition completion of the source allocation audit, with the
//! promises it enforced. V is not a body-effect verifier, so native passes check
//! what they introduce against these records; import recomputes each record
//! from current fully checked source or its matching toolchain precheck.
use crate::*;
use hirsel_syntax::ExprId;
use hirsel_types::{
    Allocation, DropAllocation, ParamId, PassingMode, Signature, SignatureKind, TypeId,
    TypeInterner, TypeKind,
};

/// Storage whose destruction the audited body relies on.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CleanupSite {
    /// A taken parameter the body may drop.
    Parameter(ParamId),
    /// The returned value, dropped by the caller.
    Result,
    /// A finalizer's remaining fields, dropped after its body.
    Fields,
}
/// The audit of one selected executable definition completed without errors.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
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
fn drop_allocation(types: &TypeInterner, ty: TypeId) -> DropAllocation {
    types
        .get(ty)
        .map(|row| row.drop_allocation.clone())
        .unwrap_or_default()
}
impl SourceAudit {
    /// Only an error-free analysis has a completed audit; diagnostics stay the
    /// caller's to report.
    pub(crate) fn completed(
        types: &TypeInterner,
        signature: &Signature,
        implementation_revision: ContentHash,
        analyzed: &AnalyzedBody,
        representation: Option<&RepresentationRecord>,
    ) -> Self {
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
                    .map(|record| drop_allocation(types, record.private_shape))
                    .or_else(|| {
                        declaration
                            .representation
                            .map(|shape| drop_allocation(types, shape))
                    })
                    .map(|fields| (CleanupSite::Fields, fields))
                    .into_iter()
                    .collect(),
            ),
            SignatureKind::Const { ty: _, .. }
            | SignatureKind::Family(_)
            | SignatureKind::Implementation(_) => (false, vec![]),
        };
        let checked = &analyzed.typed.checked;
        let closures = checked
            .closures
            .iter()
            .map(|closure| {
                let allocation = match types.get(closure.ty).map(|row| &row.kind) {
                    Some(TypeKind::Function(function)) => function.allocation.clone(),
                    _ => Allocation::MayAllocate,
                };
                (closure.expression, allocation)
            })
            .collect();
        Self {
            definition: signature.definition.path().clone(),
            implementation_revision,
            signature: signature.fingerprint,
            noalloc,
            cleanup,
            closures,
            invocations: analyzed.call_allocations.clone(),
        }
    }
}

/// The products of H and modes that native export needs, without their query state.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct SourceAuditProducts {
    pub audit: SourceAudit,
    pub materialized: Vec<(ExprId, hirsel_tir::Materialized)>,
}
impl QueryEngine {
    /// The completed allocation audit of an executable source definition.
    pub fn source_audit(&mut self, path: &DefPath) -> Result<SourceAudit, QueryError> {
        self.source_audit_products(path)
            .map(|products| products.audit)
    }

    pub(crate) fn source_audit_products(
        &mut self,
        path: &DefPath,
    ) -> Result<SourceAuditProducts, QueryError> {
        self.prepare_private_summaries();
        if let Some(products) = self.load_source_audit(path) {
            return Ok(products);
        }
        self.s1.begin_requests();
        self.begin_evaluation_validation();
        self.execution_stack.push(crate::memo::ReadsBuilder::new());
        let result = self.compute_source_audit(path);
        let reads = self
            .execution_stack
            .pop()
            .expect("source audit execution frame")
            .finish();
        let depth = self.s1.finish_requests(self.depth_limit());
        let (_, acceptance, selections) = self.end_evaluation_validation();
        let products = result?;
        let reads = reads.map_err(|_| QueryError::NotImplemented)?;
        self.store_source_audit(&products, &reads, depth, selections, acceptance)?;
        Ok(products)
    }
    fn compute_source_audit(&mut self, path: &DefPath) -> Result<SourceAuditProducts, QueryError> {
        let signature = self.item_signature(path)?.value;
        let representation = if matches!(signature.kind, SignatureKind::Type(_)) {
            match self.owner_representation(path, path) {
                Ok(record) => Some(record),
                Err(QueryError::NotImplemented) => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let analyzed = self.mode_definition(path)?;
        self.read_execution_signatures(&analyzed.dependencies);
        if crate::queries::has_errors(&analyzed.diagnostics) {
            return Err(QueryError::Diagnostics(analyzed.diagnostics));
        }
        let typed = &analyzed.value.typed;
        let revision = typed.syntax.text_hash;
        let audit = SourceAudit::completed(
            &self.types,
            &signature,
            revision,
            &analyzed.value,
            representation.as_ref(),
        );
        let comptime = self.prepare_comptime(&analyzed.value, &signature)?;
        Ok(SourceAuditProducts {
            audit,
            materialized: comptime.materialized,
        })
    }
}
