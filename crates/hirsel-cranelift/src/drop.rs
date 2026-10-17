//! Copies and releases of inline values: a semantic Copy adds one credit per counted holder
//! of the layout's scan program, and an inline `Release` walks the layout's drop program in
//! drop order, releasing each counted leaf through `hirsel_rt_v2_release` with the
//! descriptor its own header names. A failing leaf does not stop the walk; its status is
//! accumulated and the failure edge is taken after it, as a block release accumulates.

use cranelift_codegen::ir::{InstBuilder, Value, condcodes::IntCC, types};
use cranelift_frontend::Switch;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{DropStep, LayoutId, LayoutKind, ScanStep};
use hirsel_lir::runtime_abi::RuntimeEntry;

use crate::function::{FnEmitter, Place};
use crate::module::{Emitted, PTR};

impl<'a> FnEmitter<'_, 'a, '_> {
    /// Adds one credit to every counted holder of the value at `place`.
    pub(crate) fn dup_holders(&mut self, place: Place) -> Emitted {
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
                ScanStep::Sequence { .. } | ScanStep::Columns(_) => {
                    return self.unsupported("a copy of an inline sequence");
                }
            }
        }
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
        let record = self.layout(layout)?;
        // Byte ranges of the top-level fields the mask marks dead.
        let dead: Vec<(u64, u64)> = match live {
            MaskRef::All => Vec::new(),
            MaskRef::Static(mask) => {
                let Some(mask) = self.unit.tables.masks.get(mask.index()) else {
                    return self.invalid("a mask outside the mask table");
                };
                let words = self.pool(mask.words, &self.unit.pools.mask_words)?;
                if record.tag.is_some() {
                    return self.unsupported("a live mask on an inline variant release");
                }
                let mut dead = Vec::new();
                for field in &record.fields {
                    let i = field.ordinal as usize;
                    let set = words.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 == 1);
                    if !set {
                        let size = self.fixed_size(field.layout)?;
                        dead.push((field.offset, field.offset + size));
                    }
                }
                dead
            }
            MaskRef::Dynamic(_) => {
                return self.unsupported("a dynamic live mask on an inline release");
            }
        };
        let anchor = self.cleanup_code(anchor)?;
        let anchor = self.u64c(anchor);
        let (status, _) = self.stack_bytes(1, 1);
        let zero = self.b.ins().iconst(types::I8, 0);
        self.store_at(zero, status, 0);
        let walk = Walk { anchor, status };
        for step in &record.drop {
            let offset = match step {
                DropStep::Release { offset }
                | DropStep::Environment { offset }
                | DropStep::CloseHandle { offset }
                | DropStep::Child { offset, .. }
                | DropStep::RepeatReverse { offset, .. } => Some(*offset),
                _ => None,
            };
            if offset.is_some_and(|o| dead.iter().any(|&(a, b)| a <= o && o < b)) {
                continue;
            }
            self.drop_step(walk, place.ptr, step)?;
        }
        let failed = self.load_ty(types::I8, status, 0);
        self.status_guard(failed, failure)
    }

    /// `status |= result`.
    fn accumulate(&mut self, walk: Walk, result: Value) {
        let before = self.load_ty(types::I8, walk.status, 0);
        let after = self.b.ins().bor(before, result);
        self.store_at(after, walk.status, 0);
    }

    /// Consumes one credit of `block` through the runtime, with its header's descriptor.
    fn release_leaf(&mut self, walk: Walk, block: Value) -> Emitted {
        let descriptor = self.block_descriptor(block);
        let null = self.b.ins().iconst(PTR, 0);
        let context = self.context;
        let status = self.rt1(
            RuntimeEntry::Release,
            &[context, block, descriptor, null, walk.anchor],
        )?;
        self.accumulate(walk, status);
        Ok(())
    }

    fn drop_steps(&mut self, walk: Walk, base: Value, steps: &'a [DropStep]) -> Emitted {
        for step in steps {
            self.drop_step(walk, base, step)?;
        }
        Ok(())
    }

    /// One step of a drop program over the value at `base`.
    fn drop_step(&mut self, walk: Walk, base: Value, step: &'a DropStep) -> Emitted {
        match step {
            DropStep::Release { offset } | DropStep::Environment { offset } => {
                let block = self.load_ty(PTR, base, *offset);
                self.release_leaf(walk, block)
            }
            DropStep::CloseHandle { offset } => {
                // The Io adapter closes the handle, shielded; a failing close is recorded and
                // the walk goes on.
                let handle = self.load_ty(types::I64, base, *offset);
                let context = self.context;
                let status =
                    self.rt1(RuntimeEntry::CloseHandle, &[context, handle, walk.anchor])?;
                self.accumulate(walk, status);
                Ok(())
            }
            DropStep::Finalize => self.unsupported("an inline drop that reaches a finalizer"),
            DropStep::Child { offset, layout } => {
                let child = self.layout(*layout)?;
                if child.finalizer.is_some() {
                    return self.unsupported("an inline drop that reaches a finalizer");
                }
                let at = self.offset(base, *offset);
                self.drop_steps(walk, at, &child.drop)
            }
            DropStep::ActiveVariant {
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
                    self.drop_steps(walk, base, steps)?;
                    self.b.ins().jump(done, &[]);
                }
                self.b.switch_to_block(done);
                Ok(())
            }
            DropStep::RepeatReverse {
                offset,
                count,
                stride,
                element,
            } => {
                let drop = &self.layout(*element)?.drop;
                if drop.is_empty() {
                    return Ok(());
                }
                let first = self.offset(base, *offset);
                let count = self.u64c(*count);
                self.drop_elements(walk, first, count, *stride, drop)
            }
            DropStep::ElementsReverse {
                count_offset,
                offset,
                stride,
                element,
            } => {
                let drop = &self.layout(*element)?.drop;
                if drop.is_empty() {
                    return Ok(());
                }
                let count = self.load_ty(types::I64, base, *count_offset);
                let first = self.offset(base, *offset);
                self.drop_elements(walk, first, count, *stride, drop)
            }
            DropStep::ColumnsReverse(_) => {
                self.unsupported("an inline drop of structure-of-arrays columns")
            }
            DropStep::Thaw { .. } => self.invalid("compiled drop code that reaches a frozen value"),
        }
    }

    /// `drop` over each of `count` elements `stride` bytes apart from `first`, the highest
    /// index first.
    fn drop_elements(
        &mut self,
        walk: Walk,
        first: Value,
        count: Value,
        stride: u64,
        drop: &'a [DropStep],
    ) -> Emitted {
        let (head, body, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        let left = self.b.append_block_param(head, types::I64);
        self.b.ins().jump(head, &[count.into()]);
        self.b.switch_to_block(head);
        let none = self.b.ins().icmp_imm_s(IntCC::Equal, left, 0);
        self.b.ins().brif(none, done, &[], body, &[]);
        self.b.switch_to_block(body);
        let index = self.b.ins().iadd_imm_s(left, -1);
        let offset = self.b.ins().imul_imm_s(index, stride as i64);
        let element = self.b.ins().iadd(first, offset);
        self.drop_steps(walk, element, drop)?;
        self.b.ins().jump(head, &[index.into()]);
        self.b.switch_to_block(done);
        Ok(())
    }
}

/// An inline drop's cleanup anchor and its accumulated status byte.
#[derive(Clone, Copy)]
struct Walk {
    anchor: Value,
    status: Value,
}
