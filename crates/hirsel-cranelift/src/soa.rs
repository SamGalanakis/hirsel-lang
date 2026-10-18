//! Capacity-based columns: retain row coordinates until projection, gather or scatter.
use cranelift_codegen::ir::{InstBuilder, Value, condcodes::IntCC, types};
use hirsel_lir::ir::SourceAnchorId;
use hirsel_lir::layout_types::{FieldLayout, LayoutId};
use hirsel_lir::runtime_abi::{EXTENT_MAX, RuntimeEntry};

use crate::function::{FnEmitter, Place, Shape, TRAP, V};
use crate::module::Emitted;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ColumnRow {
    data: Value,
    capacity: Value,
    index: Value,
}

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn column_shape(&self, element: LayoutId) -> Emitted<(u64, u64, u64)> {
        let mut width = 0;
        let mut quantum = 1;
        for field in &self.layout(element)?.fields {
            width += self.fixed_size(field.layout)?;
            quantum = quantum.max(u64::from(self.layout(field.layout)?.align));
        }
        let data = hirsel_lir::layout::soa_data_offset(quantum)
            .or_else(|e| self.invalid(format!("{e:?}")))?;
        Ok((data, quantum, width))
    }

    pub(crate) fn column_capacity(&self, count: u64, element: LayoutId) -> Emitted<u64> {
        let (_, quantum, _) = self.column_shape(element)?;
        hirsel_lir::layout::align_up(count, quantum).or_else(|e| self.invalid(format!("{e:?}")))
    }

    pub(crate) fn column_row(
        &mut self,
        place: Place,
        element: LayoutId,
        index: Value,
    ) -> Emitted<Place> {
        let (offset, _, _) = self.column_shape(element)?;
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

    pub(crate) fn column_field(&mut self, place: Place, field: &FieldLayout) -> Emitted<Place> {
        let row = place.columns.expect("column row");
        let mut prefix = 0;
        for before in &self.layout(place.layout)?.fields {
            if before.ordinal == field.ordinal {
                break;
            }
            prefix += self.fixed_size(before.layout)?;
        }
        let stride = self.fixed_size(field.layout)?;
        let column = self.b.ins().imul_imm_s(row.capacity, prefix as i64);
        let offset = self.b.ins().imul_imm_s(row.index, stride as i64);
        let offset = self.b.ins().iadd(column, offset);
        Ok(Place {
            columns: None,
            ptr: self.offset_by(row.data, offset),
            layout: field.layout,
            align: self.layout(field.layout)?.align,
            block: place.block,
        })
    }

    pub(crate) fn gather(&mut self, place: Place) -> Emitted<Place> {
        if place.columns.is_none() {
            return Ok(place);
        }
        let temp = self.stack(place.layout)?;
        for field in &self.layout(place.layout)?.fields {
            let from = self.column_field(place, field)?;
            let to = self.field_place(temp, field.offset, field.layout, field.unaligned)?;
            let shape = self.layout_shape(field.layout)?;
            let v = self.load(shape, from);
            self.store(shape, v, to)?;
        }
        Ok(temp)
    }

    pub(crate) fn scatter(&mut self, place: Place, shape: Shape, value: V) -> Emitted {
        if place.columns.is_none() {
            return self.store(shape, value, place);
        }
        let temp = self.stack(place.layout)?;
        self.store(shape, value, temp)?;
        for field in &self.layout(place.layout)?.fields {
            let from = self.field_place(temp, field.offset, field.layout, field.unaligned)?;
            let to = self.column_field(place, field)?;
            let shape = self.layout_shape(field.layout)?;
            let v = self.load(shape, from);
            self.store(shape, v, to)?;
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
        let unique = self.is_unique(old);
        let enough = self
            .b
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, capacity, wanted);
        let keep = self.b.ins().band(unique, enough);
        let grow = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(keep, done, &[], grow, &[]);
        self.b.switch_to_block(grow);
        let (data, quantum, width) = self.column_shape(element)?;
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
        let fresh = self.alloc_block(
            payload,
            extent,
            self.layout(payload)?.align,
            SourceAnchorId(0),
        )?;
        let target = self.payload_of(fresh, payload)?;
        self.store_at(count, target.ptr, 0);
        self.store_at(cap, target.ptr, 8);
        let mut prefix = 0;
        for field in &self.layout(element)?.fields {
            let stride = self.fixed_size(field.layout)?;
            let to = self.b.ins().imul_imm_s(cap, prefix as i64);
            let to = self.b.ins().iadd_imm_s(to, data as i64);
            let to = self.offset_by(target.ptr, to);
            let from = self.b.ins().imul_imm_s(capacity, prefix as i64);
            let from = self.b.ins().iadd_imm_s(from, data as i64);
            let from = self.offset_by(source.ptr, from);
            let bytes = self.b.ins().imul_imm_s(count, stride as i64);
            self.copy_dynamic(to, from, bytes);
            prefix += stride;
        }
        let moved = self.b.create_block();
        let copied = self.b.create_block();
        let joined = self.b.create_block();
        self.b.ins().brif(unique, moved, &[], copied, &[]);
        self.b.switch_to_block(moved);
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
            &[self.context, old, descriptor, null, zero],
        )?;
        self.b.ins().jump(joined, &[]);
        self.b.switch_to_block(joined);
        self.store_at(fresh, place.ptr, 0);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        Ok(())
    }
}
