//! Separate physical checkpoints. The SSA counter is flushed only at ABI boundaries.
use crate::function::{FnEmitter, flags};
use crate::module::Emitted;
use cranelift_codegen::ir::{self, InstBuilder, Value, condcodes::IntCC, types};
use hirsel_lir::layout_types::LayoutKind;

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
        let state = match self.sig.ahead {
            Some(index) => self.entry_params[index],
            None => self
                .ahead_helper(false, &[self.context])?
                .expect("getter result"),
        };
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
        self.ahead_barrier_reason(4)
    }
    pub(crate) fn ahead_failure_barrier(&mut self) -> Emitted {
        self.ahead_barrier_reason(hirsel_lir::runtime_abi::AHEAD_FAILURE_BARRIER)
    }
    fn ahead_barrier_reason(&mut self, reason: u32) -> Emitted {
        if let Some(state) = self.ahead_state {
            self.ahead_flush();
            let reason = self.b.ins().iconst(types::I32, i64::from(reason));
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
    let mut read_ids = std::collections::BTreeMap::new();
    for host in plan.roots.values().filter_map(|r| r.host.as_ref()) {
        for (&(unit, call), recipe) in &host.reads {
            if read_ids.contains_key(&(unit, call)) {
                continue;
            }
            let name = hirsel_lir::ahead::read_metadata_name(unit, call, plan.identity);
            let mut bytes = Vec::with_capacity(16);
            for word in [
                registry
                    .index(recipe.descriptor)
                    .ok_or_else(|| crate::CodegenError::Object {
                        message: "missing selected byte descriptor".into(),
                    })?,
                recipe.maximum,
                host.spines[&unit].deferred_releases.len().max(1) as u32,
                0,
            ] {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            let mut image = DataDescription::new();
            image.define(bytes.into_boxed_slice());
            image.set_align(8);
            let (data, _) = em.declare_data(&name, Linkage::Export)?;
            em.define_data(&name, data, &image)?;
            em.manifested.exports.insert(
                name.clone(),
                ObjectSymbol {
                    name,
                    kind: ObjectSymbolKind::Data,
                    definition: None,
                    abi: None,
                },
            );
            read_ids.insert((unit, call), data);
        }
    }
    for (id, root) in &plan.roots {
        let name = hirsel_lir::ahead::metadata_name(*id, plan.identity);
        // A certified host-return root whose body plans Resumable enters its
        // resumable ahead code (v4); a save continues in its ordinary dispatcher.
        let resumable = root
            .host
            .as_ref()
            .filter(|_| bounds.contains_key(id))
            .and(inputs.continuation)
            .and_then(|plans| plans.get(*id))
            .filter(|p| p.representation == hirsel_lir::continuation::Representation::Resumable);
        let (version, size) = match (&root.host, resumable) {
            _ if root.cpu.is_some() => (6u32, 56),
            (_, Some(_)) => (4u32, 56),
            (Some(_), None) => (3u32, 40),
            (None, None) => (2u32, 32),
        };
        let mut info = vec![0u8; size];
        info[..4].copy_from_slice(&version.to_le_bytes());
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
            if let Some(cpu) = &root.cpu {
                let wrapper = inputs.layouts.layout(recipe.wrapper).unwrap();
                for (offset, layout) in [
                    (32, wrapper.fields[0].layout),
                    (36, cpu.shared_payload),
                    (40, wrapper.fields[1].layout),
                ] {
                    let descriptor =
                        registry
                            .index(layout)
                            .ok_or_else(|| crate::CodegenError::Object {
                                message: "missing CPU capture descriptor".into(),
                            })?;
                    info[offset..offset + 4].copy_from_slice(&descriptor.to_le_bytes());
                }
                let offset = u32::try_from(wrapper.fields[1].offset).map_err(|_| {
                    crate::CodegenError::Object {
                        message: "CPU jobs offset overflow".into(),
                    }
                })?;
                info[44..48].copy_from_slice(&offset.to_le_bytes());
                for (start, field) in [(48, 3), (52, 4)] {
                    let offset = u32::try_from(wrapper.fields[field].offset).map_err(|_| {
                        crate::CodegenError::Object {
                            message: "CPU claim capture offset overflow".into(),
                        }
                    })?;
                    info[start..start + 4].copy_from_slice(&offset.to_le_bytes());
                }
            }
            if let Some(resumed) = resumable {
                let bytes = u32::try_from(resumed.cost.frame_bytes).map_err(|_| {
                    crate::CodegenError::Object {
                        message: "a resumable ahead frame beyond u32".into(),
                    }
                })?;
                info[40..44].copy_from_slice(&bytes.to_le_bytes());
            }
            description.define(info.into_boxed_slice());
            description.set_align(8);
            let symbol = &plan.symbols[id];
            let alternate = if resumable.is_some() {
                hirsel_lir::ahead::alternate_resume_name(&symbol.definition, *id)
            } else {
                hirsel_lir::ahead::alternate_name(&symbol.definition, *id)
            };
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
            if let Some(host) = &root.host {
                let mut list = DataDescription::new();
                list.define(vec![0u8; host.reads.len() * 8].into_boxed_slice());
                list.set_align(8);
                for (i, key) in host.reads.keys().enumerate() {
                    let read = em.module.declare_data_in_data(read_ids[key], &mut list);
                    list.write_data_addr(i as u32 * 8, read, 0);
                }
                let list_name = format!("{name}_reads");
                let (list_id, _) = em.declare_data(&list_name, Linkage::Local)?;
                em.define_data(&list_name, list_id, &list)?;
                let mut bytes = vec![0u8; 56];
                bytes[..4].copy_from_slice(&1u32.to_le_bytes());
                bytes[8..16].copy_from_slice(&(host.reads.len() as u64).to_le_bytes());
                bytes[24..56].copy_from_slice(&root.graph.identity.0);
                let mut image = DataDescription::new();
                image.define(bytes.into_boxed_slice());
                image.set_align(8);
                let list_data = em.module.declare_data_in_data(list_id, &mut image);
                image.write_data_addr(16, list_data, 0);
                let root_name = format!("{name}_host");
                let (root_id, _) = em.declare_data(&root_name, Linkage::Local)?;
                em.define_data(&root_name, root_id, &image)?;
                let data = em.module.declare_data_in_data(root_id, &mut description);
                description.write_data_addr(32, data, 0);
            }
            if resumable.is_some() {
                let dispatcher =
                    hirsel_lir::continuation::resume_name(&symbol.definition, *id, None);
                let sig = ir::Signature::new(em.call_conv);
                let code = em.import_function(&dispatcher, &sig)?;
                em.manifest_import(
                    &dispatcher,
                    ObjectSymbolKind::Function,
                    Some(&symbol.definition),
                    None,
                );
                let f = em.module.declare_func_in_data(code, &mut description);
                description.write_function_addr(48, f);
            }
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

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn host_read_boundary(
        &mut self,
        call: hirsel_lir::ir::CallId,
        enter: bool,
    ) -> Emitted {
        use hirsel_lir::{ahead, runtime_abi::RuntimeEntry, stage::ObjectSymbolKind};
        let Some(state) = self.ahead_state else {
            return Ok(());
        };
        if self
            .em
            .inputs
            .unit()
            .ahead
            .is_some_and(|plan| plan.cpu_claim(self.unit.header.instance, call))
        {
            self.rt(
                if enter {
                    RuntimeEntry::CpuAheadEnter
                } else {
                    RuntimeEntry::CpuAheadArm
                },
                &[if enter { state } else { self.context }],
            )?;
            if enter {
                self.ahead_reload();
            }
            return Ok(());
        }
        let Some((buffer, name)) = self.em.inputs.unit().ahead.and_then(|p| {
            p.host_read(self.unit.header.instance, call).map(|r| {
                (
                    r.buffer,
                    ahead::read_metadata_name(self.unit.header.instance, call, p.identity),
                )
            })
        }) else {
            return Ok(());
        };
        self.em
            .manifest_import(&name, ObjectSymbolKind::Data, None, None);
        let (id, _) = self
            .em
            .declare_data(&name, cranelift_module::Linkage::Import)?;
        let recipe = self.data_addr(id, 0);
        if enter {
            self.rt(RuntimeEntry::HostEnter, &[state, recipe])?;
            self.ahead_reload();
        } else {
            let Some(place) = self.slots[buffer.index()] else {
                return self.invalid("missing selected buffer slot");
            };
            self.rt(RuntimeEntry::HostArm, &[self.context, recipe, place.ptr])?;
        }
        Ok(())
    }
    pub(crate) fn host_instruction(&mut self, inst: &hirsel_lir::ir::Inst) -> Emitted<bool> {
        if self.cpu_release()? {
            return Ok(true);
        }
        use hirsel_lir::{ir::Inst, runtime_abi::RuntimeEntry};
        let Some(state) = self.ahead_state else {
            return Ok(false);
        };
        let Some(spine) = self
            .em
            .inputs
            .unit()
            .ahead
            .and_then(|p| p.host_spine(self.unit.header.instance))
        else {
            return Ok(false);
        };
        let i = self.instruction_index;
        if spine.barriers.contains(&i) {
            self.ahead_barrier()?;
        }
        let Some(&slot) = spine.deferred_releases.get(&i) else {
            return Ok(false);
        };
        let Inst::Release { value, .. } = *inst else {
            return self.invalid("deferred cleanup is not a release");
        };
        let active = self.rt1(RuntimeEntry::HostActive, &[state])?;
        let active = self.b.ins().icmp_imm(IntCC::NotEqual, active, 0);
        let held = self.b.create_block();
        let ordinary = self.b.create_block();
        let next = self.b.create_block();
        self.b.ins().brif(active, held, &[], ordinary, &[]);
        self.b.switch_to_block(held);
        let block = self.get1(value)?;
        let fuel = self.b.ins().load(
            types::I64,
            flags(),
            self.context,
            hirsel_lir::runtime_abi::CTX_FUEL_OFFSET as i32,
        );
        let slot = self.b.ins().iconst(types::I32, i64::from(slot));
        self.rt(RuntimeEntry::HostDefer, &[state, slot, block, fuel])?;
        self.b.ins().jump(next, &[]);
        self.b.switch_to_block(ordinary);
        self.ahead_barrier()?;
        self.instruction_body(inst)?;
        self.b.ins().jump(next, &[]);
        self.b.switch_to_block(next);
        Ok(true)
    }
}

impl FnEmitter<'_, '_, '_> {
    /// Borrow the live mapper result into a saved ABI image on success only.
    pub(crate) fn cpu_publish(&mut self, call: hirsel_lir::ir::CallId) -> Emitted {
        let Some(summary) = self
            .em
            .inputs
            .unit()
            .ahead
            .and_then(|plan| plan.cpu_summary(self.unit.header.instance, call))
            .cloned()
        else {
            return Ok(());
        };
        let (entry, state) = match self.ahead_state {
            Some(state) => (
                hirsel_lir::runtime_abi::RuntimeEntry::CpuAheadPublish,
                state,
            ),
            None => (
                hirsel_lir::runtime_abi::RuntimeEntry::CpuPublish,
                self.context,
            ),
        };
        let image = self.stack(summary.layout)?;
        let shape = self.value_shape(summary.value)?;
        let value = self.get(summary.value)?;
        self.store(shape, value, image)?;
        let Some(ordinal) = self.slots[summary.ordinal.index()] else {
            return self.invalid("CPU summary without its ordinal capture");
        };
        let ordinal = self.load_ty(types::I64, ordinal.ptr, 0);
        let (record, _) = self.stack_bytes(48, 8);
        let version = self.b.ins().iconst(types::I32, 1);
        let pending = self.b.ins().iconst(types::I32, 0);
        let bytes = self.u64c(summary.bytes);
        let descriptor = self.descriptor_value(summary.layout)?;
        let count = self
            .b
            .ins()
            .iconst(types::I32, i64::from(summary.retained_count));
        let retained = if summary.retained_count == 0 {
            self.u64c(0)
        } else {
            let (table, _) = self.stack_bytes(u64::from(summary.retained_count) * 8, 8);
            let zero = self.u64c(0);
            for i in 0..summary.retained_count {
                self.store_at(zero, table, u64::from(i) * 8);
            }
            self.cpu_retained(image.ptr, &summary.owners, table, &mut 0)?;
            table
        };
        for (offset, value) in [
            (0, version),
            (4, pending),
            (8, ordinal),
            (16, image.ptr),
            (24, bytes),
            (32, descriptor),
            (36, count),
            (40, retained),
        ] {
            self.store_at(value, record, offset);
        }
        self.ahead_flush();
        let status = self.rt1(entry, &[state, record])?;
        self.ahead_reload();
        let refused = self.b.ins().icmp_imm(IntCC::NotEqual, status, 0);
        self.b.ins().trapnz(refused, crate::function::TRAP);
        Ok(())
    }
}

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn cpu_reserve(&mut self) -> Emitted<bool> {
        if !self.em.inputs.unit().alternate {
            return Ok(false);
        }
        let Some((recipe, ordinal_offset)) = self
            .em
            .inputs
            .unit()
            .ahead
            .and_then(|p| {
                p.cpu_growth(
                    self.unit.header.instance,
                    self.instruction_index,
                    self.em.inputs.unit().layouts,
                )
            })
            .map(|(recipe, offset)| (recipe.clone(), offset))
        else {
            return Ok(false);
        };
        let Some(state) = self.ahead_state else {
            return self.invalid("CPU growth without its physical state");
        };
        use hirsel_lir::ahead::CpuGrowthPlace;
        let place = match recipe.place {
            CpuGrowthPlace::Address(address) => Some(self.place(address)?),
            CpuGrowthPlace::Copy { .. } => None,
        };
        let old = match recipe.place {
            CpuGrowthPlace::Address(_) => self.load_ptr(place.unwrap().ptr),
            CpuGrowthPlace::Copy { value, .. } => self.get1(value)?,
        };
        if matches!(recipe.place, CpuGrowthPlace::Address(_))
            && recipe.capacity.is_none()
            && !self.tracking()
        {
            return Ok(true);
        }
        let done =
            if matches!(recipe.place, CpuGrowthPlace::Address(_)) && recipe.capacity.is_none() {
                let slow = self.b.create_block();
                let done = self.b.create_block();
                let tracked = self.is_tracked(old);
                self.b.set_cold_block(slow);
                self.b.ins().brif(tracked, slow, &[], done, &[]);
                self.b.switch_to_block(slow);
                Some(done)
            } else {
                None
            };
        let environment = self.load_ty(
            types::I64,
            state,
            hirsel_lir::runtime_abi::AHEAD_WORKER_ENVIRONMENT_OFFSET,
        );
        let ordinal = self.load_ty(types::I64, environment, ordinal_offset);
        let (record, _) = self.stack_bytes(88, 8);
        let version = self.b.ins().iconst(types::I32, 1);
        let zero32 = self.b.ins().iconst(types::I32, 0);
        let zero = self.u64c(0);
        let LayoutKind::Block { payload } = self.layout(recipe.layout)?.kind else {
            return self.invalid("CPU sequence handoff without a block payload");
        };
        let descriptor = self.descriptor_value(payload)?;
        let size = self.u64c(recipe.size);
        let align = self.u64c(u64::from(recipe.align));
        let kind = self.b.ins().iconst(
            types::I32,
            match recipe.place {
                CpuGrowthPlace::Copy { .. } => 5,
                CpuGrowthPlace::Address(_) if recipe.capacity.is_some() => 1,
                CpuGrowthPlace::Address(_) => 4,
            },
        );
        let capacity = match recipe.capacity {
            Some(capacity) => self.get1(capacity)?,
            None => zero,
        };
        for (offset, value) in [
            (0, version),
            (4, kind),
            (8, ordinal),
            (16, old),
            (24, zero),
            (32, descriptor),
            (36, zero32),
            (40, zero),
            (48, zero),
            (56, zero),
            (64, size),
            (72, align),
            (80, capacity),
        ] {
            self.store_at(value, record, offset);
        }
        self.ahead_flush();
        let status = self.rt1(
            hirsel_lir::runtime_abi::RuntimeEntry::CpuAheadGrow,
            &[state, record],
        )?;
        self.ahead_reload();
        let refused = self.b.ins().icmp_imm_s(IntCC::NotEqual, status, 0);
        self.b.ins().trapnz(refused, crate::function::TRAP);
        let grown = self.load_ty(types::I64, record, 16);
        match recipe.place {
            CpuGrowthPlace::Copy { result, .. } => {
                self.define1(result, grown)?;
            }
            CpuGrowthPlace::Address(address) => {
                self.store_at(grown, place.unwrap().ptr, 0);
                if recipe.capacity.is_some() {
                    if let Some(var) = self
                        .whole_slot(address)
                        .and_then(|slot| self.root_facts[slot.index()])
                    {
                        let yes = self.b.ins().iconst(types::I8, 1);
                        self.b.def_var(var, yes);
                    }
                }
            }
        }
        if let Some(done) = done {
            self.b.ins().jump(done, &[]);
            self.b.switch_to_block(done);
        }
        Ok(true)
    }
}

impl FnEmitter<'_, '_, '_> {
    fn cpu_retained(
        &mut self,
        base: Value,
        scan: &[hirsel_lir::layout_types::ScanStep],
        table: Value,
        cursor: &mut u64,
    ) -> Emitted {
        use hirsel_lir::layout_types::ScanStep;
        for step in scan {
            match step {
                ScanStep::Counted { offset } | ScanStep::SliceOwner { offset } => {
                    let root = self.load_ty(types::I64, base, *offset);
                    self.store_at(root, table, *cursor * 8);
                    *cursor += 1;
                }
                ScanStep::Inline { offset, layout } => {
                    let child = self.offset(base, *offset);
                    let scan = self.layout(*layout)?.scan.clone();
                    self.cpu_retained(child, &scan, table, cursor)?;
                }
                ScanStep::Repeat {
                    offset,
                    count,
                    stride,
                    element,
                } => {
                    let scan = self.layout(*element)?.scan.clone();
                    for i in 0..*count {
                        let child = self.offset(base, offset + i * stride);
                        self.cpu_retained(child, &scan, table, cursor)?;
                    }
                }
                ScanStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let active = self.load_tag(base, *tag_offset, *form);
                    let active = self.b.ins().uextend(types::I64, active);
                    let done = self.b.create_block();
                    let mut switch = cranelift_frontend::Switch::new();
                    let mut arms = Vec::new();
                    for (variant, steps) in variants {
                        let arm = self.b.create_block();
                        switch.set_entry(u128::from(*variant), arm);
                        arms.push((arm, steps));
                    }
                    switch.emit(&mut self.b, active, done);
                    for (arm, steps) in arms {
                        self.b.switch_to_block(arm);
                        self.cpu_retained(base, steps, table, cursor)?;
                        self.b.ins().jump(done, &[]);
                    }
                    self.b.switch_to_block(done);
                }
                _ => return self.invalid("uncertified CPU retained root"),
            }
        }
        Ok(())
    }
}

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn cpu_region(
        &mut self,
        region: Value,
        layout: hirsel_lir::layout_types::LayoutId,
        extent: Value,
        align: u32,
    ) -> Emitted<Option<Value>> {
        if !self.em.inputs.unit().alternate {
            return Ok(None);
        }
        let Some(plan) = self.em.inputs.unit().ahead else {
            return Ok(None);
        };
        let Some(recipe) = plan.cpu_regional(self.unit.header.instance, self.instruction_index)
        else {
            return Ok(None);
        };
        if recipe.layout != layout || recipe.align != align {
            return self.invalid("CPU regional layout mismatch");
        }
        let Some(offset) =
            plan.cpu_ordinal(self.unit.header.instance, self.em.inputs.unit().layouts)
        else {
            return self.invalid("ambiguous CPU regional ordinal");
        };
        let Some(state) = self.ahead_state else {
            return self.invalid("CPU region without physical state");
        };
        let environment = self.load_ty(
            types::I64,
            state,
            hirsel_lir::runtime_abi::AHEAD_WORKER_ENVIRONMENT_OFFSET,
        );
        let ordinal = self.load_ty(types::I64, environment, offset);
        let (record, _) = self.stack_bytes(88, 8);
        let version = self.b.ins().iconst(types::I32, 1);
        let kind = self.b.ins().iconst(types::I32, 3);
        let zero32 = self.b.ins().iconst(types::I32, 0);
        let zero = self.u64c(0);
        let descriptor = self.descriptor_value(layout)?;
        let align = self.u64c(u64::from(align));
        for (offset, value) in [
            (0, version),
            (4, kind),
            (8, ordinal),
            (16, zero),
            (24, region),
            (32, descriptor),
            (36, zero32),
            (40, extent),
            (48, align),
            (56, zero),
            (64, zero),
            (72, zero),
            (80, zero),
        ] {
            self.store_at(value, record, offset);
        }
        self.ahead_flush();
        let status = self.rt1(
            hirsel_lir::runtime_abi::RuntimeEntry::CpuAheadGrow,
            &[state, record],
        )?;
        self.ahead_reload();
        let refused = self.b.ins().icmp_imm_s(IntCC::NotEqual, status, 0);
        self.b.ins().trapnz(refused, crate::function::TRAP);
        Ok(Some(self.load_ty(types::I64, record, 16)))
    }
}

impl FnEmitter<'_, '_, '_> {
    fn cpu_release(&mut self) -> Emitted<bool> {
        if self.ownership.is_some() || self.em.inputs.unit().fuel {
            return Ok(false);
        }
        let Some(state) = self.ahead_state else {
            return Ok(false);
        };
        let Some((value, payload)) = self.em.inputs.unit().ahead.and_then(|plan| {
            plan.cpu_release(
                self.unit,
                self.instruction_index,
                self.em.inputs.unit().layouts,
            )
        }) else {
            return Ok(false);
        };
        let Some(offset) =
            self.em.inputs.unit().ahead.and_then(|p| {
                p.cpu_ordinal(self.unit.header.instance, self.em.inputs.unit().layouts)
            })
        else {
            return self.invalid("CPU release without a job ordinal");
        };
        let environment = self.load_ty(
            types::I64,
            state,
            hirsel_lir::runtime_abi::AHEAD_WORKER_ENVIRONMENT_OFFSET,
        );
        let ordinal = self.load_ty(types::I64, environment, offset);
        let (record, _) = self.stack_bytes(88, 8);
        let version = self.b.ins().iconst(types::I32, 1);
        let kind = self.b.ins().iconst(types::I32, 6);
        let zero32 = self.b.ins().iconst(types::I32, 0);
        let zero = self.u64c(0);
        let pointer = self.get1(value)?;
        let descriptor = self.descriptor_value(payload)?;
        for (offset, value) in [
            (0, version),
            (4, kind),
            (8, ordinal),
            (16, pointer),
            (24, zero),
            (32, descriptor),
            (36, zero32),
            (40, zero),
            (48, zero),
            (56, zero),
            (64, zero),
            (72, zero),
            (80, zero),
        ] {
            self.store_at(value, record, offset);
        }
        self.ahead_flush();
        let status = self.rt1(
            hirsel_lir::runtime_abi::RuntimeEntry::CpuAheadGrow,
            &[state, record],
        )?;
        self.ahead_reload();
        let refused = self.b.ins().icmp_imm_s(IntCC::NotEqual, status, 0);
        self.b.ins().trapnz(refused, crate::function::TRAP);
        Ok(true)
    }
}
