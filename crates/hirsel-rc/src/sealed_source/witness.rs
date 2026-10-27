//! Original Lower observation retained before discovery/import may release or
//! rewrite a body. No candidate LIR is accepted by any constructor in this file.
use super::{Context, SourceError, payloads};
use hirsel_layout::instance::InstanceKey;
use hirsel_lir::operation::{NativeType, OperationArgument, OperationDeclarations};
use hirsel_lir::sealed_payload_source::*;
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_tir::{DefinitionRef, InstructionKind, Rvalue, Terminator};
use hirsel_types::{TypeId, TypeInterner};
use std::collections::BTreeMap;

mod entry;
pub use entry::{SourceCallableEntry, SourceCapture, SourceEntryKind, SourceParameter};

/// Capture at the actual original-Lower miss, before discovery/import/drop.
/// The source-query identity and owner revision come from the actual query owner.
#[derive(Clone)]
pub struct SourceInput {
    source: ContentHash,
    revision: ContentHash,
    definition: DefinitionRef,
}

/// Body-free demand data bound to the actual original source observation and
/// base instance. This is not a ReceiptContract or a V/C acceptance witness.
#[derive(Clone)]
pub struct SourceDemandProjection {
    reference: hirsel_lir::invocations::ReferenceBody,
    source: ContentHash,
    revision: ContentHash,
    projection: super::demand::BodyProjection,
}
impl SourceDemandProjection {
    pub fn reference(&self) -> &hirsel_lir::invocations::ReferenceBody {
        &self.reference
    }
    pub fn source_identity(&self) -> ContentHash {
        self.source
    }
    pub fn implementation_revision(&self) -> ContentHash {
        self.revision
    }
    pub fn projection(&self) -> &super::demand::BodyProjection {
        &self.projection
    }
}

/// Source-derived expectations for one instance. Unavailable is represented by
/// a factory error / absent provider row, never by an empty expectation vector.
#[derive(Clone)]
pub struct SourceExpectation {
    body: u32,
    witness: SealSourceWitness,
    events: Vec<SourceEvent>,
    payloads: Vec<SourcePayload>,
}

impl SourceInput {
    pub fn capture(source: ContentHash, revision: ContentHash, definition: DefinitionRef) -> Self {
        Self {
            source,
            revision,
            definition,
        }
    }

    /// Select the retained original source body for demand expansion, before
    /// a native recipe restores or rewrites an instance. The query owner calls
    /// `capture` at its independently validated original-Lower miss. Numeric
    /// body selection is scoped by that definition; candidate anchors/IR and
    /// a consuming derived key cannot substitute another original body.
    ///
    /// This supplies data to `demand::analyze`, not proof that a call expansion
    /// is complete. The query owner still supplies every original target,
    /// operation/capture expansion and complete closed type/profile product.
    pub fn demand_body(&self, key: &InstanceKey) -> Result<&hirsel_tir::Body, SourceError> {
        if self.definition.path.path() != key.definition() {
            return Err(SourceError::Malformed("source witness definition identity"));
        }
        self.definition
            .bodies
            .get(key.body() as usize)
            .ok_or(SourceError::Malformed("source witness body identity"))
    }

    /// Capture dependency data at the independently observed original-Lower
    /// miss, using the original base InstanceKey (before profile/demand-derived
    /// requests). Caller retains and rechecks this observation separately from
    /// semantic requirements. Actual call/closure expansions remain mandatory
    /// inputs to analyze_compact, even after this SourceInput is released.
    pub fn demand_projection(
        &self,
        base: &InstanceKey,
    ) -> Result<SourceDemandProjection, SourceError> {
        let body = self.demand_body(base)?;
        Ok(SourceDemandProjection {
            reference: hirsel_lir::invocations::ReferenceBody {
                definition: base.definition().clone(),
                instance: base.digest(),
                body: base.body(),
            },
            source: self.source,
            revision: self.revision,
            projection: super::demand::BodyProjection::capture(body)?,
        })
    }

    /// Current closed subset: constructor-only source bodies. Call, mutation
    /// and held-environment rules require reviewed source-operation expansion;
    /// they remain explicit refusals rather than a guessed expected-empty row.
    pub fn derive(
        &self,
        key: &InstanceKey,
        types: &TypeInterner,
        parameters: &[u32],
        substitute: impl FnMut(TypeId) -> Result<TypeId, SourceError>,
    ) -> Result<SourceExpectation, SourceError> {
        let body = self.demand_body(key)?;
        for block in &body.blocks {
            if matches!(block.terminator, Terminator::Call { .. }) {
                return Err(SourceError::Unsupported(
                    "source witness needs reviewed call event expansion",
                ));
            }
            if block
                .instructions
                .iter()
                .any(|i| matches!(i.kind, InstructionKind::Replace { .. }))
            {
                return Err(SourceError::Unsupported(
                    "source witness needs mutation event expansion",
                ));
            }
        }
        if body
            .rvalues
            .iter()
            .any(|r| matches!(r, Rvalue::Closure { .. }))
        {
            return Err(SourceError::Unsupported(
                "source witness needs closed held code instances",
            ));
        }
        let anchors = hirsel_tir::source_anchors(&self.definition);
        let payloads = payloads(
            &Context {
                body,
                body_id: hirsel_tir::BodyId(key.body()),
                types,
                source_anchors: &anchors,
                parameters,
            },
            substitute,
        )?;
        let mut events = Vec::new();
        for payload in &payloads {
            if matches!(
                payload.recipe.operation,
                PayloadOperation::Allocate {
                    destination: PayloadDestination::Arena(_)
                }
            ) {
                let allocated = match &payload.root_type {
                    NativeType::Box(_) => true,
                    NativeType::Builtin(kind, _) => match kind {
                        hirsel_lir::operation::BuiltinType::List
                        | hirsel_lir::operation::BuiltinType::Array
                        | hirsel_lir::operation::BuiltinType::Map
                        | hirsel_lir::operation::BuiltinType::Set
                        | hirsel_lir::operation::BuiltinType::SortedMap => true,
                        hirsel_lir::operation::BuiltinType::Inline
                        | hirsel_lir::operation::BuiltinType::Packed
                        | hirsel_lir::operation::BuiltinType::Simd => false,
                        _ => {
                            return Err(SourceError::Unsupported(
                                "source allocation classification needs a reviewed builtin rule",
                            ));
                        }
                    },
                    NativeType::Tuple(_)
                    | NativeType::Record(_)
                    | NativeType::Variant(_)
                    | NativeType::Primitive(_) => false,
                    NativeType::Named(..) | NativeType::Function(_) => {
                        return Err(SourceError::Unsupported(
                            "source allocation classification needs complete held/nominal declarations",
                        ));
                    }
                };
                if allocated {
                    events.push(SourceEvent {
                        anchor: payload.recipe.source.clone(),
                        kind: SourceEventKind::Allocate,
                        root_type: payload.root_type.clone(),
                    });
                }
            }
        }
        let witness = SealSourceWitness::from_expectations(
            self.source,
            key.definition().clone(),
            self.revision,
            key.digest(),
            &events,
            &payloads,
        );
        Ok(SourceExpectation {
            body: key.body(),
            witness,
            events,
            payloads,
        })
    }
}

impl SourceExpectation {
    pub fn witness(&self) -> &SealSourceWitness {
        &self.witness
    }
    pub fn source(&self) -> SourceSeal {
        SourceSeal {
            reference: hirsel_lir::invocations::ReferenceBody {
                definition: self.witness.definition.clone(),
                instance: self.witness.instance,
                body: self.body,
            },
            witness: self.witness.clone(),
            events: self.events.clone(),
            payloads: self.payloads.clone(),
        }
    }
    pub fn events(&self) -> &[SourceEvent] {
        &self.events
    }
    pub fn payloads(&self) -> &[SourcePayload] {
        &self.payloads
    }
}

/// Actual independent provider used by C and by NativeCache observation/recheck.
/// It delegates ordinary declaration/layout logic without manufacturing missing
/// source rows, including reviewed library bodies not covered by this subset.
pub struct SourceDeclarations {
    registry: hirsel_layout::operation::OperationRegistry,
    sources: BTreeMap<(DefPath, ContentHash), SourceExpectation>,
}

impl SourceDeclarations {
    pub fn new(registry: hirsel_layout::operation::OperationRegistry) -> Self {
        Self {
            registry,
            sources: BTreeMap::new(),
        }
    }
    pub fn insert(&mut self, source: SourceExpectation) -> Result<(), SourceError> {
        let key = (source.witness.definition.clone(), source.witness.instance);
        if let Some(old) = self.sources.get(&key) {
            if old.witness != source.witness {
                return Err(SourceError::Malformed(
                    "conflicting original source expectations",
                ));
            }
        } else {
            self.sources.insert(key, source);
        }
        Ok(())
    }
}

impl OperationDeclarations for SourceDeclarations {
    fn pretransfer_adapters(&self) -> &[hirsel_lir::pretransfer::AdapterReference] {
        self.registry.pretransfer_adapters()
    }
    fn requires_structural_reference(&self) -> bool {
        self.registry.requires_structural_reference()
    }
    fn structural_access(
        &self,
        root: &NativeType,
        layouts: &hirsel_lir::LayoutTable,
        target: &hirsel_lir::TargetSpec,
    ) -> Result<hirsel_lir::structural_access::AccessPlan, hirsel_lir::layout::LayoutError> {
        self.registry.structural_access(root, layouts, target)
    }
    fn requires_invocation_reference(&self) -> bool {
        self.registry.requires_invocation_reference()
    }
    fn invocations(
        &self,
        definition: &DefPath,
    ) -> Option<&[hirsel_lir::invocations::ReferenceInvocation]> {
        self.registry.invocations(definition)
    }
    fn invocation_body(&self, definition: &DefPath, instance: ContentHash) -> Option<u32> {
        self.registry.invocation_body(definition, instance)
    }
    fn seal_descriptor(
        &self,
        root: &NativeType,
        layouts: &hirsel_lir::LayoutTable,
        target: &hirsel_lir::TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, hirsel_lir::layout::LayoutError> {
        self.registry.seal_descriptor(root, layouts, target)
    }

    fn seal_descriptor_for_source(
        &self,
        definition: &DefPath,
        instance: ContentHash,
        root: &NativeType,
        layouts: &hirsel_lir::LayoutTable,
        target: &hirsel_lir::TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, hirsel_lir::layout::LayoutError> {
        let selected = self
            .sources
            .get(&(definition.clone(), instance))
            .is_some_and(|source| {
                source.payloads.iter().any(|payload| {
                    &payload.root_type == root
                        || payload.recipe.inputs.iter().any(|input| &input.ty == root)
                }) || source.events.iter().any(|event| &event.root_type == root)
            });
        if !selected {
            return Err(hirsel_lir::layout::LayoutError::Invalid(
                "missing original source storage type".into(),
            ));
        }
        self.registry.seal_descriptor(root, layouts, target)
    }

    fn pretransfer_abi(
        &self,
        reference: &hirsel_lir::pretransfer::AdapterReference,
        target: &hirsel_lir::TargetSpec,
    ) -> Option<(hirsel_lir::FnAbi, hirsel_lir::LayoutTable)> {
        self.registry.pretransfer_abi(reference, target)
    }

    fn seal_source_witness(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<SealSourceWitness> {
        self.sources
            .get(&(definition.clone(), instance))
            .map(|s| s.witness.clone())
    }
    fn seal_events(&self, definition: &DefPath, instance: ContentHash) -> Option<Vec<SourceEvent>> {
        self.sources
            .get(&(definition.clone(), instance))
            .map(|s| s.events.clone())
    }
    fn seal_payloads(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<Vec<SourcePayload>> {
        self.sources
            .get(&(definition.clone(), instance))
            .map(|s| s.payloads.clone())
    }
    fn derive(
        &self,
        operation: &hirsel_core_contract::OperationId,
        arguments: &[OperationArgument],
        target: &hirsel_lir::TargetSpec,
    ) -> Result<(hirsel_lir::FnAbi, hirsel_lir::LayoutTable), hirsel_lir::layout::LayoutError> {
        self.registry.derive(operation, arguments, target)
    }
}
