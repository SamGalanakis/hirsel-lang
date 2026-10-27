//! Receipt declarations from an independently rechecked current original graph.
//! This snapshot supplies C inputs, never executable entry or custody authority.
use super::{SourceError, factory::*, provider};
use hirsel_lir::operation::{NativeType, OperationArgument, OperationDeclarations};
use hirsel_lir::sealed_payload_source::ReceiptSource;
use hirsel_lir::{FnAbi, LayoutTable, TargetSpec, layout::LayoutError};
use hirsel_rc::sealed_source::{SourceCallableEntry, contract::Port, profile::CallableCase};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::TypeId;
use std::collections::BTreeMap;

/// Construct a fresh view for each current source snapshot, including cache
/// replay. Saved graph contents cannot select their own original roots or V.
/// Ordinary declaration queries retain the supplied provider's behaviour.
pub struct ReceiptDeclarations<'a> {
    ordinary: &'a (dyn OperationDeclarations + Sync),
    sources: BTreeMap<(DefPath, ContentHash), ReceiptSource>,
}

impl NamedGraph {
    pub fn declarations<'a>(
        &self,
        provider: &dyn provider::SelectionProvider,
        roots: impl IntoIterator<Item = NamedInput>,
        cases: &mut impl FnMut(
            &SourceCallableEntry,
            Port,
            TypeId,
        ) -> Result<Vec<CallableCase>, SourceError>,
        ordinary: &'a (dyn OperationDeclarations + Sync),
    ) -> Result<ReceiptDeclarations<'a>, SourceError> {
        self.recheck(provider, roots, cases)?;
        let mut sources = BTreeMap::new();
        for request in &self.requests {
            if let Some(source) = request.receipt_source()? {
                let key = (source.original.definition.clone(), source.instance);
                if sources
                    .insert(key, source.clone())
                    .is_some_and(|old| old != source)
                {
                    return Err(SourceError::Malformed(
                        "conflicting selected receipt declarations",
                    ));
                }
            }
        }
        Ok(ReceiptDeclarations { ordinary, sources })
    }
}

impl OperationDeclarations for ReceiptDeclarations<'_> {
    fn receipt_source(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<hirsel_lir::sealed_payload_source::ReceiptSource> {
        self.sources.get(&(definition.clone(), instance)).cloned()
    }

    fn seal_source_witness(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<hirsel_lir::sealed_payload_source::SealSourceWitness> {
        self.ordinary.seal_source_witness(definition, instance)
    }
    fn seal_events(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<Vec<hirsel_lir::sealed_payload_source::SourceEvent>> {
        self.ordinary.seal_events(definition, instance)
    }
    fn seal_payloads(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<Vec<hirsel_lir::sealed_payload_source::SourcePayload>> {
        self.ordinary.seal_payloads(definition, instance)
    }
    fn seal_descriptor(
        &self,
        source: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, LayoutError> {
        self.ordinary.seal_descriptor(source, table, target)
    }
    fn seal_descriptor_for_source(
        &self,
        definition: &DefPath,
        instance: ContentHash,
        source: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, LayoutError> {
        self.ordinary
            .seal_descriptor_for_source(definition, instance, source, table, target)
    }
    fn pretransfer_adapters(&self) -> &[hirsel_lir::pretransfer::AdapterReference] {
        self.ordinary.pretransfer_adapters()
    }
    fn pretransfer_abi(
        &self,
        reference: &hirsel_lir::pretransfer::AdapterReference,
        target: &TargetSpec,
    ) -> Option<(FnAbi, LayoutTable)> {
        self.ordinary.pretransfer_abi(reference, target)
    }

    fn requires_structural_reference(&self) -> bool {
        self.ordinary.requires_structural_reference()
    }
    fn structural_access(
        &self,
        source: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<hirsel_lir::structural_access::AccessPlan, LayoutError> {
        self.ordinary.structural_access(source, table, target)
    }
    fn requires_invocation_reference(&self) -> bool {
        self.ordinary.requires_invocation_reference()
    }
    fn invocations(
        &self,
        definition: &DefPath,
    ) -> Option<&[hirsel_lir::invocations::ReferenceInvocation]> {
        self.ordinary.invocations(definition)
    }
    fn invocation_body(&self, definition: &DefPath, instance: ContentHash) -> Option<u32> {
        self.ordinary.invocation_body(definition, instance)
    }
    fn derive(
        &self,
        operation: &hirsel_core_contract::OperationId,
        arguments: &[OperationArgument],
        target: &TargetSpec,
    ) -> Result<(FnAbi, LayoutTable), LayoutError> {
        self.ordinary.derive(operation, arguments, target)
    }
}
