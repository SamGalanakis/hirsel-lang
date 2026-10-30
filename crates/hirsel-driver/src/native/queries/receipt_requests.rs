//! Receipt request reconstruction from current original source, before C replay.
use super::{source_inputs::NativeSourceProvider, *};
use crate::native::receipt_source::{factory::*, provider::SelectionProvider};
use hirsel_engine::{NativeRecords, store::Domain};
use hirsel_lir::sealed_payload_source::ReceiptSource;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) const ROOTS: Domain = Domain {
    kind: 0x4e00_002f,
    schema: 1,
};
pub(super) type Sources = BTreeMap<(hirsel_syntax::DefPath, ContentHash), ReceiptSource>;

#[derive(Serialize, Deserialize)]
struct Root {
    definition: hirsel_syntax::DefPath,
    body: u32,
    encoding: Vec<u8>,
    actuals: hirsel_rc::instance::Actuals,
}

pub(super) fn encode(
    source: &impl NativeSourceProvider,
    records: &NativeRecords,
    roots: &[NamedInput],
) -> Result<Vec<u8>, String> {
    records
        .encode(
            source,
            &roots
                .iter()
                .map(|r| Root {
                    definition: r.base.definition().clone(),
                    body: r.base.body(),
                    encoding: r.base.encoding().to_vec(),
                    actuals: r.actuals.clone(),
                })
                .collect::<Vec<_>>(),
        )
        .ok_or_else(|| "receipt root encoding failed".into())
}

fn roots(
    source: &impl NativeSourceProvider,
    target: &TargetSpec,
) -> Result<Vec<NamedInput>, String> {
    let declarations = source.declarations();
    let owners = hirsel_rc::library::owners(
        &declarations.program.types,
        &declarations.program.signatures,
    )
    .over(&declarations.program.owners);
    let mut layouts = hirsel_layout::layout::LayoutBuilder::new(target, &owners);
    layouts.prepare_native_controls();
    let (instances, roots) = hirsel_rc::instance::seed_instances(
        &declarations.program,
        &declarations.context,
        &mut layouts,
    )
    .map_err(|e| format!("receipt original root registration: {e:?}"))?;
    Ok(roots
        .into_iter()
        .filter(|id| instances.roles[id.index()] == hirsel_rc::instance::InstanceRole::Function)
        .map(|id| NamedInput {
            base: instances.lexical_key(id).clone(),
            actuals: instances.actuals[id.index()].clone(),
        })
        .collect())
}

pub(super) fn current(
    source: &impl NativeSourceProvider,
    records: &NativeRecords,
    store: &dyn hirsel_engine::store::ResultStore,
    target: &TargetSpec,
    reference: Option<Reference>,
) -> Result<(NamedGraph, Sources), String> {
    let roots = roots(source, target)?;
    if roots.is_empty() || roots.len() > hirsel_rc::instance::MAX_INSTANCES {
        return Err("receipt root limit".into());
    }
    if let Some(reference) = reference {
        if reference.domain != ROOTS {
            return Err("foreign receipt root product".into());
        }
        let bytes = store
            .get_content(reference)
            .ok_or("missing receipt root product")?;
        if bytes.as_ref() != encode(source, records, &roots)?.as_slice() {
            return Err(
                "receipt root product differs from independently selected source roots".into(),
            );
        }
    }
    let provider = source
        .receipt_provider()
        .ok_or("current receipt source provider unavailable")?;
    reconstruct(&provider, roots, &source.declarations().program)
}

pub(super) fn reconstruct(
    provider: &dyn SelectionProvider,
    roots: Vec<NamedInput>,
    program: &hirsel_rc::instance::Program,
) -> Result<(NamedGraph, Sources), String> {
    let mut cases = |_: &hirsel_rc::sealed_source::SourceCallableEntry, _, _| {
        Err(hirsel_rc::sealed_source::SourceError::Unsupported(
            "receipt callable boundary profiles require environment custody",
        ))
    };
    let graph = named_graph(provider, roots.clone(), &mut cases)
        .map_err(|e| format!("receipt source graph: {e:?}"))?;
    let declarations = graph
        .declarations(provider, roots, &mut cases, &())
        .map_err(|e| format!("current receipt declarations: {e:?}"))?;
    let mut sources = BTreeMap::new();
    for request in &graph.requests {
        if let Some(expected) = request
            .receipt_source()
            .map_err(|e| format!("receipt source: {e:?}"))?
        {
            let actual = hirsel_lir::operation::OperationDeclarations::receipt_source(
                &declarations,
                &expected.original.definition,
                expected.instance,
            )
            .ok_or("rechecked receipt declaration missing")?;
            sources.insert(
                (actual.original.definition.clone(), actual.instance),
                actual,
            );
        }
    }
    for operation in &graph.operations {
        if !operation.operation.definition.is_core("arena.seal") {
            continue;
        }
        let context = &graph.contexts[operation.caller.index()];
        let call = &context.source.demand().projection().calls()[&operation.block];
        let hirsel_tir::Callee::Definition { path, arguments } = &call.callee else {
            return Err("reviewed seal lacks original actuals".into());
        };
        let [hirsel_types::GenericArgument::Type(root)] = arguments.as_slice() else {
            return Err("reviewed seal root actual".into());
        };
        let root = hirsel_rc::instance::substitute(
            provider.types(),
            &hirsel_types::TypeMemo::default(),
            *root,
            &context.input.actuals,
            true,
        )
        .map_err(|e| format!("reviewed seal actual: {e:?}"))?;
        let request = hirsel_rc::library::Request::operation(
            program,
            usize::MAX,
            path,
            vec![hirsel_types::GenericArgument::Type(root)],
        )
        .map_err(|e| format!("reviewed seal declaration: {e:?}"))?;
        let Some(source) = request.receipt else {
            continue;
        };
        sources.insert(
            (source.original.definition.clone(), source.instance),
            source,
        );
    }
    Ok((graph, sources))
}

pub(super) fn lexical(
    source: &mut impl NativeSourceProvider,
    target: &TargetSpec,
    selected: &hirsel_rc::sealed_source::contract::registry::Request,
) -> Result<hirsel_layout::instance::InstanceKey, String> {
    let entry = selected.declaration().entry();
    if matches!(
        entry.kind(),
        hirsel_rc::sealed_source::SourceEntryKind::Definition { .. }
    ) {
        return Ok(selected.ordinary_key().clone());
    }
    let (original, _) = source.original_source(&entry.reference().definition)?;
    let declarations = source.declarations();
    let owners = hirsel_rc::library::owners(
        &declarations.program.types,
        &declarations.program.signatures,
    )
    .over(&declarations.program.owners);
    let mut layouts = hirsel_layout::layout::LayoutBuilder::new(target, &owners);
    layouts.prepare_native_controls();
    hirsel_rc::instance::receipt_closure_binding(
        &declarations.program.types,
        original.definition(),
        entry,
        &mut layouts,
    )
    .map(|(key, _)| key)
    .map_err(|error| format!("current receipt closure lexical identity: {error:?}"))
}

pub(super) fn descriptor(
    source: &ReceiptSource,
    ordinary: &dyn OperationDeclarations,
    root: &NativeType,
    table: &LayoutTable,
    target: &TargetSpec,
) -> Result<hirsel_lir::sealed::SealDescriptor, LayoutError> {
    let profiles = source
        .profiles
        .iter()
        .filter(|p| &p.profile.root.ty == root)
        .collect::<Vec<_>>();
    if profiles.is_empty() || profiles.iter().any(|p| !p.profile.functions.is_empty()) {
        return Err(LayoutError::Invalid(
            "receipt descriptor lacks a complete original data profile".into(),
        ));
    }
    ordinary.seal_descriptor(root, table, target)
}

/// Owned declarations for all later C passes in this immutable source snapshot.
pub(super) struct Declarations {
    ordinary: Arc<dyn hirsel_lir::operation::OperationDeclarations + Send + Sync>,
    sources: Sources,
}
impl Declarations {
    pub(super) fn new(
        ordinary: Arc<dyn hirsel_lir::operation::OperationDeclarations + Send + Sync>,
        sources: Sources,
    ) -> Self {
        Self { ordinary, sources }
    }
}

use hirsel_lir::operation::{NativeType, OperationArgument, OperationDeclarations};
use hirsel_lir::{FnAbi, LayoutTable, TargetSpec, layout::LayoutError};
use hirsel_syntax::DefPath;
impl OperationDeclarations for Declarations {
    fn receipt_source(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<hirsel_lir::sealed_payload_source::ReceiptSource> {
        self.sources
            .get(&(definition.clone(), instance))
            .cloned()
            .or_else(|| self.ordinary.receipt_source(definition, instance))
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
    fn seal_environments(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<hirsel_lir::sealed_payload_source::SourceEnvironments> {
        self.ordinary.seal_environments(definition, instance)
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
        if let Some(selected) = self.sources.get(&(definition.clone(), instance)) {
            return descriptor(selected, self.ordinary.as_ref(), source, table, target);
        }
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
