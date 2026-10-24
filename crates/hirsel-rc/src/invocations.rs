//! Invocation evidence from independently validated source and instance plans.
//!
//! This factory reads no candidate LIR. Query-backed importers can supply one
//! original source definition without reconstructing a whole-program Lowered.
use hirsel_core_contract::OperationId;
use hirsel_layout::instance::InstanceView;
use hirsel_lir::{
    abi_types::FnAbiId,
    invocations::{ReferenceBody, ReferenceInvocation},
    ir::{Symbol, SymbolKind},
    pretransfer::AdapterReference,
};
use hirsel_syntax::DefPath;
use hirsel_tir::{BodyId, Definition};
use hirsel_types::{GenericArgument, TypeInterner};
use std::collections::BTreeMap;

/// Re-derive call identities and legal self restarts from original validated
/// TIR plus its independent declaration universe, never candidate markers.
pub fn source_rows(
    types: &TypeInterner,
    definition: &Definition,
    signatures: &dyn hirsel_validate::SignatureLookup,
    operations: &BTreeMap<DefPath, OperationId>,
) -> Result<Vec<ReferenceInvocation>, &'static str> {
    hirsel_validate::invocation_coverage(types, definition, signatures).map(|coverage| {
        coverage
            .into_iter()
            .map(|(body, block, statement, restart)| {
                let callee =
                    match &definition.bodies[body as usize].blocks[block as usize].terminator {
                        hirsel_tir::Terminator::Call {
                            callee: hirsel_tir::Callee::Definition { path, .. },
                            ..
                        } => Some(path.clone()),
                        _ => None,
                    };
                ReferenceInvocation {
                    body,
                    block,
                    statement,
                    restart,
                    primitive: callee
                        .as_ref()
                        .and_then(|path| operations.get(path))
                        .cloned(),
                    callee,
                }
            })
            .collect()
    })
}

/// The association belongs to the independent instance/specialization plan.
/// Neither the source body's ordinal nor its selected symbol comes from LIR.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceInstance {
    pub body: ReferenceBody,
    pub invocations: Vec<ReferenceInvocation>,
}

/// Bind already-derived original source rows to an independently selected unit.
/// A specialization may have a new symbol while retaining its original body.
pub fn bind_source(
    original: &Definition,
    rows: Vec<ReferenceInvocation>,
    selected: &Symbol,
    body: BodyId,
) -> Result<SourceInstance, &'static str> {
    if body.0 as usize >= original.bodies.len() {
        return Err("selected invocation body is outside original source");
    }
    if selected.kind != SymbolKind::Function {
        return Err("selected invocation symbol is not a function");
    }
    let instance = selected
        .instance
        .ok_or("selected invocation symbol has no instance")?;
    Ok(SourceInstance {
        body: ReferenceBody {
            definition: selected.definition.clone(),
            instance,
            body: body.0,
        },
        invocations: rows,
    })
}

/// One query's source input and independently planned concrete selection.
pub fn source_instance(
    types: &TypeInterner,
    original: &Definition,
    signatures: &dyn hirsel_validate::SignatureLookup,
    operations: &BTreeMap<DefPath, OperationId>,
    selected: &Symbol,
    body: BodyId,
) -> Result<SourceInstance, &'static str> {
    bind_source(
        original,
        source_rows(types, original, signatures, operations)?,
        selected,
        body,
    )
}

/// Bind V-owned compact original observations without restoring a Definition.
/// The caller supplies the independent instance selection, exactly as for the
/// miss factory; candidate LIR supplies neither the owner nor body ordinal.
pub fn source_instance_from_output(
    facts: &hirsel_validate::VerifiedInvocations,
    source: &DefPath,
    operations: &BTreeMap<DefPath, OperationId>,
    selected: &Symbol,
    body: BodyId,
) -> Result<SourceInstance, &'static str> {
    if facts.owner().path() != source {
        return Err("invocation observations belong to another original source");
    }
    if body.0 >= facts.body_count() {
        return Err("selected invocation body is outside original source");
    }
    if selected.kind != SymbolKind::Function {
        return Err("selected invocation symbol is not a function");
    }
    let instance = selected
        .instance
        .ok_or("selected invocation symbol has no instance")?;
    Ok(SourceInstance {
        body: ReferenceBody {
            definition: selected.definition.clone(),
            instance,
            body: body.0,
        },
        invocations: facts
            .calls()
            .iter()
            .map(|call| ReferenceInvocation {
                body: call.body,
                block: call.block,
                statement: call.statement,
                restart: call.restart,
                primitive: call
                    .callee
                    .as_ref()
                    .and_then(|path| operations.get(path))
                    .cloned(),
                callee: call.callee.clone(),
            })
            .collect(),
    })
}

/// Bind a materialized keyed adapter and its primitive from the original
/// instance plans. ABI is the planner's ABI, never a candidate call's selector.
pub fn keyed_adapter(
    adapter: &InstanceView,
    adapter_actuals: &[GenericArgument],
    primitive: &InstanceView,
    primitive_actuals: &[GenericArgument],
    abi: FnAbiId,
) -> Result<AdapterReference, &'static str> {
    if hirsel_lir::pretransfer::primitive(adapter.key.definition()).as_ref()
        != Some(primitive.key.definition())
        || adapter_actuals != primitive_actuals
    {
        return Err("keyed adapter differs from independently planned primitive or actuals");
    }
    let symbol = |view: &InstanceView| Symbol {
        kind: SymbolKind::Function,
        definition: view.key.definition().clone(),
        instance: Some(view.key.digest()),
    };
    Ok(AdapterReference {
        adapter: symbol(adapter),
        primitive: symbol(primitive),
        abi,
    })
}
