//! Copies and releases of inline values: a semantic Copy adds one credit per counted holder
//! of the layout's scan program, and an inline `Release` walks the layout's drop program in
//! drop order, releasing each counted leaf through `hirsel_rt_v2_release` with the
//! descriptor its own header names. A failing leaf does not stop the walk; its status is
//! accumulated and the failure edge is taken after it, as a block release accumulates.

use cranelift_codegen::ir::{InstBuilder, Value, condcodes::IntCC, types};
use cranelift_frontend::Switch;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, LayoutKind, ScanStep};
use hirsel_lir::runtime_abi::RuntimeEntry;

use crate::function::{FnEmitter, Place};
use crate::module::{Emitted, PTR};

impl<'a> FnEmitter<'_, 'a, '_> {
    /// Adds one credit to every counted holder of the value at `place`.
    pub(crate) fn dup_holders(&mut self, place: Place) -> Emitted {
        let place = self.gather(place)?;
        let record = self.layout(place.layout)?;
        if let LayoutKind::Block { .. } | LayoutKind::Box { .. } = record.kind {
            let block = self.load_ptr(place.ptr);
            return self.dup_block(block);
        }
        self.dup_scan(place.ptr, &record.scan)
    }

    fn dup_scan(&mut self, base: Value, scan: &'a [ScanStep]) -> Emitted {
        for step in scan {
            match step {
                ScanStep::Counted { offset }
                | ScanStep::SliceOwner { offset }
                | ScanStep::Environment { offset } => {
                    let block = self.load_ty(PTR, base, *offset);
                    self.dup_block(block)?;
                }
                ScanStep::Uncounted { .. } | ScanStep::Handle { .. } => {}
                ScanStep::Inline { offset, layout } => {
                    let child = self.offset(base, *offset);
                    let scan = &self.layout(*layout)?.scan;
                    self.dup_scan(child, scan)?;
                }
                ScanStep::Repeat {
                    offset,
                    count,
                    stride,
                    element,
                } => {
                    let scan = &self.layout(*element)?.scan;
                    for i in 0..*count {
                        let child = self.offset(base, offset + i * stride);
                        self.dup_scan(child, scan)?;
                    }
                }
                ScanStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let active = self.load_tag(base, *tag_offset, *form);
                    let active = self.b.ins().uextend(types::I64, active);
                    let done = self.b.create_block();
                    let mut switch = Switch::new();
                    let mut arms = Vec::new();
                    for (variant, steps) in variants {
                        let arm = self.b.create_block();
                        switch.set_entry(u128::from(*variant), arm);
                        arms.push((arm, steps));
                    }
                    switch.emit(&mut self.b, active, done);
                    for (arm, steps) in arms {
                        self.b.switch_to_block(arm);
                        self.dup_scan(base, steps)?;
                        self.b.ins().jump(done, &[]);
                    }
                    self.b.switch_to_block(done);
                }
                ScanStep::Sequence {
                    count_offset,
                    offset,
                    stride,
                    element,
                } => {
                    let count = self.load_ty(types::I64, base, *count_offset);
                    let first = self.offset(base, *offset);
                    let scan = &self.layout(*element)?.scan;
                    self.dup_elements(first, count, *stride, scan)?;
                }
                ScanStep::Columns(columns) => {
                    let count = self.load_ty(types::I64, base, columns.count_offset);
                    let capacity = self.load_ty(types::I64, base, columns.capacity_offset);
                    for column in &columns.columns {
                        let offset = self.b.ins().imul_imm_s(capacity, column.prefix as i64);
                        let data = self.offset(base, columns.data_offset);
                        let first = self.offset_by(data, offset);
                        let scan = &self.layout(column.layout)?.scan;
                        self.dup_elements(first, count, column.stride, scan)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn dup_elements(
        &mut self,
        first: Value,
        count: Value,
        stride: u64,
        scan: &'a [ScanStep],
    ) -> Emitted {
        if scan.is_empty() {
            return Ok(());
        }
        let head = self.b.create_block();
        let body = self.b.create_block();
        let done = self.b.create_block();
        let index = self.b.append_block_param(head, types::I64);
        let zero = self.u64c(0);
        self.b.ins().jump(head, &[zero.into()]);
        self.b.switch_to_block(head);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, index, count);
        self.b.ins().brif(more, body, &[], done, &[]);
        self.b.switch_to_block(body);
        let offset = self.b.ins().imul_imm_s(index, stride as i64);
        let child = self.offset_by(first, offset);
        self.dup_scan(child, scan)?;
        let next = self.b.ins().iadd_imm_s(index, 1);
        self.b.ins().jump(head, &[next.into()]);
        self.b.switch_to_block(done);
        Ok(())
    }

    /// `Release` of an owning inline value: its drop program walked in drop order.
    /// `live` gates top-level fields.
    pub(crate) fn release_inline(
        &mut self,
        value: ValueId,
        layout: LayoutId,
        live: MaskRef,
        anchor: CleanupAnchorId,
        failure: FailureId,
    ) -> Emitted {
        let ty = self.value_type(value)?;
        if ty.layout != layout
            || !matches!(
                ty.repr,
                ValueRepr::Aggregate | ValueRepr::Callable | ValueRepr::Handle
            )
        {
            return self.invalid("an inline release at another layout or representation");
        }
        let place = self.stack(layout)?;
        let shape = self.value_shape(value)?;
        let v = self.get(value)?;
        self.store(shape, v, place)?;
        let status = self.finalize_inline(place, live, anchor)?;
        self.status_guard(status, failure)
    }

    fn finalize_inline(
        &mut self,
        place: Place,
        live: MaskRef,
        anchor: CleanupAnchorId,
    ) -> Emitted<Value> {
        // The runtime already implements finalizers, columns, ordered child drops and
        // field-ordinal live masks. Use its inline continuation rather than duplicating
        // that interpreter here (including its shielding and failure accumulation).
        let descriptor = self.descriptor_value(place.layout)?;
        let mask = self.live_mask(live)?;
        let anchor = self.cleanup_code(anchor)?;
        let (anchor, operation_site) = self.failure_sites(anchor);
        // hirsel_rt_v2_cleanup: two u32s, a u64 cursor and two pointers on both targets.
        let (continuation, _) = self.stack_bytes(32, 8);
        self.rt1(
            RuntimeEntry::Finalize,
            &[
                self.context,
                descriptor,
                place.ptr,
                mask,
                continuation,
                anchor,
                operation_site,
            ],
        )
    }
}

#[cfg(test)]
mod tests;
