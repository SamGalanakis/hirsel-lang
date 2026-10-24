//! The production bridge and ELF reader fail closed for an actual dynamic frame.
use llvm_sys::{core::*, target::*, target_machine::*};
use std::ptr;

unsafe extern "C" {
    fn hirsel_llvm_emit_stack_sizes(machine: LLVMTargetMachineRef);
}

#[test]
fn final_fixed_frame_is_measured_and_dynamic_alloca_has_no_certificate() {
    unsafe {
        assert_eq!(LLVM_InitializeNativeTarget(), 0);
        assert_eq!(LLVM_InitializeNativeAsmPrinter(), 0);
        let ctx = LLVMContextCreate();
        let module = LLVMModuleCreateWithNameInContext(c"ahead_frames".as_ptr(), ctx);
        let builder = LLVMCreateBuilderInContext(ctx);
        let void = LLVMVoidTypeInContext(ctx);
        let word = LLVMInt64TypeInContext(ctx);
        let byte = LLVMInt8TypeInContext(ctx);
        let pointer = LLVMPointerTypeInContext(ctx, 0);
        let mut sink_args = [pointer];
        let sink_type = LLVMFunctionType(void, sink_args.as_mut_ptr(), 1, 0);
        let sink = LLVMAddFunction(module, c"frame_sink".as_ptr(), sink_type);
        for dynamic in [false, true] {
            let mut args = [word];
            let ty = LLVMFunctionType(void, args.as_mut_ptr(), u32::from(dynamic), 0);
            let name = if dynamic { c"dynamic" } else { c"fixed" };
            let f = LLVMAddFunction(module, name.as_ptr(), ty);
            let block = LLVMAppendBasicBlockInContext(ctx, f, c"entry".as_ptr());
            LLVMPositionBuilderAtEnd(builder, block);
            let count = if dynamic {
                LLVMGetParam(f, 0)
            } else {
                LLVMConstInt(word, 4096, 0)
            };
            let storage = LLVMBuildArrayAlloca(builder, byte, count, c"storage".as_ptr());
            LLVMSetAlignment(storage, 16);
            let kind = LLVMGetEnumAttributeKindForName(c"noredzone".as_ptr(), 9);
            assert_ne!(kind, 0);
            LLVMAddAttributeAtIndex(
                f,
                llvm_sys::LLVMAttributeFunctionIndex,
                LLVMCreateEnumAttribute(ctx, kind, 0),
            );
            let mut call_args = [storage];
            LLVMBuildCall2(
                builder,
                sink_type,
                sink,
                call_args.as_mut_ptr(),
                1,
                c"".as_ptr(),
            );
            LLVMBuildRetVoid(builder);
        }
        let triple = c"x86_64-unknown-linux-gnu";
        let mut target = ptr::null_mut();
        let mut error = ptr::null_mut();
        assert_eq!(
            LLVMGetTargetFromTriple(triple.as_ptr(), &mut target, &mut error),
            0
        );
        let machine = LLVMCreateTargetMachine(
            target,
            triple.as_ptr(),
            c"x86-64".as_ptr(),
            c"".as_ptr(),
            LLVMCodeGenOptLevel::LLVMCodeGenLevelNone,
            LLVMRelocMode::LLVMRelocPIC,
            LLVMCodeModel::LLVMCodeModelDefault,
        );
        assert!(!machine.is_null());
        hirsel_llvm_emit_stack_sizes(machine);
        LLVMSetTarget(module, triple.as_ptr());
        let data = LLVMCreateTargetDataLayout(machine);
        LLVMSetModuleDataLayout(module, data);
        let mut buffer = ptr::null_mut();
        assert_eq!(
            LLVMTargetMachineEmitToMemoryBuffer(
                machine,
                module,
                LLVMCodeGenFileType::LLVMObjectFile,
                &mut error,
                &mut buffer
            ),
            0
        );
        let bytes = std::slice::from_raw_parts(
            LLVMGetBufferStart(buffer).cast::<u8>(),
            LLVMGetBufferSize(buffer),
        );
        let frames = hirsel_lir::object::llvm_stack_sizes(bytes).unwrap();
        assert!(frames["fixed"] >= 4096 + 8);
        assert!(
            !frames.contains_key("dynamic"),
            "missing evidence must never mean frame zero"
        );
        LLVMDisposeMemoryBuffer(buffer);
        LLVMDisposeTargetData(data);
        LLVMDisposeTargetMachine(machine);
        LLVMDisposeBuilder(builder);
        LLVMDisposeModule(module);
        LLVMContextDispose(ctx);
    }
}
