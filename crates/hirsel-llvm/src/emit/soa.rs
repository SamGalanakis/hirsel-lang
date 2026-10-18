//! Capacity-based structure-of-arrays storage. Element places retain their row
//! coordinates until a field projection, gather or scatter consumes them.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct ColumnRow<'m> {
    data: Value<'m>,
    capacity: Value<'m>,
    index: Value<'m>,
}

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(super) fn column_shape(&self, element: LayoutId) -> Emitted<(u64, u64, u64)> {
        let fields = &self.layout(element)?.fields;
        let mut width = 0;
        let mut quantum = 1;
        for field in fields {
            width += self.fixed_size(field.layout)?;
            quantum = quantum.max(u64::from(self.layout(field.layout)?.align));
        }
        let data = hirsel_lir::layout::soa_data_offset(quantum)
            .or_else(|e| self.invalid(format!("{e:?}")))?;
        Ok((data, quantum, width))
    }

    pub(super) fn column_capacity(&self, count: u64, element: LayoutId) -> Emitted<u64> {
        let (_, quantum, _) = self.column_shape(element)?;
        hirsel_lir::layout::align_up(count, quantum).or_else(|e| self.invalid(format!("{e:?}")))
    }

    pub(super) fn column_row(
        &self,
        place: Place<'m>,
        element: LayoutId,
        index: Value<'m>,
    ) -> Emitted<Place<'m>> {
        let (offset, _, _) = self.column_shape(element)?;
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

    pub(super) fn column_field(&self, place: Place<'m>, field: &FieldLayout) -> Emitted<Place<'m>> {
        let row = place.columns.expect("column row");
        let mut prefix = 0;
        for before in &self.layout(place.layout)?.fields {
            if before.ordinal == field.ordinal {
                break;
            }
            prefix += self.fixed_size(before.layout)?;
        }
        let column = self
            .builder
            .binary(BinOp::Mul, row.capacity, self.const_u64(prefix));
        let stride = self.fixed_size(field.layout)?;
        let offset = self
            .builder
            .binary(BinOp::Mul, row.index, self.const_u64(stride));
        let offset = self.builder.binary(BinOp::Add, column, offset);
        Ok(Place {
            ptr: self.builder.offset(row.data, offset),
            layout: field.layout,
            align: self.layout(field.layout)?.align,
            block: place.block,
            columns: None,
        })
    }

    pub(super) fn gather(&self, place: Place<'m>) -> Emitted<Place<'m>> {
        if place.columns.is_none() {
            return Ok(place);
        }
        let temp = self.stack(place.layout)?;
        for field in &self.layout(place.layout)?.fields {
            let from = self.column_field(place, field)?;
            let to = self.field_place(temp, field.offset, field.layout, field.unaligned)?;
            let shape = self.layout_shape(field.layout)?;
            self.store(shape, self.load(shape, from), to);
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
        for field in &self.layout(place.layout)?.fields {
            let from = self.field_place(temp, field.offset, field.layout, field.unaligned)?;
            let to = self.column_field(place, field)?;
            let shape = self.layout_shape(field.layout)?;
            self.store(shape, self.load(shape, from), to);
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
        let (data, quantum, width) = self.column_shape(element)?;
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
        let mut prefix = 0;
        for field in &self.layout(element)?.fields {
            let stride = self.fixed_size(field.layout)?;
            let at = |base, capacity| {
                let offset = self
                    .builder
                    .binary(BinOp::Mul, capacity, self.const_u64(prefix));
                let offset = self
                    .builder
                    .binary(BinOp::Add, offset, self.const_u64(data));
                self.builder.offset(base, offset)
            };
            let bytes = self
                .builder
                .binary(BinOp::Mul, count, self.const_u64(stride));
            self.builder.memcpy(
                at(target.ptr, cap),
                self.layout(field.layout)?.align,
                at(source.ptr, capacity),
                self.layout(field.layout)?.align,
                bytes,
            );
            prefix += stride;
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
