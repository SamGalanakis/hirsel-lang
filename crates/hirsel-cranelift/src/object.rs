//! The canonical object stream, shared by artifacts and generated launchers.
use cranelift_object::{ObjectProduct, object};
use object::read::elf::{FileHeader, Rela, SectionHeader};
use object::{LittleEndian, SectionIndex, elf};

use crate::{CodegenError, module::Emitted};

pub(crate) fn finish(product: ObjectProduct) -> Emitted<Vec<u8>> {
    let mut bytes = product.emit().map_err(object_error)?;
    relax_got_loads(&mut bytes).map_err(object_error)?;
    Ok(bytes)
}

fn object_error(error: impl std::fmt::Display) -> CodegenError {
    CodegenError::Object {
        message: error.to_string(),
    }
}

fn relax_got_loads(bytes: &mut [u8]) -> object::read::Result<()> {
    let endian = LittleEndian;
    let file = object::read::elf::ElfFile64::<LittleEndian>::parse(&*bytes)?;
    if file.elf_header().e_machine(endian) != elf::EM_X86_64 {
        return Ok(());
    }
    let sections = file.elf_section_table();
    let mut patches = Vec::new();
    for section in sections.iter() {
        let Some((relocations, _)) = section.rela(endian, &*bytes)? else {
            continue;
        };
        let code = sections.section(SectionIndex(section.sh_info(endian) as usize))?;
        if !code.sh_flags(endian).contains(elf::SHF_EXECINSTR) {
            continue;
        }
        let data = code.data(endian, &*bytes)?;
        for (index, relocation) in relocations.iter().enumerate() {
            if relocation.r_type(endian, false) != elf::R_X86_64_GOTPCREL
                || relocation.r_addend(endian) != -4
            {
                continue;
            }
            let offset = relocation.r_offset(endian) as usize;
            let Some(kind) = relaxable_instruction(data, offset) else {
                continue;
            };
            // ELF64 r_info's low word is the type; retain the symbol, addend,
            // section order and every code byte. ObjectFile reads these bytes
            // only after normalization, so all consumers see the same object.
            let info =
                section.sh_offset(endian) as usize + index * std::mem::size_of_val(relocation) + 8;
            patches.push((info, kind));
        }
    }
    for (offset, kind) in patches {
        bytes[offset..offset + 4].copy_from_slice(&kind.0.to_le_bytes());
    }
    Ok(())
}

fn relaxable_instruction(code: &[u8], displacement: usize) -> Option<elf::RelocationType> {
    // Only the pinned backend's RIP-relative MOV loads and indirect CALL/JMP
    // forms have the psABI relaxation contract. Other GOT references stay plain.
    if displacement.checked_add(4)? > code.len() || displacement < 2 {
        return None;
    }
    let (opcode, modrm) = (code[displacement - 2], code[displacement - 1]);
    if modrm & 0xc7 != 0x05 {
        return None;
    }
    if opcode == 0x8b && displacement >= 3 && matches!(code[displacement - 3], 0x48 | 0x4c) {
        Some(elf::R_X86_64_REX_GOTPCRELX)
    } else if opcode == 0xff && matches!(modrm, 0x15 | 0x25) {
        Some(elf::R_X86_64_GOTPCRELX)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cranelift_codegen::ir::{AbiParam, InstBuilder, types};
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
    use cranelift_module::{Linkage, Module};
    use cranelift_object::{ObjectBuilder, ObjectModule};
    use hirsel_lir::{emission::OptLevel, layout_types::TargetSpec};

    #[test]
    fn unit_calls_are_direct_on_both_targets_and_lld_relaxes_got_addresses() {
        for target in [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ] {
            check_linked_calls(&target);
        }
    }

    fn check_linked_calls(target: &TargetSpec) {
        let new_module = || {
            ObjectModule::new(
                ObjectBuilder::new(
                    crate::module::isa(target, OptLevel::None).unwrap(),
                    "relaxation",
                    cranelift_module::default_libcall_names(),
                )
                .unwrap(),
            )
        };
        let mut callee = new_module();
        let mut signature = callee.make_signature();
        signature.returns.push(AbiParam::new(types::I64));
        let mut frontend = FunctionBuilderContext::new();
        for name in ["callee", "runtime_address"] {
            let id = callee
                .declare_function(name, Linkage::Export, &signature)
                .unwrap();
            let mut context = callee.make_context();
            context.func.signature = signature.clone();
            let mut b = FunctionBuilder::new(&mut context.func, &mut frontend);
            let block = b.create_block();
            b.switch_to_block(block);
            let value = b.ins().iconst(types::I64, 42);
            b.ins().return_(&[value]);
            b.seal_all_blocks();
            b.finalize(callee.target_config());
            callee.define_function(id, &mut context).unwrap();
        }
        let callee = finish(callee.finish()).unwrap();
        let mut caller = new_module();
        let imported = caller
            .declare_function("callee", Linkage::Import, &signature)
            .unwrap();
        let runtime = caller
            .declare_function("runtime_address", Linkage::Import, &signature)
            .unwrap();
        let mut manifested = hirsel_lir::emission::Manifested::default();
        manifested.import(
            "callee",
            hirsel_lir::stage::ObjectSymbolKind::Function,
            None,
        );
        manifested.import(
            "runtime_address",
            hirsel_lir::stage::ObjectSymbolKind::Runtime,
            None,
        );
        for (name, address, target) in [
            ("caller", false, imported),
            ("address", true, runtime),
            ("runtime_caller", false, runtime),
        ] {
            let id = caller
                .declare_function(name, Linkage::Export, &signature)
                .unwrap();
            let mut context = caller.make_context();
            context.func.signature = signature.clone();
            let mut b = FunctionBuilder::new(&mut context.func, &mut frontend);
            let block = b.create_block();
            b.switch_to_block(block);
            let reference = caller.declare_func_in_func(target, b.func);
            let value = if address {
                b.ins().func_addr(types::I64, reference)
            } else {
                let call = b.ins().call(reference, &[]);
                b.inst_results(call)[0]
            };
            b.ins().return_(&[value]);
            b.seal_all_blocks();
            b.finalize(caller.target_config());
            crate::module::colocate_hirsel_symbols(&caller, &manifested, &mut context.func);
            caller.define_function(id, &mut context).unwrap();
        }
        let product = caller.finish();
        let plain = product.object.write().unwrap();
        let caller = finish(product).unwrap();
        let object = hirsel_lir::object::ObjectFile::read(target, caller.clone()).unwrap();
        let arm = target.triple.starts_with("aarch64");
        let direct_kind = if arm {
            elf::R_AARCH64_CALL26.0
        } else {
            elf::R_X86_64_PLT32.0
        };
        assert!(
            object
                .relocations
                .iter()
                .any(|r| r.symbol == "callee" && r.kind == direct_kind),
            "{:?}",
            object.relocations
        );
        assert!(
            !object
                .relocations
                .iter()
                .any(|r| r.symbol == "runtime_address" && r.kind == direct_kind)
        );
        if !arm {
            assert!(
                object
                    .relocations
                    .iter()
                    .any(|r| r.kind == elf::R_X86_64_REX_GOTPCRELX.0)
            );
            assert!(
                !object
                    .relocations
                    .iter()
                    .any(|r| r.kind == elf::R_X86_64_GOTPCREL.0)
            );
        }
        let root = std::env::temp_dir().join(format!(
            "hirsel-relaxation-{}-{}",
            std::process::id(),
            target.triple
        ));
        std::fs::create_dir_all(&root).unwrap();
        for (name, bytes) in [
            ("callee.o", callee),
            ("plain.o", plain),
            ("caller.o", caller),
        ] {
            std::fs::write(root.join(name), bytes).unwrap();
        }
        let prefix = std::path::PathBuf::from(std::env::var_os("LLVM_SYS_211_PREFIX").unwrap());
        let link = |input: &str, output: &str| {
            let result = std::process::Command::new(prefix.join("bin/ld.lld"))
                .args(["-shared", "-Bsymbolic"])
                .arg(root.join(input))
                .arg(root.join("callee.o"))
                .arg("-o")
                .arg(root.join(output))
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            std::fs::read(root.join(output)).unwrap()
        };
        let before = link("plain.o", "before.so");
        let after = link("caller.o", "after.so");
        if !arm {
            let (_, before_address) = function(&before, "address");
            let (_, after_address) = function(&after, "address");
            println!("x86 GOT address before {before_address:02x?}; after {after_address:02x?}");
            assert!(
                before_address.windows(2).any(|bytes| bytes == [0x48, 0x8b]),
                "{before_address:02x?}"
            );
            assert!(
                after_address.windows(2).any(|bytes| bytes == [0x48, 0x8d]),
                "{after_address:02x?}"
            );
        }
        let (callee, _) = function(&after, "callee");
        let (caller, code) = function(&after, "caller");
        println!(
            "{} caller at {caller:#x}: {code:02x?}; callee at {callee:#x}",
            target.triple
        );
        let direct = if arm {
            code.chunks_exact(4).enumerate().any(|(offset, bytes)| {
                let word = u32::from_le_bytes(bytes.try_into().unwrap());
                let displacement = i64::from(((word & 0x03ff_ffff) << 6) as i32 >> 4);
                word & 0xfc00_0000 == 0x9400_0000
                    && (caller as i64 + (offset * 4) as i64 + displacement) as u64 == callee
            })
        } else {
            code.windows(5).enumerate().any(|(offset, bytes)| {
                bytes[0] == 0xe8
                    && (caller as i64
                        + offset as i64
                        + 5
                        + i64::from(i32::from_le_bytes(bytes[1..].try_into().unwrap())))
                        as u64
                        == callee
            })
        };
        assert!(
            direct,
            "{} cross-object call must be direct: {code:02x?}",
            target.triple
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    fn function(bytes: &[u8], name: &str) -> (u64, Vec<u8>) {
        use object::{Object, ObjectSection, ObjectSymbol};
        let file = object::read::elf::ElfFile64::<LittleEndian>::parse(bytes).unwrap();
        let symbol = file
            .symbols()
            .find(|symbol| symbol.name() == Ok(name))
            .unwrap();
        let section = file
            .section_by_index(symbol.section_index().unwrap())
            .unwrap();
        let start = (symbol.address() - section.address()) as usize;
        (
            symbol.address(),
            section.data().unwrap()[start..start + symbol.size() as usize].to_vec(),
        )
    }

    #[test]
    fn only_hirsel_functions_are_marked_colocated() {
        use hirsel_lir::emission::Manifested;
        use hirsel_lir::stage::ObjectSymbolKind as K;
        for target in [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ] {
            let mut module = ObjectModule::new(
                ObjectBuilder::new(
                    crate::module::isa(&target, OptLevel::None).unwrap(),
                    "classification",
                    cranelift_module::default_libcall_names(),
                )
                .unwrap(),
            );
            let signature = module.make_signature();
            let mut function = cranelift_codegen::ir::Function::new();
            let mut manifested = Manifested::default();
            let mut references = Vec::new();
            for (name, kind) in [
                ("source", Some(K::Function)),
                ("runtime", Some(K::Runtime)),
                ("foreign", Some(K::Foreign)),
                ("__divti3", None),
            ] {
                let id = module
                    .declare_function(name, Linkage::Import, &signature)
                    .unwrap();
                let reference = module.declare_func_in_func(id, &mut function);
                if let Some(kind) = kind {
                    manifested.import(name, kind, None);
                }
                references.push((reference, name, kind == Some(K::Function)));
            }
            crate::module::colocate_hirsel_symbols(&module, &manifested, &mut function);
            for (reference, name, expected) in references {
                assert_eq!(
                    function.dfg.ext_funcs[reference].colocated, expected,
                    "{name}"
                );
            }
        }
    }
    #[test]
    fn only_validated_got_load_call_and_jump_encodings_are_relaxable() {
        for rex in [0x48, 0x4c] {
            for reg in 0..8 {
                assert_eq!(
                    relaxable_instruction(&[rex, 0x8b, 5 | reg << 3, 0, 0, 0, 0], 3),
                    Some(elf::R_X86_64_REX_GOTPCRELX)
                );
            }
        }
        for opcode in [0x15, 0x25] {
            assert_eq!(
                relaxable_instruction(&[0xff, opcode, 0, 0, 0, 0], 2),
                Some(elf::R_X86_64_GOTPCRELX)
            );
        }
        for code in [
            &[0x48, 0x89, 5, 0, 0, 0, 0][..],    // store
            &[0x48, 0x8b, 4, 0, 0, 0, 0][..],    // SIB
            &[0x48, 0x8b, 0xc5, 0, 0, 0, 0][..], // register
            &[0x48, 0x8b, 5, 0, 0, 0][..],       // truncated displacement
        ] {
            assert_eq!(relaxable_instruction(code, 3), None);
        }
    }
}
