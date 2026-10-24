//! Separate physical checkpoints. The SSA counter is flushed only at ABI boundaries.
use crate::function::{FnEmitter, flags};
use crate::module::Emitted;
use cranelift_codegen::ir::{self, InstBuilder, Value, condcodes::IntCC, types};

impl FnEmitter<'_, '_, '_> {
    fn ahead_helper(&mut self, handoff: bool, args: &[Value]) -> Emitted<Option<Value>> {
        self.rt(
            if handoff {
                hirsel_lir::runtime_abi::RuntimeEntry::AheadHandoff
            } else {
                hirsel_lir::runtime_abi::RuntimeEntry::AheadState
            },
            args,
        )
    }
    pub(crate) fn ahead_init(&mut self) -> Emitted {
        if !self.em.inputs.unit().alternate {
            return Ok(());
        }
        let state = self
            .ahead_helper(false, &[self.context])?
            .expect("getter result");
        self.ahead_state = Some(state);
        self.ahead_counter = Some(self.b.declare_var(types::I64));
        self.ahead_reload();
        Ok(())
    }
    pub(crate) fn ahead_flush(&mut self) {
        if let (Some(state), Some(counter)) = (self.ahead_state, self.ahead_counter) {
            let left = self.b.use_var(counter);
            self.b.ins().store(flags(), left, state, 0);
        }
    }
    pub(crate) fn ahead_reload(&mut self) {
        if let (Some(state), Some(counter)) = (self.ahead_state, self.ahead_counter) {
            let left = self.b.ins().load(types::I64, flags(), state, 0);
            self.b.def_var(counter, left);
        }
    }
    pub(crate) fn ahead_barrier(&mut self) -> Emitted {
        if let Some(state) = self.ahead_state {
            self.ahead_flush();
            let reason = self.b.ins().iconst(types::I32, 4);
            self.ahead_helper(true, &[state, reason])?;
            self.ahead_reload();
        }
        Ok(())
    }
    pub(crate) fn ahead_point(&mut self, logical: bool) -> Emitted {
        let (Some(state), Some(counter)) = (self.ahead_state, self.ahead_counter) else {
            return Ok(());
        };
        let left = self.b.use_var(counter);
        let left = self.b.ins().iadd_imm(left, -1);
        self.b.def_var(counter, left);
        let expired = self.b.ins().icmp_imm(IntCC::SignedLessThanOrEqual, left, 0);
        let mut reason = self.b.ins().uextend(types::I32, expired);
        if logical && self.em.inputs.unit().fuel {
            let offset = hirsel_lir::runtime_abi::CTX_FUEL_OFFSET as i32;
            let fuel = self.b.ins().load(types::I64, flags(), self.context, offset);
            let fuel = self.b.ins().iadd_imm(fuel, -1);
            self.b.ins().store(flags(), fuel, self.context, offset);
            let spent = self.b.ins().icmp_imm(IntCC::SignedLessThan, fuel, 0);
            let spent = self.b.ins().uextend(types::I32, spent);
            let spent = self.b.ins().ishl_imm(spent, 1);
            reason = self.b.ins().bor(reason, spent);
        }
        let spent = self.b.ins().icmp_imm(IntCC::NotEqual, reason, 0);
        let (cold, next) = (self.b.create_block(), self.b.create_block());
        self.b.set_cold_block(cold);
        self.b.ins().brif(spent, cold, &[], next, &[]);
        self.b.switch_to_block(cold);
        self.ahead_flush();
        self.ahead_helper(true, &[state, reason])?;
        self.ahead_reload();
        self.b.ins().jump(next, &[]);
        self.b.switch_to_block(next);
        Ok(())
    }
}

/// Immutable metadata is emitted only after the exact alternate object has
/// supplied every frame. A refused root exports the same record with null entry.
pub(crate) fn metadata(
    inputs: &hirsel_lir::emission::EmitInputs<'_>,
    plan: &hirsel_lir::ahead::AheadPlan,
    bounds: &std::collections::BTreeMap<hirsel_syntax::ContentHash, u64>,
    registry: &hirsel_lir::descriptor_registry::DescriptorRegistry,
) -> Emitted<hirsel_lir::stage::ObjectArtifact> {
    use crate::module::{Emitter, Inputs};
    use cranelift_module::{DataDescription, Linkage, Module};
    use hirsel_lir::emission::{artifact, symbol_name};
    use hirsel_lir::stage::{ObjectSymbol, ObjectSymbolKind};
    let mut em = Emitter::new(Inputs::units(inputs), false)?;
    for (id, root) in &plan.roots {
        let name = hirsel_lir::ahead::metadata_name(*id, plan.identity);
        let mut info = vec![0u8; 32];
        info[..4].copy_from_slice(&2u32.to_le_bytes());
        let mut description = DataDescription::new();
        if let Some(&bound) = bounds.get(id) {
            let recipe = &root.recipe;
            let recipe_name = format!("{name}_recipe");
            let mut bytes = Vec::with_capacity(40);
            for word in [
                registry.index(recipe.wrapper),
                if recipe.source_extent == 0 {
                    Some(2)
                } else {
                    recipe.source.and_then(|l| registry.index(l))
                },
                Some(recipe.wrapper_extent),
                Some(recipe.callable_offset),
                Some(recipe.live_word_offset),
                Some(recipe.source_extent),
            ] {
                let word = word.ok_or_else(|| crate::CodegenError::Object {
                    message: "missing recipe descriptor".into(),
                })?;
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            bytes.extend_from_slice(&recipe.live_mask.to_le_bytes());
            bytes.extend_from_slice(&[0u8; 8]);
            let mut image = DataDescription::new();
            image.define(bytes.into_boxed_slice());
            image.set_align(8);
            let source = if recipe.source_environment {
                symbol_name(
                    &recipe.source_code.definition,
                    recipe.source_code.instance.as_ref(),
                )
            } else {
                hirsel_lir::emission::function_thunk_name(
                    &recipe.source_code.definition,
                    recipe.source_code.instance.as_ref(),
                )
            };
            let abi = &inputs.layouts.fn_abis[recipe.source_abi.0 as usize];
            let sig = em.entry_signature(abi, &recipe.source_code.definition)?;
            let code = em.import_function(&source, &sig.ir)?;
            em.manifest_import(
                &source,
                ObjectSymbolKind::Function,
                Some(&recipe.source_code.definition),
                Some(abi),
            );
            let f = em.module.declare_func_in_data(code, &mut image);
            image.write_function_addr(32, f);
            let (recipe_id, _) = em.declare_data(&recipe_name, Linkage::Local)?;
            em.define_data(&recipe_name, recipe_id, &image)?;
            info[16..24].copy_from_slice(&bound.to_le_bytes());
            description.define(info.into_boxed_slice());
            description.set_align(8);
            let symbol = &plan.symbols[id];
            let alternate = hirsel_lir::ahead::alternate_name(&symbol.definition, *id);
            // Only an address relocation: the real definition keeps its original ABI.
            let sig = ir::Signature::new(em.call_conv);
            let code = em.import_function(&alternate, &sig)?;
            em.manifest_import(
                &alternate,
                ObjectSymbolKind::Function,
                Some(&symbol.definition),
                None,
            );
            let f = em.module.declare_func_in_data(code, &mut description);
            description.write_function_addr(8, f);
            let data = em.module.declare_data_in_data(recipe_id, &mut description);
            description.write_data_addr(24, data, 0);
        } else {
            description.define(info.into_boxed_slice());
            description.set_align(8);
        }
        let (data, _) = em.declare_data(&name, Linkage::Export)?;
        em.define_data(&name, data, &description)?;
        em.manifested.exports.insert(
            name.clone(),
            ObjectSymbol {
                name,
                kind: ObjectSymbolKind::Data,
                definition: None,
                abi: None,
            },
        );
    }
    em.manifested.settle_imports();
    let manifested = em.manifested;
    let bytes = crate::object::finish(em.module.finish())?;
    let file = hirsel_lir::object::ObjectFile::read(inputs.target, bytes)
        .map_err(|message| crate::CodegenError::Object { message })?;
    artifact(file, manifested, inputs, crate::TOOLCHAIN)
        .map_err(|problem| crate::CodegenError::Manifest { problem })
}
