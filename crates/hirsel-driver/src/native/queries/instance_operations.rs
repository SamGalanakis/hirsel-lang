//! Reconstruct the independent source observation scope of one base Instance.
//! Snapshot-wide unions retain declarations for every owner, while a base C
//! receipt observes only its selected source instance or reviewed library plan.
use hirsel_lir::operation::{NativeType, OperationArgument, OperationDeclarations};
use hirsel_lir::{FnAbi, LayoutTable, TargetSpec, layout::LayoutError};
use hirsel_syntax::{ContentHash, DefPath};

pub(super) struct InstanceOperations<'a> {
    pub declarations: &'a (dyn OperationDeclarations + Sync),
    pub source: Option<(&'a DefPath, ContentHash)>,
}
impl OperationDeclarations for InstanceOperations<'_> {
    fn seal_source_witness(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<hirsel_lir::sealed_payload_source::SealSourceWitness> {
        self.declarations.seal_source_witness(definition, instance)
    }
    fn seal_events(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<Vec<hirsel_lir::sealed_payload_source::SourceEvent>> {
        self.declarations.seal_events(definition, instance)
    }
    fn seal_payloads(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<Vec<hirsel_lir::sealed_payload_source::SourcePayload>> {
        self.declarations.seal_payloads(definition, instance)
    }
    fn seal_descriptor(
        &self,
        source: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, LayoutError> {
        self.declarations.seal_descriptor(source, table, target)
    }
    fn seal_descriptor_for_source(
        &self,
        definition: &DefPath,
        instance: ContentHash,
        source: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, LayoutError> {
        self.declarations
            .seal_descriptor_for_source(definition, instance, source, table, target)
    }
    fn pretransfer_adapters(&self) -> &[hirsel_lir::pretransfer::AdapterReference] {
        self.declarations.pretransfer_adapters()
    }
    fn pretransfer_abi(
        &self,
        reference: &hirsel_lir::pretransfer::AdapterReference,
        target: &TargetSpec,
    ) -> Option<(FnAbi, LayoutTable)> {
        self.declarations.pretransfer_abi(reference, target)
    }
    fn requires_structural_reference(&self) -> bool {
        self.source.is_some() && self.declarations.requires_structural_reference()
    }
    fn structural_access(
        &self,
        source: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<hirsel_lir::structural_access::AccessPlan, LayoutError> {
        self.declarations.structural_access(source, table, target)
    }
    fn requires_invocation_reference(&self) -> bool {
        self.source.is_some() && self.declarations.requires_invocation_reference()
    }
    fn invocations(
        &self,
        definition: &DefPath,
    ) -> Option<&[hirsel_lir::invocations::ReferenceInvocation]> {
        self.source
            .filter(|(selected, _)| *selected == definition)?;
        self.declarations.invocations(definition)
    }
    fn invocation_body(&self, definition: &DefPath, instance: ContentHash) -> Option<u32> {
        self.source
            .filter(|(selected, id)| *selected == definition && *id == instance)?;
        self.declarations.invocation_body(definition, instance)
    }
    fn derive(
        &self,
        operation: &hirsel_core_contract::OperationId,
        arguments: &[OperationArgument],
        target: &TargetSpec,
    ) -> Result<(FnAbi, LayoutTable), LayoutError> {
        self.declarations.derive(operation, arguments, target)
    }
}
