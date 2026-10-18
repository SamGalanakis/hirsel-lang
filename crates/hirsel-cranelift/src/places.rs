//! Addresses: a root and its projection, with every bounds (MEM-14) and tag (MEM-13) check
//! run on the way, as `hirsel-llvm`'s `places` computes them.

use cranelift_codegen::ir::{InstBuilder, Value, condcodes::IntCC, types};
use hirsel_lir::emission::slice_storage;
use hirsel_lir::ir::*;
use hirsel_lir::layout::{payload_offset, sequence_elements_offset};
use hirsel_lir::layout_types::{LayoutId, LayoutKind, ScanStep, SizeSpec};

use crate::function::{FnEmitter, Place, low_bit};
use crate::module::{Emitted, PTR};

impl FnEmitter<'_, '_, '_> {
    /// The slot an address names whole: no projection from it.
    pub(crate) fn whole_slot(&self, address: AddressId) -> Option<SlotId> {
        let info = self.unit.addresses.get(address.index())?;
        if info.projection.len != 0 {
            return None;
        }
        match info.base {
            AddressBase::Slot(slot) => Some(slot),
            AddressBase::Address(base) => self.whole_slot(base),
            AddressBase::Value(_) => None,
        }
    }

    /// Computes an address from its root, running every bounds and tag check on the way;
    /// any failing check takes `failure` before the address is used.
    pub(crate) fn prepare(&mut self, address: AddressId, failure: FailureId) -> Emitted<Place> {
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
                let pointer = self.get1(value)?;
                if let LayoutKind::Block { payload } | LayoutKind::Box { payload } =
                    self.layout(layout)?.kind
                {
                    // An uncounted value (region storage) points at its payload.
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
            AddressBase::Address(base) => self.prepare(base, failure)?,
        };
        let mut steps = self.pool(info.projection, &self.unit.pools.projections)?;
        // A slot of an uncounted type (region storage) holds a pointer at its payload.
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
                let pointer = self.load_ptr(place.ptr);
                place = self.uncounted_payload(pointer, payload)?;
                if steps.first() == Some(&ProjectionStep::Payload) {
                    steps = &steps[1..];
                }
            }
        }
        for &step in steps {
            place = self.project(place, step, failure)?;
        }
        if place.layout != self.lir_type(info.ty)?.layout {
            return self.invalid("an address whose projection does not reach its type");
        }
        Ok(place)
    }

    /// A value as its layout reads it: an uncounted reference (region storage) at a counted
    /// block layout is rebased to where that block's header would be.
    pub(crate) fn counted_view(&mut self, value: ValueId) -> Emitted<crate::function::V> {
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
        let pointer = self.get1(value)?;
        Ok(crate::function::V::One(
            self.b
                .ins()
                .iadd_imm_s(pointer, (offset as i64).wrapping_neg()),
        ))
    }

    /// Stored fields follow their logical layout's pointer encoding: a read into an
    /// uncounted SSA value reverses `counted_view`.
    pub(crate) fn read_uncounted(
        &mut self,
        value: crate::function::V,
        ty: LirTypeId,
        address: AddressId,
    ) -> Emitted<crate::function::V> {
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
            let Some(pointer) = value.one() else {
                return self.invalid("an uncounted reference that is not a pointer");
            };
            return Ok(crate::function::V::One(
                self.payload_of(pointer, payload)?.ptr,
            ));
        }
        Ok(value)
    }

    /// Regional flat storage includes an immortal header before its payload; other
    /// uncounted storage has no block.
    pub(crate) fn uncounted_payload(
        &mut self,
        pointer: Value,
        payload: LayoutId,
    ) -> Emitted<Place> {
        let record = self.layout(payload)?;
        let block = if record.kind == LayoutKind::Flat {
            let offset = match payload_offset(record.align) {
                Ok(offset) => offset,
                Err(error) => return self.invalid(format!("{error:?}")),
            };
            Some(
                self.b
                    .ins()
                    .iadd_imm_s(pointer, (offset as i64).wrapping_neg()),
            )
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

    /// The payload a block reference value or field points at.
    pub(crate) fn payload(&mut self, pointer: Value, layout: LayoutId) -> Emitted<Place> {
        let record = self.layout(layout)?;
        let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) = record.kind else {
            return self.invalid("an address through a value that is not a block reference");
        };
        let uncounted = record
            .scan
            .iter()
            .any(|step| matches!(step, ScanStep::Uncounted { .. }));
        if uncounted {
            return self.uncounted_payload(pointer, payload);
        }
        self.payload_of(pointer, payload)
    }

    pub(crate) fn project(
        &mut self,
        place: Place,
        step: ProjectionStep,
        failure: FailureId,
    ) -> Emitted<Place> {
        let mut place = place;
        if step == ProjectionStep::Payload {
            if !matches!(self.layout(place.layout)?.kind, LayoutKind::Box { .. }) {
                return self.invalid("Payload requires a Box address");
            }
            let pointer = self.load_ptr(place.ptr);
            return self.payload(pointer, place.layout);
        }
        if let LayoutKind::Block { .. } | LayoutKind::Box { .. } = self.layout(place.layout)?.kind {
            let pointer = self.load_ptr(place.ptr);
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
                // earlier variable field's u64 length prefix.
                if record.kind == LayoutKind::Flat {
                    let mut earlier: Vec<_> = record
                        .fields
                        .iter()
                        .filter(|f| f.offset < field.offset)
                        .collect();
                    earlier.sort_by_key(|f| f.offset);
                    let mut cursor = self.u64c(0);
                    let mut moved = false;
                    for earlier in earlier {
                        if matches!(self.layout(earlier.layout)?.size, SizeSpec::Dynamic(_)) {
                            let at = self.offset(place.ptr, earlier.offset);
                            let at = self.offset_by(at, cursor);
                            let prefix = self.load_u64(at);
                            cursor = self.b.ins().iadd(cursor, prefix);
                            moved = true;
                        }
                    }
                    if moved {
                        let base = self.offset_by(place.ptr, cursor);
                        let ptr = self.offset(base, field.offset);
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
                let active = self.load_tag(place.ptr, tag.offset, tag.form);
                let wrong = self
                    .b
                    .ins()
                    .icmp_imm_s(IntCC::NotEqual, active, i64::from(variant));
                self.guard(wrong, failure)?;
                self.field_place(place, field.offset, field.layout, field.unaligned)
            }
            ProjectionStep::Slot(index) => {
                if let LayoutKind::SoaColumns { element } = record.kind {
                    let index = self.index64(index)?;
                    return self.column_row(place, element, index);
                }
                // An element slot below the capacity, with no count check: the library
                // body proves the bound.
                let LayoutKind::Sequence { element } = record.kind else {
                    return self.invalid("an element slot of a layout that is no sequence");
                };
                let offset = match sequence_elements_offset(self.layout(element)?) {
                    Ok(offset) => offset,
                    Err(error) => return self.invalid(format!("{error:?}")),
                };
                let stride = self.fixed_size(element)?;
                let index = self.index64(index)?;
                let scaled = self.b.ins().imul_imm_s(index, stride as i64);
                let at = self.b.ins().iadd_imm_s(scaled, offset as i64);
                let align = place
                    .align
                    .min(low_bit(stride))
                    .min(low_bit(offset))
                    .min(self.layout(element)?.align);
                Ok(Place {
                    columns: None,
                    ptr: self.offset_by(place.ptr, at),
                    layout: element,
                    align,
                    block: place.block,
                })
            }
            ProjectionStep::FlatAt { index, offset }
                if matches!(record.kind, LayoutKind::Sequential { .. }) =>
            {
                let LayoutKind::Sequential { element } = record.kind else {
                    unreachable!("matched above")
                };
                let count = self.load_u64(place.ptr);
                self.checked_index(index, count, failure)?;
                let first = self.sequential_first(place.ptr, element)?;
                self.flat_content = Some(first);
                let offset = self.index64(offset)?;
                Ok(Place {
                    columns: None,
                    ptr: self.offset_by(first, offset),
                    layout: element,
                    align: self.layout(element)?.align.min(place.align),
                    block: place.block,
                })
            }
            ProjectionStep::FlatAt { index, offset } => {
                let count = self.flat_count(place)?;
                self.checked_index(index, count, failure)?;
                let (element, variable) = self.flat_sequence(place.layout)?;
                if !variable {
                    return self.invalid("a flat walk of a sequence of fixed elements");
                }
                let content = self.offset(place.ptr, 16);
                self.flat_content = Some(content);
                let offset = self.index64(offset)?;
                Ok(Place {
                    columns: None,
                    ptr: self.offset_by(content, offset),
                    layout: element,
                    align: 1,
                    block: place.block,
                })
            }
            ProjectionStep::Index(index)
                if matches!(record.kind, LayoutKind::Sequential { .. }) =>
            {
                let LayoutKind::Sequential { element } = record.kind else {
                    unreachable!("matched above")
                };
                let count = self.load_u64(place.ptr);
                let index = self.checked_index(index, count, failure)?;
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
                let count = self.flat_count(place)?;
                let index = self.checked_index(index, count, failure)?;
                let (element, variable) = self.flat_sequence(place.layout)?;
                let ptr = if variable {
                    let content = self.offset(place.ptr, 16);
                    self.flat_content = Some(content);
                    self.flat_walk(content, index)
                } else {
                    let stride = self.fixed_size(element)?;
                    let scaled = self.b.ins().imul_imm_s(index, stride as i64);
                    let at = self.offset(place.ptr, 8);
                    self.offset_by(at, scaled)
                };
                Ok(Place {
                    columns: None,
                    ptr,
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
                let index = self.checked_index(index, count, failure)?;
                self.column_row(place, element, index)
            }
            ProjectionStep::Index(index) => {
                let (element, count, base, element_offset) = match record.kind {
                    LayoutKind::Inline { element, count } => (element, self.u64c(count), place, 0),
                    LayoutKind::Simd { element, lanes } => {
                        (element, self.u64c(u64::from(lanes)), place, 0)
                    }
                    LayoutKind::Slice { element } => {
                        let word = |ordinal| {
                            record
                                .fields
                                .iter()
                                .find(|field| field.ordinal == ordinal)
                                .map(|field| field.offset)
                        };
                        let (Some(owner_at), Some(start_at), Some(length_at)) =
                            (word(0), word(1), word(2))
                        else {
                            return self.invalid("a slice without owner, start and length");
                        };
                        let owner = self.load_ty(PTR, place.ptr, owner_at);
                        let start = self.load_ty(types::I64, place.ptr, start_at);
                        let count = self.load_ty(types::I64, place.ptr, length_at);
                        let Some(storage) = slice_storage(self.em.layouts(), element) else {
                            return self.invalid("a slice without its byte sequence layout");
                        };
                        let payload = self.payload_of(owner, storage)?;
                        let bytes = self.offset(payload.ptr, 8);
                        let base = Place {
                            columns: None,
                            ptr: self.offset_by(bytes, start),
                            layout: storage,
                            align: 1,
                            block: Some(owner),
                        };
                        (element, count, base, 0)
                    }
                    LayoutKind::Sequence { element } => {
                        let count = self.load_ty(types::I64, place.ptr, 0);
                        let offset = match sequence_elements_offset(self.layout(element)?) {
                            Ok(offset) => offset,
                            Err(error) => return self.invalid(format!("{error:?}")),
                        };
                        (element, count, place, offset)
                    }
                    _ => return self.unsupported("an index into this layout kind"),
                };
                let stride = self.fixed_size(element)?;
                let index64 = self.checked_index(index, count, failure)?;
                let scaled = self.b.ins().imul_imm_s(index64, stride as i64);
                let offset = self.b.ins().iadd_imm_s(scaled, element_offset as i64);
                let align = base
                    .align
                    .min(low_bit(stride))
                    .min(low_bit(element_offset))
                    .min(self.layout(element)?.align);
                Ok(Place {
                    columns: None,
                    ptr: self.offset_by(base.ptr, offset),
                    layout: element,
                    align,
                    block: base.block,
                })
            }
        }
    }

    /// An index value as a u64 word, for an index the validator bounds.
    fn index64(&mut self, index: ValueId) -> Emitted<Value> {
        let v = self.get1(index)?;
        let ty = self.b.func.dfg.value_type(v);
        Ok(match ty.bits() {
            64 => v,
            128 => self.b.ins().ireduce(types::I64, v),
            _ => self.b.ins().uextend(types::I64, v),
        })
    }

    /// The integer `index` as an `i64` below `count`; a negative, wider or outside index
    /// takes `failure` (MEM-14).
    pub(crate) fn checked_index(
        &mut self,
        index: ValueId,
        count: Value,
        failure: FailureId,
    ) -> Emitted<Value> {
        let kind = self.scalar_kind(index)?;
        let Some((width, signed)) = crate::scalar::integer(kind) else {
            return self.invalid("an index that is not an integer");
        };
        let index = self.get1(index)?;
        let mut failed = self.b.ins().iconst(types::I8, 0);
        if signed {
            let negative = self.icmp_const(IntCC::SignedLessThan, index, 0);
            failed = self.b.ins().bor(failed, negative);
        }
        let index64 = match width.cmp(&64) {
            std::cmp::Ordering::Less => self.b.ins().uextend(types::I64, index),
            std::cmp::Ordering::Equal => index,
            std::cmp::Ordering::Greater => {
                let high = self.b.ins().ushr_imm_s(index, 64);
                let wide = self.icmp_const(IntCC::NotEqual, high, 0);
                failed = self.b.ins().bor(failed, wide);
                self.b.ins().ireduce(types::I64, index)
            }
        };
        // MEM-14: the index must be below the count.
        let outside = self
            .b
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, index64, count);
        failed = self.b.ins().bor(failed, outside);
        self.guard(failed, failure)?;
        Ok(index64)
    }
}
