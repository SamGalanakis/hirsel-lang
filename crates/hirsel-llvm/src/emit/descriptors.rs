use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    /// The `const uint64_t *live_mask` of a release: null when every field is live, else
    /// the mask's words.
    pub(super) fn live_mask(&mut self, live: MaskRef) -> Emitted<Value<'m>> {
        let ptr = self.module.ptr_type();
        let i64_ty = self.module.int_type(64);
        match live {
            MaskRef::All => Ok(self.module.const_zero(ptr)),
            MaskRef::Static(mask) => {
                let Some(mask) = self.unit.tables.masks.get(mask.index()) else {
                    return self.invalid("a mask outside the mask table");
                };
                let words: Vec<_> = self
                    .pool(mask.words, &self.unit.pools.mask_words)?
                    .iter()
                    .map(|&w| self.module.const_int(i64_ty, u128::from(w)))
                    .collect();
                if words.is_empty() {
                    return self.invalid("a static mask without words");
                }
                let ty = self.module.array_type(i64_ty, words.len() as u64);
                let global = self.module.private_constant("hirsel.mask", ty, 8);
                self.module
                    .set_initializer(global, self.module.const_array(i64_ty, &words));
                Ok(global)
            }
            MaskRef::Dynamic(mask) => {
                let Shape::Scalar(kind) = self.value_shape(mask)? else {
                    return self.invalid("a mask that is not a scalar");
                };
                let Some((width, false)) = scalar::integer(kind) else {
                    return self.invalid("a mask that is not an unsigned integer");
                };
                let mut word = self.get(mask)?;
                if width < 64 {
                    word = self.builder.cast(Cast::ZExt, word, i64_ty);
                } else if width > 64 {
                    return self.unsupported("a live mask wider than one word");
                }
                let spill = self.prologue.alloca(i64_ty, 8);
                self.builder.store(word, spill, 8);
                Ok(spill)
            }
        }
    }

    /// The LLVM types of `hirsel_rt_v2_layout` and `hirsel_rt_v2_step`.
    pub(super) fn descriptor_types(&self) -> (Type<'m>, Type<'m>) {
        let (ptr, i32_ty, i64_ty) = (
            self.module.ptr_type(),
            self.module.int_type(32),
            self.module.int_type(64),
        );
        (
            self.module.struct_type(&[
                i32_ty, i32_ty, i64_ty, i64_ty, ptr, ptr, i64_ty, i32_ty, i32_ty, i64_ty,
            ]),
            self.module.struct_type(&[
                i32_ty, i32_ty, i32_ty, i32_ty, i64_ty, i64_ty, i64_ty, i64_ty, ptr,
            ]),
        )
    }

    /// The unit's codec `codec` as static `hirsel_rt_v2_codec` data: its descriptor bytes and
    /// its program, whose descriptor indices are the linked registry's (runtime ABI 9).
    pub(super) fn codec(&mut self, codec: CodecId) -> Emitted<Value<'m>> {
        let Some(plan) = self.unit.tables.codecs.get(codec.index()) else {
            return self.invalid("a codec outside the codec table");
        };
        for layout in plan.allocated(self.inputs.layouts) {
            self.descriptor_pointer(layout)?;
        }
        let registry = &self.inputs.registry;
        let words = match plan.program(self.inputs.layouts, &|id| registry.index(id)) {
            Ok(words) => words,
            Err(error) => return self.invalid(format!("a codec plan off its layouts: {error}")),
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
        let items: Vec<_> = words
            .iter()
            .map(|&w| self.module.const_int(i64_ty, u128::from(w)))
            .collect();
        self.module
            .set_initializer(program, self.module.const_array(i64_ty, &items));
        let global = self.module.private_constant(
            "hirsel.codec",
            self.module.struct_type(&[ptr, i64_ty, ptr, i64_ty]),
            8,
        );
        let initializer = self.module.const_struct(&[
            descriptor,
            self.const_u64(plan.descriptor.len() as u64),
            program,
            self.const_u64(words.len() as u64),
        ]);
        self.module.set_initializer(global, initializer);
        Ok(global)
    }

    /// The header stores the linked descriptor index and storage kind.
    pub(super) fn block_descriptor(&self, block: Value<'m>) -> Value<'m> {
        let word = self.builder.load(
            self.module.int_type(32),
            self.builder.offset(block, self.const_u64(4)),
            4,
        );
        self.builder.binary(
            BinOp::And,
            word,
            self.module.const_int(self.module.int_type(32), 0x1fff_ffff),
        )
    }

    pub(super) fn descriptor(&mut self, layout: LayoutId) -> Emitted<Value<'m>> {
        self.descriptor_pointer(layout)?;
        let Some(index) = self.inputs.registry.index(layout) else {
            return self.invalid("a layout absent from the linked descriptor registry");
        };
        Ok(self
            .module
            .const_int(self.module.int_type(32), index.into()))
    }

    pub(super) fn empty_descriptor(&mut self) -> Value<'m> {
        let name = format!("hirsel.layout.empty.{}", self.inputs.registry.symbol());
        if let Some(global) = self.module.named_global(&name) {
            return global;
        }
        let (ty, _) = self.descriptor_types();
        self.manifested.descriptors.insert(name.clone());
        let global = self.module.linkonce_constant(&name, ty, 8);
        let null = self.module.const_zero(self.module.ptr_type());
        let initializer = self.descriptor_initializer(0, 1, null, null, 0, 0, 0, 0);
        self.module.set_initializer(global, initializer);
        global
    }

    pub(super) fn registry_table(&mut self) -> Emitted<()> {
        let name = self.inputs.registry.symbol();
        if self.module.named_global(&name).is_some() {
            return Ok(());
        }
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

    /// A descriptor's initializer.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn descriptor_initializer(
        &self,
        size: u64,
        align: u64,
        finalizer: Value<'m>,
        steps: Value<'m>,
        step_count: u64,
        field_count: u32,
        flags: u32,
        logical: u64,
    ) -> Value<'m> {
        let (i32_ty, i64_ty) = (self.module.int_type(32), self.module.int_type(64));
        let u32_ = |n: u32| self.module.const_int(i32_ty, u128::from(n));
        let u64_ = |n: u64| self.module.const_int(i64_ty, u128::from(n));
        self.module.const_struct(&[
            u32_(LAYOUT_MAGIC),
            u32_(LAYOUT_DESCRIPTOR_VERSION),
            u64_(size),
            u64_(align),
            finalizer,
            steps,
            u64_(step_count),
            u32_(field_count),
            u32_(flags),
            u64_(logical),
        ])
    }

    /// The `hirsel_rt_v2_layout` (version 2) of `layout`, emitted once per module with every
    /// descriptor its steps name, as `hirsel_lir::rt_program` lowers them. A layout with a user
    /// finalizer names its drop wrapper.
    pub(super) fn descriptor_pointer(&mut self, layout: LayoutId) -> Emitted<Value<'m>> {
        if let Some(global) = self
            .module
            .named_global(&descriptor_name(self.layout(layout)?))
        {
            return Ok(global);
        }
        let descriptors = match rt_program::descriptors(self.inputs.layouts, &[layout]) {
            Ok(descriptors) => descriptors,
            Err(error) => {
                return self.invalid(format!(
                    "no runtime descriptor for layout {}: {error:?}",
                    layout.0
                ));
            }
        };
        let ptr = self.module.ptr_type();
        let (layout_ty, step_ty) = self.descriptor_types();
        // Declare every new descriptor first: a Finalize step names another.
        let mut new = Vec::new();
        for d in &descriptors {
            let name = descriptor_name(self.layout(d.layout)?);
            if self.module.named_global(&name).is_none() {
                self.manifested.descriptors.insert(name.clone());
                new.push((d, self.module.linkonce_constant(&name, layout_ty, 8)));
            }
        }
        let i32_ty = self.module.int_type(32);
        let i64_ty = self.module.int_type(64);
        for (d, global) in new {
            let mut steps = Vec::new();
            for step in &d.steps {
                let target = match step.layout {
                    Some(l) => match self.module.named_global(&descriptor_name(self.layout(l)?)) {
                        Some(target) => target,
                        None => return self.invalid("a step naming a layout without a descriptor"),
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
                let name = descriptor_name(self.layout(d.layout)?)
                    .replace("hirsel.layout.", "hirsel.steps.");
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
            let initializer = self.descriptor_initializer(
                d.size,
                d.align,
                finalizer,
                steps_ptr,
                d.steps.len() as u64,
                d.field_count,
                d.flags,
                d.logical,
            );
            self.module.set_initializer(global, initializer);
        }
        match self
            .module
            .named_global(&descriptor_name(self.layout(layout)?))
        {
            Some(global) => Ok(global),
            None => self.invalid("a descriptor that was not emitted"),
        }
    }

    /// The drop thunk of `layout`, whose finalizer is `finalizer`: a private wrapper with the
    /// runtime's `hirsel_rt_v2_drop_thunk` type that calls the drop body's unit, named
    /// `symbol_name` of its definition instance, with the self address (Borrow for source bodies, Mut for library bodies), and returns its
    /// status. The unit records its own failure; the cleanup anchor is not its to report.
    pub(super) fn drop_wrapper(
        &mut self,
        layout: LayoutId,
        finalizer: &FinalizerRef,
    ) -> Emitted<Value<'m>> {
        let Some(instance) = self.inputs.finalizers.get(&layout) else {
            return self.invalid(format!(
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
                return self.invalid(format!(
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
        let wrapper = match self
            .module
            .function(&format!("hirsel.drop.{}", layout.0), ty)
        {
            Ok(wrapper) => wrapper,
            Err(what) => return self.invalid(what),
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

    /// Flat content needs an owner a callee can copy. Reserve its real immortal
    /// header once with the regional payload; other regional storage stays headerless.
    pub(super) fn reserve_region(
        &mut self,
        mark: Value<'m>,
        layout: LayoutId,
        extent: Value<'m>,
        failure: FailureId,
    ) -> Emitted<Place<'m>> {
        let record = self.layout(layout)?;
        let (flat, align) = (record.kind == LayoutKind::Flat, record.align);
        let offset = if flat {
            match align_up(IMMORTAL_PREFIX_BYTES, u64::from(align.max(8))) {
                Ok(n) => n,
                Err(error) => return self.invalid(format!("{error:?}")),
            }
        } else {
            0
        };
        // An immortal owner's copy may read its rounded header capacity.
        // Reserve that capacity, including padding, with the payload.
        let capacity = if flat {
            let rounded = self.builder.binary(BinOp::Add, extent, self.const_u64(15));
            self.builder
                .binary(BinOp::And, rounded, self.const_u64(!15))
        } else {
            extent
        };
        let size = self
            .builder
            .binary(BinOp::Add, capacity, self.const_u64(offset));
        let reserve = self.runtime(RuntimeEntry::RegionReserve)?;
        let out = self.prologue.alloca(self.module.ptr_type(), 8);
        let status = self.builder.call(
            reserve,
            &[
                self.context,
                mark,
                size,
                self.const_u64(u64::from(if flat { align.max(8) } else { align })),
                out,
            ],
        );
        self.status_guard(status, failure)?;
        let storage = self.builder.load(self.module.ptr_type(), out, 8);
        let owner = if flat {
            let header = self.builder.offset(storage, self.const_u64(offset - 8));
            let descriptor = self.descriptor(layout)?;
            let bind = self.runtime(RuntimeEntry::RegionOwner)?;
            self.builder.call(
                bind,
                &[
                    self.context,
                    mark,
                    header,
                    descriptor,
                    capacity,
                    self.const_u64(u64::from(align)),
                ],
            );
            Some(header)
        } else {
            None
        };
        Ok(Place {
            columns: None,
            ptr: self.builder.offset(storage, self.const_u64(offset)),
            layout,
            align,
            block: owner,
        })
    }

    /// A static immortal block named `name`: a counted header with `COUNT_IMMORTAL`, the
    /// static domain and `descriptor`, then `bytes` at the payload offset of `align`. The
    /// runtime never writes it: dups and releases of an immortal count return at once.
    /// A private static immortal block of `bytes`, with the pointer-sized
    /// cell at each offset of `references` holding that block reference (a
    /// relocation; its bytes are zero).
    pub(super) fn static_block(
        &mut self,
        name: &str,
        descriptor: Value<'m>,
        align: u32,
        bytes: &[u8],
        references: &[(u64, Value<'m>)],
    ) -> Emitted<Value<'m>> {
        let physical = align.max(8);
        let start = match align_up(IMMORTAL_PREFIX_BYTES, u64::from(physical)) {
            Ok(n) => n,
            Err(_) => return self.invalid("static extent overflow"),
        };
        let header_offset = start - 8;
        if let Some(global) = self.module.named_global(name) {
            return Ok(self.module.const_offset(global, header_offset));
        }
        let (ptr, i8_ty, i32_ty) = (
            self.module.ptr_type(),
            self.module.int_type(8),
            self.module.int_type(32),
        );
        // Static extents have exactly their initialized bytes. Padding them
        // like heap classes would give [] spare capacity and send its first
        // push through a detach instead of directly through sequence growth.
        let capacity = bytes.len();
        let word = self
            .module
            .const_or(descriptor, self.module.const_int(i32_ty, 2u128 << 29));
        let prefix = IMMORTAL_PREFIX.map(|field| match field {
            PrefixWord::Capacity => self.const_u64(capacity as u64),
            PrefixWord::Align => self.const_u64(u64::from(physical)),
            PrefixWord::Count => self.module.const_int(i32_ty, u128::from(COUNT_IMMORTAL)),
            PrefixWord::Descriptor => word,
        });
        let prefix_ty = self.module.struct_type(
            &IMMORTAL_PREFIX.map(|field| self.module.int_type(8 * field.bytes() as u32)),
        );
        let padding = vec![0u8; (start - IMMORTAL_PREFIX_BYTES) as usize];
        let (payload_ty, payload) = if references.is_empty() {
            (
                self.module.array_type(i8_ty, bytes.len() as u64),
                self.module.const_bytes(bytes),
            )
        } else {
            // Byte runs between the reference cells, packed so each cell lies
            // at its own offset.
            let word = u64::from(self.inputs.target.pointer_bytes);
            let mut sorted = references.to_vec();
            sorted.sort_by_key(|(offset, _)| *offset);
            let (mut types, mut parts) = (Vec::new(), Vec::new());
            let mut at = 0u64;
            for (offset, target) in sorted {
                if offset < at || offset + word > bytes.len() as u64 {
                    return self.invalid("a static reference outside its block");
                }
                let run = &bytes[at as usize..offset as usize];
                types.push(self.module.array_type(i8_ty, run.len() as u64));
                parts.push(self.module.const_bytes(run));
                types.push(ptr);
                parts.push(target);
                at = offset + word;
            }
            let run = &bytes[at as usize..];
            types.push(self.module.array_type(i8_ty, run.len() as u64));
            parts.push(self.module.const_bytes(run));
            (
                self.module.packed_struct_type(&types),
                self.module.const_packed_struct(&parts),
            )
        };
        let ty = self.module.packed_struct_type(&[
            self.module.array_type(i8_ty, padding.len() as u64),
            prefix_ty,
            payload_ty,
        ]);
        let global = self.module.private_constant(name, ty, physical);
        let initializer = self.module.const_packed_struct(&[
            self.module.const_bytes(&padding),
            self.module.const_struct(&prefix),
            payload,
        ]);
        self.module.set_initializer(global, initializer);
        Ok(self.module.const_offset(global, header_offset))
    }

    /// The environment of a function reference: one static immortal block with an empty
    /// payload and the empty descriptor, never released (a static is consumed without credit).
    pub(super) fn empty_environment(&mut self) -> Emitted<Value<'m>> {
        self.empty_descriptor();
        let index = self.module.const_int(self.module.int_type(32), 2);
        self.static_block("hirsel.env.empty", index, 1, &[], &[])
    }

    /// The static block of a String or Bytes literal: a counted `Sequence` payload of `payload`
    /// (its u64 element count, its u64 capacity, the count, then the bytes).
    pub(super) fn literal_block(&mut self, payload: LayoutId, bytes: &[u8]) -> Emitted<Value<'m>> {
        if self.inputs.target.endian != Endian::Little {
            return self.unsupported("a static literal on a big-endian target");
        }
        let LayoutKind::Sequence { element } = self.layout(payload)?.kind else {
            return self.invalid("a text literal whose storage is not a sequence");
        };
        if self.fixed_size(element)? != 1 {
            return self.invalid("a text literal of elements wider than a byte");
        }
        let start = match sequence_elements_offset(self.layout(element)?) {
            Ok(start) => start as usize,
            Err(_) => return self.invalid("a sequence element offset beyond u64"),
        };
        let mut data = vec![0u8; start];
        let count = (bytes.len() as u64).to_le_bytes();
        data[..8].copy_from_slice(&count);
        let capacity = SEQUENCE_CAPACITY_OFFSET as usize;
        data[capacity..capacity + 8].copy_from_slice(&count);
        data.extend_from_slice(bytes);
        let descriptor = self.descriptor(payload)?;
        let align = self.layout(payload)?.align;
        let name = format!(
            "hirsel.literal.{}",
            ContentHash::of_parts("hirsel.native-literal.v1", [data.as_slice()])
                .0
                .iter()
                .take(8)
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        self.static_block(&name, descriptor, align, &data, &[])
    }
}
