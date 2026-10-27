//! The program's glue object (`hirsel_lir::glue`): every number the program assigns, as
//! content-named symbols the unit objects read, plus the runtime descriptors and the
//! registry that give those numbers meaning. It is regenerated at every link, so a unit
//! object never depends on the program it is linked into.
use super::*;
use hirsel_lir::codec::CodecPlan;
use hirsel_lir::emission::{GlueCallback, GlueInputs};
use hirsel_lir::glue::{
    GlueCodec, codec_symbol, descriptor_index_symbol, descriptor_name, descriptor_word_symbol,
    failure_code_symbol, literal_symbol, static_symbol,
};
use hirsel_lir::statics::StaticData;

/// The LLVM types of `hirsel_rt_v2_layout` and `hirsel_rt_v2_step`.
fn descriptor_types<'m>(module: &'m Module<'m>) -> (Type<'m>, Type<'m>) {
    let (ptr, i32_ty, i64_ty) = (module.ptr_type(), module.int_type(32), module.int_type(64));
    (
        module.struct_type(&[
            i32_ty, i32_ty, i64_ty, i64_ty, ptr, ptr, i64_ty, i32_ty, i32_ty, i64_ty, ptr, ptr,
            i64_ty,
        ]),
        module.struct_type(&[
            i32_ty, i32_ty, i32_ty, i32_ty, i64_ty, i64_ty, i64_ty, i64_ty, ptr,
        ]),
    )
}

/// The byte offset of a static immortal block's header from its symbol, for a payload of
/// `align`: the immortal prefix ends at the payload, the header is its last 8 bytes.
pub(super) fn static_header_offset(align: u32) -> Option<u64> {
    align_up(IMMORTAL_PREFIX_BYTES, u64::from(align.max(8)))
        .ok()
        .map(|start| start - 8)
}

/// A static immortal block named `name` (see `FnEmitter::static_block`): a counted header
/// with `COUNT_IMMORTAL`, the static domain and the descriptor index `descriptor`, then
/// `bytes` at the payload offset of `align`, with the pointer-sized cell at each offset of
/// `references` holding that block reference (a relocation; its bytes are zero). Returns
/// the global and its header's address.
pub(super) fn static_block<'m>(
    module: &'m Module<'m>,
    target: &TargetSpec,
    name: &str,
    descriptor: Value<'m>,
    align: u32,
    bytes: &[u8],
    references: &[(u64, Value<'m>)],
    exported: bool,
) -> Result<(Value<'m>, Value<'m>), String> {
    let header_offset = static_header_offset(align).ok_or("static extent overflow")?;
    if let Some(global) = module.named_global(name) {
        return Ok((global, module.const_offset(global, header_offset)));
    }
    let (ty, initializer) = static_image(module, target, descriptor, align, bytes, references)?;
    let physical = align.max(8);
    let global = if exported {
        module.exported_constant(name, ty, physical)
    } else {
        module.private_constant(name, ty, physical)
    };
    module.set_initializer(global, initializer);
    Ok((global, module.const_offset(global, header_offset)))
}

/// The type and initializer of a static immortal block (`static_block`).
pub(super) fn static_image<'m>(
    module: &'m Module<'m>,
    target: &TargetSpec,
    descriptor: Value<'m>,
    align: u32,
    bytes: &[u8],
    references: &[(u64, Value<'m>)],
) -> Result<(Type<'m>, Value<'m>), String> {
    let physical = align.max(8);
    let header_offset = static_header_offset(align).ok_or("static extent overflow")?;
    let start = header_offset + 8;
    let (i8_ty, i32_ty) = (module.int_type(8), module.int_type(32));
    // Static extents have exactly their initialized bytes. Padding them like heap classes
    // would give [] spare capacity and send its first push through a detach instead of
    // directly through sequence growth.
    let capacity = bytes.len();
    let word = module.const_or(descriptor, module.const_int(i32_ty, 2u128 << 29));
    let prefix = IMMORTAL_PREFIX.map(|field| match field {
        PrefixWord::Capacity => module.const_int(module.int_type(64), capacity as u128),
        PrefixWord::Align => module.const_int(module.int_type(64), u128::from(physical)),
        PrefixWord::Count => module.const_int(i32_ty, u128::from(COUNT_IMMORTAL)),
        PrefixWord::Descriptor => word,
    });
    let prefix_ty =
        module.struct_type(&IMMORTAL_PREFIX.map(|field| module.int_type(8 * field.bytes() as u32)));
    let padding = vec![0u8; (start - IMMORTAL_PREFIX_BYTES) as usize];
    let (payload_ty, payload) = static_payload(module, target, bytes, references)?;
    let ty = module.packed_struct_type(&[
        module.array_type(i8_ty, padding.len() as u64),
        prefix_ty,
        payload_ty,
    ]);
    let initializer = module.const_packed_struct(&[
        module.const_bytes(&padding),
        module.const_struct(&prefix),
        payload,
    ]);
    Ok((ty, initializer))
}

/// A static block's payload: `bytes`, with the pointer-sized cell at each offset of
/// `references` holding that block reference (a relocation; its bytes are zero).
pub(super) fn static_payload<'m>(
    module: &'m Module<'m>,
    target: &TargetSpec,
    bytes: &[u8],
    references: &[(u64, Value<'m>)],
) -> Result<(Type<'m>, Value<'m>), String> {
    let (ptr, i8_ty) = (module.ptr_type(), module.int_type(8));
    if references.is_empty() {
        return Ok((
            module.array_type(i8_ty, bytes.len() as u64),
            module.const_bytes(bytes),
        ));
    }
    // Byte runs between the reference cells, packed so each cell lies at its own offset.
    let word = u64::from(target.pointer_bytes);
    let mut sorted = references.to_vec();
    sorted.sort_by_key(|(offset, _)| *offset);
    let (mut types, mut parts) = (Vec::new(), Vec::new());
    let mut at = 0u64;
    for (offset, target) in sorted {
        if offset < at || offset + word > bytes.len() as u64 {
            return Err("a static reference outside its block".into());
        }
        let run = &bytes[at as usize..offset as usize];
        types.push(module.array_type(i8_ty, run.len() as u64));
        parts.push(module.const_bytes(run));
        types.push(ptr);
        parts.push(target);
        at = offset + word;
    }
    let run = &bytes[at as usize..];
    types.push(module.array_type(i8_ty, run.len() as u64));
    parts.push(module.const_bytes(run));
    Ok((
        module.packed_struct_type(&types),
        module.const_packed_struct(&parts),
    ))
}

/// Emits the glue object.
struct ProgramEmitter<'m, 'a> {
    module: &'m Module<'m>,
    inputs: &'a GlueInputs<'a>,
    manifested: Manifested,
}

type Glued<T = ()> = Result<T, CodegenError>;

fn invalid<T>(what: impl Into<String>) -> Glued<T> {
    Err(CodegenError::Glue { what: what.into() })
}

impl<'m, 'a> ProgramEmitter<'m, 'a> {
    fn layout(&self, id: LayoutId) -> Glued<&'a LayoutRecord> {
        match self.inputs.layouts.layout(id) {
            Some(record) => Ok(record),
            None => invalid(format!("no layout {}", id.0)),
        }
    }

    fn const_u64(&self, n: u64) -> Value<'m> {
        self.module
            .const_int(self.module.int_type(64), u128::from(n))
    }

    fn const_u32(&self, n: u32) -> Value<'m> {
        self.module
            .const_int(self.module.int_type(32), u128::from(n))
    }

    /// Declares the data export `name`.
    fn export(&mut self, name: &str) {
        self.manifested.exports.insert(
            name.to_owned(),
            ObjectSymbol {
                name: name.to_owned(),
                kind: ObjectSymbolKind::Data,
                definition: None,
                abi: None,
            },
        );
    }

    /// The registry index of `layout`.
    fn index(&self, layout: LayoutId) -> Glued<u32> {
        match self.inputs.registry.index(layout) {
            Some(index) => Ok(index),
            None => invalid(format!(
                "layout {} is absent from the linked descriptor registry",
                layout.0
            )),
        }
    }

    /// A descriptor's initializer.
    #[allow(clippy::too_many_arguments)]
    fn descriptor_initializer(
        &self,
        size: u64,
        align: u64,
        finalizer: Value<'m>,
        steps: Value<'m>,
        step_count: u64,
        field_count: u32,
        flags: u32,
        logical: u64,
        logical_count: Value<'m>,
        scan: Value<'m>,
        scan_words: u64,
    ) -> Value<'m> {
        self.module.const_struct(&[
            self.const_u32(LAYOUT_MAGIC),
            self.const_u32(LAYOUT_DESCRIPTOR_VERSION),
            self.const_u64(size),
            self.const_u64(align),
            finalizer,
            steps,
            self.const_u64(step_count),
            self.const_u32(field_count),
            self.const_u32(flags),
            self.const_u64(logical),
            logical_count,
            scan,
            self.const_u64(scan_words),
        ])
    }

    /// The empty descriptor the registry starts with (index 2, the empty environment's).
    fn empty_descriptor(&mut self) -> Value<'m> {
        let name = format!("hirsel.layout.empty.{}", self.inputs.registry.symbol());
        if let Some(global) = self.module.named_global(&name) {
            return global;
        }
        let (ty, _) = descriptor_types(self.module);
        self.manifested.descriptors.insert(name.clone());
        let global = self.module.linkonce_constant(&name, ty, 8);
        let null = self.module.const_zero(self.module.ptr_type());
        let initializer = self.descriptor_initializer(0, 1, null, null, 0, 0, 0, 0, null, null, 0);
        self.module.set_initializer(global, initializer);
        global
    }

    /// The registry table and its binding `hirsel.program.registry`.
    fn registry_table(&mut self) -> Glued {
        let name = self.inputs.registry.symbol();
        let ids = self.inputs.registry.layouts.clone();
        let mut values = vec![self.empty_descriptor()];
        for id in ids {
            values.push(self.descriptor_pointer(id)?);
        }
        let ptr = self.module.ptr_type();
        let global = self
            .module
            .registry_constant(&name, self.module.array_type(ptr, values.len() as u64));
        self.manifested.descriptors.insert(name);
        self.module
            .set_initializer(global, self.module.const_array(ptr, &values));
        let binding_name = "hirsel.program.registry";
        let binding = self.module.registry_constant(
            binding_name,
            self.module.struct_type(&[ptr, self.module.int_type(64)]),
        );
        self.manifested.descriptors.insert(binding_name.into());
        self.module.set_initializer(
            binding,
            self.module
                .const_struct(&[global, self.const_u64(values.len() as u64)]),
        );
        Ok(())
    }

    /// Every registry layout's index cell `hirsel.desc.<key>.index`.
    fn index_cells(&mut self) -> Glued {
        let i32_ty = self.module.int_type(32);
        for id in self.inputs.registry.layouts.clone() {
            let name = descriptor_index_symbol(&self.inputs.keys.key(id));
            if self.module.named_global(&name).is_some() {
                return invalid(format!("two registry layouts share the cell `{name}`"));
            }
            let cell = self.module.exported_constant(&name, i32_ty, 4);
            let index = self.index(id)?;
            self.module.set_initializer(cell, self.const_u32(index));
            self.export(&name);
        }
        Ok(())
    }

    /// Every rule's failure-code cell `hirsel.fail.<rule>.code`.
    fn failure_cells(&mut self) -> Glued {
        let word = self.module.int_type(64);
        for definition in &self.inputs.glue.foreign {
            let name = hirsel_lir::glue::foreign_dispatch_symbol(definition);
            let cell = self.module.exported_constant(&name, word, 8);
            let value = self.inputs.foreign.get(definition).copied().unwrap_or(0);
            self.module.set_initializer(cell, self.const_u64(value));
            self.export(&name);
        }
        let i32_ty = self.module.int_type(32);
        for (rule, code) in &self.inputs.failures.codes {
            let name = failure_code_symbol(rule);
            let cell = self.module.exported_constant(&name, i32_ty, 4);
            self.module.set_initializer(cell, self.const_u32(code.rule));
            self.export(&name);
        }
        Ok(())
    }

    /// The `hirsel_rt_v2_layout` (version 2) of `layout`, emitted once with every
    /// descriptor its steps name, as `hirsel_lir::rt_program` lowers them. A layout with a
    /// user finalizer names its drop wrapper.
    fn descriptor_pointer(&mut self, layout: LayoutId) -> Glued<Value<'m>> {
        let keys = self.inputs.keys;
        self.layout(layout)?;
        if let Some(global) = self
            .module
            .named_global(&descriptor_name(&keys.key(layout)))
        {
            return Ok(global);
        }
        let descriptors = match rt_program::descriptors_with_storage(
            self.inputs.layouts,
            &[layout],
            &self.inputs.registry.map_storage,
        ) {
            Ok(descriptors) => descriptors,
            Err(error) => {
                return invalid(format!(
                    "no runtime descriptor for layout {}: {error:?}",
                    layout.0
                ));
            }
        };
        let ptr = self.module.ptr_type();
        let (layout_ty, step_ty) = descriptor_types(self.module);
        // Declare every new descriptor first: a Finalize step names another.
        let mut new = Vec::new();
        for d in &descriptors {
            let name = descriptor_name(&keys.key(d.layout));
            if self.module.named_global(&name).is_none() {
                self.manifested.descriptors.insert(name.clone());
                new.push((
                    d,
                    name.clone(),
                    self.module.linkonce_constant(&name, layout_ty, 8),
                ));
            }
        }
        let i32_ty = self.module.int_type(32);
        let i64_ty = self.module.int_type(64);
        for (d, name, global) in new {
            let mut steps = Vec::new();
            for step in &d.steps {
                let target = match step.layout {
                    Some(l) => match self.module.named_global(&descriptor_name(&keys.key(l))) {
                        Some(target) => target,
                        None => return invalid("a step naming a layout without a descriptor"),
                    },
                    None => self.module.const_zero(ptr),
                };
                debug_assert!(step.kind != StepKind::Finalize || step.layout.is_some());
                let u32_ = |n: u32| self.module.const_int(i32_ty, u128::from(n));
                let u64_ = |n: u64| self.module.const_int(i64_ty, u128::from(n));
                steps.push(self.module.const_struct(&[
                    u32_(step.kind.code()),
                    u32_(step.field),
                    u32_(step.variant),
                    u32_(step.span),
                    u64_(step.offset),
                    u64_(step.stride),
                    u64_(step.count),
                    u64_(step.count_offset),
                    target,
                ]));
            }
            let steps_ptr = if steps.is_empty() {
                self.module.const_zero(ptr)
            } else {
                let ty = self.module.array_type(step_ty, steps.len() as u64);
                let name = name.replace("hirsel.layout.", "hirsel.steps.");
                self.manifested.descriptors.insert(name.clone());
                let array = self.module.linkonce_constant(&name, ty, 8);
                self.module
                    .set_initializer(array, self.module.const_array(step_ty, &steps));
                array
            };
            let finalizer = match &d.finalizer {
                Some(finalizer) => self.drop_wrapper(d.layout, finalizer)?,
                None => self.module.const_zero(ptr),
            };
            let logical_count = if let Some(program) = d.logical_count {
                let word = self.module.int_type(64);
                let ty = self.module.array_type(word, 5);
                let name = name.replace("hirsel.layout.", "hirsel.logical-count.");
                self.manifested.descriptors.insert(name.clone());
                let global = self.module.linkonce_constant(&name, ty, 8);
                let words: Vec<_> = program.iter().map(|&n| self.const_u64(n)).collect();
                self.module
                    .set_initializer(global, self.module.const_array(word, &words));
                global
            } else {
                self.module.const_zero(ptr)
            };
            let linked_scan = d.scan.linked(|id| self.inputs.registry.index(id));
            let scan_words = linked_scan.as_ref().map_or(u64::MAX, |w| w.len() as u64);
            let scan = match linked_scan {
                Some(words) if !words.is_empty() => {
                    let word = self.module.int_type(64);
                    let ty = self.module.array_type(word, words.len() as u64);
                    let scan_name = name.replace("hirsel.layout.", "hirsel.scan.");
                    self.manifested.descriptors.insert(scan_name.clone());
                    let array = self.module.linkonce_constant(&scan_name, ty, 8);
                    let values: Vec<_> = words.iter().map(|&n| self.const_u64(n)).collect();
                    self.module
                        .set_initializer(array, self.module.const_array(word, &values));
                    array
                }
                _ => self.module.const_zero(ptr),
            };
            let initializer = self.descriptor_initializer(
                d.size,
                d.align,
                finalizer,
                steps_ptr,
                d.steps.len() as u64,
                d.field_count,
                d.flags,
                d.logical,
                logical_count,
                scan,
                scan_words,
            );
            self.module.set_initializer(global, initializer);
        }
        match self
            .module
            .named_global(&descriptor_name(&keys.key(layout)))
        {
            Some(global) => Ok(global),
            None => invalid("a descriptor that was not emitted"),
        }
    }

    /// The drop thunk of `layout`, whose finalizer is `finalizer`: a private wrapper with the
    /// runtime's `hirsel_rt_v2_drop_thunk` type that calls the drop body's unit, named
    /// `symbol_name` of its definition instance, with the self address (Borrow for source
    /// bodies, Mut for library bodies), and returns its status. The unit records its own
    /// failure; the cleanup anchor is not its to report.
    fn drop_wrapper(&mut self, layout: LayoutId, finalizer: &FinalizerRef) -> Glued<Value<'m>> {
        let Some(instance) = self.inputs.finalizers.get(&layout) else {
            return invalid(format!(
                "layout {} has a finalizer but no drop body instance",
                layout.0
            ));
        };
        let ptr = self.module.ptr_type();
        let i8_ty = self.module.int_type(8);
        let body_name = symbol_name(&finalizer.definition, Some(instance));
        let caller_site = finalizer.definition.0.package == DefPath::core("").0.package;
        let i64_ty = self.module.int_type(64);
        let body_slots = if caller_site {
            vec![ptr, i64_ty, i64_ty, ptr]
        } else {
            vec![ptr, ptr]
        };
        let body_ty = self.module.function_type(i8_ty, &body_slots);
        let body = match self.module.function(&body_name, body_ty) {
            Ok(body) => body,
            Err(_) => {
                return invalid(format!(
                    "the drop body `{body_name}` does not take (context, self address)"
                ));
            }
        };
        self.manifested
            .imports
            .entry(body_name.clone())
            .or_insert_with(|| ObjectSymbol {
                name: body_name.clone(),
                kind: ObjectSymbolKind::Function,
                definition: Some(finalizer.definition.clone()),
                abi: None,
            });
        let ty = self
            .module
            .function_type(i8_ty, &[ptr, ptr, i64_ty, i64_ty]);
        let name = format!("hirsel.drop.{}", self.inputs.keys.key(layout).hex());
        let wrapper = match self.module.function(&name, ty) {
            Ok(wrapper) => wrapper,
            Err(what) => return invalid(what),
        };
        self.module.make_private(wrapper);
        let builder = Builder::new(self.module);
        builder.position(self.module.append_block(wrapper));
        let mut arguments = vec![self.module.parameter(wrapper, 0)];
        if caller_site {
            arguments.extend([
                self.module.parameter(wrapper, 2),
                self.module.parameter(wrapper, 3),
            ]);
        }
        arguments.push(self.module.parameter(wrapper, 1));
        let status = builder.call(body, &arguments);
        builder.ret(status);
        Ok(wrapper.value)
    }

    /// The `hirsel_rt_v2_check_thunk` of a codec check (SER-1): `(ctx, value, verdict) ->
    /// status` calling the check on the borrowed value at `value`, of `layout`, and storing
    /// its bool verdict as a byte.
    fn check_wrapper(
        &mut self,
        check: &hirsel_lir::glue::GlueCheck,
        layout: LayoutId,
    ) -> Glued<Value<'m>> {
        let definition = check.definition.clone();
        let abi = fn_abi(self.inputs.layouts, check.abi, &definition)?;
        let name = symbol_name(&definition, check.instance.as_ref());
        self.manifested
            .imports
            .entry(name.clone())
            .or_insert_with(|| ObjectSymbol {
                name: name.clone(),
                kind: ObjectSymbolKind::Function,
                definition: Some(definition.clone()),
                abi: Some(abi_digest(abi)),
            });
        let wrapper_name = format!("hirsel.check.{name}");
        let (ptr, i8_ty) = (self.module.ptr_type(), self.module.int_type(8));
        let ty = self.module.function_type(i8_ty, &[ptr, ptr, ptr]);
        let wrapper = match self.module.function(&wrapper_name, ty) {
            Ok(wrapper) => wrapper,
            Err(what) => return invalid(what),
        };
        if self.module.defines(&wrapper_name) {
            return Ok(wrapper.value);
        }
        self.module.make_private(wrapper);
        let signature = Signature::of(self.module, self.inputs.layouts, abi, &definition)?;
        let callee = signature.declare(
            self.module,
            self.inputs.layouts,
            &name,
            &definition,
            false,
            Aliasing::of(self.inputs.aliasing_unstated),
        )?;
        let builder = Builder::new(self.module);
        builder.position(self.module.append_block(wrapper));
        let value = self.module.parameter(wrapper, 1);
        let mut arguments = Vec::new();
        let mut result = None;
        for hidden in &signature.hidden {
            match hidden {
                Hidden::Context => arguments.push(self.module.parameter(wrapper, 0)),
                Hidden::Result => {
                    let place = builder.alloca(i8_ty, 1);
                    arguments.push(place);
                    result = Some(place);
                }
                _ => return invalid("a codec check with a hidden slot besides its result"),
            }
        }
        let [parameter] = abi.parameters.as_slice() else {
            return invalid("a codec check without exactly one parameter");
        };
        match parameter.passing {
            ParameterPassing::ReadOnlyAddress => arguments.push(value),
            ParameterPassing::ByValue => {
                let scalars = &self.layout(layout)?.abi.scalars;
                if scalars.len() != parameter.slots.len() {
                    return invalid("a codec check's slots differ from its layout's");
                }
                // Each slot is its scalar's bytes in memory (a Bool's byte is 0 or 1).
                for (&(offset, _), slot) in scalars.iter().zip(&parameter.slots) {
                    let at = builder.offset(value, self.const_u64(offset));
                    let of = signature::physical(self.module, slot.ty);
                    arguments.push(builder.load(of, at, 1));
                }
            }
            _ => return invalid("a codec check's value passed other than by borrow"),
        }
        let returned = builder.call(callee, &arguments);
        signature.attribute_call(self.module, returned);
        let verdict = match (signature.value, result) {
            (Some((kind, field)), None) => {
                let word = builder.extract(returned, field);
                let ty = value_word(self.module, kind);
                let bit = decode_word(&builder, kind, word, ty);
                builder.cast(Cast::ZExt, bit, i8_ty)
            }
            (None, Some(place)) => builder.load(i8_ty, place, 1),
            _ => return invalid("a codec check without its bool result"),
        };
        let status = if signature.value.is_some() {
            builder.extract(returned, signature.status_field)
        } else {
            returned
        };
        builder.store(verdict, self.module.parameter(wrapper, 2), 1);
        builder.ret(status);
        Ok(wrapper.value)
    }

    /// The codec `codec` as the exported `hirsel_rt_v2_codec` record
    /// `hirsel.codec.<key>`: its descriptor bytes and its program, whose descriptor
    /// indices are the linked registry's (runtime ABI 9), and its check thunks.
    fn codec(&mut self, codec: &GlueCodec) -> Glued {
        let plan: &CodecPlan = &codec.plan;
        for layout in plan.allocated(self.inputs.layouts) {
            self.descriptor_pointer(layout)?;
        }
        let registry = &self.inputs.registry;
        let words = match plan.program(self.inputs.layouts, &|id| registry.index(id)) {
            Ok(words) => words,
            Err(error) => return invalid(format!("a codec plan off its layouts: {error}")),
        };
        let (ptr, i8_ty, i64_ty) = (
            self.module.ptr_type(),
            self.module.int_type(8),
            self.module.int_type(64),
        );
        let descriptor = self.module.private_constant(
            "hirsel.codec.descriptor",
            self.module.array_type(i8_ty, plan.descriptor.len() as u64),
            1,
        );
        self.module
            .set_initializer(descriptor, self.module.const_bytes(&plan.descriptor));
        let program = self.module.private_constant(
            "hirsel.codec.program",
            self.module.array_type(i64_ty, words.len() as u64),
            8,
        );
        let items: Vec<_> = words.iter().map(|&w| self.const_u64(w)).collect();
        self.module
            .set_initializer(program, self.module.const_array(i64_ty, &items));
        let checks = if codec.checks.is_empty() {
            self.module.const_zero(ptr)
        } else {
            let mut thunks = Vec::with_capacity(codec.checks.len());
            for (check, value) in codec.checks.iter().zip(plan.check_layouts()) {
                let Some(layout) = value else {
                    return invalid("a codec check no CHECKED node calls");
                };
                thunks.push(self.check_wrapper(check, layout)?);
            }
            let table = self.module.private_constant(
                "hirsel.codec.checks",
                self.module.array_type(ptr, thunks.len() as u64),
                8,
            );
            self.module
                .set_initializer(table, self.module.const_array(ptr, &thunks));
            table
        };
        let name = codec_symbol(&codec.key);
        let global = self.module.exported_constant(
            &name,
            self.module
                .struct_type(&[ptr, i64_ty, ptr, i64_ty, ptr, i64_ty]),
            8,
        );
        let initializer = self.module.const_struct(&[
            descriptor,
            self.const_u64(plan.descriptor.len() as u64),
            program,
            self.const_u64(words.len() as u64),
            checks,
            self.const_u64(codec.checks.len() as u64),
        ]);
        self.module.set_initializer(global, initializer);
        self.export(&name);
        Ok(())
    }

    /// `layout`'s `descriptor_word_symbol`, which units' views of glue blocks of it name.
    fn descriptor_word(&mut self, layout: LayoutId) -> Glued {
        let name = descriptor_word_symbol(&self.inputs.keys.key(layout));
        if !self.manifested.exports.contains_key(&name) {
            let index = self.index(layout)?;
            self.module.absolute_symbol(&name, u64::from(index));
            self.export(&name);
        }
        Ok(())
    }

    /// The static immortal blocks of `data`, exported: block 0 as `static_symbol`, the
    /// blocks it reaches as `<static_symbol>.<index>`, each with its payload.
    fn statics(&mut self, data: &StaticData) -> Glued {
        data.validate(
            self.inputs.layouts,
            u64::from(self.inputs.target.pointer_bytes),
        )
        .map_err(|what| CodegenError::Glue { what })?;
        let name = static_symbol(&data.symbol);
        // A block names only blocks after it, so each is emitted after its targets.
        let mut emitted: Vec<Option<Value<'m>>> = vec![None; data.blocks.len()];
        for (index, block) in data.blocks.iter().enumerate().rev() {
            if let SizeSpec::Fixed(size) = self.layout(block.layout)?.size
                && size != block.bytes.len() as u64
            {
                return invalid("static data of another size than its layout");
            }
            let mut references = Vec::with_capacity(block.references.len());
            for r in &block.references {
                let target = r.target as usize;
                let Some(Some(target)) = emitted.get(target).filter(|_| target > index) else {
                    return invalid("a static reference to no later block");
                };
                references.push((r.offset, *target));
            }
            for function in &block.functions {
                let symbol = &function.symbol;
                let thunk = function_thunk_name(&symbol.definition, symbol.instance.as_ref());
                self.manifested.import(
                    &thunk,
                    ObjectSymbolKind::Function,
                    Some(&symbol.definition),
                );
                let code = self.module.function_address(&thunk);
                let descriptor = self.module.const_int(self.module.int_type(32), 2);
                let (_, environment) = static_block(
                    self.module,
                    self.inputs.target,
                    "hirsel.env.empty",
                    descriptor,
                    1,
                    &[],
                    &[],
                    false,
                )
                .map_err(|what| CodegenError::Glue { what })?;
                references.push((function.offset, code));
                references.push((
                    function.offset + u64::from(self.inputs.target.pointer_bytes),
                    environment,
                ));
            }
            references.sort_by_key(|(offset, _)| *offset);
            self.descriptor_pointer(block.layout)?;
            self.descriptor_word(block.layout)?;
            let descriptor = self.const_u32(self.index(block.layout)?);
            let align = self.layout(block.layout)?.align;
            let block_name = match index {
                0 => name.clone(),
                _ => format!("{name}.{index}"),
            };
            let (_, header) = static_block(
                self.module,
                self.inputs.target,
                &block_name,
                descriptor,
                align,
                &block.bytes,
                &references,
                true,
            )
            .map_err(|what| CodegenError::Glue { what })?;
            self.export(&block_name);
            emitted[index] = Some(header);
        }
        Ok(())
    }

    /// The text literal of `bytes` at the sequence payload `payload`, exported as
    /// `literal_symbol`.
    fn literal(&mut self, payload: LayoutId, data: &[u8]) -> Glued {
        self.descriptor_pointer(payload)?;
        self.descriptor_word(payload)?;
        let descriptor = self.const_u32(self.index(payload)?);
        let align = self.layout(payload)?.align;
        let name = literal_symbol(&self.inputs.keys.key(payload), data);
        static_block(
            self.module,
            self.inputs.target,
            &name,
            descriptor,
            align,
            data,
            &[],
            true,
        )
        .map_err(|what| CodegenError::Glue { what })?;
        self.export(&name);
        Ok(())
    }

    /// The C callback thunk of `callback` (PROG-4), calling its unit's entry.
    fn callback(&mut self, callback: &GlueCallback) -> Glued {
        let definition = &callback.definition;
        let abi = fn_abi(self.inputs.layouts, callback.abi, definition)?;
        let name = symbol_name(definition, callback.instance.as_ref());
        let external = Signature::of(self.module, self.inputs.layouts, abi, definition)?;
        let entry = external.declare(
            self.module,
            self.inputs.layouts,
            &name,
            definition,
            false,
            Aliasing::of(self.inputs.aliasing_unstated),
        )?;
        self.manifested
            .imports
            .entry(name.clone())
            .or_insert_with(|| ObjectSymbol {
                name: name.clone(),
                kind: ObjectSymbolKind::Function,
                definition: Some(definition.clone()),
                abi: Some(abi_digest(abi)),
            });
        let thunk = callback_thunk_name(definition, callback.instance.as_ref());
        foreign::define_callback_thunk(
            self.module,
            &mut self.manifested,
            self.inputs.layouts,
            self.inputs.target.c_convention,
            &thunk,
            entry,
            &external,
            definition,
        )?;
        self.manifested.exports.insert(
            thunk.clone(),
            ObjectSymbol {
                name: thunk,
                kind: ObjectSymbolKind::Function,
                definition: Some(definition.clone()),
                abi: Some(abi_digest(abi)),
            },
        );
        Ok(())
    }
}

/// The program's glue object for `inputs` (see the module), at `default<O0>`: it holds
/// data and thunks only.
pub fn emit_glue(inputs: &GlueInputs<'_>) -> Result<ObjectArtifact, CodegenError> {
    check_loaded()?;
    if !inputs.registry.validates(inputs.layouts, inputs.finalizers) {
        return Err(CodegenError::Manifest {
            problem: "foreign or noncanonical descriptor registry".into(),
        });
    }
    let (bytes, manifested) = {
        let machine = machine(inputs.target, false)?;
        let context = Context::new();
        let module = Module::new(&context, "hirsel.glue");
        module.set_target(&machine);
        let mut emitter = ProgramEmitter {
            module: &module,
            inputs,
            manifested: Manifested::default(),
        };
        emitter.registry_table()?;
        emitter.index_cells()?;
        emitter.failure_cells()?;
        for codec in &inputs.glue.codecs {
            emitter.codec(codec)?;
        }
        for symbol in &inputs.glue.statics {
            let Some(data) = inputs.statics.iter().find(|d| d.symbol == *symbol) else {
                return invalid(format!("no static data for {}", symbol.definition));
            };
            emitter.statics(data)?;
        }
        for (payload, data) in &inputs.glue.literals {
            emitter.literal(*payload, data)?;
        }
        for callback in inputs.callbacks {
            emitter.callback(callback)?;
        }
        let mut manifested = emitter.manifested;
        manifested.settle_imports();
        #[cfg(feature = "native-tsan")]
        crate::tsan::manifest(&mut manifested);
        (
            optimize_and_emit(
                &module,
                &machine,
                OptLevel::None,
                hirsel_lir::emission::validate_stages(),
                false,
            )?,
            manifested,
        )
    };
    let file = ObjectFile::read(inputs.target, bytes)
        .map_err(|message| CodegenError::Object { message })?;
    hirsel_lir::emission::glue_artifact(file, manifested, inputs.target, &toolchain())
        .map_err(|problem| CodegenError::Manifest { problem })
}
