//! What the Ready stage must have discharged before emission.
//!
//! The vocabulary check already rejects semantic Drop and unavailable
//! placeholders, the structure check rejects a missing placement, and the
//! credit replay rejects a pending (unconsumed) token. This module adds the
//! remaining Ready obligations: no unresolved call destination, no operation
//! without a native implementation (a scalar intrinsic is an instruction, and
//! a runtime primitive's template resolves to a symbol for the call's scalar
//! kinds, `abi::resolve_symbol`), and no `Copy` whose recipe has a plainer
//! spelling. A Copy of a Dup or Fields layout is the Ready primitive (a load
//! plus one increment per counted leaf); a Bits copy is `Read Bits` and a
//! FlatBytes copy is `FlatCopy`, and any other recipe is unresolved. LIR
//! carries no TypeId and no size-parameter term, so an unresolved generic
//! parameter cannot be written.
use super::{Cx, Report, Site, ViolationKind as K};
use crate::effect::Opcode;
use crate::ir::{BlockId, Callee, DestinationClass, Inst, Terminator};
use crate::layout_types::CopyRecipe;
use hirsel_core_contract::{Disposition, Engine};

pub(super) fn check<S>(cx: &Cx<'_, S>, report: &mut Report) {
    let unit = cx.unit;
    for (b, block) in unit.blocks.iter().enumerate() {
        let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
        for (i, inst) in insts.iter().enumerate() {
            if let Inst::Copy { result, .. } = *inst
                && !matches!(
                    cx.layouts.copy(cx.value_layout(result)),
                    Some(CopyRecipe::Dup | CopyRecipe::Fields)
                )
            {
                report.add(
                    Site::at(BlockId(b as u32), i as u32),
                    K::Vocabulary(Opcode::Copy),
                );
            }
        }
    }
    for (b, term) in unit.terminators.iter().enumerate() {
        let Terminator::Call { call, .. } = *term else {
            continue;
        };
        let site = Site::at(BlockId(b as u32), unit.blocks[b].instructions.len);
        let call = &unit.tables.calls[call.index()];
        if matches!(call.destination, Some(DestinationClass::Inferred)) {
            report.add(site, K::UnresolvedDestination);
        }
        if let Callee::Operation(op) = call.callee {
            let id = &unit.tables.operations[op.index()];
            // A scalar intrinsic is an instruction at Ready (LIR schema
            // revision 6), never a call; a runtime primitive's symbol
            // template must name a symbol for this call's scalar kinds.
            let supported =
                cx.manifest
                    .row(id)
                    .is_some_and(|row| match row.dispositions.get(Engine::Native) {
                        Disposition::Body => true,
                        Disposition::RuntimePrimitive(template) => {
                            crate::abi::resolve_call_symbol(template, unit, call).is_some()
                        }
                        _ => false,
                    });
            if !supported {
                report.add(site, K::UnsupportedOperation(op));
            }
        }
    }
}
