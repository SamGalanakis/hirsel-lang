//! Cleanup calls are reconstructed from typed finalizer declarations. Their
//! admissions are separate from the source calls and canonical library recipe.
use super::ReferenceInvocation;
use crate::{LayoutTable, ir::*, operation::OperationDeclarations, stage::Stage};

pub(super) fn references<S: Stage>(
    unit: &LirUnit<S>,
    declarations: &dyn OperationDeclarations,
    layouts: &LayoutTable,
    body: u32,
) -> Result<Vec<ReferenceInvocation>, String> {
    let mut references = Vec::new();
    for (block, row) in unit.blocks.iter().enumerate() {
        let Some(Terminator::Call { call, .. }) = unit.terminators.get(block) else {
            continue;
        };
        let site = unit
            .tables
            .calls
            .get(call.index())
            .ok_or("cleanup call outside table")?;
        let Callee::Direct(symbol) = site.callee else {
            continue;
        };
        let symbol = unit
            .tables
            .symbols
            .get(symbol.index())
            .ok_or("cleanup symbol outside table")?;
        let Some(instance) = symbol.instance else {
            continue;
        };
        let Some(selected) = declarations.invocation_body(&symbol.definition, instance) else {
            continue;
        };
        let instructions = row
            .instructions
            .get(&unit.instructions)
            .ok_or("cleanup instruction range")?;
        let Some((address, anchor)) = instructions.iter().rev().find_map(|inst| match inst {
            Inst::FinalizeBegin { address, anchor } => Some((*address, *anchor)),
            _ => None,
        }) else {
            continue;
        };
        let address_info = unit
            .addresses
            .get(address.index())
            .ok_or("cleanup address outside table")?;
        let ty = unit
            .types
            .get(address_info.ty.index())
            .ok_or("cleanup type outside table")?;
        let layout = layouts
            .layout(ty.layout)
            .ok_or("cleanup layout outside table")?;
        let Some(finalizer) = &layout.finalizer else {
            continue;
        };
        if finalizer.definition != symbol.definition || finalizer.body != selected {
            continue;
        }
        let cleanup = unit
            .tables
            .cleanup_anchors
            .get(anchor.index())
            .ok_or("cleanup anchor outside table")?;
        let origin = unit
            .tables
            .source_anchors
            .get(site.anchor.index())
            .ok_or("cleanup origin outside table")?;
        if cleanup.body != body
            || origin.body != body
            || origin.block != u32::MAX
            || origin.statement != call.0
            || origin.site != cleanup.site
        {
            return Err("finalizer invocation differs from its cleanup origin".into());
        }
        let arguments = site
            .arguments
            .get(&unit.pools.call_arguments)
            .ok_or("cleanup argument range")?;
        let [argument] = arguments else {
            return Err("finalizer invocation has extra arguments".into());
        };
        let ArgumentValue::Value(value) = argument.value else {
            return Err("source finalizer invocation must borrow its resource".into());
        };
        if argument.passing != hirsel_core_contract::Passing::Borrow || !instructions.iter().any(|inst| {
            matches!(inst, Inst::Read { result, address: at, mode: ReadMode::Borrow, .. } if *result == value && *at == address)
        }) {
            return Err("finalizer invocation borrows another resource".into());
        }
        references.push(ReferenceInvocation {
            body,
            block: u32::MAX,
            statement: call.0,
            restart: false,
            primitive: None,
            callee: Some(finalizer.definition.clone()),
        });
    }
    Ok(references)
}
