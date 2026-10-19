//! Package-test selection, emitted by the same backend as its test entries.
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, MemFlagsData, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_module::{Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use hirsel_lir::{emission::OptLevel, layout_types::TargetSpec};

use crate::{
    CodegenError,
    module::{Emitted, PTR},
};

pub(crate) fn emit(target: &TargetSpec, entries: &[&str]) -> Emitted<Vec<u8>> {
    let isa = crate::module::isa(target, OptLevel::None)?;
    let builder = ObjectBuilder::new(
        isa,
        "hirsel-tests",
        cranelift_module::default_libcall_names(),
    )
    .map_err(refuse)?;
    let mut module = ObjectModule::new(builder);
    let mut signature = module.make_signature();
    signature
        .params
        .extend([AbiParam::new(types::I32), AbiParam::new(PTR)]);
    signature.returns.push(AbiParam::new(types::I32));
    let main = module
        .declare_function("main", Linkage::Export, &signature)
        .map_err(refuse)?;
    let entries: Vec<_> = entries
        .iter()
        .map(|name| {
            module
                .declare_function(name, Linkage::Import, &signature)
                .map_err(refuse)
        })
        .collect::<Emitted<_>>()?;
    let mut context = module.make_context();
    context.func.signature = signature;
    let mut frontend = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut context.func, &mut frontend);
        let start = b.create_block();
        let select = b.create_block();
        let digit = b.create_block();
        let parse = b.create_block();
        let accumulate = b.create_block();
        let dispatch = b.create_block();
        let invalid = b.create_block();
        b.append_block_param(digit, PTR);
        b.append_block_param(digit, types::I64);
        b.append_block_params_for_function_params(start);
        b.switch_to_block(start);
        let argc = b.block_params(start)[0];
        let argv = b.block_params(start)[1];
        let valid = b.ins().icmp_imm_u(ir::condcodes::IntCC::Equal, argc, 2);
        b.ins().brif(valid, select, &[], invalid, &[]);
        b.switch_to_block(select);
        let argument = b.ins().load(PTR, MemFlagsData::new(), argv, 8);
        let first = b.ins().load(types::I8, MemFlagsData::new(), argument, 0);
        let nonempty = b.ins().icmp_imm_u(ir::condcodes::IntCC::NotEqual, first, 0);
        let zero = b.ins().iconst(types::I64, 0);
        b.ins().brif(
            nonempty,
            digit,
            &[argument.into(), zero.into()],
            invalid,
            &[],
        );
        b.switch_to_block(digit);
        let at = b.block_params(digit)[0];
        let index = b.block_params(digit)[1];
        let byte = b.ins().load(types::I8, MemFlagsData::new(), at, 0);
        let end = b.ins().icmp_imm_u(ir::condcodes::IntCC::Equal, byte, 0);
        b.ins().brif(end, dispatch, &[], parse, &[]);
        b.switch_to_block(parse);
        let digit_value = b.ins().iadd_imm_s(byte, -48);
        let decimal = b
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::UnsignedLessThan, digit_value, 10);
        b.ins().brif(decimal, accumulate, &[], invalid, &[]);
        b.switch_to_block(accumulate);
        let wide = b.ins().uextend(types::I64, digit_value);
        let tens = b.ins().imul_imm_s(index, 10);
        let value = b.ins().iadd(tens, wide);
        let next = b.ins().iadd_imm_s(at, 1);
        let named = b.ins().icmp_imm_u(
            ir::condcodes::IntCC::UnsignedLessThan,
            value,
            entries.len() as i64,
        );
        b.ins()
            .brif(named, digit, &[next.into(), value.into()], invalid, &[]);
        b.switch_to_block(dispatch);
        let mut switch = Switch::new();
        let mut cases = Vec::new();
        for (index, _) in entries.iter().enumerate() {
            let block = b.create_block();
            switch.set_entry(index as u128, block);
            cases.push((index, block));
        }
        switch.emit(&mut b, index, invalid);
        for (index, block) in cases {
            b.switch_to_block(block);
            let entry = module.declare_func_in_func(entries[index as usize], b.func);
            b.func.dfg.ext_funcs[entry].colocated = true;
            let argc = b.ins().iconst(types::I32, 1);
            let argv = b.ins().iadd_imm_s(argv, 8);
            let call = b.ins().call(entry, &[argc, argv]);
            let code = b.inst_results(call)[0];
            b.ins().return_(&[code]);
        }
        b.switch_to_block(invalid);
        let code = b.ins().iconst(types::I32, 3);
        b.ins().return_(&[code]);
        b.seal_all_blocks();
        b.finalize(module.target_config());
    }
    module.define_function(main, &mut context).map_err(refuse)?;
    crate::object::finish(module.finish())
}

fn refuse(error: impl std::fmt::Display) -> CodegenError {
    CodegenError::Object {
        message: format!("test dispatcher: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatcher_selects_entries_and_refuses_malformed_or_out_of_range_indices() {
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let root =
            std::env::temp_dir().join(format!("hirsel-clif-dispatcher-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let isa = crate::module::isa(&target, OptLevel::None).unwrap();
        let mut module = ObjectModule::new(
            ObjectBuilder::new(
                isa,
                "test-entries",
                cranelift_module::default_libcall_names(),
            )
            .unwrap(),
        );
        let mut sig = module.make_signature();
        sig.params
            .extend([AbiParam::new(types::I32), AbiParam::new(PTR)]);
        sig.returns.push(AbiParam::new(types::I32));
        for (index, name) in ["test_0", "test_1"].into_iter().enumerate() {
            let id = module
                .declare_function(name, Linkage::Export, &sig)
                .unwrap();
            let mut context = module.make_context();
            context.func.signature = sig.clone();
            let mut frontend = FunctionBuilderContext::new();
            let mut b = FunctionBuilder::new(&mut context.func, &mut frontend);
            let block = b.create_block();
            b.append_block_params_for_function_params(block);
            b.switch_to_block(block);
            // Only the selected text is visible to the adapter, at argv[0].
            let argc = b.block_params(block)[0];
            let argv = b.block_params(block)[1];
            let text = b.ins().load(PTR, MemFlagsData::new(), argv, 0);
            let first = b.ins().load(types::I8, MemFlagsData::new(), text, 0);
            let first = b.ins().uextend(types::I32, first);
            let code = b.ins().iadd_imm_s(first, index as i64 + 10 - 48);
            let valid = b.ins().icmp_imm_u(ir::condcodes::IntCC::Equal, argc, 1);
            let bad = b.ins().iconst(types::I32, 99);
            let code = b.ins().select(valid, code, bad);
            b.ins().return_(&[code]);
            b.seal_all_blocks();
            b.finalize(module.target_config());
            module.define_function(id, &mut context).unwrap();
        }
        std::fs::write(
            root.join("entries.o"),
            crate::object::finish(module.finish()).unwrap(),
        )
        .unwrap();
        let dispatcher = emit(&target, &["test_0", "test_1"]).unwrap();
        std::fs::write(root.join("dispatcher.o"), dispatcher).unwrap();
        let prefix = std::path::PathBuf::from(std::env::var_os("LLVM_SYS_211_PREFIX").unwrap());
        let result = std::process::Command::new(prefix.join("bin/hirsel-clang"))
            .arg(root.join("entries.o"))
            .arg(root.join("dispatcher.o"))
            .arg("-o")
            .arg(root.join("tests"))
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        for (args, expected) in [
            (vec!["0"], 10),
            (vec!["1"], 12),
            (vec!["00"], 10),
            (vec!["01"], 11),
            (vec![], 3),
            (vec!["0", "1"], 3),
            (vec![""], 3),
            (vec!["2"], 3),
            (vec!["-1"], 3),
            (vec!["+1"], 3),
            (vec![" 1"], 3),
            (vec!["0x1"], 3),
            (vec!["1x"], 3),
            (vec!["999999999999999999999999999999"], 3),
        ] {
            let result = std::process::Command::new(root.join("tests"))
                .args(&args)
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(expected), "{args:?}: {result:?}");
        }
        // The same dispatcher also emits a target-correct AArch64 object.
        let arm = TargetSpec::aarch64_unknown_linux_gnu();
        hirsel_lir::object::ObjectFile::read(&arm, emit(&arm, &["test_0", "test_1"]).unwrap())
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
