use super::*;
use hirsel_lir::glue::{
    codec_symbol, descriptor_index_symbol, descriptor_word_symbol, literal_symbol,
};

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

    /// `layout`'s descriptor index, loaded from its glue cell (`hirsel_lir::glue`): units
    /// carry no number the program assigns.
    pub(super) fn descriptor(&mut self, layout: LayoutId) -> Emitted<Value<'m>> {
        self.layout(layout)?;
        let name = descriptor_index_symbol(&self.inputs.keys.key(layout));
        let i32_ty = self.module.int_type(32);
        let cell = self.glue_cell(&name, i32_ty);
        Ok(self.builder.load(i32_ty, cell, 4))
    }

    /// The glue data symbol `name`, declared as `ty` and recorded as an import.
    pub(super) fn glue_cell(&mut self, name: &str, ty: Type<'m>) -> Value<'m> {
        self.manifested.import(name, ObjectSymbolKind::Data, None);
        self.module.external_constant(name, ty)
    }

    /// The unit's codec `codec`: glue's `hirsel_rt_v2_codec` record of its `CodecKey`,
    /// bound to the program's registry there (`hirsel_lir::glue`).
    pub(super) fn codec(&mut self, codec: CodecId) -> Emitted<Value<'m>> {
        let Some(plan) = self.unit.tables.codecs.get(codec.index()) else {
            return self.invalid("a codec outside the codec table");
        };
        let key = match hirsel_lir::glue::GlueCodec::new(
            plan,
            self.unit,
            self.inputs.layouts,
            self.inputs.keys,
        ) {
            Ok(codec) => codec.key,
            Err(error) => return self.invalid(format!("a codec plan off its layouts: {error}")),
        };
        let ty = self.module.array_type(self.module.int_type(8), 0);
        Ok(self.glue_cell(&codec_symbol(&key), ty))
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
        if let Some(storage) = self.cpu_region(mark, layout, extent, record.align)? {
            return Ok(Place {
                ptr: storage,
                layout,
                align: record.align,
                block: None,
                columns: None,
            });
        }
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

    /// The environment of a function reference: one static immortal block with an empty
    /// payload and the empty descriptor (index 2, which the runtime fixes, not the
    /// program), never released (a static is consumed without credit).
    pub(super) fn empty_environment(&mut self) -> Emitted<Value<'m>> {
        let index = self.module.const_int(self.module.int_type(32), 2);
        program::static_block(
            self.module,
            self.inputs.target,
            "hirsel.env.empty",
            index,
            1,
            &[],
            &[],
            false,
        )
        .map(|(_, header)| header)
        .or_else(|what| self.invalid(what))
    }

    /// The static block of a String or Bytes literal: glue's `literal_symbol` block of the
    /// counted `Sequence` payload `payload` (its u64 element count, its u64 capacity, the
    /// count, then the bytes), whose prefix holds the program's descriptor index.
    pub(super) fn literal_block(&mut self, payload: LayoutId, bytes: &[u8]) -> Emitted<Value<'m>> {
        if self.inputs.target.endian != Endian::Little {
            return self.unsupported("a static literal on a big-endian target");
        }
        let data = match hirsel_lir::glue::literal_data(self.inputs.layouts, payload, bytes) {
            Ok(data) => data,
            Err(what) => return self.invalid(what),
        };
        let name = literal_symbol(&self.inputs.keys.key(payload), &data);
        self.glue_block(&name, payload, &data, &[])
    }

    /// The header of glue's static immortal block `name` of the payload layout `layout`,
    /// holding `bytes` with the block references `references`. The block is declared
    /// `available_externally` with that content, so reads of it fold; its descriptor word
    /// names glue's `descriptor_word_symbol`, so a folded read of it is a relocation.
    pub(super) fn glue_block(
        &mut self,
        name: &str,
        layout: LayoutId,
        bytes: &[u8],
        references: &[(u64, Value<'m>)],
    ) -> Emitted<Value<'m>> {
        let align = self.layout(layout)?.align;
        let Some(offset) = program::static_header_offset(align) else {
            return self.invalid("static extent overflow");
        };
        self.manifested.import(name, ObjectSymbolKind::Data, None);
        let global = match self.module.named_global(name) {
            Some(global) => global,
            None => {
                let word = descriptor_word_symbol(&self.inputs.keys.key(layout));
                let i8_ty = self.module.int_type(8);
                let symbol = self.glue_cell(&word, i8_ty);
                let descriptor = self
                    .module
                    .const_ptr_to_int(symbol, self.module.int_type(32));
                let (ty, initializer) = program::static_image(
                    self.module,
                    self.inputs.target,
                    descriptor,
                    align,
                    bytes,
                    references,
                )
                .or_else(|what| self.invalid(what))?;
                self.module
                    .available_constant(name, ty, align.max(8), initializer)
            }
        };
        Ok(self.module.const_offset(global, offset))
    }
}
