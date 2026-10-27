use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    /// The active variant ordinal (an `i32`) of the tagged value at `place`.
    pub(super) fn active_tag(&self, place: Place<'m>) -> Emitted<Value<'m>> {
        if place.columns.is_some() {
            return self.column_tag(place);
        }
        let record = self.layout(place.layout)?;
        let Some(tag) = &record.tag else {
            return self.invalid("a tag read of an untagged layout");
        };
        Ok(self.tag_at(place, tag.offset, tag.form, record.abi.unaligned))
    }

    /// The active variant ordinal (an `i32`) of a variant whose tag lies `offset` bytes into
    /// `place` in `form`: the only tag reader, so the tag encoding has one place to change.
    pub(super) fn tag_at(
        &self,
        place: Place<'m>,
        offset: u64,
        form: TagForm,
        unaligned: bool,
    ) -> Value<'m> {
        load_tag(
            &self.builder,
            place.ptr,
            place.align,
            offset,
            form,
            unaligned,
        )
    }

    /// Makes `variant` the active variant of the tagged storage at `place`; the only tag
    /// writer.
    pub(super) fn store_tag(&self, place: Place<'m>, variant: u32) -> Emitted {
        if place.columns.is_some() {
            return self.store_column_tag(place, variant);
        }
        let record = self.layout(place.layout)?;
        let Some(tag) = &record.tag else {
            return self.invalid("a tag write of an untagged layout");
        };
        let at = self.field_place(place, tag.offset, place.layout, record.abi.unaligned)?;
        match tag.form {
            TagForm::Ordinal => {
                let i32_ty = self.module.int_type(32);
                self.builder.store(
                    self.module.const_int(i32_ty, u128::from(variant)),
                    at.ptr,
                    at.align.min(4),
                );
            }
            // A null-pointer niche: the fieldless variant's word is null; the dataful
            // variant's block reference field fills it.
            TagForm::Niche { nullary, .. } if variant == nullary => {
                let ptr_ty = self.module.ptr_type();
                self.builder
                    .store(self.module.const_zero(ptr_ty), at.ptr, at.align.min(8));
            }
            TagForm::LiveBit { .. } => return self.invalid("a live word is not a variant tag"),
            TagForm::Niche { .. } => {}
        }
        Ok(())
    }

    /// An inline construction: zeroed storage (so padding and inactive payload bytes are
    /// defined), then each field at its offset, read back as the value's bytes.
    pub(super) fn construct_site(&self, construct: ConstructId) -> Emitted<&'a ConstructSite> {
        match self.unit.tables.constructs.get(construct.index()) {
            Some(site) => Ok(site),
            None => self.invalid("a construction outside the table"),
        }
    }

    pub(super) fn construct(
        &mut self,
        result: ValueId,
        construct: ConstructId,
        failure: FailureId,
    ) -> Emitted {
        let site = self.construct_site(construct)?;
        let ty = self.value_type(result)?;
        if matches!(
            self.layout(ty.layout)?.kind,
            LayoutKind::Block { .. } | LayoutKind::Box { .. }
        ) {
            if ty.repr == ValueRepr::Block
                && let Some((builder, released)) = self.dying_builder(site)?
            {
                return self.adopt_builder(result, site, builder, released, failure);
            }
            return self.construct_block(result, site, None, failure);
        }
        if ty.layout != site.layout || site.constructor == ConstructorKind::Box {
            return self.invalid("an inline construction of another layout");
        }
        let shape = self.shape(ty)?;
        let place = self.stack(site.layout)?;
        if self.layout(site.layout)?.kind == LayoutKind::Flat {
            // A fixed flat record: its fields' bytes, which fill it exactly.
            let parts = self.flat_parts(site)?;
            self.write_flat(place.ptr, &parts)?;
            let value = self.load(shape, place);
            return self.define(result, value);
        }
        self.builder
            .memset(place.ptr, 0, self.fixed_size(site.layout)?, place.align);
        self.fill(place, site, &[])?;
        let value = self.load(shape, place);
        self.define(result, value)
    }

    /// A heap flat record whose content is exactly one list field built by a flat builder
    /// (LIR schema revision 22) that this block borrows for the construction and then
    /// takes and releases with nothing between: the builder and its released value.
    /// The record's encoding is the builder's payload with its two header words
    /// rewritten, so a unique builder's block can become the record's.
    pub(super) fn dying_builder(
        &self,
        site: &ConstructSite,
    ) -> Emitted<Option<(ValueId, ValueId)>> {
        let record = self.layout(site.layout)?;
        if !matches!(
            site.placement,
            Some(Placement::Fixed(StorageOwner::Heap(_)))
        ) || site.constructor == ConstructorKind::Box
            || record.kind != LayoutKind::Flat
        {
            return Ok(None);
        }
        let [field] = record.fields.as_slice() else {
            return Ok(None);
        };
        let list = self.layout(field.layout)?;
        let encoded = matches!(
            (&list.kind, &list.size, list.fields.as_slice()),
            (LayoutKind::Flat, SizeSpec::Dynamic(_), [content]) if content.offset == 16
        );
        let values = self.pool(site.fields, &self.unit.pools.fields)?;
        let ([value], 0, true) = (values, field.offset, encoded) else {
            return Ok(None);
        };
        let LayoutKind::Block { payload } = self.layout(self.value_type(value.value)?.layout)?.kind
        else {
            return Ok(None);
        };
        let LayoutKind::Sequential { element } = self.layout(payload)?.kind else {
            return Ok(None);
        };
        if self.sequential_offset(element)? != 16 {
            return Ok(None);
        }
        let (b, i) = self.at;
        let code = self.pool(
            self.unit.blocks[b.index()].instructions,
            &self.unit.instructions,
        )?;
        let borrowed = code[..i].iter().rev().find_map(|inst| match *inst {
            Inst::Read {
                result,
                address,
                reaching,
                mode: ReadMode::Borrow,
            } if result == value.value => Some((address, reaching)),
            _ => None,
        });
        let Some((address, reaching)) = borrowed else {
            return Ok(None);
        };
        let mut taken = None;
        for inst in &code[i + 1..] {
            match *inst {
                Inst::EndLoan { .. } => {}
                Inst::Read {
                    result,
                    address: a,
                    reaching: r,
                    mode: ReadMode::Take,
                } if taken.is_none() && a == address && r == reaching => taken = Some(result),
                Inst::Release { value: v, .. } if Some(v) == taken => {
                    return Ok(Some((value.value, v)));
                }
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    /// `dying_builder`'s construction. A unique builder's block becomes the record's: its
    /// count and used words become the list's length prefix and count, and its descriptor
    /// the record's; the builder's release is then skipped. A shared one is copied.
    pub(super) fn adopt_builder(
        &mut self,
        result: ValueId,
        site: &'a ConstructSite,
        builder: ValueId,
        released: ValueId,
        failure: FailureId,
    ) -> Emitted {
        let block = self.get(builder)?;
        let i32_ty = self.module.int_type(32);
        let count = self.builder.load(i32_ty, block, 8);
        let unique = self
            .builder
            .icmp(IntPredicate::Eq, count, self.module.const_int(i32_ty, 1));
        let (adopt, copy, join) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        self.builder.cond_br(unique, adopt, copy);
        self.builder.position(copy);
        self.construct_block(result, site, None, failure)?;
        let Some(copied) = self.values[result.index()].take() else {
            return self.invalid("a flat construction that defined no value");
        };
        let copied_end = self.builder.current();
        self.builder.br(join);
        self.builder.position(adopt);
        let Some(payload) = self.layout_payload(self.value_type(builder)?.layout)? else {
            return self.invalid("a builder that holds no block");
        };
        let data = self.payload_of(block, payload)?.ptr;
        let used_at = self.builder.offset(data, self.const_u64(8));
        let elements = self.load_u64(data);
        let used = self.load_u64(used_at);
        let length = self.builder.binary(BinOp::Add, used, self.const_u64(8));
        self.builder.store(length, data, 8);
        self.builder.store(elements, used_at, 8);
        let descriptor_at = self.builder.offset(block, self.const_u64(4));
        let old = self.builder.load(i32_ty, descriptor_at, 4);
        let kept = self.builder.binary(
            BinOp::And,
            old,
            self.module.const_int(
                i32_ty,
                u128::from(!hirsel_lir::runtime_abi::DESCRIPTOR_INDEX_MASK),
            ),
        );
        let index = self.descriptor(site.layout)?;
        let retyped = self.builder.binary(BinOp::Or, kept, index);
        self.builder.store(retyped, descriptor_at, 4);
        let adopt_end = self.builder.current();
        self.builder.br(join);
        self.builder.position(join);
        let value = self.builder.phi(self.module.ptr_type());
        self.builder.add_incoming(value, copied, copied_end);
        self.builder.add_incoming(value, block, adopt_end);
        let i1 = self.module.int_type(1);
        let taken = self.builder.phi(i1);
        self.builder
            .add_incoming(taken, self.module.const_zero(i1), copied_end);
        self.builder
            .add_incoming(taken, self.module.const_int(i1, 1), adopt_end);
        self.adopted.insert(released, taken);
        self.define(result, value)
    }

    /// The fields a record, tuple or variant construction stores: the record's, or its own
    /// variant's.
    pub(super) fn constructed_fields(&self, site: &'a ConstructSite) -> Emitted<&'a [FieldLayout]> {
        let record = self.layout(site.layout)?;
        match site.constructor {
            ConstructorKind::Tuple | ConstructorKind::Record(_) => Ok(&record.fields),
            ConstructorKind::Variant { variant, .. } => {
                let Some(tag) = &record.tag else {
                    return self.invalid("a variant construction of an untagged layout");
                };
                match tag.variants.iter().find(|v| v.ordinal == variant) {
                    Some(active) => Ok(&active.fields),
                    None => self.invalid("a variant the layout does not have"),
                }
            }
            _ => self.invalid("fields of a construction that is no record, tuple or variant"),
        }
    }

    /// The fields a `Reuse` keeps (LIR schema revision 5): the set ordinals of `mask` among
    /// the construction's fields.
    pub(super) fn kept_fields(
        &self,
        site: &'a ConstructSite,
        mask: MaskId,
    ) -> Emitted<Vec<&'a FieldLayout>> {
        let Some(mask) = self.unit.tables.masks.get(mask.index()) else {
            return self.invalid("a mask outside the mask table");
        };
        let words = self.pool(mask.words, &self.unit.pools.mask_words)?;
        let fields = self.constructed_fields(site)?;
        let set = |i: u32| {
            words
                .get(i as usize / 64)
                .is_some_and(|w| w >> (i % 64) & 1 == 1)
        };
        Ok(fields.iter().filter(|f| set(f.ordinal)).collect())
    }

    /// Stores the construction's value of `field` at its offset in `place`.
    pub(super) fn store_field(
        &self,
        place: Place<'m>,
        site: &'a ConstructSite,
        field: &FieldLayout,
    ) -> Emitted {
        let values = self.pool(site.fields, &self.unit.pools.fields)?;
        let Some(value) = values.iter().find(|v| v.field == field.ordinal) else {
            return self.invalid("a construction without a value for its field");
        };
        let at = self.field_place(place, field.offset, field.layout, field.unaligned)?;
        self.store(
            self.value_shape(value.value)?,
            self.counted_view(value.value)?,
            at,
        );
        Ok(())
    }

    /// Stores a construction's fields, except the ordinals in `skip`, into zeroed storage of
    /// its layout at `place`.
    pub(super) fn fill(
        &mut self,
        place: Place<'m>,
        site: &'a ConstructSite,
        skip: &[u32],
    ) -> Emitted {
        let record = self.layout(site.layout)?;
        let values = self.pool(site.fields, &self.unit.pools.fields)?;
        if !skip.is_empty()
            && !matches!(
                site.constructor,
                ConstructorKind::Tuple
                    | ConstructorKind::Record(_)
                    | ConstructorKind::Variant { .. }
            )
        {
            return self
                .invalid("kept fields of a construction that is no record, tuple or variant");
        }
        match site.constructor {
            ConstructorKind::Tuple | ConstructorKind::Record(_) => {}
            ConstructorKind::Variant { variant, .. } => self.store_tag(place, variant)?,
            // The boxed value is the whole payload.
            ConstructorKind::Box => {
                let [value] = values else {
                    return self.invalid("a box of other than one value");
                };
                self.store(
                    self.value_shape(value.value)?,
                    self.counted_view(value.value)?,
                    place,
                );
                return Ok(());
            }
            ConstructorKind::Sequence => {
                let (element, start) = match record.kind {
                    LayoutKind::Inline { element, .. } | LayoutKind::Simd { element, .. } => {
                        (element, 0)
                    }
                    LayoutKind::Sequential { element } => {
                        // MEM-13: the count, the used bytes, then each element back to
                        // back at its active variant's extent.
                        let count = self.const_u64(values.len() as u64);
                        self.builder.store(count, place.ptr, place.align.min(8));
                        let mut ordered: Vec<_> = values.iter().collect();
                        ordered.sort_by_key(|v| v.field);
                        let first = self.sequential_first(place.ptr, element)?;
                        let mut used = self.const_u64(0);
                        for value in ordered {
                            let at = self.builder.offset(first, used);
                            let bytes = self.store_packed(value.value, element, at)?;
                            used = self.builder.binary(BinOp::Add, used, bytes);
                        }
                        let used_at = self.builder.offset(place.ptr, self.const_u64(8));
                        self.builder.store(used, used_at, place.align.min(8));
                        return Ok(());
                    }
                    LayoutKind::SoaColumns { element } => {
                        let count = self.const_u64(values.len() as u64);
                        let capacity =
                            self.const_u64(self.column_capacity(values.len() as u64, element)?);
                        self.builder.store(count, place.ptr, place.align.min(8));
                        self.builder.store(
                            capacity,
                            self.builder.offset(place.ptr, self.const_u64(8)),
                            place.align.min(8),
                        );
                        for value in values {
                            let row = self.column_row(
                                place,
                                element,
                                self.const_u64(u64::from(value.field)),
                            )?;
                            self.scatter(
                                row,
                                self.value_shape(value.value)?,
                                self.counted_view(value.value)?,
                            )?;
                        }
                        return Ok(());
                    }
                    LayoutKind::Sequence { element } => {
                        // The element count, the capacity (the count: never more than the
                        // block's slots), then the elements from their alignment (runtime
                        // ABI 5).
                        let count = self.const_u64(values.len() as u64);
                        self.builder.store(count, place.ptr, place.align.min(8));
                        let capacity = self
                            .builder
                            .offset(place.ptr, self.const_u64(SEQUENCE_CAPACITY_OFFSET));
                        self.builder.store_access(
                            count,
                            capacity,
                            place.align.min(8),
                            Access::Capacity,
                        );
                        match sequence_elements_offset(self.layout(element)?) {
                            Ok(start) => (element, start),
                            Err(_) => return self.invalid("a sequence element offset beyond u64"),
                        }
                    }
                    _ => return self.unsupported("a sequence of this layout"),
                };
                let stride = self.fixed_size(element)?;
                for value in values {
                    let offset = start + u64::from(value.field) * stride;
                    let at = self.field_place(place, offset, element, false)?;
                    self.store(
                        self.value_shape(value.value)?,
                        self.counted_view(value.value)?,
                        at,
                    );
                }
                return Ok(());
            }
        }
        let fields = self.constructed_fields(site)?;
        for value in values {
            let Some(field) = fields.iter().find(|f| f.ordinal == value.field) else {
                return self.invalid("a constructed field the layout does not have");
            };
            if !skip.contains(&field.ordinal) {
                self.store_field(place, site, field)?;
            }
        }
        Ok(())
    }

    /// `hirsel_rt_v2_alloc` of a heap block of `layout` with `extent` payload bytes. It cannot
    /// fail (runtime ABI 3): host exhaustion terminates environmentally. The block's count is
    /// the alloc postcondition, one holder, restated as a store so that LLVM folds the
    /// uniqueness tests and the release of a block that has not escaped, and may then delete
    /// the allocation with its free (`runtime::allocator`). A constant small request calls
    /// its pool's fast path, inlined after optimization (runtime ABI 4, `runtime::alloc_fast`).
    pub(super) fn alloc_block(
        &mut self,
        layout: LayoutId,
        extent: Value<'m>,
        align: u32,
        anchor: SourceAnchorId,
    ) -> Emitted<Value<'m>> {
        let descriptor = self.descriptor(layout)?;
        let alloc = self.runtime(RuntimeEntry::Alloc)?;
        let anchor = self.anchor_code(anchor)?;
        let i8_ty = self.module.int_type(8);
        // A constant small request calls its pool's fast path (runtime ABI 4).
        let alloc = match extent
            .known_u64()
            .and_then(|extent| small_pool(extent, u64::from(align)))
        {
            Some(pool) => crate::runtime::alloc_fast(self.module, alloc, pool),
            None => alloc,
        };
        let block = self.builder.call(
            alloc,
            &[
                self.context,
                descriptor,
                extent,
                self.const_u64(u64::from(align)),
                self.module.const_int(i8_ty, DomainTag::Heap as u128),
                self.const_u64(anchor),
            ],
        );
        let count = self
            .builder
            .offset(block, self.const_u64(COUNT_OFFSET.into()));
        let i32_ty = self.module.int_type(32);
        self.builder.store_access(
            self.module.const_int(i32_ty, COUNT_UNIQUE.into()),
            count,
            4,
            Access::Count,
        );
        Ok(block)
    }

    /// T03 Dup of the counted block `block`, inline below the context's dup bound.
    pub(super) fn dup_block(&mut self, block: Value<'m>) -> Emitted {
        let dup = self.runtime(RuntimeEntry::Dup)?;
        crate::runtime::dup(&self.builder, self.function, dup, self.context, block);
        Ok(())
    }

    /// T03 Dup of the `Shared` root `block`: an inline CAS below the context's dup bound,
    /// else `hirsel_rt_v2_root_dup` (runtime ABI 25).
    pub(super) fn root_dup_block(&mut self, block: Value<'m>) -> Emitted {
        let dup = self.runtime(RuntimeEntry::RootDup)?;
        crate::runtime::root_dup(&self.builder, self.function, dup, self.context, block);
        Ok(())
    }

    /// Whether `value` is a `Shared` root reference, whose count only the root entries touch.
    pub(super) fn is_root_value(&self, value: ValueId) -> Emitted<bool> {
        Ok(matches!(
            self.layout(self.value_type(value)?.layout)?.kind,
            LayoutKind::Shared { .. }
        ))
    }

    /// T13 Reuse of `token` as a heap block of `layout` with `extent` payload bytes: the
    /// block, or `failure` when `hirsel_rt_v2_reuse` reports one.
    pub(super) fn reuse_block(
        &mut self,
        token: Value<'m>,
        layout: LayoutId,
        extent: Value<'m>,
        align: u32,
        failure: FailureId,
    ) -> Emitted<Value<'m>> {
        use hirsel_lir::runtime_abi::{
            COUNT_OFFSET, COUNT_UNIQUE, CTX_FREE_INLINE_OFFSET, DESCRIPTOR_INDEX_MASK,
            LAYOUT_OFFSET,
        };
        let descriptor = self.descriptor(layout)?;
        let (ptr, i32_ty) = (self.module.ptr_type(), self.module.int_type(32));
        let half = |n: u64| self.module.const_int(i32_ty, u128::from(n));
        // A token is installed inline while the context frees inline: the runtime would only
        // check its key and record its state, which diagnostic mode, measurement builds, the
        // system allocator and memcheck keep, and those turn the inline paths off.
        let enabled = self.builder.load(
            i32_ty,
            self.builder
                .offset(self.context, self.const_u64(CTX_FREE_INLINE_OFFSET)),
            4,
        );
        let fast = self.builder.binary(
            BinOp::And,
            self.builder
                .icmp(IntPredicate::Ne, token, self.module.const_zero(ptr)),
            self.builder.icmp(IntPredicate::Ne, enabled, half(0)),
        );
        let [inline, slow, done] = std::array::from_fn(|_| self.module.append_block(self.function));
        self.builder.cond_br(fast, inline, slow);
        self.builder.position(inline);
        self.builder.store_access(
            half(COUNT_UNIQUE),
            self.builder.offset(token, self.const_u64(COUNT_OFFSET)),
            4,
            crate::context::Access::Count,
        );
        let word = self.builder.offset(token, self.const_u64(LAYOUT_OFFSET));
        let kind = self.builder.binary(
            BinOp::And,
            self.builder.load(i32_ty, word, 4),
            half(u64::from(!DESCRIPTOR_INDEX_MASK)),
        );
        self.builder
            .store(self.builder.binary(BinOp::Or, kind, descriptor), word, 4);
        self.builder.br(done);
        self.builder.position(slow);
        let out = self.prologue.alloca(ptr, 8);
        let reuse = self.runtime(RuntimeEntry::Reuse)?;
        let status = self.builder.call(
            reuse,
            &[
                self.context,
                token,
                descriptor,
                extent,
                self.const_u64(u64::from(align)),
                out,
            ],
        );
        self.status_guard(status, failure)?;
        let block = self.builder.load(ptr, out, 8);
        let from = self.builder.current();
        self.builder.br(done);
        self.builder.position(done);
        let phi = self.builder.phi(ptr);
        self.builder.add_incoming(phi, token, inline);
        self.builder.add_incoming(phi, block, from);
        Ok(phi)
    }

    /// A block at an arena token: a counted heap block when the
    /// token is null (`heap`), else an immortal block bump-allocated in that arena. An `Arena`
    /// value can be the inert heap token as well as an open arena record.
    /// `nullable` controls whether this destination admits the heap token. A tracked site's
    /// arena block (a List, Array, Map or Set, MEM-6) is unshared and has its arena's record
    /// before its extent.
    pub(super) fn arena_block(
        &mut self,
        token: Value<'m>,
        nullable: bool,
        site: &'a ConstructSite,
        extent: Value<'m>,
        align: u32,
        flat: bool,
        failure: FailureId,
    ) -> Emitted<Value<'m>> {
        let ptr = self.module.ptr_type();
        let out = self.prologue.alloca(ptr, 8);
        let done = self.module.append_block(self.function);
        if nullable {
            let heap = self
                .builder
                .icmp(IntPredicate::Eq, token, self.module.const_zero(ptr));
            let (on_heap, in_arena) = (
                self.module.append_block(self.function),
                self.module.append_block(self.function),
            );
            self.builder.cond_br(heap, on_heap, in_arena);
            self.builder.position(on_heap);
            let block = self.alloc_block(site.layout, extent, align, site.anchor)?;
            self.builder.store(block, out, 8);
            self.builder.br(done);
            self.builder.position(in_arena);
        }
        // A bounded arena is charged before the bump (MEM-6).
        if self.tracking() {
            let bytes = self.construction_charge(site)?;
            self.charge_construction(token, bytes, site, failure)?;
        }
        // A flat immortal owner's copy may read its capacity rounded to 16, as a region's.
        let capacity = if flat {
            let rounded = self.builder.binary(BinOp::Add, extent, self.const_u64(15));
            self.builder
                .binary(BinOp::And, rounded, self.const_u64(!15))
        } else {
            extent
        };
        let slow = self.module.append_block(self.function);
        let fast = self.module.append_block(self.function);
        let cursor_address = self.builder.offset(
            token,
            self.const_u64(hirsel_lir::runtime_abi::REGION_CURSOR_OFFSET),
        );
        let limit_address = self.builder.offset(
            token,
            self.const_u64(hirsel_lir::runtime_abi::REGION_END_OFFSET),
        );
        let cursor = self.builder.load(ptr, cursor_address, 8);
        let limit = self.builder.load(ptr, limit_address, 8);
        let word = self.module.int_type(64);
        let cursor = self.builder.cast(Cast::PtrToInt, cursor, word);
        let limit = self.builder.cast(Cast::PtrToInt, limit, word);
        let physical = u64::from(align.max(8));
        let record = self.layout(site.layout)?;
        // The cursor is always word aligned (every reservation ends on a word), so a block
        // of a fixed layout (LAYOUT_NO_EXTENT) is its header at the cursor, or at the first
        // padded position for a wider payload alignment, then its payload: one bump and one
        // limit test. A null limit (none yet, or diagnostics) fails the test. A tracked one
        // (a Map's or Set's, MEM-6) has the tracked prefix, its extent words unused.
        let bare = hirsel_lir::rt_program::no_extent(record) && !self.tracks(site);
        let (block, next) = match (bare, &record.size) {
            (true, &SizeSpec::Fixed(size)) => {
                let block = if physical == 8 {
                    cursor
                } else {
                    let payload =
                        self.builder
                            .binary(BinOp::Add, cursor, self.const_u64(8 + physical - 1));
                    let payload =
                        self.builder
                            .binary(BinOp::And, payload, self.const_u64(!(physical - 1)));
                    self.builder.binary(BinOp::Sub, payload, self.const_u64(8))
                };
                let next = self.builder.binary(
                    BinOp::Add,
                    block,
                    self.const_u64(8 + size.next_multiple_of(8)),
                );
                let fits = self.builder.icmp(IntPredicate::Ule, next, limit);
                self.builder.cond_br(fits, fast, slow);
                self.builder.position(fast);
                (block, next)
            }
            _ => {
                // Worst-case payload padding after the immortal prefix. No inbounds GEP
                // or overflowing request reaches a store.
                let prefix = if self.tracks(site) {
                    hirsel_lir::runtime_abi::TRACKED_PREFIX_BYTES
                } else {
                    IMMORTAL_PREFIX_BYTES
                };
                let overhead = prefix + physical - 8;
                let request = self
                    .builder
                    .binary(BinOp::Add, capacity, self.const_u64(overhead));
                let room = self.builder.binary(BinOp::Sub, limit, cursor);
                let in_segment = self.builder.icmp(IntPredicate::Ule, cursor, limit);
                let fits = self.builder.icmp(IntPredicate::Ule, request, room);
                // The runtime rejects larger requests before host-size conversion.
                let bounded = self.builder.icmp(
                    IntPredicate::Ule,
                    capacity,
                    self.const_u64(i64::MAX as u64 / 2 - overhead),
                );
                let fits = self.builder.binary(BinOp::And, in_segment, fits);
                let fits = self.builder.binary(BinOp::And, fits, bounded);
                self.builder.cond_br(fits, fast, slow);
                self.builder.position(fast);
                let payload =
                    self.builder
                        .binary(BinOp::Add, cursor, self.const_u64(prefix + physical - 1));
                let payload =
                    self.builder
                        .binary(BinOp::And, payload, self.const_u64(!(physical - 1)));
                let block = self.builder.binary(BinOp::Sub, payload, self.const_u64(8));
                let rounded = self.builder.binary(BinOp::Add, capacity, self.const_u64(7));
                let rounded = self.builder.binary(BinOp::And, rounded, self.const_u64(!7));
                let next = self.builder.binary(BinOp::Add, payload, rounded);
                let at = self.builder.cast(Cast::IntToPtr, block, ptr);
                let extent_address = self
                    .builder
                    .offset(at, self.const_u64(0u64.wrapping_sub(16)));
                self.builder.store(capacity, extent_address, 8);
                let align_address = self
                    .builder
                    .offset(at, self.const_u64(0u64.wrapping_sub(8)));
                self.builder
                    .store(self.const_u64(physical), align_address, 8);
                if self.tracks(site) {
                    let region_address = self.builder.offset(
                        at,
                        self.const_u64(
                            0u64.wrapping_sub(hirsel_lir::runtime_abi::TRACKED_REGION_BACK),
                        ),
                    );
                    self.builder.store(token, region_address, 8);
                }
                (block, next)
            }
        };
        let next = self.builder.cast(Cast::IntToPtr, next, ptr);
        self.builder.store(next, cursor_address, 8);
        let block = self.builder.cast(Cast::IntToPtr, block, ptr);
        let i32_ty = self.module.int_type(32);
        let count = if self.tracks(site) {
            hirsel_lir::runtime_abi::COUNT_ARENA_UNIQUE
        } else {
            COUNT_IMMORTAL
        };
        self.builder
            .store(self.module.const_int(i32_ty, u128::from(count)), block, 8);
        let descriptor = self.descriptor(site.layout)?;
        let kind = self.module.const_int(
            i32_ty,
            u128::from(hirsel_lir::runtime_abi::KIND_REGION << hirsel_lir::runtime_abi::KIND_SHIFT),
        );
        let descriptor = self.builder.binary(BinOp::Or, descriptor, kind);
        let descriptor_address = self.builder.offset(block, self.const_u64(4));
        self.builder.store(descriptor, descriptor_address, 4);
        self.builder.store(block, out, 8);
        self.builder.br(done);
        self.builder.position(slow);
        let descriptor = self.descriptor(site.layout)?;
        let alloc = self.runtime(RuntimeEntry::ArenaAlloc)?;
        let flags = if self.tracks(site) {
            hirsel_lir::runtime_abi::ALLOC_TRACKED
        } else {
            0
        };
        let block = self.builder.call(
            alloc,
            &[
                self.context,
                token,
                descriptor,
                capacity,
                self.const_u64(u64::from(align)),
                self.module.const_int(i32_ty, u128::from(flags)),
            ],
        );
        self.builder.store(block, out, 8);
        self.builder.br(done);
        self.builder.position(done);
        Ok(self.builder.load(ptr, out, 8))
    }

    /// After an arena block's payload is filled: when the token names an arena and the
    /// payload holds a heap credit (a counted child that is not immortal, or any owned
    /// content other than counted children), the arena schedules the payload's drop program
    /// for its close (MEM-4). A payload of arena children schedules nothing. A tracked
    /// block (MEM-6) with a drop program is always scheduled: pushes may move heap credits
    /// into it.
    pub(super) fn arena_hold(
        &mut self,
        token: Value<'m>,
        nullable: bool,
        payload: Place<'m>,
        layout: LayoutId,
        tracked: bool,
        failure: FailureId,
    ) -> Emitted {
        let record = self.layout(layout)?;
        if record.drop.is_empty() && record.finalizer.is_none() {
            return Ok(());
        }
        let steps = record.drop.clone();
        let finalized = record.finalizer.is_some();
        let ptr = self.module.ptr_type();
        let flag_ty = self.module.int_type(1);
        let flag = self.prologue.alloca(flag_ty, 1);
        self.builder.store(
            self.module.const_int(flag_ty, u128::from(finalized)),
            flag,
            1,
        );
        let (schedule, done) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        if nullable {
            let walk = self.module.append_block(self.function);
            let heap = self
                .builder
                .icmp(IntPredicate::Eq, token, self.module.const_zero(ptr));
            self.builder.cond_br(heap, done, walk);
            self.builder.position(walk);
        }
        if tracked {
            self.builder
                .store(self.module.const_int(flag_ty, 1), flag, 1);
        } else {
            self.arena_credits(payload.ptr, payload.align, &steps, flag)?;
        }
        let holds = self.builder.load(flag_ty, flag, 1);
        self.builder.cond_br(holds, schedule, done);
        self.builder.position(schedule);
        let descriptor = self.descriptor(layout)?;
        let hold = self.runtime(RuntimeEntry::RegionSchedule)?;
        let status = self
            .builder
            .call(hold, &[self.context, token, descriptor, payload.ptr]);
        self.status_guard(status, failure)?;
        self.builder.br(done);
        self.builder.position(done);
        Ok(())
    }

    /// Set `flag` where the drop program `steps` over the bytes at `base` would release a
    /// counted block that is not immortal, or would do anything but release counted children.
    pub(super) fn arena_credits(
        &mut self,
        base: Value<'m>,
        align: u32,
        steps: &[DropStep],
        flag: Value<'m>,
    ) -> Emitted {
        let ptr = self.module.ptr_type();
        let flag_ty = self.module.int_type(1);
        let i32_ty = self.module.int_type(32);
        let at = |this: &Self, offset: u64| {
            if offset == 0 {
                base
            } else {
                this.builder.offset(base, this.const_u64(offset))
            }
        };
        for step in steps {
            match step {
                DropStep::Release { offset } => {
                    let child = self.builder.load(
                        ptr,
                        at(self, *offset),
                        align.min(low_bit(*offset)).min(8),
                    );
                    let word = if hirsel_lir::runtime_abi::COUNT_OFFSET == 0 {
                        child
                    } else {
                        self.builder
                            .offset(child, self.const_u64(hirsel_lir::runtime_abi::COUNT_OFFSET))
                    };
                    let count = self.builder.load(i32_ty, word, 4);
                    // Immortal and tracked arena blocks (MEM-6) hold no credit.
                    let counted = if self.tracking() {
                        self.builder.icmp(
                            IntPredicate::Ult,
                            count,
                            self.module.const_int(
                                i32_ty,
                                u128::from(hirsel_lir::runtime_abi::COUNT_ARENA_SHARED),
                            ),
                        )
                    } else {
                        self.builder.icmp(
                            IntPredicate::Ne,
                            count,
                            self.module.const_int(i32_ty, u128::from(COUNT_IMMORTAL)),
                        )
                    };
                    let old = self.builder.load(flag_ty, flag, 1);
                    self.builder
                        .store(self.builder.binary(BinOp::Or, old, counted), flag, 1);
                }
                DropStep::Child { offset, layout } => {
                    let record = self.layout(*layout)?;
                    if record.finalizer.is_some() {
                        self.builder
                            .store(self.module.const_int(flag_ty, 1), flag, 1);
                    }
                    let steps = record.drop.clone();
                    let base = at(self, *offset);
                    self.arena_credits(base, align.min(low_bit(*offset)), &steps, flag)?;
                }
                DropStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let tag = load_tag(&self.builder, base, align, *tag_offset, *form, false);
                    let join = self.module.append_block(self.function);
                    for (variant, steps) in variants {
                        if steps.is_empty() {
                            continue;
                        }
                        let (yes, next) = (
                            self.module.append_block(self.function),
                            self.module.append_block(self.function),
                        );
                        let active = self.builder.icmp(
                            IntPredicate::Eq,
                            tag,
                            self.module.const_int(i32_ty, u128::from(*variant)),
                        );
                        self.builder.cond_br(active, yes, next);
                        self.builder.position(yes);
                        self.arena_credits(base, align, steps, flag)?;
                        self.builder.br(join);
                        self.builder.position(next);
                    }
                    self.builder.br(join);
                    self.builder.position(join);
                }
                // Finalizers, environments, handles and element loops: schedule.
                _ => self
                    .builder
                    .store(self.module.const_int(flag_ty, 1), flag, 1),
            }
        }
        Ok(())
    }

    /// A block construction: a heap block (allocated, or installed in `token`'s storage), or
    /// uncounted storage reserved in a compiler-local region, whose zeroed payload then
    /// receives the fields. Payload cleanup is scheduled unless C has proved
    /// that its children are uncounted and end with the region.
    pub(super) fn construct_block(
        &mut self,
        result: ValueId,
        site: &'a ConstructSite,
        reuse: Option<(Value<'m>, Option<MaskId>)>,
        failure: FailureId,
    ) -> Emitted {
        let token = reuse.map(|(token, _)| token);
        let repr = self.value_type(result)?.repr;
        let record = self.layout(site.layout)?;
        let align = record.align;
        let flat = record.kind == LayoutKind::Flat && site.constructor != ConstructorKind::Box;
        if flat && reuse.is_some() {
            return self.invalid("a reuse into a flat construction");
        }
        let parts = if flat {
            self.flat_parts(site)?
        } else {
            Vec::new()
        };
        let extent = match (&record.size, record.kind.clone()) {
            (SizeSpec::Fixed(size), _) => *size,
            // A flat record's extent is its parts' bytes, known once they are read.
            (SizeSpec::Dynamic(_), LayoutKind::Flat) if flat => 0,
            (SizeSpec::Dynamic(_), LayoutKind::SoaColumns { element }) => {
                let count = self.pool(site.fields, &self.unit.pools.fields)?.len() as u64;
                let capacity = self.column_capacity(count, element)?;
                let shape = self.soa(element)?;
                let (data, width) = (shape.data_offset, shape.width);
                capacity
                    .checked_mul(width)
                    .and_then(|b| b.checked_add(data))
                    .ok_or_else(|| CodegenError::Invalid {
                        definition: self.definition().clone(),
                        what: "column extent overflow".into(),
                    })?
            }
            (SizeSpec::Dynamic(_), LayoutKind::Sequence { element }) => {
                let count = self.pool(site.fields, &self.unit.pools.fields)?.len() as u64;
                let start = sequence_elements_offset(self.layout(element)?).ok();
                let stride = self.fixed_size(element)?;
                match count
                    .checked_mul(stride)
                    .and_then(|bytes| bytes.checked_add(start?))
                {
                    Some(extent) => extent,
                    None => return self.invalid("a sequence extent beyond u64"),
                }
            }
            // The header; the elements' bytes are known once their tags are read.
            (SizeSpec::Dynamic(_), LayoutKind::Sequential { element }) => {
                self.sequential_offset(element)?
            }
            (SizeSpec::Dynamic(_), _) => {
                return self.unsupported("a dynamic block extent other than a sequence");
            }
        };
        let extent_value = if flat {
            self.flat_extent(&parts)?
        } else if let LayoutKind::Sequential { element } = record.kind {
            let mut bytes = self.const_u64(extent);
            for value in self.pool(site.fields, &self.unit.pools.fields)? {
                let temp = self.stack(element)?;
                self.store(
                    self.value_shape(value.value)?,
                    self.counted_view(value.value)?,
                    temp,
                );
                let extent = self.extent(element, temp.ptr)?;
                bytes = self.builder.binary(BinOp::Add, bytes, extent);
            }
            bytes
        } else {
            self.const_u64(extent)
        };
        // The token of an arena or caller destination, whose credits it may hold.
        let mut arena = None;
        let (value, payload) = match (site.placement, repr) {
            (Some(Placement::Fixed(StorageOwner::Heap(_))), ValueRepr::Block) => {
                let block = match token {
                    None => self.alloc_block(site.layout, extent_value, align, site.anchor)?,
                    Some(token) => {
                        self.reuse_block(token, site.layout, extent_value, align, failure)?
                    }
                };
                let payload = self.payload_of(block, site.layout)?;
                (block, payload)
            }
            (Some(Placement::Fixed(StorageOwner::LocalRegion(region))), ValueRepr::Uncounted)
                if token.is_none() =>
            {
                let Some(&record) = self.regions.get(region.index()) else {
                    return self.invalid("a region outside the region table");
                };
                let payload = self.reserve_region(record, site.layout, extent_value, failure)?;
                let storage = payload.ptr;
                if !site.arena_no_drop && !self.layout(site.layout)?.drop.is_empty() {
                    let descriptor = self.descriptor(site.layout)?;
                    let schedule = self.runtime(RuntimeEntry::RegionSchedule)?;
                    let status = self
                        .builder
                        .call(schedule, &[self.context, record, descriptor, storage]);
                    self.status_guard(status, failure)?;
                }
                (storage, payload)
            }
            (Some(Placement::Fixed(StorageOwner::Arena(id))), ValueRepr::Block)
                if token.is_none() =>
            {
                let Some(instance) = self.unit.tables.arenas.get(id.index()) else {
                    return self.invalid("an arena outside the arena table");
                };
                let Some(Some(slot)) = self.slots.get(instance.slot.index()).copied() else {
                    return self.invalid("an arena token slot without storage");
                };
                let token = self.load(Shape::Scalar(ScalarKind::Pointer), slot);
                let block =
                    self.arena_block(token, true, site, extent_value, align, flat, failure)?;
                arena = Some((token, true));
                (block, self.payload_of(block, site.layout)?)
            }
            (Some(Placement::CallerDestination), ValueRepr::Block) if token.is_none() => {
                let Some(index) = self
                    .signature
                    .hidden_index(|h| matches!(h, Hidden::Destination))
                else {
                    return self.invalid("a caller destination in a function without one");
                };
                let token = self.module.parameter(self.function, index);
                let block =
                    self.arena_block(token, true, site, extent_value, align, flat, failure)?;
                arena = Some((token, true));
                (block, self.payload_of(block, site.layout)?)
            }
            _ => {
                return self
                    .invalid("a block construction whose placement and representation disagree");
            }
        };
        if flat {
            self.write_flat(payload.ptr, &parts)?;
            return self.define(result, value);
        }
        // Reused storage keeps the bytes of each kept field (LIR schema revision 5): neither
        // zeroed nor stored, unless the token was null and the storage is fresh.
        let kept = match reuse {
            Some((_, Some(mask))) => self.kept_fields(site, mask)?,
            _ => Vec::new(),
        };
        let mut ranges = Vec::new();
        for field in &kept {
            ranges.push((field.offset, field.offset + self.fixed_size(field.layout)?));
        }
        ranges.sort_unstable();
        let mut at = 0;
        for (start, end) in ranges.iter().copied().chain([(extent, extent)]) {
            if start > at {
                let gap = self.field_place(payload, at, site.layout, false)?;
                self.builder
                    .memset(gap.ptr, 0, start - at, align.min(low_bit(at)));
            }
            at = at.max(end);
        }
        let skip: Vec<u32> = kept.iter().map(|f| f.ordinal).collect();
        self.fill(payload, site, &skip)?;
        if let Some((token, nullable)) = arena.filter(|_| !site.arena_no_drop || self.tracks(site))
        {
            self.arena_hold(
                token,
                nullable,
                payload,
                site.layout,
                self.tracks(site),
                failure,
            )?;
        }
        if let (Some(token), false) = (token, kept.is_empty()) {
            let null = self.module.const_zero(self.module.ptr_type());
            let fresh = self.builder.icmp(IntPredicate::Eq, token, null);
            let (store, done) = (
                self.module.append_block(self.function),
                self.module.append_block(self.function),
            );
            self.builder.cond_br(fresh, store, done);
            self.builder.position(store);
            for field in kept {
                self.store_field(payload, site, field)?;
            }
            self.builder.br(done);
            self.builder.position(done);
        }
        self.define(result, value)
    }
}
