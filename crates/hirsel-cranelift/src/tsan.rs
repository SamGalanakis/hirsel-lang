#[cfg(test)]
#[path = "../../../tools/tests/native_tsan.rs"]
mod control;

use crate::{
    CodegenError,
    module::{Emitted, Emitter, PTR},
};
use cranelift_codegen::{
    cursor::{Cursor, FuncCursor},
    ir::{self, AbiParam, InstBuilder, InstructionData, Opcode},
};
use cranelift_module::Module;
use hirsel_lir::stage::ObjectSymbolKind;

impl Emitter<'_> {
    pub(crate) fn instrument_thread(&mut self, function: &mut ir::Function) -> Emitted {
        let mut signature = self.module.make_signature();
        signature
            .params
            .extend([AbiParam::new(PTR), AbiParam::new(PTR)]);
        let mut imports = Vec::new();
        for name in ["__tsan_read_range", "__tsan_write_range"] {
            self.manifested
                .import(name, ObjectSymbolKind::Runtime, None);
            let id = self.import_function(name, &signature)?;
            imports.push(self.module.declare_func_in_func(id, function));
        }
        signature.params.pop();
        let entry = self.import_function("__tsan_func_entry", &signature)?;
        let entry = self.module.declare_func_in_func(entry, function);
        signature.params.clear();
        let exit = self.import_function("__tsan_func_exit", &signature)?;
        let exit = self.module.declare_func_in_func(exit, function);
        for name in ["__tsan_func_entry", "__tsan_func_exit"] {
            self.manifested
                .import(name, ObjectSymbolKind::Runtime, None);
        }
        // compiler-rt tsan_interface.h: __tsan_{read,write}_range. Unlike a
        // scalar annotation, ranges also cover underaligned and aggregate data.
        let mut accesses = Vec::new();
        let mut returns = Vec::new();
        for block in function.layout.blocks() {
            for inst in function.layout.block_insts(block) {
                let data = &function.dfg.insts[inst];
                let opcode = data.opcode();
                if opcode == Opcode::Return {
                    returns.push(inst);
                }
                let (address, offset, ty, write) = match *data {
                    InstructionData::Load { arg, offset, .. } => (
                        arg,
                        i32::from(offset),
                        function.dfg.value_type(function.dfg.inst_results(inst)[0]),
                        false,
                    ),
                    InstructionData::Store { args, offset, .. } => (
                        args[1],
                        i32::from(offset),
                        function.dfg.value_type(args[0]),
                        true,
                    ),
                    _ if opcode.can_load() || opcode.can_store() => {
                        return Err(CodegenError::Object {
                            message: format!("TSan does not cover {opcode}"),
                        });
                    }
                    _ => continue,
                };
                let bytes = match opcode {
                    Opcode::Uload8 | Opcode::Sload8 | Opcode::Istore8 => 1,
                    Opcode::Uload16 | Opcode::Sload16 | Opcode::Istore16 => 2,
                    Opcode::Uload32 | Opcode::Sload32 | Opcode::Istore32 => 4,
                    _ => ty.bytes(),
                };
                accesses.push((inst, address, offset, bytes, write));
            }
        }
        let first = function
            .layout
            .first_inst(function.layout.entry_block().expect("function entry"))
            .expect("entry instruction");
        let mut cursor = FuncCursor::new(function).at_inst(first);
        let pc = cursor.ins().get_return_address(PTR);
        cursor.ins().call(entry, &[pc]);
        for inst in returns {
            FuncCursor::new(function)
                .at_inst(inst)
                .ins()
                .call(exit, &[]);
        }
        for (inst, address, offset, bytes, write) in accesses {
            let mut cursor = FuncCursor::new(function).at_inst(inst);
            let address = cursor.ins().iadd_imm_s(address, i64::from(offset));
            let length = cursor.ins().iconst(PTR, i64::from(bytes));
            cursor
                .ins()
                .call(imports[usize::from(write)], &[address, length]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::control;
    use super::*;
    use crate::module::Inputs;
    use cranelift_codegen::ir::{MemFlagsData, types};
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
    use cranelift_module::Linkage;
    use hirsel_lir::{
        emission::OptLevel,
        layout_types::{LayoutTable, TargetSpec},
    };

    #[test]
    fn native_tsan_covers_narrow_unaligned_and_wide_memory_accesses() {
        for target in [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ] {
            for opt in [OptLevel::None, OptLevel::Speed] {
                let layouts = LayoutTable::new(&target);
                let keys = hirsel_lir::LayoutKeys::new(&layouts);
                let inputs = Inputs {
                    target: &target,
                    layouts: &layouts,
                    keys: &keys,
                    opt,
                    unit: None,
                    glue: None,
                };
                let mut emitter = Emitter::new(inputs, true).unwrap();
                let mut context = emitter.module.make_context();
                context.func.signature.params.push(AbiParam::new(PTR));
                let id = emitter
                    .module
                    .declare_function("mapper", Linkage::Export, &context.func.signature)
                    .unwrap();
                let mut frontend = FunctionBuilderContext::new();
                let mut b = FunctionBuilder::new(&mut context.func, &mut frontend);
                let block = b.create_block();
                b.append_block_params_for_function_params(block);
                b.switch_to_block(block);
                let pointer = b.block_params(block)[0];
                let flags = MemFlagsData::new();
                let value = b.ins().uload8(types::I64, flags, pointer, -3);
                b.ins().istore16(flags, value, pointer, 7);
                let wide = b.ins().load(types::I128, flags, pointer, 11);
                b.ins().store(flags, wide, pointer, 27);
                b.ins().return_(&[]);
                b.seal_all_blocks();
                b.finalize(emitter.module.target_config());
                emitter.compile("mapper", id, &mut context).unwrap();
                let text = emitter.clif.as_ref().unwrap();
                for expected in [
                    "iconst.i64 -3",
                    "iconst.i64 1",
                    "iconst.i64 7",
                    "iconst.i64 2",
                    "iconst.i64 16",
                ] {
                    assert!(text.contains(expected), "missing {expected}: {text}");
                }
                assert_eq!(text.matches("call fn").count(), 6, "{text}");
                if target.triple.starts_with("x86_64") && opt == OptLevel::Speed {
                    let mut context = emitter.module.make_context();
                    context.func.signature.params.push(AbiParam::new(PTR));
                    let id = emitter
                        .module
                        .declare_function("tsan_mapper", Linkage::Export, &context.func.signature)
                        .unwrap();
                    let mut b = FunctionBuilder::new(&mut context.func, &mut frontend);
                    let block = b.create_block();
                    b.append_block_params_for_function_params(block);
                    b.switch_to_block(block);
                    let cell = b.block_params(block)[0];
                    let value = b.ins().load(types::I64, flags, cell, 0);
                    let next = b.ins().iadd_imm_s(value, 1);
                    b.ins().store(flags, next, cell, 0);
                    b.ins().return_(&[]);
                    b.seal_all_blocks();
                    b.finalize(emitter.module.target_config());
                    emitter.compile("tsan_mapper", id, &mut context).unwrap();
                }
                let bytes = crate::object::finish(emitter.module.finish()).unwrap();
                if target.triple.starts_with("x86_64") && opt == OptLevel::Speed {
                    control::emitted_access_control(&bytes);
                }
                let object = hirsel_lir::object::ObjectFile::read(&target, bytes).unwrap();
                assert_eq!(
                    object.imports,
                    [
                        "__tsan_func_entry",
                        "__tsan_func_exit",
                        "__tsan_read_range",
                        "__tsan_write_range"
                    ]
                );
            }
        }
    }
}
