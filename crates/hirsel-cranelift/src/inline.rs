//! Inline the checked private push body with Cranelift's own CFG/value remapping.
//! See cranelift-codegen 0.136.3, src/inline.rs:16-21 and 64-80. The body keeps
//! its physical checkpoints, failure edges, tracked-growth test and strict-C stores.
use cranelift_codegen::inline::{Inline, InlineCommand};
use cranelift_codegen::ir;
use cranelift_module::FuncId;
use std::borrow::Cow;
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct PrivatePush(BTreeMap<FuncId, ir::Function>);

impl PrivatePush {
    pub(crate) fn insert(&mut self, id: FuncId, body: ir::Function) {
        self.0.insert(id, body);
    }
}

impl Inline for PrivatePush {
    fn inline(
        &mut self,
        caller: &ir::Function,
        _call_inst: ir::Inst,
        _call_opcode: ir::Opcode,
        callee: ir::FuncRef,
        _call_args: &[ir::Value],
    ) -> InlineCommand<'_> {
        let reference = &caller.dfg.ext_funcs[callee];
        let ir::ExternalName::User(name) = reference.name else {
            return InlineCommand::KeepCall;
        };
        let name = &caller.params.user_named_funcs()[name];
        let Some(body) = (name.namespace == 0)
            .then(|| self.0.get(&FuncId::from_u32(name.index)))
            .flatten()
        else {
            return InlineCommand::KeepCall;
        };
        if caller.dfg.signatures[reference.signature] != body.signature {
            return InlineCommand::KeepCall;
        }
        InlineCommand::Inline {
            callee: Cow::Borrowed(body),
            visit_callee: false,
        }
    }
}
