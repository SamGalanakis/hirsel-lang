//! H derives cleanup admissions from the independently selected resource
//! finalizer and body inventory. It does not consume C's reconstructed rows.
use crate::violation::{Checked, ViolationKind as V, err};
use hirsel_lir::{
    LayoutTable,
    invocations::{ReferenceBody, ReferenceInvocation},
    ir::*,
    stage::Stage,
};

pub(super) fn targets<S: Stage>(
    unit: &LirUnit<S>,
    bodies: &[ReferenceBody],
    layouts: &LayoutTable,
    owner_body: u32,
) -> Checked<Vec<Target>> {
    let malformed = |text: &str| err(V::FrameImbalance, text);
    let mut result = Vec::new();
    for (block, row) in unit.blocks.iter().enumerate() {
        let Some(Terminator::Call { call, .. }) = unit.terminators.get(block) else {
            continue;
        };
        let code = row
            .instructions
            .get(&unit.instructions)
            .ok_or_else(|| malformed("cleanup instruction range"))?;
        let Some((address, cleanup)) = code.iter().rev().find_map(|inst| match inst {
            Inst::FinalizeBegin { address, anchor } => Some((*address, *anchor)),
            _ => None,
        }) else {
            continue;
        };
        let resource = unit
            .addresses
            .get(address.index())
            .and_then(|address| unit.types.get(address.ty.index()))
            .and_then(|ty| layouts.layout(ty.layout))
            .ok_or_else(|| malformed("cleanup resource layout missing"))?;
        let Some(finalizer) = &resource.finalizer else {
            continue;
        };
        let call = unit
            .tables
            .calls
            .get(call.index())
            .ok_or_else(|| malformed("cleanup call missing"))?;
        let Callee::Direct(symbol) = call.callee else {
            continue;
        };
        let symbol = unit
            .tables
            .symbols
            .get(symbol.index())
            .ok_or_else(|| malformed("cleanup symbol missing"))?;
        let mut selected = bodies.iter().filter(|body| {
            body.definition == symbol.definition && Some(body.instance) == symbol.instance
        });
        let Some(selected_body) = selected.next() else {
            continue;
        };
        if selected.next().is_some() {
            return Err(malformed("duplicate independent cleanup body selection"));
        }
        if symbol.definition != finalizer.definition || selected_body.body != finalizer.body {
            return Err(malformed(
                "cleanup call differs from independently selected finalizer",
            ));
        }
        let cleanup = unit
            .tables
            .cleanup_anchors
            .get(cleanup.index())
            .ok_or_else(|| malformed("cleanup anchor missing"))?;
        let origin = unit
            .tables
            .source_anchors
            .get(call.anchor.index())
            .ok_or_else(|| malformed("cleanup origin missing"))?;
        if cleanup.body != owner_body
            || origin.body != owner_body
            || origin.block != u32::MAX
            || origin.statement != call.anchor.0
            || origin.site != cleanup.site
        {
            return Err(malformed(
                "cleanup invocation differs from its independent cleanup origin",
            ));
        }
        let arguments = call
            .arguments
            .get(&unit.pools.call_arguments)
            .ok_or_else(|| malformed("cleanup arguments missing"))?;
        let [argument] = arguments else {
            return Err(malformed(
                "cleanup invocation must borrow exactly one resource",
            ));
        };
        let ArgumentValue::Value(value) = argument.value else {
            return Err(malformed("cleanup invocation must borrow its resource"));
        };
        if argument.passing != hirsel_core_contract::Passing::Borrow
            || !code.iter().any(|inst| {
                matches!(inst, Inst::Read { result, address: at, mode: ReadMode::Borrow, .. }
                if *result == value && *at == address)
            })
        {
            return Err(malformed("cleanup invocation borrows a different resource"));
        }
        result.push(Target {
            address,
            reference: ReferenceInvocation {
                body: owner_body,
                block: u32::MAX,
                statement: call.anchor.0,
                restart: false,
                primitive: None,
                callee: Some(finalizer.definition.clone()),
            },
        });
    }
    Ok(result)
}

pub(super) struct Target {
    pub reference: ReferenceInvocation,
    pub address: AddressId,
}
