//! Capacity-based columns (MEM-7): retain row coordinates until projection, tag access,
//! gather or scatter. A variant's row is its u32 tag and its payload; its columns hold the
//! tag narrowed and the payload's bytes.
use cranelift_codegen::ir::{InstBuilder, Value, condcodes::IntCC, types};
use hirsel_lir::ir::SourceAnchorId;
use hirsel_lir::layout::{SoaPart, SoaShape, SoaSpan};
use hirsel_lir::layout_types::{LayoutId, ScalarKind};
use hirsel_lir::runtime_abi::{EXTENT_MAX, RuntimeEntry};

use crate::function::{FnEmitter, Place, Shape, TRAP, V, low_bit};
use crate::module::Emitted;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ColumnRow {
    data: Value,
    capacity: Value,
    index: Value,
}

/// The Cranelift type of a tag column's ordinal.
fn tag_type(scalar: ScalarKind) -> types::Type {
    match scalar {
        ScalarKind::U8 => types::I8,
        ScalarKind::U16 => types::I16,
        _ => types::I32,
    }
}

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn soa(&self, element: LayoutId) -> Emitted<SoaShape> {
        self.em
            .layouts()
            .soa_shape(element)
            .or_else(|e| self.invalid(format!("{e:?}")))
    }

    pub(crate) fn column_capacity(&self, count: u64, element: LayoutId) -> Emitted<u64> {
        let quantum = self.soa(element)?.quantum;
        hirsel_lir::layout::align_up(count, quantum).or_else(|e| self.invalid(format!("{e:?}")))
    }

    pub(crate) fn column_row(
        &mut self,
        place: Place,
        element: LayoutId,
        index: Value,
    ) -> Emitted<Place> {
        let offset = self.soa(element)?.data_offset;
        let capacity = self.load_ty(types::I64, place.ptr, 8);
        let data = self.offset(place.ptr, offset);
        Ok(Place {
            columns: Some(ColumnRow {
                data,
                capacity,
                index,
            }),
            layout: element,
            ..place
        })
    }

    /// The address of `span`'s element in the row `place` names.
    fn column_element(&mut self, place: Place, span: &SoaSpan) -> Value {
        let row = place.columns.expect("column row");
        let column = self.b.ins().imul_imm_s(row.capacity, span.prefix as i64);
        let offset = self.b.ins().imul_imm_s(row.index, span.stride as i64);
        let offset = self.b.ins().iadd(column, offset);
        self.offset_by(row.data, offset)
    }

    /// The place of the row's bytes at `offset` holding `layout`: a record field's own
    /// column, or a part of a variant's payload column.
    pub(crate) fn column_field(
        &mut self,
        place: Place,
        offset: u64,
        layout: LayoutId,
    ) -> Emitted<Place> {
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
            ptr = self.offset(ptr, within);
            align = align.min(low_bit(within)).min(span.align);
        }
        Ok(Place {
            columns: None,
            ptr,
            layout,
            align,
            block: place.block,
        })
    }

    /// The active ordinal (an `i32`) of a variant row, from its tag column.
    pub(crate) fn column_tag(&mut self, place: Place) -> Emitted<Value> {
        let shape = self.soa(place.layout)?;
        let Some((span, scalar)) = shape.tag() else {
            return self.invalid("a tag read of a record row");
        };
        let at = self.column_element(place, span);
        let narrow = self.load_ty(tag_type(scalar), at, 0);
        Ok(if scalar == ScalarKind::U32 {
            narrow
        } else {
            self.b.ins().uextend(types::I32, narrow)
        })
    }

    /// Makes `variant` the active ordinal of a variant row.
    pub(crate) fn store_column_tag(&mut self, place: Place, variant: u32) -> Emitted {
        let shape = self.soa(place.layout)?;
        let Some((span, scalar)) = shape.tag() else {
            return self.invalid("a tag write of a record row");
        };
        let at = self.column_element(place, span);
        let value = self.b.ins().iconst(tag_type(scalar), i64::from(variant));
        self.store_at(value, at, 0);
        Ok(())
    }

    /// Copies one column's element between the row `place` names and `temp`, its row image.
    fn transfer(&mut self, place: Place, temp: Place, span: &SoaSpan, out: bool) -> Emitted {
        let column = self.column_element(place, span);
        let image = self.offset(temp.ptr, span.row_offset);
        match span.part {
            SoaPart::Field { layout, .. } => {
                let shape = self.layout_shape(layout)?;
                let align = self.layout(layout)?.align;
                let at = |ptr| Place {
                    columns: None,
                    ptr,
                    layout,
                    align,
                    block: place.block,
                };
                let (from, to) = if out {
                    (at(column), at(image))
                } else {
                    (at(image), at(column))
                };
                let v = self.load(shape, from);
                self.store(shape, v, to)?;
            }
            SoaPart::Tag(scalar) => {
                if out {
                    let narrow = self.load_ty(tag_type(scalar), column, 0);
                    let wide = if scalar == ScalarKind::U32 {
                        narrow
                    } else {
                        self.b.ins().uextend(types::I32, narrow)
                    };
                    self.store_at(wide, image, 0);
                } else {
                    let wide = self.load_ty(types::I32, image, 0);
                    let narrow = if scalar == ScalarKind::U32 {
                        wide
                    } else {
                        self.b.ins().ireduce(tag_type(scalar), wide)
                    };
                    self.store_at(narrow, column, 0);
                }
            }
            SoaPart::Payload => {
                let align = span.align.min(low_bit(span.row_offset)).min(temp.align);
                if out {
                    self.copy(image, column, span.stride, align);
                } else {
                    self.copy(column, image, span.stride, align);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn gather(&mut self, place: Place) -> Emitted<Place> {
        if place.columns.is_none() {
            return Ok(place);
        }
        let temp = self.stack(place.layout)?;
        for span in &self.soa(place.layout)?.spans {
            self.transfer(place, temp, span, true)?;
        }
        Ok(temp)
    }

    pub(crate) fn scatter(&mut self, place: Place, shape: Shape, value: V) -> Emitted {
        if place.columns.is_none() {
            return self.store(shape, value, place);
        }
        let temp = self.stack(place.layout)?;
        self.store(shape, value, temp)?;
        for span in &self.soa(place.layout)?.spans {
            self.transfer(place, temp, span, false)?;
        }
        Ok(())
    }

    /// Moving a unique block transfers credits; copying a shared block duplicates each row.
    pub(crate) fn reserve_columns(
        &mut self,
        place: Place,
        payload: LayoutId,
        element: LayoutId,
        wanted: Value,
    ) -> Emitted {
        let old = self.load_ptr(place.ptr);
        let source = self.payload_of(old, payload)?;
        let count = self.load_u64(source.ptr);
        let capacity = self.load_ty(types::I64, source.ptr, 8);
        // MEM-6: an unshared tracked arena block is written in place too.
        let unique = self.is_writable(old);
        let enough = self
            .b
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, capacity, wanted);
        let keep = self.b.ins().band(unique, enough);
        let grow = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(keep, done, &[], grow, &[]);
        self.b.switch_to_block(grow);
        let shape = self.soa(element)?;
        let (data, quantum, width) = (shape.data_offset, shape.quantum, shape.width);
        let more = self.b.ins().icmp(IntCC::UnsignedGreaterThan, wanted, count);
        let requested = self.b.ins().select(more, wanted, count);
        let bump = self.u64c(quantum - 1);
        let rounded = self.flat_add(requested, bump);
        let cap = self.b.ins().band_imm_s(rounded, !(quantum - 1) as i64);
        if width != 0 {
            let limit = self.u64c((EXTENT_MAX - data) / width);
            let invalid = self.b.ins().icmp(IntCC::UnsignedGreaterThan, cap, limit);
            self.b.ins().trapnz(invalid, TRAP);
        }
        let bytes = self.b.ins().imul_imm_s(cap, width as i64);
        let extent = self.b.ins().iadd_imm_s(bytes, data as i64);
        // A tracked arena block regrows in its arena, uncharged (MEM-6).
        let tracked = if self.tracking() {
            Some(self.is_tracked(old))
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
                    self.b.create_block(),
                    self.b.create_block(),
                    self.b.create_block(),
                );
                let fresh = self.b.append_block_param(grown, types::I64);
                self.b.ins().brif(tracked, arena, &[], heap, &[]);
                self.b.switch_to_block(arena);
                let align = self.u64c(u64::from(self.layout(payload)?.align));
                let regrown = self.rt1(
                    RuntimeEntry::ArenaRegrow,
                    &[self.context, old, extent, align],
                )?;
                self.b.ins().jump(grown, &[regrown.into()]);
                self.b.switch_to_block(heap);
                let allocated = self.alloc_block(
                    payload,
                    extent,
                    self.layout(payload)?.align,
                    SourceAnchorId(0),
                )?;
                self.b.ins().jump(grown, &[allocated.into()]);
                self.b.switch_to_block(grown);
                fresh
            }
        };
        let target = self.payload_of(fresh, payload)?;
        self.store_at(count, target.ptr, 0);
        self.store_at(cap, target.ptr, 8);
        // Every column moves together, as Zig's MultiArrayList.setCapacity copies each
        // field's items (multi_array_list.zig:543-569).
        for span in &shape.spans {
            let to = self.b.ins().imul_imm_s(cap, span.prefix as i64);
            let to = self.b.ins().iadd_imm_s(to, data as i64);
            let to = self.offset_by(target.ptr, to);
            let from = self.b.ins().imul_imm_s(capacity, span.prefix as i64);
            let from = self.b.ins().iadd_imm_s(from, data as i64);
            let from = self.offset_by(source.ptr, from);
            let bytes = self.b.ins().imul_imm_s(count, span.stride as i64);
            self.copy_dynamic(to, from, bytes);
        }
        let moved = self.b.create_block();
        let copied = self.b.create_block();
        let joined = self.b.create_block();
        self.b.ins().brif(unique, moved, &[], copied, &[]);
        self.b.switch_to_block(moved);
        if let Some(tracked) = tracked {
            // An unshared tracked block's storage is abandoned in its arena, emptied so its
            // scheduled drop finds no row.
            let (abandon, free) = (self.b.create_block(), self.b.create_block());
            self.b.ins().brif(tracked, abandon, &[], free, &[]);
            self.b.switch_to_block(abandon);
            let zero = self.u64c(0);
            self.store_at(zero, source.ptr, 0);
            self.b.ins().jump(joined, &[]);
            self.b.switch_to_block(free);
        }
        self.rt(RuntimeEntry::Free, &[self.context, old])?;
        self.b.ins().jump(joined, &[]);
        self.b.switch_to_block(copied);
        let head = self.b.create_block();
        let body = self.b.create_block();
        let exit = self.b.create_block();
        let index = self.b.append_block_param(head, types::I64);
        let zero = self.u64c(0);
        self.b.ins().jump(head, &[zero.into()]);
        self.b.switch_to_block(head);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, index, count);
        self.b.ins().brif(more, body, &[], exit, &[]);
        self.b.switch_to_block(body);
        let row = self.column_row(target, element, index)?;
        self.dup_holders(row)?;
        let next = self.b.ins().iadd_imm_s(index, 1);
        self.b.ins().jump(head, &[next.into()]);
        self.b.switch_to_block(exit);
        let descriptor = self.block_descriptor(old);
        let null = self.u64c(0);
        self.rt(
            RuntimeEntry::Release,
            &[self.context, old, descriptor, null, zero, zero],
        )?;
        self.b.ins().jump(joined, &[]);
        self.b.switch_to_block(joined);
        self.store_at(fresh, place.ptr, 0);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        Ok(())
    }
}
