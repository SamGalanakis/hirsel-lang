//! Reconstruct the independent source observation scope of one base Instance.
//! Snapshot-wide unions retain declarations for every owner, while a base C
//! receipt observes only its selected source instance or reviewed library plan.
use hirsel_lir::operation::{NativeType, OperationArgument, OperationDeclarations};
use hirsel_lir::{FnAbi, LayoutTable, TargetSpec, layout::LayoutError};
use hirsel_syntax::{ContentHash, DefPath};

pub(super) struct InstanceOperations<'a> {
    pub declarations: &'a (dyn OperationDeclarations + Sync),
    pub source: Option<(&'a DefPath, ContentHash)>,
    pub all_sources: bool,
    pub pretransfer: Option<Vec<hirsel_lir::pretransfer::AdapterReference>>,
}

/// Numeric selectors belong to the current table. Match full independently
/// derived ABI content before using an adapter with a portable unit binding.
pub(super) fn bind_pretransfer(
    declarations: &(dyn OperationDeclarations + Sync),
    table: &LayoutTable,
    target: &TargetSpec,
) -> Result<Vec<hirsel_lir::pretransfer::AdapterReference>, String> {
    let keys = hirsel_lir::LayoutKeys::new(table);
    declarations
        .pretransfer_adapters()
        .iter()
        .map(|reference| {
            let (abi, source) = declarations
                .pretransfer_abi(reference, target)
                .ok_or("missing independently derived adapter ABI")?;
            let expected = hirsel_lir::LayoutKeys::new(&source).fn_abi(&abi);
            let index = table
                .fn_abis
                .iter()
                .position(|abi| keys.fn_abi(abi) == expected)
                .ok_or("independently planned adapter ABI is absent from bound layouts")?;
            Ok(hirsel_lir::pretransfer::AdapterReference {
                abi: hirsel_lir::FnAbiId(index as u32),
                ..reference.clone()
            })
        })
        .collect()
}
impl OperationDeclarations for InstanceOperations<'_> {
    fn receipt_source(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<hirsel_lir::sealed_payload_source::ReceiptSource> {
        self.declarations.receipt_source(definition, instance)
    }

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
        self.pretransfer
            .as_deref()
            .unwrap_or_else(|| self.declarations.pretransfer_adapters())
    }
    fn pretransfer_abi(
        &self,
        reference: &hirsel_lir::pretransfer::AdapterReference,
        target: &TargetSpec,
    ) -> Option<(FnAbi, LayoutTable)> {
        let original = self
            .declarations
            .pretransfer_adapters()
            .iter()
            .find(|original| {
                original.adapter == reference.adapter && original.primitive == reference.primitive
            })?;
        self.declarations.pretransfer_abi(original, target)
    }
    fn requires_structural_reference(&self) -> bool {
        (self.all_sources || self.source.is_some())
            && self.declarations.requires_structural_reference()
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
        (self.all_sources || self.source.is_some())
            && self.declarations.requires_invocation_reference()
    }
    fn invocations(
        &self,
        definition: &DefPath,
    ) -> Option<&[hirsel_lir::invocations::ReferenceInvocation]> {
        if !self.all_sources {
            self.source
                .filter(|(selected, _)| *selected == definition)?;
        }
        self.declarations.invocations(definition)
    }
    fn invocation_body(&self, definition: &DefPath, instance: ContentHash) -> Option<u32> {
        // The owner's call inventory above remains scoped. A synthesized
        // cleanup call separately checks its target's exact independently
        // selected body against the layout's declared finalizer. Filtering
        // those identities to the caller hides a required C dependency.
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
