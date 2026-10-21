use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    // Sequence blocks (LIR schema revision 7).

    /// The payload a block reference layout points at.
    pub(super) fn layout_payload(&self, layout: LayoutId) -> Emitted<Option<LayoutId>> {
        Ok(match self.layout(layout)?.kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => Some(payload),
            _ => None,
        })
    }

    /// The element layout of a counted sequence block layout.
    pub(super) fn sequence_element(&self, layout: LayoutId) -> Emitted<Option<LayoutId>> {
        let Some(payload) = self.layout_payload(layout)? else {
            return Ok(None);
        };
        Ok(match self.layout(payload)?.kind {
            LayoutKind::Sequence { element } | LayoutKind::SoaColumns { element } => Some(element),
            _ => None,
        })
    }

    /// The payload address of a sequence block value and its element layout.
    pub(super) fn sequence_payload(&mut self, value: ValueId) -> Emitted<(Value<'m>, LayoutId)> {
        let layout = self.value_type(value)?.layout;
        let (Some(payload), Some(element)) =
            (self.layout_payload(layout)?, self.sequence_element(layout)?)
        else {
            return self.invalid("a sequence operand that is no sequence block");
        };
        let pointer = self.counted_view(value)?;
        Ok((self.payload_of(pointer, payload)?.ptr, element))
    }

    /// Elements the block's storage holds: its capacity word (runtime ABI 5), one load.
    /// The word is never more than the storage's slots, and never less than the count.
    /// Zero-sized elements report `u64::MAX`.
    pub(super) fn sequence_capacity(&mut self, value: ValueId) -> Emitted<Value<'m>> {
        let (data, element) = self.sequence_payload(value)?;
        let Some(payload) = self.layout_payload(self.value_type(value)?.layout)? else {
            return self.invalid("sequence capacity without payload");
        };
        if self.fixed_size(element)? == 0
            && !matches!(self.layout(payload)?.kind, LayoutKind::SoaColumns { .. })
        {
            return Ok(self.const_u64(u64::MAX));
        }
        let at = self
            .builder
            .offset(data, self.const_u64(SEQUENCE_CAPACITY_OFFSET));
        Ok(self
            .builder
            .load_access(self.module.int_type(64), at, 8, Access::Capacity))
    }

    // FlatFormatV0 (LIR schema revision 4).

    /// The u64 at `at`, which flat values store unaligned.
    pub(super) fn load_u64(&self, at: Value<'m>) -> Value<'m> {
        self.builder.load(self.module.int_type(64), at, 1)
    }

    /// The element of a flat sequence encoding (List, Array, String, Bytes) and whether its
    /// elements are variable (each its own prefixed encoding, after the prefix and count).
    pub(super) fn flat_sequence(&self, layout: LayoutId) -> Emitted<(LayoutId, bool)> {
        let record = self.layout(layout)?;
        match (&record.kind, &record.size, record.fields.as_slice()) {
            (LayoutKind::Flat, SizeSpec::Dynamic(_), [only]) if only.offset == 16 => Ok((
                only.layout,
                !matches!(self.layout(only.layout)?.size, SizeSpec::Fixed(_)),
            )),
            (LayoutKind::Flat, SizeSpec::Dynamic(_), [only])
                if only.offset == 8
                    && matches!(self.layout(only.layout)?.size, SizeSpec::Fixed(_)) =>
            {
                Ok((only.layout, false))
            }
            _ => self.invalid("a flat part that is no sequence"),
        }
    }

    /// The element count of the flat sequence encoding at `place`: stored after the prefix
    /// for variable elements, else the prefix's byte length over the element size.
    pub(super) fn flat_count(&self, place: Place<'m>) -> Emitted<Value<'m>> {
        let (element, variable) = self.flat_sequence(place.layout)?;
        if variable || self.fixed_size(element)? == 0 {
            return Ok(self.load_u64(self.builder.offset(place.ptr, self.const_u64(8))));
        }
        let size = self.fixed_size(element)?;
        let bytes = self.load_u64(place.ptr);
        Ok(self
            .builder
            .binary(BinOp::UDiv, bytes, self.const_u64(size)))
    }

    /// The address of the `index`th prefixed encoding from `first`, each `8 + prefix` bytes.
    pub(super) fn flat_walk(&self, first: Value<'m>, index: Value<'m>) -> Value<'m> {
        let (i64_ty, ptr_ty) = (self.module.int_type(64), self.module.ptr_type());
        let entry = self.builder.current();
        let (head, body, exit) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        self.builder.br(head);
        self.builder.position(head);
        let k = self.builder.phi(i64_ty);
        let cursor = self.builder.phi(ptr_ty);
        let more = self.builder.icmp(IntPredicate::Ult, k, index);
        self.builder.cond_br(more, body, exit);
        self.builder.position(body);
        let length = self.load_u64(cursor);
        let past = self.builder.offset(cursor, self.const_u64(8));
        let next = self.builder.offset(past, length);
        let k1 = self.builder.binary(BinOp::Add, k, self.const_u64(1));
        self.builder.br(head);
        self.builder.add_incoming(k, self.const_u64(0), entry);
        self.builder.add_incoming(k, k1, body);
        self.builder.add_incoming(cursor, first, entry);
        self.builder.add_incoming(cursor, next, body);
        self.builder.position(exit);
        cursor
    }

    /// The payload offset of a sequential sequence's first element (MEM-13): past its
    /// count and used-bytes words, aligned to the element.
    pub(super) fn sequential_offset(&self, element: LayoutId) -> Emitted<u64> {
        match sequential_elements_offset(self.layout(element)?) {
            Ok(offset) => Ok(offset),
            Err(_) => self.invalid("a sequential element offset beyond u64"),
        }
    }

    pub(super) fn sequential_first(
        &self,
        payload: Value<'m>,
        element: LayoutId,
    ) -> Emitted<Value<'m>> {
        let offset = self.sequential_offset(element)?;
        Ok(self.builder.offset(payload, self.const_u64(offset)))
    }

    /// The element `index` elements past `first` in a sequential sequence: each step
    /// passes one element's active extent.
    pub(super) fn sequential_walk(
        &self,
        first: Value<'m>,
        element: LayoutId,
        index: Value<'m>,
    ) -> Emitted<Value<'m>> {
        let (i64_ty, ptr_ty) = (self.module.int_type(64), self.module.ptr_type());
        let entry = self.builder.current();
        let (head, body, exit) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        self.builder.br(head);
        self.builder.position(head);
        let k = self.builder.phi(i64_ty);
        let cursor = self.builder.phi(ptr_ty);
        let more = self.builder.icmp(IntPredicate::Ult, k, index);
        self.builder.cond_br(more, body, exit);
        self.builder.position(body);
        let bytes = self.extent(element, cursor)?;
        let next = self.builder.offset(cursor, bytes);
        let k1 = self.builder.binary(BinOp::Add, k, self.const_u64(1));
        let latch = self.builder.current();
        self.builder.br(head);
        self.builder.add_incoming(k, self.const_u64(0), entry);
        self.builder.add_incoming(k, k1, latch);
        self.builder.add_incoming(cursor, first, entry);
        self.builder.add_incoming(cursor, next, latch);
        self.builder.position(exit);
        Ok(cursor)
    }

    /// Stores the dynamic Packed `value` at `at` within its active extent, which it
    /// returns: a sequential element holds no bytes past it (MEM-13).
    pub(super) fn store_packed(
        &self,
        value: ValueId,
        element: LayoutId,
        at: Value<'m>,
    ) -> Emitted<Value<'m>> {
        let temp = self.stack(element)?;
        self.store(self.value_shape(value)?, self.get(value)?, temp);
        let bytes = self.extent(element, temp.ptr)?;
        self.builder
            .memcpy(at, temp.align, temp.ptr, temp.align, bytes);
        Ok(bytes)
    }

    /// `load`, except that a dynamic Packed value reads only its active extent: as an
    /// element of a sequential sequence it may end its storage (MEM-13).
    pub(super) fn load_bounded(&self, shape: Shape, place: Place<'m>) -> Emitted<Value<'m>> {
        let place = self.gather(place)?;
        if hirsel_lir::layout::packed_extents(self.layout(place.layout)?).is_none() {
            return Ok(self.load(shape, place));
        }
        let temp = self.stack(place.layout)?;
        let bytes = self.extent(place.layout, place.ptr)?;
        self.builder
            .memcpy(temp.ptr, temp.align, place.ptr, place.align, bytes);
        Ok(self.load(shape, temp))
    }

    /// The byte extent of the value of `layout` at `at`: its fixed size, or its size
    /// program run over the stored bytes, as `hirsel_lir::layout::extent` reads it.
    pub(super) fn extent(&self, layout: LayoutId, at: Value<'m>) -> Emitted<Value<'m>> {
        match &self.layout(layout)?.size {
            SizeSpec::Fixed(size) => Ok(self.const_u64(*size)),
            SizeSpec::Dynamic(program) => self.size_program(program, at),
        }
    }

    pub(super) fn size_program(&self, program: &[SizeTerm], at: Value<'m>) -> Emitted<Value<'m>> {
        let i64_ty = self.module.int_type(64);
        let mut stack: Vec<Value<'m>> = Vec::new();
        let pop = |stack: &mut Vec<Value<'m>>| match stack.pop() {
            Some(v) => Ok(v),
            None => self.invalid("a size program that underflows"),
        };
        for term in program {
            let value = match term {
                SizeTerm::Constant(n) => self.const_u64(*n),
                SizeTerm::ReadLength { offset, scalar } => {
                    let Some((width, false)) = scalar::integer(*scalar) else {
                        return self.invalid("a length that is not an unsigned integer");
                    };
                    if width > 64 {
                        return self.unsupported("a length wider than u64");
                    }
                    let length = self.builder.load(
                        self.module.int_type(width),
                        self.builder.offset(at, self.const_u64(*offset)),
                        1,
                    );
                    if width < 64 {
                        self.builder.cast(Cast::ZExt, length, i64_ty)
                    } else {
                        length
                    }
                }
                SizeTerm::Add | SizeTerm::Multiply => {
                    let (b, a) = (pop(&mut stack)?, pop(&mut stack)?);
                    let op = match term {
                        SizeTerm::Add => BinOp::Add,
                        _ => BinOp::Mul,
                    };
                    self.builder.binary(op, a, b)
                }
                SizeTerm::AlignUp(n) => {
                    if !n.is_power_of_two() {
                        return self.invalid("a size alignment that is not a power of two");
                    }
                    let a = pop(&mut stack)?;
                    let bumped = self.builder.binary(BinOp::Add, a, self.const_u64(n - 1));
                    self.builder
                        .binary(BinOp::And, bumped, self.const_u64(!(n - 1)))
                }
                SizeTerm::Dup => match stack.last() {
                    Some(&top) => top,
                    None => return self.invalid("a size program that underflows"),
                },
                SizeTerm::At(inner) => {
                    let offset = pop(&mut stack)?;
                    self.size_program(inner, self.builder.offset(at, offset))?
                }
                SizeTerm::ByTag {
                    tag_offset,
                    variants,
                } => {
                    let tag = self.builder.load(
                        self.module.int_type(32),
                        self.builder.offset(at, self.const_u64(*tag_offset)),
                        1,
                    );
                    let done = self.module.append_block(self.function);
                    let bad = self.module.append_block(self.function);
                    let here = self.builder.current();
                    let mut cases = Vec::new();
                    let mut incoming = Vec::new();
                    for (variant, inner) in variants {
                        let arm = self.module.append_block(self.function);
                        self.builder.position(arm);
                        let size = self.size_program(inner, at)?;
                        incoming.push((size, self.builder.current()));
                        self.builder.br(done);
                        cases.push((
                            self.module
                                .const_int(self.module.int_type(32), u128::from(*variant)),
                            arm,
                        ));
                    }
                    // A tag without a size is an invariant failure, never a guess.
                    self.builder.position(bad);
                    self.builder.unreachable();
                    self.builder.position(here);
                    self.builder.switch(tag, bad, &cases);
                    self.builder.position(done);
                    let size = self.builder.phi(i64_ty);
                    for (value, from) in incoming {
                        self.builder.add_incoming(size, value, from);
                    }
                    size
                }
            };
            stack.push(value);
        }
        match stack.as_slice() {
            [size] => Ok(*size),
            _ => self.invalid("a size program that leaves no single size"),
        }
    }

    /// The bytes a `Slice { u8 }` value shows: its owner's sequence payload from `8 + start`,
    /// and its length.
    pub(super) fn slice_bytes(&self, slice: ValueId) -> Emitted<(Value<'m>, Value<'m>)> {
        let layout = self.value_type(slice)?.layout;
        let record = self.layout(layout)?;
        let LayoutKind::Slice { element } = record.kind else {
            return self.invalid("an encoded byte source that is not a slice");
        };
        let place = self.stack(layout)?;
        self.store(self.value_shape(slice)?, self.get(slice)?, place);
        let word = |ordinal: u32| match record.fields.iter().find(|f| f.ordinal == ordinal) {
            Some(field) => Ok(self.builder.offset(place.ptr, self.const_u64(field.offset))),
            None => self.invalid("a slice layout without its words"),
        };
        let owner = self.builder.load(self.module.ptr_type(), word(0)?, 8);
        let (start, length) = (self.load_u64(word(1)?), self.load_u64(word(2)?));
        let Some(storage) =
            slice_storage_with_index(self.inputs.layouts, self.inputs.index, element)
        else {
            return self.invalid("a slice without its element's sequence layout");
        };
        let payload = self.payload_of(owner, storage)?;
        let data = self.builder.offset(payload.ptr, self.const_u64(8));
        Ok((self.builder.offset(data, start), length))
    }

    /// The place of a sequential sequence block an append writes, its payload
    /// and element layouts.
    pub(super) fn sequential_place(
        &self,
        address: AddressId,
    ) -> Emitted<(Place<'m>, LayoutId, LayoutId)> {
        let place = self.place(address)?;
        let Some(payload) = self.layout_payload(place.layout)? else {
            return self.invalid("an append to a place that holds no block");
        };
        let LayoutKind::Sequential { element } = self.layout(payload)?.kind else {
            return self.invalid("an append to a block that is not sequential");
        };
        Ok((place, payload, element))
    }

    /// Room for `bytes` more past the used bytes of the sequential block at
    /// `place`, growing the storage geometrically when it is full (MEM-13): the
    /// payload address and the used bytes before the append.
    pub(super) fn sequential_room(
        &mut self,
        place: Place<'m>,
        payload: LayoutId,
        first: u64,
        bytes: Value<'m>,
    ) -> Emitted<(Value<'m>, Value<'m>)> {
        let (old, _, used) = self.sequential_used(place, payload)?;
        let keep = self.builder.binary(BinOp::Add, used, self.const_u64(first));
        let need = self.builder.binary(BinOp::Add, keep, bytes);
        let data = self.sequential_grow(place, payload, old, keep, need)?;
        Ok((data, used))
    }

    /// The block the sequential place holds, its payload address and its used bytes.
    pub(super) fn sequential_used(
        &self,
        place: Place<'m>,
        payload: LayoutId,
    ) -> Emitted<(Value<'m>, Value<'m>, Value<'m>)> {
        let old = self.load(Shape::Scalar(ScalarKind::Pointer), place);
        let data = self.payload_of(old, payload)?.ptr;
        let used = self.builder.load_access(
            self.module.int_type(64),
            self.builder.offset(data, self.const_u64(8)),
            8,
            Access::Used,
        );
        Ok((old, data, used))
    }

    /// Room for `need` bytes from the payload's start of the sequential block
    /// `old` at `place`, growing it geometrically when it is full and keeping
    /// its first `keep` bytes: the payload address.
    pub(super) fn sequential_grow(
        &mut self,
        place: Place<'m>,
        payload: LayoutId,
        old: Value<'m>,
        keep: Value<'m>,
        need: Value<'m>,
    ) -> Emitted<Value<'m>> {
        let align = u64::from(self.layout(payload)?.align);
        let ptr_ty = self.module.ptr_type();
        let usable = crate::runtime::capacity(&self.builder, self.function, old);
        let full = self.builder.icmp(IntPredicate::Ugt, need, usable);
        let here = self.builder.current();
        let (grow, done) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        self.builder.cond_br(full, grow, done);
        self.builder.position(grow);
        let twice = self.builder.binary(BinOp::Add, usable, usable);
        let more = self.builder.icmp(IntPredicate::Ugt, need, twice);
        let wanted = self.builder.select(more, need, twice);
        let entry = self.runtime(RuntimeEntry::BlockGrow)?;
        let grown = self.builder.call(
            entry,
            &[self.context, old, keep, wanted, self.const_u64(align)],
        );
        let grew = self.builder.current();
        self.builder.br(done);
        self.builder.position(done);
        let block = self.builder.phi(ptr_ty);
        self.builder.add_incoming(block, old, here);
        self.builder.add_incoming(block, grown, grew);
        self.store(Shape::Scalar(ScalarKind::Pointer), block, place);
        Ok(self.payload_of(block, payload)?.ptr)
    }

    /// Growth copies the initialized header unchanged. The returned pointer
    /// may differ, so LLVM needs this fact again after the opaque runtime call.
    pub(super) fn preserved_builder_used(&self, data: Value<'m>, used: Value<'m>) -> Emitted {
        let after = self.builder.load_access(
            self.module.int_type(64),
            self.builder.offset(data, self.const_u64(8)),
            8,
            Access::Used,
        );
        self.builder
            .assume(self.builder.icmp(IntPredicate::Eq, after, used))
            .or_else(|e| self.invalid(e))
    }

    /// Where the length word of the open element's field `field` lies in a
    /// builder's element of layout `element` (LIR schema revision 24): past
    /// the prefix and the fixed fields before it.
    pub(super) fn open_word(&self, element: LayoutId, field: u32) -> Emitted<u64> {
        let record = self.layout(element)?;
        let content = match record.fields.as_slice() {
            [only] if record.kind == LayoutKind::Flat && only.offset == 8 => only.layout,
            _ => return self.invalid("an open element of an unprefixed builder"),
        };
        let content = self.layout(content)?;
        let Some(at) = content.fields.iter().find(|f| f.ordinal == field) else {
            return self.invalid("an open element of a missing field");
        };
        for earlier in content.fields.iter().filter(|f| f.ordinal < field) {
            if !matches!(self.layout(earlier.layout)?.size, SizeSpec::Fixed(_)) {
                return self.invalid("an open field after a variable one");
            }
        }
        Ok(8 + at.offset)
    }

    /// One more element of `bytes` in the sequential payload at `data`.
    pub(super) fn sequential_grew(&self, data: Value<'m>, used: Value<'m>, bytes: Value<'m>) {
        let count = self.load_u64(data);
        let count = self.builder.binary(BinOp::Add, count, self.const_u64(1));
        self.builder.store(count, data, 8);
        let used = self.builder.binary(BinOp::Add, used, bytes);
        self.builder.store_access(
            used,
            self.builder.offset(data, self.const_u64(8)),
            8,
            Access::Used,
        );
    }

    /// The FlatFormatV0 parts of a flat construction in field order: a fixed field's value,
    /// or an encoded source's bytes (a String or Bytes slice, or a variable flat record's
    /// block payload) behind its u64 length prefix. Encoded sources are only read.
    pub(super) fn flat_parts(&mut self, site: &'a ConstructSite) -> Emitted<Vec<FlatPart<'m>>> {
        self.flat_parts_around(site, None)
    }

    /// `flat_parts` of a construction completing an open element: `open` is
    /// the field built in place and its length (LIR schema revision 24).
    pub(super) fn flat_parts_around(
        &mut self,
        site: &'a ConstructSite,
        open: Option<(u32, Value<'m>)>,
    ) -> Emitted<Vec<FlatPart<'m>>> {
        let record = self.layout(site.layout)?;
        let values = self.pool(site.fields, &self.unit.pools.fields)?;
        let mut declared: Vec<&FieldLayout> = record.fields.iter().collect();
        declared.sort_by_key(|f| f.ordinal);
        if declared.len() != values.len() + usize::from(open.is_some()) {
            return self.invalid("a flat construction without a value for every field");
        }
        let mut parts = Vec::new();
        for field in declared {
            if let Some((ordinal, length)) = open
                && field.ordinal == ordinal
            {
                parts.push(FlatPart::InPlace { length });
                continue;
            }
            let Some(value) = values.iter().find(|v| v.field == field.ordinal) else {
                return self.invalid("a flat construction without a value for its field");
            };
            let source = self.value_type(value.value)?.layout;
            let field_record = self.layout(field.layout)?;
            if let SizeSpec::Fixed(size) = field_record.size {
                parts.push(FlatPart::Fixed {
                    value: value.value,
                    size,
                });
                continue;
            }
            if let LayoutKind::Block { payload } = self.layout(source)?.kind
                && let LayoutKind::Sequential { element } = self.layout(payload)?.kind
            {
                let data = self.payload_of(self.get(value.value)?, payload)?.ptr;
                let first = self.sequential_offset(element)?;
                parts.push(FlatPart::Built { data, first });
                continue;
            }
            if let LayoutKind::Block { payload } = self.layout(source)?.kind
                && matches!(self.layout(payload)?.kind, LayoutKind::Sequence { .. })
            {
                let source = if self.value_type(value.value)?.repr == ValueRepr::Uncounted {
                    self.uncounted_payload(self.get(value.value)?, payload)?
                } else {
                    self.payload_of(self.get(value.value)?, payload)?
                };
                parts.push(FlatPart::Sequence {
                    source,
                    encoding: field.layout,
                });
                continue;
            }
            let content = match field_record.fields.as_slice() {
                [only] if field_record.kind == LayoutKind::Flat && only.offset == 8 => only.layout,
                _ => return self.unsupported("encoding a list into a flat field"),
            };
            let (bytes, length) = match self.layout(source)?.kind {
                LayoutKind::Slice { .. } => self.slice_bytes(value.value)?,
                LayoutKind::Block { payload } | LayoutKind::Box { payload }
                    if payload == content =>
                {
                    let at = self.payload_of(self.get(value.value)?, payload)?;
                    (at.ptr, self.extent(payload, at.ptr)?)
                }
                _ => return self.invalid("a flat field source that has no encoding"),
            };
            parts.push(FlatPart::Encoded { bytes, length });
        }
        Ok(parts)
    }

    /// The byte extent of `parts` once written.
    pub(super) fn flat_extent(&self, parts: &[FlatPart<'m>]) -> Emitted<Value<'m>> {
        let mut total = self.const_u64(0);
        for part in parts {
            let size = match *part {
                FlatPart::Fixed { size, .. } => self.const_u64(size),
                FlatPart::Encoded { length, .. } => self.flat_add(length, self.const_u64(8))?,
                FlatPart::Sequence { source, encoding } => {
                    self.flat_encode_sequence(source, encoding, None)?
                }
                FlatPart::Built { data, .. } => {
                    let used = self.load_u64(self.builder.offset(data, self.const_u64(8)));
                    self.flat_add(used, self.const_u64(16))?
                }
                FlatPart::InPlace { length } => self.flat_add(length, self.const_u64(8))?,
            };
            total = self.flat_add(total, size)?;
        }
        Ok(total)
    }

    /// Writes `parts` one after another from `at`.
    pub(super) fn write_flat(&self, at: Value<'m>, parts: &[FlatPart<'m>]) -> Emitted {
        let mut cursor = at;
        for part in parts {
            match *part {
                FlatPart::Fixed { value, size } => {
                    let layout = self.value_type(value)?.layout;
                    let place = Place {
                        columns: None,
                        ptr: cursor,
                        layout,
                        align: 1,
                        block: None,
                    };
                    self.store(self.value_shape(value)?, self.get(value)?, place);
                    cursor = self.builder.offset(cursor, self.const_u64(size));
                }
                FlatPart::Encoded { bytes, length } => {
                    self.builder.store(length, cursor, 1);
                    let data = self.builder.offset(cursor, self.const_u64(8));
                    self.builder.memcpy(data, 1, bytes, 1, length);
                    cursor = self.builder.offset(data, length);
                }
                FlatPart::Sequence { source, encoding } => {
                    let size = self.flat_encode_sequence(source, encoding, Some(cursor))?;
                    cursor = self.builder.offset(cursor, size);
                }
                FlatPart::Built { data, first } => {
                    let count = self.load_u64(data);
                    let used = self.load_u64(self.builder.offset(data, self.const_u64(8)));
                    let length = self.builder.binary(BinOp::Add, used, self.const_u64(8));
                    self.builder.store(length, cursor, 1);
                    self.builder
                        .store(count, self.builder.offset(cursor, self.const_u64(8)), 1);
                    let to = self.builder.offset(cursor, self.const_u64(16));
                    let from = self.builder.offset(data, self.const_u64(first));
                    self.builder.memcpy(to, 1, from, 1, used);
                    cursor = self.builder.offset(to, used);
                }
                FlatPart::InPlace { length } => {
                    let data = self.builder.offset(cursor, self.const_u64(8));
                    cursor = self.builder.offset(data, length);
                }
            }
        }
        Ok(())
    }

    /// Size overflow is an implementation invariant fault, before allocating or
    /// writing an extent. It must not wrap into an undersized allocation.
    pub(super) fn flat_add(&self, left: Value<'m>, right: Value<'m>) -> Emitted<Value<'m>> {
        let (sum, overflow) = scalar::int_arith(
            &self.builder,
            BinaryOp::Add,
            ArithMode::Checked,
            ScalarKind::U64,
            left,
            right,
        )
        .or_else(|e| self.invalid(e))?;
        let bad = self.module.append_block(self.function);
        let good = self.module.append_block(self.function);
        self.builder
            .cond_br(overflow.expect("checked add flag"), bad, good);
        self.builder.position(bad);
        let trap = self
            .module
            .intrinsic("llvm.trap", &[])
            .or_else(|e| self.invalid(e))?;
        self.builder.call(trap, &[]);
        self.builder.unreachable();
        self.builder.position(good);
        Ok(sum)
    }

    /// Size and optionally write an ordinary sequence at its flat encoding.
    /// Both walks read the same borrowed, stable source; no element credit moves.
    pub(super) fn flat_encode_sequence(
        &self,
        source: Place<'m>,
        encoding: LayoutId,
        output: Option<Value<'m>>,
    ) -> Emitted<Value<'m>> {
        let LayoutKind::Sequence { element } = self.layout(source.layout)?.kind else {
            return self.invalid("a flat sequence source without sequence storage");
        };
        let (encoded_element, variable) = self.flat_sequence(encoding)?;
        let counted = variable || self.fixed_size(encoded_element)? == 0;
        let count = self.load_u64(source.ptr);
        let stride = self.fixed_size(element)?;
        let start = sequence_elements_offset(self.layout(element)?)
            .or_else(|e| self.invalid(format!("{e:?}")))?;
        let entry = self.builder.current();
        let head = self.module.append_block(self.function);
        let body = self.module.append_block(self.function);
        let exit = self.module.append_block(self.function);
        self.builder.br(head);
        self.builder.position(head);
        let index = self.builder.phi(self.module.int_type(64));
        let total = self.builder.phi(self.module.int_type(64));
        let more = self.builder.icmp(IntPredicate::Ult, index, count);
        self.builder.cond_br(more, body, exit);
        self.builder.position(body);
        let offset = self
            .builder
            .binary(BinOp::Mul, index, self.const_u64(stride));
        let base = self.builder.offset(source.ptr, self.const_u64(start));
        let child = Place {
            columns: None,
            ptr: self.builder.offset(base, offset),
            layout: element,
            align: self.layout(element)?.align,
            block: source.block,
        };
        let at = output.map(|p| self.builder.offset(p, total));
        let size = self.flat_encode_element(child, encoded_element, at)?;
        let next_total = self.flat_add(total, size)?;
        let next_index = self.builder.binary(BinOp::Add, index, self.const_u64(1));
        let back = self.builder.current();
        self.builder.br(head);
        self.builder.add_incoming(index, self.const_u64(0), entry);
        self.builder.add_incoming(index, next_index, back);
        self.builder
            .add_incoming(total, self.const_u64(if counted { 16 } else { 8 }), entry);
        self.builder.add_incoming(total, next_total, back);
        self.builder.position(exit);
        if let Some(output) = output {
            let length = self.builder.binary(BinOp::Sub, total, self.const_u64(8));
            self.builder.store(length, output, 1);
            if counted {
                self.builder
                    .store(count, self.builder.offset(output, self.const_u64(8)), 1);
            }
        }
        Ok(total)
    }

    /// Encode one source element at the layout C has independently checked.
    pub(super) fn flat_encode_element(
        &self,
        source: Place<'m>,
        encoding: LayoutId,
        output: Option<Value<'m>>,
    ) -> Emitted<Value<'m>> {
        if let SizeSpec::Fixed(size) = self.layout(encoding)?.size {
            if let Some(output) = output {
                self.builder
                    .memcpy(output, 1, source.ptr, source.align, self.const_u64(size));
            }
            return Ok(self.const_u64(size));
        }
        let [content] = self.layout(encoding)?.fields.as_slice() else {
            return self.invalid("a flat element encoding without its content");
        };
        let (bytes, length) = match self.layout(source.layout)?.kind {
            LayoutKind::Block { payload } => {
                let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), source);
                let payload_place = self.payload(pointer, source.layout)?;
                if matches!(self.layout(payload)?.kind, LayoutKind::Sequence { .. }) {
                    return self.flat_encode_sequence(payload_place, encoding, output);
                }
                if content.offset != 8 || payload != content.layout {
                    return self.invalid("a flat element source of another payload layout");
                }
                (payload_place.ptr, self.extent(payload, payload_place.ptr)?)
            }
            LayoutKind::Slice { element } => {
                let record = self.layout(source.layout)?;
                let word = |ordinal| {
                    record
                        .fields
                        .iter()
                        .find(|f| f.ordinal == ordinal)
                        .map(|f| self.builder.offset(source.ptr, self.const_u64(f.offset)))
                        .ok_or_else(|| CodegenError::Invalid {
                            definition: self.definition().clone(),
                            what: "a flat slice source without its words".into(),
                        })
                };
                let owner = self
                    .builder
                    .load(self.module.ptr_type(), word(0)?, source.align);
                let start = self.load_u64(word(1)?);
                let length = self.load_u64(word(2)?);
                let storage =
                    slice_storage_with_index(self.inputs.layouts, self.inputs.index, element)
                        .ok_or_else(|| CodegenError::Invalid {
                            definition: self.definition().clone(),
                            what: "a flat slice source without byte storage".into(),
                        })?;
                let payload = self.payload_of(owner, storage)?;
                let data = self.builder.offset(payload.ptr, self.const_u64(8));
                (self.builder.offset(data, start), length)
            }
            _ => return self.invalid("a flat element source without an encoding"),
        };
        let size = self.flat_add(length, self.const_u64(8))?;
        if let Some(output) = output {
            self.builder.store(length, output, 1);
            let data = self.builder.offset(output, self.const_u64(8));
            self.builder.memcpy(data, 1, bytes, 1, length);
        }
        Ok(size)
    }
}
