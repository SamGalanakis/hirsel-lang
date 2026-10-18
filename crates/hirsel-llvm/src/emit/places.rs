use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(super) fn place(&self, address: AddressId) -> Emitted<Place<'m>> {
        match self.addresses.get(address.index()).copied().flatten() {
            Some(place) => Ok(place),
            None => self.invalid(format!("address {} used before its preparation", address.0)),
        }
    }

    /// Computes an address from its root, running every bounds and tag check on the way;
    /// any failing check takes `failure` before the address is used.
    pub(super) fn prepare(&mut self, address: AddressId, failure: FailureId) -> Emitted<Place<'m>> {
        let Some(info) = self.unit.addresses.get(address.index()) else {
            return self.invalid("no such address");
        };
        let mut place = match info.base {
            AddressBase::Slot(slot) => match self.slots.get(slot.index()).copied().flatten() {
                Some(place) => place,
                None => return self.invalid("an address of a slot without storage"),
            },
            AddressBase::Value(value) => {
                let ty = self.value_type(value)?;
                let layout = ty.layout;
                let pointer = self.get(value)?;
                if let LayoutKind::Block { payload } | LayoutKind::Box { payload } =
                    self.layout(layout)?.kind
                {
                    // An uncounted value (region storage) points at its payload whatever its
                    // layout's holders are.
                    if ty.repr == ValueRepr::Uncounted {
                        self.uncounted_payload(pointer, payload)?
                    } else {
                        self.payload(pointer, layout)?
                    }
                } else {
                    // A pointer to a value of its own layout (a `mut` capture's address).
                    Place {
                        columns: None,
                        ptr: pointer,
                        layout,
                        align: self.layout(layout)?.align,
                        block: None,
                    }
                }
            }
            AddressBase::Address(base) => {
                // Bases are prepared from scratch along with their checks; the validator
                // rejects address cycles.
                self.prepare(base, failure)?
            }
        };
        let mut steps = self.pool(info.projection, &self.unit.pools.projections)?;
        // A slot of an uncounted type (region storage) holds a pointer at its payload,
        // whatever its layout's holders are: a projection starts there, as one through an
        // uncounted value does.
        if let (AddressBase::Slot(slot), false) = (info.base, steps.is_empty()) {
            let ty = match self.unit.slots.get(slot.index()) {
                Some(info) => self.lir_type(info.ty)?,
                None => return self.invalid("no such slot"),
            };
            if let (
                ValueRepr::Uncounted,
                LayoutKind::Block { payload } | LayoutKind::Box { payload },
            ) = (ty.repr, self.layout(ty.layout)?.kind.clone())
            {
                let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                place = self.uncounted_payload(pointer, payload)?;
                if steps.first() == Some(&ProjectionStep::Payload) {
                    steps = &steps[1..];
                }
            }
        }
        let mut known = match info.base {
            AddressBase::Value(value) => self.preserved_lengths[value.index()],
            _ => None,
        };
        let skipped = info.projection.len as usize - steps.len();
        for (i, &step) in steps.iter().enumerate() {
            self.known_count = known.take();
            place = self.project(
                place,
                step,
                failure,
                info.unchecked.contains(&((i + skipped) as u32)),
            )?;
        }
        self.known_count = None;
        if place.layout != self.lir_type(info.ty)?.layout {
            return self.invalid("an address whose projection does not reach its type");
        }
        Ok(place)
    }

    /// A value as its layout reads it: an uncounted reference (region storage) at a counted
    /// block layout is rebased to where that block's header would be, so code that adds the
    /// payload offset reaches its payload. Only the address is computed; no header is read.
    pub(super) fn counted_view(&self, value: ValueId) -> Emitted<Value<'m>> {
        let ty = self.value_type(value)?;
        let v = self.get(value)?;
        let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) =
            self.layout(ty.layout)?.kind
        else {
            return Ok(v);
        };
        let counted = !self
            .layout(ty.layout)?
            .scan
            .iter()
            .any(|step| matches!(step, ScanStep::Uncounted { .. }));
        if ty.repr != ValueRepr::Uncounted || !counted {
            return Ok(v);
        }
        let offset = match payload_offset(self.layout(payload)?.align) {
            Ok(offset) => offset,
            Err(error) => return self.invalid(format!("{error:?}")),
        };
        let back = self
            .module
            .const_int(self.module.int_type(64), (offset as u128).wrapping_neg());
        Ok(self.builder.offset(v, back))
    }

    /// Stored fields follow their logical layout's pointer encoding. A read into
    /// an uncounted SSA value reverses the address-only bridge used by counted_view.
    pub(super) fn read_uncounted(
        &self,
        value: Value<'m>,
        ty: LirTypeId,
        address: AddressId,
    ) -> Emitted<Value<'m>> {
        let ty = self.lir_type(ty)?;
        if self.whole_slot(address).is_some() || ty.repr != ValueRepr::Uncounted {
            return Ok(value);
        }
        let layout = self.layout(ty.layout)?;
        if let LayoutKind::Block { payload } | LayoutKind::Box { payload } = layout.kind
            && !layout
                .scan
                .iter()
                .any(|s| matches!(s, ScanStep::Uncounted { .. }))
        {
            return Ok(self.payload_of(value, payload)?.ptr);
        }
        Ok(value)
    }

    /// Regional flat storage includes an immortal header before its payload.
    /// Other uncounted storage has no block that a view can credit.
    pub(super) fn uncounted_payload(
        &self,
        pointer: Value<'m>,
        payload: LayoutId,
    ) -> Emitted<Place<'m>> {
        let record = self.layout(payload)?;
        let block = if record.kind == LayoutKind::Flat {
            let offset = match payload_offset(record.align) {
                Ok(offset) => offset,
                Err(error) => return self.invalid(format!("{error:?}")),
            };
            let back = self
                .module
                .const_int(self.module.int_type(64), (offset as u128).wrapping_neg());
            Some(self.builder.offset(pointer, back))
        } else {
            None
        };
        Ok(Place {
            columns: None,
            ptr: pointer,
            layout: payload,
            align: record.align,
            block,
        })
    }

    /// The payload a block reference value or field points at. A counted block's payload
    /// follows its header; an uncounted reference points at its payload directly.
    /// Regional flat payloads carry a real immortal header before that address.
    pub(super) fn payload(&self, pointer: Value<'m>, layout: LayoutId) -> Emitted<Place<'m>> {
        let record = self.layout(layout)?;
        // A `Shared` root's payload is its `Frozen` record, read only through
        // `shared.with`'s borrow (MEM-11).
        let (LayoutKind::Block { payload }
        | LayoutKind::Box { payload }
        | LayoutKind::Shared { payload }) = record.kind
        else {
            return self.invalid("an address through a value that is not a block reference");
        };
        let align = self.layout(payload)?.align;
        let uncounted = record
            .scan
            .iter()
            .any(|step| matches!(step, ScanStep::Uncounted { .. }));
        if uncounted {
            return self.uncounted_payload(pointer, payload);
        }
        let offset = match payload_offset(align) {
            Ok(offset) => offset,
            Err(error) => return self.invalid(format!("{error:?}")),
        };
        let ptr = self.builder.offset(pointer, self.const_u64(offset));
        Ok(Place {
            columns: None,
            ptr,
            layout: payload,
            align,
            block: Some(pointer),
        })
    }

    pub(super) fn project(
        &mut self,
        place: Place<'m>,
        step: ProjectionStep,
        failure: FailureId,
        unchecked: bool,
    ) -> Emitted<Place<'m>> {
        let state = self.sequence_state_at(place);
        let mut place = place;
        if step == ProjectionStep::Payload {
            if !matches!(self.layout(place.layout)?.kind, LayoutKind::Box { .. }) {
                return self.invalid("Payload requires a Box address");
            }
            let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
            return self.payload(pointer, place.layout);
        }
        if let LayoutKind::Block { .. } | LayoutKind::Box { .. } | LayoutKind::Shared { .. } =
            self.layout(place.layout)?.kind
        {
            let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
            place = self.payload(pointer, place.layout)?;
        }
        let record = self.layout(place.layout)?;
        match step {
            ProjectionStep::Payload => unreachable!("handled before implicit dereference"),
            ProjectionStep::Field(ordinal) => {
                let Some(field) = record.fields.iter().find(|f| f.ordinal == ordinal) else {
                    return self.invalid("a field the layout does not have");
                };
                // A flat field lies at its cursor: further than its static offset by each
                // earlier variable field's u64 length prefix (LIR schema revision 3).
                if record.kind == LayoutKind::Flat {
                    let mut earlier: Vec<_> = record
                        .fields
                        .iter()
                        .filter(|f| f.offset < field.offset)
                        .collect();
                    earlier.sort_by_key(|f| f.offset);
                    let i64_ty = self.module.int_type(64);
                    let mut cursor = self.const_u64(0);
                    let mut moved = false;
                    for earlier in earlier {
                        if matches!(self.layout(earlier.layout)?.size, SizeSpec::Dynamic(_)) {
                            let at = self
                                .builder
                                .offset(place.ptr, self.const_u64(earlier.offset));
                            let at = self.builder.offset(at, cursor);
                            let prefix = self.builder.load(i64_ty, at, 1);
                            cursor = self.builder.binary(BinOp::Add, cursor, prefix);
                            moved = true;
                        }
                    }
                    if moved {
                        let base = self.builder.offset(place.ptr, cursor);
                        let ptr = self.builder.offset(base, self.const_u64(field.offset));
                        return Ok(Place {
                            columns: None,
                            ptr,
                            layout: field.layout,
                            align: 1,
                            block: place.block,
                        });
                    }
                }
                self.field_place(place, field.offset, field.layout, field.unaligned)
            }
            ProjectionStep::VariantField { variant, field } => {
                let Some(tag) = &record.tag else {
                    return self.invalid("a variant field of an untagged layout");
                };
                let Some(fields) = tag.variants.iter().find(|v| v.ordinal == variant) else {
                    return self.invalid("a variant the layout does not have");
                };
                let Some(field) = fields.fields.iter().find(|f| f.ordinal == field) else {
                    return self.invalid("a field the variant does not have");
                };
                // MEM-13: the active tag must be the projected variant.
                if !unchecked {
                    let active = self.active_tag(place)?;
                    let expected = self
                        .module
                        .const_int(self.module.int_type(32), u128::from(variant));
                    let wrong = self.builder.icmp(IntPredicate::Ne, active, expected);
                    self.guard(wrong, failure)?;
                }
                self.field_place(place, field.offset, field.layout, field.unaligned)
            }
            ProjectionStep::Slot(index) => {
                // An element slot below the capacity, with no count check (LIR schema
                // revision 7): the library body proves the bound.
                if let LayoutKind::SoaColumns { element } = record.kind {
                    return self.column_row(place, element, self.get(index)?);
                }
                let LayoutKind::Sequence { element } = record.kind else {
                    return self.invalid("an element slot of a layout that is no sequence");
                };
                let offset = match sequence_elements_offset(self.layout(element)?) {
                    Ok(offset) => offset,
                    Err(error) => return self.invalid(format!("{error:?}")),
                };
                let stride = self.fixed_size(element)?;
                let scaled =
                    self.builder
                        .binary(BinOp::Mul, self.get(index)?, self.const_u64(stride));
                let at = self
                    .builder
                    .binary(BinOp::Add, scaled, self.const_u64(offset));
                let align = place
                    .align
                    .min(low_bit(stride))
                    .min(low_bit(offset))
                    .min(self.layout(element)?.align);
                Ok(Place {
                    columns: None,
                    ptr: self.builder.offset(place.ptr, at),
                    layout: element,
                    align,
                    block: place.block,
                })
            }
            ProjectionStep::FlatAt { index, offset }
                if matches!(record.kind, LayoutKind::Sequential { .. }) =>
            {
                // Element `index` of a sequential Packed sequence (MEM-13) at its known
                // offset from the first element, checked like `Index` (MEM-14).
                let LayoutKind::Sequential { element } = record.kind else {
                    unreachable!("matched above")
                };
                let count = self.load_u64(place.ptr);
                self.projection_index(index, count, failure, unchecked)?;
                let first = self.sequential_first(place.ptr, element)?;
                self.flat_content = Some(first);
                Ok(Place {
                    columns: None,
                    ptr: self.builder.offset(first, self.get(offset)?),
                    layout: element,
                    align: self.layout(element)?.align.min(place.align),
                    block: place.block,
                })
            }
            ProjectionStep::FlatAt { index, offset } => {
                // Element `index` of a variable-element flat sequence at its known content
                // offset, checked like `Index` (MEM-14).
                let count = self.flat_count(place)?;
                self.projection_index(index, count, failure, unchecked)?;
                let (element, variable) = self.flat_sequence(place.layout)?;
                if !variable {
                    return self.invalid("a flat walk of a sequence of fixed elements");
                }
                let content = self.builder.offset(place.ptr, self.const_u64(16));
                self.flat_content = Some(content);
                Ok(Place {
                    columns: None,
                    ptr: self.builder.offset(content, self.get(offset)?),
                    layout: element,
                    align: 1,
                    block: place.block,
                })
            }
            ProjectionStep::Index(index)
                if matches!(record.kind, LayoutKind::SoaColumns { .. }) =>
            {
                let LayoutKind::SoaColumns { element } = record.kind else {
                    unreachable!()
                };
                let count = self.load_u64(place.ptr);
                let index = self.projection_index(index, count, failure, unchecked)?;
                self.column_row(place, element, index)
            }
            ProjectionStep::Index(index)
                if matches!(record.kind, LayoutKind::Sequential { .. }) =>
            {
                // Element i of a sequential Packed sequence (MEM-13): walked to from the
                // first, past each earlier element's active extent.
                let LayoutKind::Sequential { element } = record.kind else {
                    unreachable!("matched above")
                };
                let count = self.load_u64(place.ptr);
                let index = self.projection_index(index, count, failure, unchecked)?;
                let first = self.sequential_first(place.ptr, element)?;
                self.flat_content = Some(first);
                let ptr = self.sequential_walk(first, element, index)?;
                Ok(Place {
                    columns: None,
                    ptr,
                    layout: element,
                    align: self.layout(element)?.align.min(place.align),
                    block: place.block,
                })
            }
            ProjectionStep::Index(index) if record.kind == LayoutKind::Flat => {
                // Element i of a flat sequence encoding lies at its cursor (LIR schema
                // revision 4): fixed elements by stride past the prefix, variable ones walked
                // past each earlier element's prefixed encoding.
                let count = self.flat_count(place)?;
                let index = self.projection_index(index, count, failure, unchecked)?;
                let (element, variable) = self.flat_sequence(place.layout)?;
                let ptr = if variable {
                    let content = self.builder.offset(place.ptr, self.const_u64(16));
                    self.flat_content = Some(content);
                    self.flat_walk(content, index)
                } else {
                    let stride = self.fixed_size(element)?;
                    let scaled = self
                        .builder
                        .binary(BinOp::Mul, index, self.const_u64(stride));
                    let start = self.layout(place.layout)?.fields[0].offset;
                    let at = self.builder.offset(place.ptr, self.const_u64(start));
                    self.builder.offset(at, scaled)
                };
                Ok(Place {
                    columns: None,
                    ptr,
                    layout: element,
                    align: 1,
                    block: place.block,
                })
            }
            ProjectionStep::Index(index) => {
                let (element, count, base, element_offset) = match record.kind {
                    LayoutKind::Inline { element, count } => {
                        (element, self.const_u64(count), place, 0)
                    }
                    LayoutKind::Simd { element, lanes } => {
                        (element, self.const_u64(u64::from(lanes)), place, 0)
                    }
                    LayoutKind::Slice { element } => {
                        // Immutable byte indexing addresses this exact interval.
                        // The bounds guard uses length before forming the byte GEP.
                        let word = |ordinal| {
                            record
                                .fields
                                .iter()
                                .find(|field| field.ordinal == ordinal)
                                .map(|field| {
                                    self.builder.offset(place.ptr, self.const_u64(field.offset))
                                })
                        };
                        let (Some(owner_at), Some(start_at), Some(length_at)) =
                            (word(0), word(1), word(2))
                        else {
                            return self.invalid("a slice without owner, start and length");
                        };
                        let owner =
                            self.builder
                                .load(self.module.ptr_type(), owner_at, place.align.min(8));
                        let start = self.builder.load(
                            self.module.int_type(64),
                            start_at,
                            place.align.min(8),
                        );
                        let count = self.builder.load(
                            self.module.int_type(64),
                            length_at,
                            place.align.min(8),
                        );
                        let Some(storage) = slice_storage(self.inputs.layouts, element) else {
                            return self.invalid("a slice without its byte sequence layout");
                        };
                        let payload = self.payload_of(owner, storage)?;
                        let bytes = self.builder.offset(payload.ptr, self.const_u64(8));
                        let base = Place {
                            columns: None,
                            ptr: self.builder.offset(bytes, start),
                            layout: storage,
                            align: 1,
                            block: Some(owner),
                        };
                        (element, count, base, 0)
                    }
                    LayoutKind::Sequence { element } => {
                        let count_at = self.field_place(place, 0, place.layout, false)?;
                        // A source element address cannot reach the length word:
                        // it starts after the aligned sequence header. Scalar writes
                        // may therefore not clobber this load (MEM-14).
                        let count = match (state, self.known_count.take()) {
                            (Some(state), _) => {
                                self.builder.load(self.module.int_type(64), state.length, 8)
                            }
                            (None, Some(length)) => length,
                            (None, None) => {
                                self.flush_viewed(place.layout)?;
                                self.builder.load_access(
                                    self.module.int_type(64),
                                    count_at.ptr,
                                    8.min(place.align),
                                    Access::Length,
                                )
                            }
                        };
                        let offset = match sequence_elements_offset(self.layout(element)?) {
                            Ok(offset) => offset,
                            Err(error) => return self.invalid(format!("{error:?}")),
                        };
                        (element, count, place, offset)
                    }
                    _ => return self.unsupported("an index into this layout kind"),
                };
                let stride = self.fixed_size(element)?;
                let index64 = self.projection_index(index, count, failure, unchecked)?;
                let scaled = self
                    .builder
                    .binary(BinOp::Mul, index64, self.const_u64(stride));
                let offset =
                    self.builder
                        .binary(BinOp::Add, scaled, self.const_u64(element_offset));
                let align = base
                    .align
                    .min(low_bit(stride))
                    .min(low_bit(element_offset))
                    .min(self.layout(element)?.align);
                Ok(Place {
                    columns: None,
                    ptr: self.builder.offset(base.ptr, offset),
                    layout: element,
                    align,
                    block: base.block,
                })
            }
        }
    }

    /// The integer `index` as an `i64` below `count`; a negative, wider or outside index
    /// takes `failure` (MEM-14).
    pub(super) fn checked_index(
        &mut self,
        index: ValueId,
        count: Value<'m>,
        failure: FailureId,
    ) -> Emitted<Value<'m>> {
        self.projection_index(index, count, failure, false)
    }

    fn projection_index(
        &mut self,
        index: ValueId,
        count: Value<'m>,
        failure: FailureId,
        unchecked: bool,
    ) -> Emitted<Value<'m>> {
        let index_kind = self.scalar_kind(index)?;
        let Some((width, signed)) = scalar::integer(index_kind) else {
            return self.invalid("an index that is not an integer");
        };
        let index = self.get(index)?;
        let i64_ty = self.module.int_type(64);
        if unchecked {
            return Ok(match width.cmp(&64) {
                std::cmp::Ordering::Less => self.builder.cast(Cast::ZExt, index, i64_ty),
                std::cmp::Ordering::Equal => index,
                std::cmp::Ordering::Greater => self.builder.cast(Cast::Trunc, index, i64_ty),
            });
        }
        let index_ty = self.module.int_type(width);
        let zero = self.module.const_int(index_ty, 0);
        let mut failed = self.module.const_int(self.module.int_type(1), 0);
        if signed {
            let negative = self.builder.icmp(IntPredicate::Slt, index, zero);
            failed = self.builder.binary(BinOp::Or, failed, negative);
        }
        let index64 = match width.cmp(&64) {
            std::cmp::Ordering::Less => self.builder.cast(Cast::ZExt, index, i64_ty),
            std::cmp::Ordering::Equal => index,
            std::cmp::Ordering::Greater => {
                let high =
                    self.builder
                        .binary(BinOp::LShr, index, self.module.const_int(index_ty, 64));
                let wide = self.builder.icmp(IntPredicate::Ne, high, zero);
                failed = self.builder.binary(BinOp::Or, failed, wide);
                self.builder.cast(Cast::Trunc, index, i64_ty)
            }
        };
        // MEM-14: the index must be below the count.
        let outside = self.builder.icmp(IntPredicate::Uge, index64, count);
        failed = self.builder.binary(BinOp::Or, failed, outside);
        self.guard(failed, failure)?;
        Ok(index64)
    }
}
