//! The object's data: runtime layout descriptors and their step programs, the descriptor
//! registry, static immortal blocks (statics, text literals, the empty environment), static
//! live masks and finalizer drop wrappers, laid out as `hirsel-llvm` lays them out.

use cranelift_codegen::ir::{self, AbiParam, InstBuilder, types};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use hirsel_lir::emission::{IMMORTAL_PREFIX, IMMORTAL_PREFIX_BYTES, PrefixWord, symbol_name};
use hirsel_lir::glue::descriptor_name;
use hirsel_lir::layout::align_up;
use hirsel_lir::layout_types::{Endian, FinalizerRef, LayoutId};
use hirsel_lir::rt_program;
use hirsel_lir::runtime_abi::{COUNT_IMMORTAL, LAYOUT_DESCRIPTOR_VERSION, LAYOUT_MAGIC};
use hirsel_lir::stage::ObjectSymbolKind;
use hirsel_syntax::DefPath;

use crate::CodegenError;
use crate::module::{CheckThunk, Emitted, Emitter, Helper, PTR};

/// Bytes of `hirsel_rt_v2_layout`.
const DESCRIPTOR_BYTES: usize = 88;
/// Bytes of `hirsel_rt_v2_step`.
const STEP_BYTES: usize = 56;

/// A static block: its data object and the offset of its header in it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StaticBlock {
    pub(crate) data: DataId,
    pub(crate) header: u64,
}

fn put_u32(bytes: &mut [u8], at: usize, n: u32) {
    bytes[at..at + 4].copy_from_slice(&n.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], at: usize, n: u64) {
    bytes[at..at + 8].copy_from_slice(&n.to_le_bytes());
}

/// A descriptor's bytes, its finalizer and steps pointers left zero for relocations.
#[allow(clippy::too_many_arguments)]
fn descriptor_bytes(
    size: u64,
    align: u64,
    step_count: u64,
    field_count: u32,
    flags: u32,
    logical: u64,
) -> Vec<u8> {
    let mut bytes = vec![0u8; DESCRIPTOR_BYTES];
    put_u32(&mut bytes, 0, LAYOUT_MAGIC);
    put_u32(&mut bytes, 4, LAYOUT_DESCRIPTOR_VERSION);
    put_u64(&mut bytes, 8, size);
    put_u64(&mut bytes, 16, align);
    put_u64(&mut bytes, 40, step_count);
    put_u32(&mut bytes, 48, field_count);
    put_u32(&mut bytes, 52, flags);
    put_u64(&mut bytes, 56, logical);
    bytes
}

impl Emitter<'_> {
    fn invalid_at<T>(definition: &DefPath, what: impl Into<String>) -> Emitted<T> {
        Err(CodegenError::Invalid {
            definition: definition.clone(),
            what: what.into(),
        })
    }

    /// The registry index of `layout`'s descriptor, which the glue object then defines.
    pub(crate) fn descriptor_index(
        &mut self,
        layout: LayoutId,
        definition: &DefPath,
    ) -> Emitted<u32> {
        self.descriptor(layout, definition)?;
        match self.inputs.glue().registry.index(layout) {
            Some(index) => Ok(index),
            None => Self::invalid_at(
                definition,
                "a layout absent from the linked descriptor registry",
            ),
        }
    }

    /// The empty descriptor the registry starts with.
    fn empty_descriptor(&mut self) -> Emitted<DataId> {
        let name = format!(
            "hirsel.layout.empty.{}",
            self.inputs.glue().registry.symbol()
        );
        let (id, new) = self.declare_data(&name, Linkage::Preemptible)?;
        if new {
            self.manifested.descriptors.insert(name.clone());
            let mut description = DataDescription::new();
            description.define(descriptor_bytes(0, 1, 0, 0, 0, 0).into_boxed_slice());
            description.set_align(8);
            self.define_data(&name, id, &description)?;
        }
        Ok(id)
    }

    /// The descriptor registry this object shares with the program's other objects, and
    /// its binding `hirsel.program.registry`.
    pub(crate) fn registry_table(&mut self, definition: &DefPath) -> Emitted {
        let name = self.inputs.glue().registry.symbol();
        if self.data.contains_key(&name) {
            return Ok(());
        }
        let mut entries = vec![self.empty_descriptor()?];
        for id in self.inputs.glue().registry.layouts.clone() {
            entries.push(self.descriptor(id, definition)?);
        }
        let (table, _) = self.declare_data(&name, Linkage::Preemptible)?;
        self.manifested.descriptors.insert(name.clone());
        let mut description = DataDescription::new();
        description.define(vec![0u8; 8 * entries.len()].into_boxed_slice());
        description.set_align(8);
        for (i, entry) in entries.iter().enumerate() {
            let gv = self.module.declare_data_in_data(*entry, &mut description);
            description.write_data_addr((8 * i) as u32, gv, 0);
        }
        self.define_data(&name, table, &description)?;
        let binding_name = "hirsel.program.registry";
        let (binding, _) = self.declare_data(binding_name, Linkage::Preemptible)?;
        self.manifested.descriptors.insert(binding_name.into());
        let mut bytes = vec![0u8; 16];
        put_u64(&mut bytes, 8, entries.len() as u64);
        let mut description = DataDescription::new();
        description.define(bytes.into_boxed_slice());
        description.set_align(8);
        let gv = self.module.declare_data_in_data(table, &mut description);
        description.write_data_addr(0, gv, 0);
        self.define_data(binding_name, binding, &description)
    }

    /// The `hirsel_rt_v2_layout` of `layout`, defined once per object with every descriptor
    /// its steps name. A layout with a user finalizer names its drop wrapper.
    pub(crate) fn descriptor(&mut self, layout: LayoutId, definition: &DefPath) -> Emitted<DataId> {
        if self.inputs.layouts.layout(layout).is_none() {
            return Self::invalid_at(definition, format!("no layout {}", layout.0));
        }
        let name = descriptor_name(&self.inputs.keys.key(layout));
        if let Some(&id) = self.data.get(&name) {
            return Ok(id);
        }
        let descriptors = match rt_program::descriptors_with_storage(
            self.inputs.layouts,
            &[layout],
            &self.inputs.glue().registry.map_storage,
        ) {
            Ok(descriptors) => descriptors,
            Err(error) => {
                return Self::invalid_at(
                    definition,
                    format!("no runtime descriptor for layout {}: {error:?}", layout.0),
                );
            }
        };
        // Declare every new descriptor first: a step names another.
        let mut new = Vec::new();
        for d in &descriptors {
            if self.inputs.layouts.layout(d.layout).is_none() {
                return Self::invalid_at(definition, format!("no layout {}", d.layout.0));
            }
            let name = descriptor_name(&self.inputs.keys.key(d.layout));
            let (id, fresh) = self.declare_data(&name, Linkage::Preemptible)?;
            if fresh {
                self.manifested.descriptors.insert(name.clone());
                new.push((d, id, name));
            }
        }
        for (d, id, name) in new {
            let mut steps_id = None;
            if !d.steps.is_empty() {
                let steps_name = name.replace("hirsel.layout.", "hirsel.steps.");
                let (steps, _) = self.declare_data(&steps_name, Linkage::Preemptible)?;
                self.manifested.descriptors.insert(steps_name.clone());
                let mut bytes = vec![0u8; STEP_BYTES * d.steps.len()];
                let mut targets = Vec::new();
                for (i, step) in d.steps.iter().enumerate() {
                    let at = STEP_BYTES * i;
                    put_u32(&mut bytes, at, step.kind.code());
                    put_u32(&mut bytes, at + 4, step.field);
                    put_u32(&mut bytes, at + 8, step.variant);
                    put_u32(&mut bytes, at + 12, step.span);
                    put_u64(&mut bytes, at + 16, step.offset);
                    put_u64(&mut bytes, at + 24, step.stride);
                    put_u64(&mut bytes, at + 32, step.count);
                    put_u64(&mut bytes, at + 40, step.count_offset);
                    if let Some(target) = step.layout {
                        if self.inputs.layouts.layout(target).is_none() {
                            return Self::invalid_at(definition, "a step naming no layout");
                        }
                        match self
                            .data
                            .get(&descriptor_name(&self.inputs.keys.key(target)))
                        {
                            Some(&target) => targets.push((at + 48, target)),
                            None => {
                                return Self::invalid_at(
                                    definition,
                                    "a step naming a layout without a descriptor",
                                );
                            }
                        }
                    }
                }
                let mut description = DataDescription::new();
                description.define(bytes.into_boxed_slice());
                description.set_align(8);
                for (at, target) in targets {
                    let gv = self.module.declare_data_in_data(target, &mut description);
                    description.write_data_addr(at as u32, gv, 0);
                }
                self.define_data(&steps_name, steps, &description)?;
                steps_id = Some(steps);
            }
            let finalizer = match &d.finalizer {
                Some(finalizer) => Some(self.drop_wrapper(d.layout, finalizer, definition)?),
                None => None,
            };
            let logical_count = if let Some(program) = d.logical_count {
                let logical_name = name.replace("hirsel.layout.", "hirsel.logical-count.");
                let (id, fresh) = self.declare_data(&logical_name, Linkage::Preemptible)?;
                if fresh {
                    self.manifested.descriptors.insert(logical_name.clone());
                    let mut description = DataDescription::new();
                    description.define(
                        program
                            .into_iter()
                            .flat_map(u64::to_le_bytes)
                            .collect::<Vec<_>>()
                            .into_boxed_slice(),
                    );
                    description.set_align(8);
                    self.define_data(&logical_name, id, &description)?;
                }
                Some(id)
            } else {
                None
            };
            let scan = d.scan.linked(|id| self.inputs.glue().registry.index(id));
            let scan_words = scan.as_ref().map_or(u64::MAX, |w| w.len() as u64);
            let scan_id = match scan {
                Some(words) if !words.is_empty() => {
                    let scan_name = name.replace("hirsel.layout.", "hirsel.scan.");
                    let (id, fresh) = self.declare_data(&scan_name, Linkage::Preemptible)?;
                    if fresh {
                        self.manifested.descriptors.insert(scan_name.clone());
                        let mut description = DataDescription::new();
                        description.define(
                            words
                                .into_iter()
                                .flat_map(u64::to_le_bytes)
                                .collect::<Vec<_>>()
                                .into_boxed_slice(),
                        );
                        description.set_align(8);
                        self.define_data(&scan_name, id, &description)?;
                    }
                    Some(id)
                }
                _ => None,
            };
            let mut bytes = descriptor_bytes(
                d.size,
                d.align,
                d.steps.len() as u64,
                d.field_count,
                d.flags,
                d.logical,
            );
            put_u64(&mut bytes, 80, scan_words);
            let mut description = DataDescription::new();
            description.define(bytes.into_boxed_slice());
            description.set_align(8);
            if let Some(scan) = scan_id {
                let gv = self.module.declare_data_in_data(scan, &mut description);
                description.write_data_addr(72, gv, 0);
            }
            if let Some(count) = logical_count {
                let gv = self.module.declare_data_in_data(count, &mut description);
                description.write_data_addr(64, gv, 0);
            }
            if let Some(finalizer) = finalizer {
                let f = self
                    .module
                    .declare_func_in_data(finalizer, &mut description);
                description.write_function_addr(24, f);
            }
            if let Some(steps) = steps_id {
                let gv = self.module.declare_data_in_data(steps, &mut description);
                description.write_data_addr(32, gv, 0);
            }
            self.define_data(&name, id, &description)?;
        }
        match self.data.get(&name) {
            Some(&id) => Ok(id),
            None => Self::invalid_at(definition, "a descriptor that was not emitted"),
        }
    }

    /// The drop thunk of `layout`: a private `hirsel_rt_v2_drop_thunk` that calls the drop
    /// body's unit with the self address and returns its status.
    fn drop_wrapper(
        &mut self,
        layout: LayoutId,
        finalizer: &FinalizerRef,
        definition: &DefPath,
    ) -> Emitted<FuncId> {
        let Some(instance) = self.inputs.glue().finalizers.get(&layout) else {
            return Self::invalid_at(
                definition,
                format!(
                    "layout {} has a finalizer but no drop body instance",
                    layout.0
                ),
            );
        };
        let body_name = symbol_name(&finalizer.definition, Some(instance));
        let caller_site = finalizer.definition.0.package == DefPath::core("").0.package;
        let mut body_sig = ir::Signature::new(self.call_conv);
        body_sig.params.push(AbiParam::new(PTR));
        if caller_site {
            body_sig
                .params
                .extend([AbiParam::new(types::I64), AbiParam::new(types::I64)]);
        }
        body_sig.params.push(AbiParam::new(PTR));
        body_sig.returns.push(AbiParam::new(types::I8).uext());
        let body = match self.import_function(&body_name, &body_sig) {
            Ok(body) => body,
            Err(_) => {
                return Self::invalid_at(
                    definition,
                    format!("the drop body `{body_name}` does not take (context, self address)"),
                );
            }
        };
        self.manifested.import(
            &body_name,
            ObjectSymbolKind::Function,
            Some(&finalizer.definition),
        );
        let mut sig = ir::Signature::new(self.call_conv);
        sig.params.extend([
            AbiParam::new(PTR),
            AbiParam::new(PTR),
            AbiParam::new(types::I64),
            AbiParam::new(types::I64),
        ]);
        sig.returns.push(AbiParam::new(types::I8).uext());
        let name = format!("hirsel.drop.{}", self.inputs.keys.key(layout).hex());
        self.helper(&name, &sig, |_| Helper::Drop { body, caller_site })
    }

    /// A static immortal block named `name` (exported from glue, else private): the
    /// immortal prefix with
    /// `COUNT_IMMORTAL` and the static domain over `descriptor`, then `bytes` at the payload
    /// offset of `align`, with the pointer-sized cell at each offset of `references` holding
    /// that block's header address.
    pub(crate) fn static_block(
        &mut self,
        name: &str,
        descriptor: u32,
        align: u32,
        bytes: &[u8],
        references: &[(u64, StaticBlock)],
        exported: bool,
        definition: &DefPath,
    ) -> Emitted<StaticBlock> {
        self.static_block_functions(
            name,
            descriptor,
            align,
            bytes,
            references,
            &[],
            exported,
            definition,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn static_block_functions(
        &mut self,
        name: &str,
        descriptor: u32,
        align: u32,
        bytes: &[u8],
        references: &[(u64, StaticBlock)],
        functions: &[hirsel_lir::statics::StaticFunction],
        exported: bool,
        definition: &DefPath,
    ) -> Emitted<StaticBlock> {
        let physical = align.max(8);
        let start = match align_up(IMMORTAL_PREFIX_BYTES, u64::from(physical)) {
            Ok(n) => n,
            Err(_) => return Self::invalid_at(definition, "static extent overflow"),
        };
        let header = start - 8;
        if let Some(&data) = self.data.get(name) {
            return Ok(StaticBlock { data, header });
        }
        let mut image = vec![0u8; (start - IMMORTAL_PREFIX_BYTES) as usize];
        for field in IMMORTAL_PREFIX {
            match field {
                PrefixWord::Capacity => image.extend((bytes.len() as u64).to_le_bytes()),
                PrefixWord::Align => image.extend(u64::from(physical).to_le_bytes()),
                PrefixWord::Count => image.extend((COUNT_IMMORTAL as u32).to_le_bytes()),
                PrefixWord::Descriptor => image.extend((descriptor | (2u32 << 29)).to_le_bytes()),
            }
        }
        if image.len() as u64 != start {
            return Self::invalid_at(definition, "an immortal prefix of another size");
        }
        image.extend_from_slice(bytes);
        let word = u64::from(self.inputs.target.pointer_bytes);
        for (offset, _) in references {
            if offset + word > bytes.len() as u64 {
                return Self::invalid_at(definition, "a static reference outside its block");
            }
        }
        let linkage = if exported {
            Linkage::Export
        } else {
            Linkage::Local
        };
        let (id, _) = self.declare_data(name, linkage)?;
        let mut description = DataDescription::new();
        description.define(image.into_boxed_slice());
        description.set_align(u64::from(physical));
        for (offset, target) in references {
            let gv = self
                .module
                .declare_data_in_data(target.data, &mut description);
            description.write_data_addr((start + offset) as u32, gv, target.header as i64);
        }
        if !functions.is_empty() {
            let environment = self.empty_environment(definition)?;
            for function in functions {
                if function.offset + 2 * word > bytes.len() as u64 {
                    return Self::invalid_at(definition, "a static callable outside its block");
                }
                let symbol = &function.symbol;
                let thunk = hirsel_lir::emission::function_thunk_name(
                    &symbol.definition,
                    symbol.instance.as_ref(),
                );
                self.manifested.import(
                    &thunk,
                    hirsel_lir::stage::ObjectSymbolKind::Function,
                    Some(&symbol.definition),
                );
                let signature = self.module.make_signature();
                let target = self.import_function(&thunk, &signature)?;
                let code = self.module.declare_func_in_data(target, &mut description);
                description.write_function_addr((start + function.offset) as u32, code);
                let env = self
                    .module
                    .declare_data_in_data(environment.data, &mut description);
                description.write_data_addr(
                    (start + function.offset + word) as u32,
                    env,
                    environment.header as i64,
                );
            }
        }
        self.define_data(name, id, &description)?;
        Ok(StaticBlock { data: id, header })
    }

    /// The environment of a function reference: one static immortal block with an empty
    /// payload and the empty descriptor (index 2, which the runtime fixes, not the program).
    pub(crate) fn empty_environment(&mut self, definition: &DefPath) -> Emitted<StaticBlock> {
        self.static_block("hirsel.env.empty", 2, 1, &[], &[], false, definition)
    }

    /// The header of glue's static immortal block `name` (`hirsel_lir::glue`), whose
    /// payload has `align`, imported.
    pub(crate) fn glue_block(
        &mut self,
        name: &str,
        align: u32,
        definition: &DefPath,
    ) -> Emitted<StaticBlock> {
        let Ok(start) = align_up(IMMORTAL_PREFIX_BYTES, u64::from(align.max(8))) else {
            return Self::invalid_at(definition, "static extent overflow");
        };
        Ok(StaticBlock {
            data: self.glue_data(name)?,
            header: start - 8,
        })
    }

    /// The glue data symbol `name`, imported and recorded in the manifest.
    pub(crate) fn glue_data(&mut self, name: &str) -> Emitted<DataId> {
        self.manifested.import(name, ObjectSymbolKind::Data, None);
        Ok(self.declare_data(name, Linkage::Import)?.0)
    }

    /// The static block of a String or Bytes literal: glue's `literal_symbol` block of the
    /// counted `Sequence` payload `payload` (its u64 element count, its u64 capacity, then
    /// the bytes), whose prefix holds the program's descriptor index.
    pub(crate) fn literal_block(
        &mut self,
        payload: LayoutId,
        bytes: &[u8],
        definition: &DefPath,
    ) -> Emitted<StaticBlock> {
        if self.inputs.target.endian != Endian::Little {
            return Err(CodegenError::Unsupported {
                definition: definition.clone(),
                what: "a static literal on a big-endian target".into(),
            });
        }
        let data = match hirsel_lir::glue::literal_data(self.inputs.layouts, payload, bytes) {
            Ok(data) => data,
            Err(what) => return Self::invalid_at(definition, what),
        };
        let Some(record) = self.inputs.layouts.layout(payload) else {
            return Self::invalid_at(definition, "no literal payload layout");
        };
        let name = hirsel_lir::glue::literal_symbol(&self.inputs.keys.key(payload), &data);
        self.glue_block(&name, record.align, definition)
    }

    /// A private constant of a static live mask's words.
    pub(crate) fn mask_words(&mut self, words: &[u64]) -> Emitted<DataId> {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.constant("hirsel.mask", bytes, 8)
    }

    /// `hirsel.drop.<layout>(ctx, self, anchor) -> i8`: the drop body's status.
    pub(crate) fn define_drop_wrapper(
        &mut self,
        mut b: FunctionBuilder<'_>,
        body: FuncId,
        caller_site: bool,
    ) {
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let params = b.block_params(entry).to_vec();
        let callee = self.module.declare_func_in_func(body, b.func);
        let args = if caller_site {
            vec![params[0], params[2], params[3], params[1]]
        } else {
            params[..2].to_vec()
        };
        let call = b.ins().call(callee, &args);
        let status = b.inst_results(call)[0];
        b.ins().return_(&[status]);
        b.seal_all_blocks();
        b.finalize(self.module.target_config());
    }

    pub(crate) fn define_drop_resume_wrapper(
        &mut self,
        mut b: FunctionBuilder<'_>,
        body: FuncId,
        caller_site: bool,
    ) {
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let params = b.block_params(entry).to_vec();
        let callee = self.module.declare_func_in_func(body, b.func);
        let mut args = vec![params[0]];
        if caller_site {
            args.extend([params[2], params[3]]);
        }
        args.extend([params[1], params[4]]);
        let call = b.ins().call(callee, &args);
        let status = b.inst_results(call)[0];
        b.ins().return_(&[status]);
        b.seal_all_blocks();
        b.finalize(self.module.target_config());
    }

    /// `hirsel.check.<symbol>(ctx, value, verdict) -> i8`: the check's status, with its bool
    /// verdict stored as a byte at `verdict`.
    pub(crate) fn define_check_wrapper(&mut self, mut b: FunctionBuilder<'_>, thunk: CheckThunk) {
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let params = b.block_params(entry).to_vec();
        let (ctx, value, verdict) = (params[0], params[1], params[2]);
        let callee = self.module.declare_func_in_func(thunk.body, b.func);
        let flags = ir::MemFlagsData::new();
        let mut args = vec![ctx];
        let result = (!thunk.register).then(|| {
            let slot = b.create_sized_stack_slot(ir::StackSlotData::new(
                ir::StackSlotKind::ExplicitSlot,
                1,
                0,
            ));
            let address = b.ins().stack_addr(PTR, slot, 0);
            args.push(address);
            address
        });
        match &thunk.slots {
            None => args.push(value),
            Some(slots) => {
                for &(offset, ty) in slots {
                    args.push(b.ins().load(ty, flags, value, offset as i32));
                }
            }
        }
        let call = b.ins().call(callee, &args);
        let results = b.inst_results(call).to_vec();
        let byte = match result {
            Some(address) => b.ins().load(types::I8, flags, address, 0),
            // A Bool is one byte, as in memory: any nonzero byte is true.
            None => {
                let low = b.ins().ireduce(
                    types::I8,
                    results[hirsel_lir::abi_types::REGISTER_VALUE_INDEX],
                );
                b.ins()
                    .icmp_imm_u(cranelift_codegen::ir::condcodes::IntCC::NotEqual, low, 0)
            }
        };
        b.ins().store(flags, byte, verdict, 0);
        b.ins().return_(&[results[usize::from(thunk.register)]]);
        b.seal_all_blocks();
        b.finalize(self.module.target_config());
    }
}
