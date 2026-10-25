//! Capacity-based structure-of-arrays storage (MEM-7). Element places retain their row
//! coordinates until a field or payload projection, tag access, gather or scatter consumes
//! them. A variant's row is its u32 tag and its payload; its columns hold the tag narrowed
//! and the payload's bytes.
use super::*;
use hirsel_lir::layout::{SoaPart, SoaShape, SoaSpan};

#[derive(Clone, Copy, Debug)]
pub(super) struct ColumnRow<'m> {
    data: Value<'m>,
    capacity: Value<'m>,
    index: Value<'m>,
}

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(super) fn soa(&self, element: LayoutId) -> Emitted<SoaShape> {
        self.inputs
            .layouts
            .soa_shape(element)
            .or_else(|e| self.invalid(format!("{e:?}")))
    }

    pub(super) fn column_capacity(&self, count: u64, element: LayoutId) -> Emitted<u64> {
        let quantum = self.soa(element)?.quantum;
        hirsel_lir::layout::align_up(count, quantum).or_else(|e| self.invalid(format!("{e:?}")))
    }

    pub(super) fn column_row(
        &self,
        place: Place<'m>,
        element: LayoutId,
        index: Value<'m>,
    ) -> Emitted<Place<'m>> {
        let offset = self.soa(element)?.data_offset;
        let capacity = self.load_u64(self.builder.offset(place.ptr, self.const_u64(8)));
        Ok(Place {
            columns: Some(ColumnRow {
                data: self.builder.offset(place.ptr, self.const_u64(offset)),
                capacity,
                index,
            }),
            layout: element,
            ..place
        })
    }

    /// The address of `span`'s element in the row `place` names.
    fn column_element(&self, place: Place<'m>, span: &SoaSpan) -> Value<'m> {
        let row = place.columns.expect("column row");
        let column = self
            .builder
            .binary(BinOp::Mul, row.capacity, self.const_u64(span.prefix));
        let offset = self
            .builder
            .binary(BinOp::Mul, row.index, self.const_u64(span.stride));
        let offset = self.builder.binary(BinOp::Add, column, offset);
        self.builder.offset(row.data, offset)
    }

    /// The place of the row's bytes at `offset` holding `layout`: a record field's own
    /// column, or a part of a variant's payload column.
    pub(super) fn column_field(
        &self,
        place: Place<'m>,
        offset: u64,
        layout: LayoutId,
    ) -> Emitted<Place<'m>> {
        let shape = self.soa(place.layout)?;
        let found = shape.spans.iter().find(|s| match s.part {
            SoaPart::Field { layout: l, .. } => s.row_offset == offset && l == layout,
            SoaPart::Payload => offset >= s.row_offset && offset < s.row_offset + s.stride.max(1),
            SoaPart::Tag(_) => false,
        });
        let Some(span) = found else {
            return self.invalid("column field absent");
        };
        let mut ptr = self.column_element(place, span);
        let mut align = self.layout(layout)?.align;
        if span.part == SoaPart::Payload && offset != span.row_offset {
            let within = offset - span.row_offset;
            ptr = self.builder.offset(ptr, self.const_u64(within));
            align = align.min(low_bit(within)).min(span.align);
        }
        Ok(Place {
            ptr,
            layout,
            align,
            block: place.block,
            columns: None,
        })
    }

    /// The active ordinal (an `i32`) of a variant row, from its tag column.
    pub(super) fn column_tag(&self, place: Place<'m>) -> Emitted<Value<'m>> {
        let shape = self.soa(place.layout)?;
        let Some((span, scalar)) = shape.tag() else {
            return self.invalid("a tag read of a record row");
        };
        let narrow = self.load(
            Shape::Scalar(scalar),
            Place {
                ptr: self.column_element(place, span),
                layout: place.layout,
                align: span.align,
                block: place.block,
                columns: None,
            },
        );
        Ok(widen_tag(self, narrow, scalar))
    }

    /// Makes `variant` the active ordinal of a variant row.
    pub(super) fn store_column_tag(&self, place: Place<'m>, variant: u32) -> Emitted {
        let shape = self.soa(place.layout)?;
        let Some((span, scalar)) = shape.tag() else {
            return self.invalid("a tag write of a record row");
        };
        let ty = self.llvm_type(Shape::Scalar(scalar));
        let value = self.module.const_int(ty, u128::from(variant));
        self.builder
            .store(value, self.column_element(place, span), span.align);
        Ok(())
    }

    /// Copies one column's element between the row `place` names and `temp`, its row image.
    fn transfer(&self, place: Place<'m>, temp: Place<'m>, span: &SoaSpan, out: bool) -> Emitted {
        let column = self.column_element(place, span);
        let image = self
            .builder
            .offset(temp.ptr, self.const_u64(span.row_offset));
        let image_align = temp.align.min(low_bit(span.row_offset));
        match span.part {
            SoaPart::Field { layout, .. } => {
                let shape = self.layout_shape(layout)?;
                let align = self.layout(layout)?.align;
                let at = |ptr, align| Place {
                    ptr,
                    layout,
                    align,
                    block: place.block,
                    columns: None,
                };
                let (from, to) = if out {
                    (at(column, align), at(image, image_align.min(align)))
                } else {
                    (at(image, image_align.min(align)), at(column, align))
                };
                self.store(shape, self.load(shape, from), to);
            }
            SoaPart::Tag(scalar) => {
                let narrow_ty = self.llvm_type(Shape::Scalar(scalar));
                let word = self.module.int_type(32);
                if out {
                    let narrow = self.builder.load(narrow_ty, column, span.align);
                    let wide = widen_tag(self, narrow, scalar);
                    self.builder.store(wide, image, image_align.min(4));
                } else {
                    let wide = self.builder.load(word, image, image_align.min(4));
                    let narrow = if scalar == ScalarKind::U32 {
                        wide
                    } else {
                        self.builder.cast(Cast::Trunc, wide, narrow_ty)
                    };
                    self.builder.store(narrow, column, span.align);
                }
            }
            SoaPart::Payload => {
                if span.stride != 0 {
                    let bytes = self.const_u64(span.stride);
                    let align = image_align.min(span.align);
                    if out {
                        self.builder.memcpy(image, align, column, span.align, bytes);
                    } else {
                        self.builder.memcpy(column, span.align, image, align, bytes);
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn gather(&self, place: Place<'m>) -> Emitted<Place<'m>> {
        if place.columns.is_none() {
            return Ok(place);
        }
        let temp = self.stack(place.layout)?;
        for span in &self.soa(place.layout)?.spans {
            self.transfer(place, temp, span, true)?;
        }
        Ok(temp)
    }

    pub(super) fn scatter(&self, place: Place<'m>, shape: Shape, value: Value<'m>) -> Emitted {
        if place.columns.is_none() {
            self.store(shape, value, place);
            return Ok(());
        }
        let temp = self.stack(place.layout)?;
        self.store(shape, value, temp);
        for span in &self.soa(place.layout)?.spans {
            self.transfer(place, temp, span, false)?;
        }
        Ok(())
    }
    /// A reserve retains a private block only when its current column capacity
    /// suffices. Otherwise every column moves to the new capacity's offsets.
    pub(super) fn reserve_columns(
        &mut self,
        place: Place<'m>,
        payload: LayoutId,
        element: LayoutId,
        wanted: Value<'m>,
    ) -> Emitted {
        let old = self.load(Shape::Scalar(ScalarKind::Pointer), place);
        let source = self.payload_of(old, payload)?;
        let count = self.load_u64(source.ptr);
        let capacity = self.load_u64(self.builder.offset(source.ptr, self.const_u64(8)));
        // MEM-6: an unshared tracked arena block is written in place too.
        let unique = if self.tracking() {
            crate::runtime::is_writable(&self.builder, old)
        } else {
            crate::runtime::is_unique(&self.builder, old)
        };
        let enough = self.builder.icmp(IntPredicate::Uge, capacity, wanted);
        let keep = self.builder.binary(BinOp::And, unique, enough);
        let grow = self.module.append_block(self.function);
        let done = self.module.append_block(self.function);
        self.builder.cond_br(keep, done, grow);
        self.builder.position(grow);
        let shape = self.soa(element)?;
        let (data, quantum, width) = (shape.data_offset, shape.quantum, shape.width);
        let more = self.builder.icmp(IntPredicate::Ugt, wanted, count);
        let requested = self.builder.select(more, wanted, count);
        let rounded = self.flat_add(requested, self.const_u64(quantum - 1))?;
        let cap = self
            .builder
            .binary(BinOp::And, rounded, self.const_u64(!(quantum - 1)));
        // Division bounds the multiplication before it can wrap.
        if width != 0 {
            let limit = self.const_u64((hirsel_lir::runtime_abi::EXTENT_MAX - data) / width);
            let invalid = self.builder.icmp(IntPredicate::Ugt, cap, limit);
            let bad = self.module.append_block(self.function);
            let valid = self.module.append_block(self.function);
            self.builder.cond_br(invalid, bad, valid);
            self.builder.position(bad);
            let trap = self
                .module
                .intrinsic("llvm.trap", &[])
                .or_else(|e| self.invalid(e))?;
            self.builder.call(trap, &[]);
            self.builder.unreachable();
            self.builder.position(valid);
        }
        let bytes = self.builder.binary(BinOp::Mul, cap, self.const_u64(width));
        let extent = self.builder.binary(BinOp::Add, bytes, self.const_u64(data));
        // A tracked arena block regrows in its arena, uncharged (MEM-6).
        let tracked = if self.tracking() {
            Some(crate::runtime::is_tracked(&self.builder, old))
        } else {
            None
        };
        let fresh = match tracked {
            None => self.alloc_block(
                payload,
                extent,
                self.layout(payload)?.align,
                SourceAnchorId(0),
            )?,
            Some(tracked) => {
                let (arena, heap, grown) = (
                    self.module.append_block(self.function),
                    self.module.append_block(self.function),
                    self.module.append_block(self.function),
                );
                self.builder.cond_br(tracked, arena, heap);
                self.builder.position(arena);
                let regrow = self.runtime(RuntimeEntry::ArenaRegrow)?;
                let align = self.const_u64(u64::from(self.layout(payload)?.align));
                let regrown = self
                    .builder
                    .call(regrow, &[self.context, old, extent, align]);
                let from_arena = self.builder.current();
                self.builder.br(grown);
                self.builder.position(heap);
                let allocated = self.alloc_block(
                    payload,
                    extent,
                    self.layout(payload)?.align,
                    SourceAnchorId(0),
                )?;
                let from_heap = self.builder.current();
                self.builder.br(grown);
                self.builder.position(grown);
                let fresh = self.builder.phi(self.module.ptr_type());
                self.builder.add_incoming(fresh, regrown, from_arena);
                self.builder.add_incoming(fresh, allocated, from_heap);
                fresh
            }
        };
        let target = self.payload_of(fresh, payload)?;
        self.builder.store(count, target.ptr, 8);
        self.builder
            .store(cap, self.builder.offset(target.ptr, self.const_u64(8)), 8);
        // Every column moves together, as Zig's MultiArrayList.setCapacity copies each
        // field's items (multi_array_list.zig:543-569).
        for span in &shape.spans {
            let at = |base, capacity| {
                let offset = self
                    .builder
                    .binary(BinOp::Mul, capacity, self.const_u64(span.prefix));
                let offset = self
                    .builder
                    .binary(BinOp::Add, offset, self.const_u64(data));
                self.builder.offset(base, offset)
            };
            let bytes = self
                .builder
                .binary(BinOp::Mul, count, self.const_u64(span.stride));
            self.builder.memcpy(
                at(target.ptr, cap),
                span.align,
                at(source.ptr, capacity),
                span.align,
                bytes,
            );
        }
        let moved = self.module.append_block(self.function);
        let copied = self.module.append_block(self.function);
        let joined = self.module.append_block(self.function);
        self.builder.cond_br(unique, moved, copied);
        self.builder.position(moved);
        if let Some(tracked) = tracked {
            // An unshared tracked block's storage is abandoned in its arena, emptied so its
            // scheduled drop finds no row.
            let (abandon, free) = (
                self.module.append_block(self.function),
                self.module.append_block(self.function),
            );
            self.builder.cond_br(tracked, abandon, free);
            self.builder.position(abandon);
            self.builder.store(self.const_u64(0), source.ptr, 8);
            self.builder.br(joined);
            self.builder.position(free);
        }
        let free = self.runtime(RuntimeEntry::Free)?;
        self.builder.call(free, &[self.context, old]);
        self.builder.br(joined);
        self.builder.position(copied);
        let entry = self.builder.current();
        let head = self.module.append_block(self.function);
        let body = self.module.append_block(self.function);
        let exit = self.module.append_block(self.function);
        self.builder.br(head);
        self.builder.position(head);
        let index = self.builder.phi(self.module.int_type(64));
        let more = self.builder.icmp(IntPredicate::Ult, index, count);
        self.builder.cond_br(more, body, exit);
        self.builder.position(body);
        let row = self.column_row(target, element, index)?;
        let image = self.gather(row)?;
        self.dup_holders(image)?;
        let next = self.builder.binary(BinOp::Add, index, self.const_u64(1));
        let back = self.builder.current();
        self.builder.br(head);
        self.builder.add_incoming(index, self.const_u64(0), entry);
        self.builder.add_incoming(index, next, back);
        self.builder.position(exit);
        let release = self.runtime(RuntimeEntry::Release)?;
        self.builder.call(
            release,
            &[
                self.context,
                old,
                self.block_descriptor(old),
                self.module.const_zero(self.module.ptr_type()),
                self.const_u64(0),
                self.const_u64(0),
            ],
        );
        self.builder.br(joined);
        self.builder.position(joined);
        self.store(Shape::Scalar(ScalarKind::Pointer), fresh, place);
        self.builder.br(done);
        self.builder.position(done);
        Ok(())
    }
}

/// A tag column's ordinal widened to the row's `i32`.
fn widen_tag<'m>(emitter: &FnEmitter<'m, '_>, narrow: Value<'m>, scalar: ScalarKind) -> Value<'m> {
    if scalar == ScalarKind::U32 {
        narrow
    } else {
        emitter
            .builder
            .cast(Cast::ZExt, narrow, emitter.module.int_type(32))
    }
}
