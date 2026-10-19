//! The object's data: runtime layout descriptors and their step programs, the descriptor
//! registry, static immortal blocks (statics, text literals, the empty environment), static
//! live masks and finalizer drop wrappers, laid out as `hirsel-llvm` lays them out.

use cranelift_codegen::ir::{self, AbiParam, InstBuilder, types};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use hirsel_lir::emission::{
    IMMORTAL_PREFIX, IMMORTAL_PREFIX_BYTES, PrefixWord, descriptor_name, symbol_name,
};
use hirsel_lir::layout::{SEQUENCE_CAPACITY_OFFSET, align_up, sequence_elements_offset};
use hirsel_lir::layout_types::{Endian, FinalizerRef, LayoutId, LayoutKind};
use hirsel_lir::rt_program;
use hirsel_lir::runtime_abi::{COUNT_IMMORTAL, LAYOUT_DESCRIPTOR_VERSION, LAYOUT_MAGIC};
use hirsel_lir::stage::ObjectSymbolKind;
use hirsel_syntax::{ContentHash, DefPath};

use crate::CodegenError;
use crate::module::{Emitted, Emitter, Helper, PTR};

/// Bytes of `hirsel_rt_v2_layout`.
const DESCRIPTOR_BYTES: usize = 72;
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

    /// The registry index of `layout`'s descriptor, which the object then defines.
    pub(crate) fn descriptor_index(
        &mut self,
        layout: LayoutId,
        definition: &DefPath,
    ) -> Emitted<u32> {
        self.descriptor(layout, definition)?;
        match self.inputs.registry.index(layout) {
            Some(index) => Ok(index),
            None => Self::invalid_at(
                definition,
                "a layout absent from the linked descriptor registry",
            ),
        }
    }

    /// The empty descriptor the registry starts with.
    fn empty_descriptor(&mut self) -> Emitted<DataId> {
        let name = format!("hirsel.layout.empty.{}", self.inputs.registry.symbol());
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
        let name = self.inputs.registry.symbol();
        if self.data.contains_key(&name) {
            return Ok(());
        }
        let mut entries = vec![self.empty_descriptor()?];
        for id in self.inputs.registry.layouts.clone() {
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
        let record = match self.inputs.layouts.layout(layout) {
            Some(record) => record,
            None => return Self::invalid_at(definition, format!("no layout {}", layout.0)),
        };
        let name = descriptor_name(record);
        if let Some(&id) = self.data.get(&name) {
            return Ok(id);
        }
        let descriptors = match rt_program::descriptors_with_storage(
            self.inputs.layouts,
            &[layout],
            &self.inputs.registry.map_storage,
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
            let Some(record) = self.inputs.layouts.layout(d.layout) else {
                return Self::invalid_at(definition, format!("no layout {}", d.layout.0));
            };
            let name = descriptor_name(record);
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
                        let Some(record) = self.inputs.layouts.layout(target) else {
                            return Self::invalid_at(definition, "a step naming no layout");
                        };
                        match self.data.get(&descriptor_name(record)) {
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
            let bytes = descriptor_bytes(
                d.size,
                d.align,
                d.steps.len() as u64,
                d.field_count,
                d.flags,
                d.logical,
            );
            let mut description = DataDescription::new();
            description.define(bytes.into_boxed_slice());
            description.set_align(8);
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
        let Some(instance) = self.inputs.finalizers.get(&layout) else {
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
        let name = format!("hirsel.drop.{}", layout.0);
        self.helper(&name, &sig, |_| Helper::Drop { body, caller_site })
    }

    /// A private static immortal block named `name`: the immortal prefix with
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
        let (id, _) = self.declare_data(name, Linkage::Local)?;
        let mut description = DataDescription::new();
        description.define(image.into_boxed_slice());
        description.set_align(u64::from(physical));
        for (offset, target) in references {
            let gv = self
                .module
                .declare_data_in_data(target.data, &mut description);
            description.write_data_addr((start + offset) as u32, gv, target.header as i64);
        }
        self.define_data(name, id, &description)?;
        Ok(StaticBlock { data: id, header })
    }

    /// The environment of a function reference: one static immortal block with an empty
    /// payload and the empty descriptor.
    pub(crate) fn empty_environment(&mut self, definition: &DefPath) -> Emitted<StaticBlock> {
        self.empty_descriptor()?;
        self.static_block("hirsel.env.empty", 2, 1, &[], &[], definition)
    }

    /// The static block of a String or Bytes literal: a counted `Sequence` payload of
    /// `payload` (its u64 element count, its u64 capacity, then the bytes).
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
        let Some(record) = self.inputs.layouts.layout(payload) else {
            return Self::invalid_at(definition, "no literal payload layout");
        };
        let LayoutKind::Sequence { element } = record.kind else {
            return Self::invalid_at(definition, "a text literal whose storage is not a sequence");
        };
        let Some(element_record) = self.inputs.layouts.layout(element) else {
            return Self::invalid_at(definition, "no literal element layout");
        };
        if element_record.size.storage() != Some(1) {
            return Self::invalid_at(definition, "a text literal of elements wider than a byte");
        }
        let start = match sequence_elements_offset(element_record) {
            Ok(start) => start as usize,
            Err(_) => return Self::invalid_at(definition, "a sequence element offset beyond u64"),
        };
        let mut data = vec![0u8; start];
        let count = (bytes.len() as u64).to_le_bytes();
        data[..8].copy_from_slice(&count);
        let capacity = SEQUENCE_CAPACITY_OFFSET as usize;
        data[capacity..capacity + 8].copy_from_slice(&count);
        data.extend_from_slice(bytes);
        let descriptor = self.descriptor_index(payload, definition)?;
        let align = record.align;
        let name = format!(
            "hirsel.literal.{}",
            ContentHash::of_parts("hirsel.native-literal.v1", [data.as_slice()])
                .0
                .iter()
                .take(8)
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        self.static_block(&name, descriptor, align, &data, &[], definition)
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
}
