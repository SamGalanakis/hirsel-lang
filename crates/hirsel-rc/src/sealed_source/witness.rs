//! Original Lower observation retained before discovery/import may release or
//! rewrite a body. No candidate LIR is accepted by any constructor in this file.
use super::{Context, SourceError, payloads};
use hirsel_layout::instance::InstanceKey;
use hirsel_lir::operation::{OperationArgument, OperationDeclarations};
use hirsel_lir::sealed_payload_source::*;
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_tir::{DefinitionRef, InstructionKind, Rvalue, Terminator};
use hirsel_types::{TypeId, TypeInterner};
use std::collections::BTreeMap;

/// Capture at the actual original-Lower miss, before discovery/import/drop.
/// The source-query identity and owner revision come from the actual query owner.
#[derive(Clone)]
pub struct SourceInput {
    source: ContentHash,
    revision: ContentHash,
    definition: DefinitionRef,
}

/// Source-derived expectations for one instance. Unavailable is represented by
/// a factory error / absent provider row, never by an empty expectation vector.
#[derive(Clone)]
pub struct SourceExpectation {
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
        if self.definition.path.path() != key.definition() {
            return Err(SourceError::Malformed("source witness definition identity"));
        }
        let body = self
            .definition
            .bodies
            .get(key.body() as usize)
            .ok_or(SourceError::Malformed("source witness body identity"))?;
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
        let events = payloads
            .iter()
            .filter_map(|payload| {
                matches!(
                    payload.recipe.operation,
                    PayloadOperation::Allocate {
                        destination: PayloadDestination::Arena(_)
                    }
                )
                .then(|| SourceEvent {
                    anchor: payload.recipe.source.clone(),
                    kind: SourceEventKind::Allocate,
                    root_type: payload.root_type.clone(),
                })
            })
            .collect::<Vec<_>>();
        let witness = SealSourceWitness::from_expectations(
            self.source,
            key.definition().clone(),
            self.revision,
            key.digest(),
            &events,
            &payloads,
        );
        Ok(SourceExpectation {
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
