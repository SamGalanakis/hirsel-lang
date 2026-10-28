//! Tracked arena blocks (MEM-6, runtime ABI 11), as `hirsel-llvm`'s `bounded` emits them
//! without its sequence-state facts: a List or Array an arena holds is tracked when the
//! program opens a bounded arena, its pushes are charged by the runtime, a writability
//! test reads its count, and a semantic copy marks it shared.

use cranelift_codegen::ir::{InstBuilder, Value, condcodes::IntCC, types};
use cranelift_frontend::Switch;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{DropStep, LayoutId};
use hirsel_lir::runtime_abi::{
    COUNT_ARENA_SHARED, COUNT_ARENA_UNIQUE, COUNT_OFFSET, COUNT_UNIQUE, RuntimeEntry,
    TRACKED_REGION_BACK,
};

use crate::function::{FnEmitter, low_bit};
use crate::module::{Emitted, PTR};

impl FnEmitter<'_, '_, '_> {
    /// Whether this program builds tracked arena blocks: only charges observe them, so a
    /// program that opens no bounded arena builds none, and its pushes and writability
    /// tests read no tracked count.
    pub(crate) fn tracking(&self) -> bool {
        self.em.inputs.unit().tracking
    }

    /// Whether the construction `site` builds a tracked arena block.
    pub(crate) fn tracks(&self, site: &ConstructSite) -> bool {
        site.tracked && self.tracking()
    }

    /// The arena record of the tracked arena block `block`.
    pub(crate) fn tracked_region(&mut self, block: Value) -> Value {
        let at = self
            .b
            .ins()
            .iadd_imm_s(block, -(TRACKED_REGION_BACK as i64));
        self.load_ptr(at)
    }

    /// `IsWritable`: a unique block, or an unshared tracked arena block.
    pub(crate) fn is_writable(&mut self, block: Value) -> Value {
        if !self.tracking() {
            return self.is_unique(block);
        }
        let count = self.load_ty(types::I32, block, COUNT_OFFSET);
        let unique = self
            .b
            .ins()
            .icmp_imm_s(IntCC::Equal, count, COUNT_UNIQUE as i64);
        let unshared = self
            .b
            .ins()
            .icmp_imm_s(IntCC::Equal, count, COUNT_ARENA_UNIQUE as i64);
        self.b.ins().bor(unique, unshared)
    }

    /// Whether `block` is a tracked arena block: its count is `COUNT_ARENA_UNIQUE` or
    /// `COUNT_ARENA_SHARED`.
    pub(crate) fn is_tracked(&mut self, block: Value) -> Value {
        let count = self.load_ty(types::I32, block, COUNT_OFFSET);
        let offset = self.b.ins().iadd_imm_s(count, -(COUNT_ARENA_SHARED as i64));
        self.b.ins().icmp_imm_s(IntCC::UnsignedLessThan, offset, 2)
    }

    /// `Share`: a semantic copy marks every unshared tracked arena block `value` holds at
    /// inline depth shared. A program that opens no bounded arena emits nothing.
    pub(crate) fn share(&mut self, value: ValueId) -> Emitted {
        if !self.tracking() {
            return Ok(());
        }
        let ty = self.value_type(value)?;
        if ty.repr == ValueRepr::Block {
            let block = self.get1(value)?;
            self.share_block(block);
            return Ok(());
        }
        let shape = self.layout_shape(ty.layout)?;
        let slot = self.stack(ty.layout)?;
        let image = self.get(value)?;
        self.store(shape, image, slot)?;
        let record = self.layout(ty.layout)?;
        self.share_steps(slot.ptr, record.align, &record.drop)
    }

    fn share_block(&mut self, block: Value) {
        let count = self.load_ty(types::I32, block, COUNT_OFFSET);
        let unshared = self
            .b
            .ins()
            .icmp_imm_s(IntCC::Equal, count, COUNT_ARENA_UNIQUE as i64);
        let (mark, done) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(unshared, mark, &[], done, &[]);
        self.b.switch_to_block(mark);
        let shared = self
            .b
            .ins()
            .iconst(types::I32, i64::from(COUNT_ARENA_SHARED as u32 as i32));
        self.store_at(shared, block, COUNT_OFFSET);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
    }

    /// Mark the blocks the drop program `steps` over the inline value at `base` releases.
    fn share_steps(&mut self, base: Value, align: u32, steps: &[DropStep]) -> Emitted {
        for step in steps {
            match step {
                DropStep::Release { offset } => {
                    let child = self.load_ty(PTR, base, *offset);
                    self.share_block(child);
                }
                DropStep::Child { offset, layout } => {
                    let steps = &self.layout(*layout)?.drop;
                    let at = self.offset(base, *offset);
                    self.share_steps(at, align.min(low_bit(*offset)), steps)?;
                }
                DropStep::RepeatReverse {
                    offset,
                    count,
                    stride,
                    element,
                } => {
                    let steps = &self.layout(*element)?.drop;
                    for i in 0..*count {
                        let offset = offset + i * stride;
                        let at = self.offset(base, offset);
                        self.share_steps(at, align.min(low_bit(offset)), steps)?;
                    }
                }
                DropStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let tag = self.load_tag(base, *tag_offset, *form);
                    let tag = self.b.ins().uextend(types::I64, tag);
                    let join = self.b.create_block();
                    let mut switch = Switch::new();
                    let mut arms = Vec::new();
                    for (variant, steps) in variants {
                        if steps.is_empty() {
                            continue;
                        }
                        let arm = self.b.create_block();
                        switch.set_entry(u128::from(*variant), arm);
                        arms.push((arm, steps));
                    }
                    switch.emit(&mut self.b, tag, join);
                    for (arm, steps) in arms {
                        self.b.switch_to_block(arm);
                        self.share_steps(base, align, steps)?;
                        self.b.ins().jump(join, &[]);
                    }
                    self.b.switch_to_block(join);
                }
                // Finalizers, environments, handles and frozen roots hold no tracked block
                // at inline depth; element loops are a sequence payload's.
                _ => {}
            }
        }
        Ok(())
    }

    /// `ArenaPush`: the runtime charges a tracked arena List at `address` one element, or
    /// `count`, and gives it room, possibly copying or relocating it within its arena; any
    /// other block it leaves as it is. An exhausted push changes nothing.
    pub(crate) fn arena_push(
        &mut self,
        address: AddressId,
        count: Option<ValueId>,
        failure: FailureId,
    ) -> Emitted {
        if self.cpu_reserve()? {
            return Ok(());
        }
        let place = self.place(address)?;
        // A Packed List's elements have no stride: the runtime only charges it and
        // copies a shared one, so it reads no element shape.
        let (size, align) = match self.sequence_element(place.layout)? {
            Some(element) => (
                self.fixed_size(element)?,
                u64::from(self.layout(element)?.align),
            ),
            None if self.sequential_payload(place.layout)? => (0, 8),
            None => return self.invalid("an arena push of a place that holds no List block"),
        };
        if !self.tracking() {
            return Ok(());
        }
        let block = self.load_ptr(place.ptr);
        let out = self.out_word();
        let (size, align) = (self.u64c(size), self.u64c(align));
        let context = self.context;
        let status = match count {
            None => self.rt1(RuntimeEntry::ArenaPush, &[context, block, size, align, out])?,
            Some(count) => {
                let count = self.get1(count)?;
                self.rt1(
                    RuntimeEntry::ArenaReserve,
                    &[context, block, count, size, align, out],
                )?
            }
        };
        let Some(anchor) = self
            .unit
            .tables
            .failures
            .get(failure.index())
            .map(|f| f.anchor)
        else {
            return self.invalid("a failure outside the failure table");
        };
        self.tracked_exhaustion(status, block, anchor, failure)?;
        let pushed = self.load_ptr(out);
        self.store_at(pushed, place.ptr, 0);
        Ok(())
    }

    /// Whether `layout` is a block of a sequential sequence of `Packed` values (MEM-13).
    fn sequential_payload(&self, layout: LayoutId) -> Emitted<bool> {
        Ok(match self.layout_payload(layout)? {
            Some(payload) => matches!(
                self.layout(payload)?.kind,
                hirsel_lir::layout_types::LayoutKind::Sequential { .. }
            ),
            None => false,
        })
    }

    /// Takes the exhaustion of the arena the tracked block `block` names when `status`
    /// is nonzero, and continues in a fresh block.
    pub(crate) fn tracked_exhaustion(
        &mut self,
        status: Value,
        block: Value,
        anchor: SourceAnchorId,
        failure: FailureId,
    ) -> Emitted {
        let (over, next) = (self.b.create_block(), self.b.create_block());
        self.b.set_cold_block(over);
        self.b.ins().brif(status, over, &[], next, &[]);
        self.b.switch_to_block(over);
        let region = self.tracked_region(block);
        let exhausted = self.exhausted_block(region, anchor, failure)?;
        self.b.ins().jump(exhausted, &[]);
        self.b.switch_to_block(next);
        Ok(())
    }
}
