//! Bounded arenas (MEM-6). An arena record's `remaining` word is its logical budget,
//! `u64::MAX` when unbounded. Every arena construction tests its charge against it and
//! lowers it inline; an excess records the arena's own `OutOfSpace` and takes the
//! construction's failure edge. A program with no `Recover` opens no bounded arena, so
//! its constructions are not charged.
use super::*;
use hirsel_lir::emission::{BOUNDED_RULE, KIND_BUG};

impl<'m, 'a> FnEmitter<'m, 'a> {
    /// A failure's `(kind, rule)` codes for `hirsel_rt_v2_failure_begin`: the category, and
    /// the rule's code loaded from its glue cell (`hirsel_lir::glue`).
    pub(super) fn failure_code(&mut self, rule: &str) -> (Value<'m>, Value<'m>) {
        let i32_ty = self.module.int_type(32);
        let cell = self.glue_cell(&hirsel_lir::glue::failure_code_symbol(rule), i32_ty);
        (
            self.module.const_int(i32_ty, u128::from(KIND_BUG)),
            self.builder.load(i32_ty, cell, 4),
        )
    }

    /// Whether this program builds tracked arena blocks (MEM-6): only charges observe them,
    /// so a program that opens no bounded arena builds none, and its pushes, writability
    /// tests and growth read no tracked count.
    pub(super) fn tracking(&self) -> bool {
        self.inputs.tracking
    }

    /// Whether the construction `site` builds a tracked arena block.
    pub(super) fn tracks(&self, site: &ConstructSite) -> bool {
        site.tracked && self.tracking()
    }

    /// A block that records the arena `region`'s own `OutOfSpace` at `anchor`, disposes
    /// `failure`'s tokens and enters its cleanup.
    pub(super) fn exhausted_block(
        &mut self,
        region: Value<'m>,
        anchor: SourceAnchorId,
        failure: FailureId,
    ) -> Emitted<BasicBlock<'m>> {
        self.exhausted_block_with_values(region, anchor, failure, &[])
    }

    fn exhausted_block_with_values(
        &mut self,
        region: Value<'m>,
        anchor: SourceAnchorId,
        failure: FailureId,
        values: &[ValueId],
    ) -> Emitted<BasicBlock<'m>> {
        let edge = self.failure_edge(failure)?;
        let here = self.builder.current();
        let block = self.module.append_block(self.function);
        self.builder.position(block);
        let (site, operation_site) = self.failure_sites(self.anchor_code(anchor)?);
        let exhausted = self.runtime(RuntimeEntry::ArenaExhausted)?;
        let (kind, rule) = self.failure_code(BOUNDED_RULE);
        self.builder.call(
            exhausted,
            &[self.context, region, kind, rule, site, operation_site],
        );
        for &value in values.iter().rev() {
            let ty = self.value_type(value)?;
            if self.unit.values[value.index()].holding != Holding::Owned
                || self.layout(ty.layout)?.drop.is_empty()
            {
                continue;
            }
            let place = self.stack(ty.layout)?;
            self.store(self.value_shape(value)?, self.get(value)?, place);
            let descriptor = self.descriptor(ty.layout)?;
            let mask = self.module.const_zero(self.module.ptr_type());
            let continuation = self
                .prologue
                .alloca(self.module.array_type(self.module.int_type(64), 4), 8);
            let finalize = self.runtime(RuntimeEntry::Finalize)?;
            self.builder.call(
                finalize,
                &[
                    self.context,
                    descriptor,
                    place.ptr,
                    mask,
                    continuation,
                    site,
                    operation_site,
                ],
            );
        }
        let dispose = self.pool(edge.dispose, &self.unit.pools.values)?;
        if !dispose.is_empty() {
            let entry = self.runtime(RuntimeEntry::Dispose)?;
            for &token in dispose {
                self.builder.call(entry, &[self.context, self.get(token)?]);
            }
        }
        self.jump(edge.cleanup)?;
        self.builder.position(here);
        Ok(block)
    }

    /// Charge `bytes` to the arena record `region` before an arena construction: fail when
    /// they exceed its budget, else lower it.
    pub(super) fn charge_construction(
        &mut self,
        region: Value<'m>,
        bytes: u64,
        site: &ConstructSite,
        failure: FailureId,
    ) -> Emitted {
        let values = self
            .pool(site.fields, &self.unit.pools.fields)?
            .iter()
            .map(|field| field.value)
            .collect::<Vec<_>>();
        self.charge_with_values(region, bytes, site.anchor, failure, &values)
    }

    fn charge_with_values(
        &mut self,
        region: Value<'m>,
        bytes: u64,
        anchor: SourceAnchorId,
        failure: FailureId,
        values: &[ValueId],
    ) -> Emitted {
        if !self.tracking() || bytes == 0 {
            return Ok(());
        }
        let word = self.module.int_type(64);
        let address = self.builder.offset(
            region,
            self.const_u64(hirsel_lir::runtime_abi::REGION_REMAINING_OFFSET),
        );
        let remaining = self.builder.load(word, address, 8);
        let bytes = self.const_u64(bytes);
        let over = self.builder.icmp(IntPredicate::Ugt, bytes, remaining);
        let exhausted = self.exhausted_block_with_values(region, anchor, failure, values)?;
        let fits = self.module.append_block(self.function);
        self.builder.cold_cond_br(over, exhausted, fits);
        self.builder.position(fits);
        let left = self.builder.binary(BinOp::Sub, remaining, bytes);
        self.builder.store(left, address, 8);
        Ok(())
    }

    /// The charge of a block construction into an arena: its payload's logical size, a
    /// sequence's per element.
    pub(super) fn construction_charge(&self, site: &ConstructSite) -> Emitted<u64> {
        let fields = self.pool(site.fields, &self.unit.pools.fields)?.len() as u64;
        match hirsel_lir::layout::block_charge(self.inputs.layouts, site.layout) {
            Some(charge) => Ok(charge.bytes(fields)),
            None => self.unsupported("a bounded-arena charge of a flat payload"),
        }
    }

    /// A deep copy's status: `STATUS_EXHAUSTED` records the destination arena's own
    /// `OutOfSpace` and takes `failure`.
    pub(super) fn copy_status(
        &mut self,
        status: Value<'m>,
        destination: Value<'m>,
        anchor: SourceAnchorId,
        failure: FailureId,
    ) -> Emitted {
        if !self.tracking() {
            return self.status_guard(status, failure);
        }
        let zero = self.module.const_int(self.module.int_type(8), 0);
        let failed = self.builder.icmp(IntPredicate::Ne, status, zero);
        let exhausted = self.exhausted_block(destination, anchor, failure)?;
        let next = self.module.append_block(self.function);
        self.builder.cold_cond_br(failed, exhausted, next);
        self.builder.position(next);
        Ok(())
    }

    /// The arena record of the tracked arena block `block` (runtime ABI 11).
    pub(super) fn tracked_region(&self, block: Value<'m>) -> Value<'m> {
        let address = self.builder.offset(
            block,
            self.const_u64(0u64.wrapping_sub(hirsel_lir::runtime_abi::TRACKED_REGION_BACK)),
        );
        self.builder.load(self.module.ptr_type(), address, 8)
    }

    /// `Share`: a semantic copy marks every unshared tracked arena block `value` holds at
    /// inline depth shared. Only charges observe it, so a program that opens no bounded
    /// arena emits nothing; a block a copy reaches through another one becomes shared when
    /// that one is copied on its write, which dups it.
    pub(super) fn share(&mut self, value: ValueId) -> Emitted {
        if !self.tracking() {
            return Ok(());
        }
        let ty = self.value_type(value)?;
        if ty.repr == ValueRepr::Block {
            let block = self.get(value)?;
            self.share_block(block);
            return Ok(());
        }
        let shape = self.layout_shape(ty.layout)?;
        let slot = self.stack(ty.layout)?;
        let image = self.get(value)?;
        self.store(shape, image, slot);
        let record = self.layout(ty.layout)?;
        let (align, steps) = (record.align, record.drop.clone());
        self.share_steps(slot.ptr, align, &steps)
    }

    fn share_block(&self, block: Value<'m>) {
        use hirsel_lir::runtime_abi::{COUNT_ARENA_SHARED, COUNT_ARENA_UNIQUE, COUNT_OFFSET};
        let i32_ty = self.module.int_type(32);
        let word = if COUNT_OFFSET == 0 {
            block
        } else {
            self.builder.offset(block, self.const_u64(COUNT_OFFSET))
        };
        let count = self
            .builder
            .load_access(i32_ty, word, 4, crate::context::Access::Count);
        let unshared = self.builder.icmp(
            IntPredicate::Eq,
            count,
            self.module
                .const_int(i32_ty, u128::from(COUNT_ARENA_UNIQUE)),
        );
        let (mark, done) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        self.builder.cond_br(unshared, mark, done);
        self.builder.position(mark);
        self.builder.store(
            self.module
                .const_int(i32_ty, u128::from(COUNT_ARENA_SHARED)),
            word,
            4,
        );
        self.builder.br(done);
        self.builder.position(done);
    }

    /// Mark the blocks the drop program `steps` over the inline value at `base` releases.
    fn share_steps(&mut self, base: Value<'m>, align: u32, steps: &[DropStep]) -> Emitted {
        let at = |this: &Self, offset: u64| {
            if offset == 0 {
                base
            } else {
                this.builder.offset(base, this.const_u64(offset))
            }
        };
        for step in steps {
            match step {
                DropStep::Release { offset } => {
                    let child = self.builder.load(
                        self.module.ptr_type(),
                        at(self, *offset),
                        align.min(low_bit(*offset)).min(8),
                    );
                    self.share_block(child);
                }
                DropStep::Child { offset, layout } => {
                    let steps = self.layout(*layout)?.drop.clone();
                    let base = at(self, *offset);
                    self.share_steps(base, align.min(low_bit(*offset)), &steps)?;
                }
                DropStep::RepeatReverse {
                    offset,
                    count,
                    stride,
                    element,
                } => {
                    let steps = self.layout(*element)?.drop.clone();
                    for i in 0..*count {
                        let offset = offset + i * stride;
                        let base = at(self, offset);
                        self.share_steps(base, align.min(low_bit(offset)), &steps)?;
                    }
                }
                DropStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let i32_ty = self.module.int_type(32);
                    let tag = load_tag(&self.builder, base, align, *tag_offset, *form, false);
                    let join = self.module.append_block(self.function);
                    for (variant, steps) in variants {
                        if steps.is_empty() {
                            continue;
                        }
                        let (yes, next) = (
                            self.module.append_block(self.function),
                            self.module.append_block(self.function),
                        );
                        let active = self.builder.icmp(
                            IntPredicate::Eq,
                            tag,
                            self.module.const_int(i32_ty, u128::from(*variant)),
                        );
                        self.builder.cond_br(active, yes, next);
                        self.builder.position(yes);
                        self.share_steps(base, align, steps)?;
                        self.builder.br(join);
                        self.builder.position(next);
                    }
                    self.builder.br(join);
                    self.builder.position(join);
                }
                // Finalizers, environments, handles and frozen roots hold no tracked block
                // at inline depth; element loops are a sequence payload's.
                _ => {}
            }
        }
        Ok(())
    }

    /// `ArenaPush`: a tracked arena List at `address` is charged one element, or `count`,
    /// and given room for them by the runtime, which may copy or relocate it within its arena
    /// (MEM-6). Any other block passes one count test; its deferred length stays in its
    /// register.
    pub(super) fn arena_push(
        &mut self,
        address: AddressId,
        count: Option<ValueId>,
        failure: FailureId,
    ) -> Emitted {
        use hirsel_lir::runtime_abi::{COUNT_ARENA_SHARED, COUNT_ARENA_UNIQUE, COUNT_OFFSET};
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
        let block = self.load(Shape::Scalar(ScalarKind::Pointer), place);
        let (slow, done) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        // A must-unique or must-untracked block is no tracked one: its push needs no
        // count read, and one read finds the block untracked for every later push.
        let state = self.sequence_state_at(place);
        let i1 = self.module.int_type(1);
        if let Some(state) = state {
            let unique = self.builder.load(i1, state.unique, 1);
            let untracked = self.builder.load(i1, state.untracked, 1);
            let known = self.builder.binary(BinOp::Or, unique, untracked);
            let test = self.module.append_block(self.function);
            self.builder.cond_br(known, done, test);
            self.builder.position(test);
        }
        let tracked = crate::runtime::is_tracked(&self.builder, block);
        let untracked = self.module.append_block(self.function);
        self.builder.cold_cond_br(tracked, slow, untracked);
        self.builder.position(untracked);
        if let Some(state) = state {
            self.builder
                .store(self.module.const_int(i1, 1), state.untracked, 1);
        }
        self.builder.br(done);
        self.builder.position(slow);
        // The runtime reads the count in the block and may move it.
        if let Some(slot) = self.whole_slot(address) {
            self.flush_length(slot.index())?;
        }
        // The block goes by value and comes back through a cell of its own, so the
        // place's slot stays promotable; an exhausted push leaves the cell as it was.
        let out = self.prologue.alloca(self.module.ptr_type(), 8);
        self.builder.store(block, out, 8);
        let status = match count {
            None => {
                let entry = self.runtime(RuntimeEntry::ArenaPush)?;
                self.builder.call(
                    entry,
                    &[
                        self.context,
                        block,
                        self.const_u64(size),
                        self.const_u64(align),
                        out,
                    ],
                )
            }
            Some(count) => {
                let entry = self.runtime(RuntimeEntry::ArenaReserve)?;
                let count = self.get(count)?;
                self.builder.call(
                    entry,
                    &[
                        self.context,
                        block,
                        count,
                        self.const_u64(size),
                        self.const_u64(align),
                        out,
                    ],
                )
            }
        };
        let pushed = self.builder.load(self.module.ptr_type(), out, 8);
        self.store(Shape::Scalar(ScalarKind::Pointer), pushed, place);
        if let Some(state) = self.sequence_state_at(place) {
            let no = self.module.const_zero(self.module.int_type(1));
            self.builder.store(no, state.unique, 1);
            self.builder.store(no, state.untracked, 1);
        }
        if self.tracking() {
            let zero = self.module.const_int(self.module.int_type(8), 0);
            let failed = self.builder.icmp(IntPredicate::Ne, status, zero);
            // An exhausted push changes nothing: the block still names its arena.
            let over = self.module.append_block(self.function);
            self.builder.cold_cond_br(failed, over, done);
            self.builder.position(over);
            let region = self.tracked_region(block);
            let anchor = self.unit.tables.failures[failure.index()].anchor;
            let exhausted = self.exhausted_block(region, anchor, failure)?;
            self.builder.br(exhausted);
        } else {
            self.builder.br(done);
        }
        self.builder.position(done);
        Ok(())
    }

    /// Whether `layout` is a block of a sequential sequence of `Packed` values (MEM-13).
    fn sequential_payload(&self, layout: LayoutId) -> Emitted<bool> {
        Ok(match self.layout_payload(layout)? {
            Some(payload) => matches!(self.layout(payload)?.kind, LayoutKind::Sequential { .. }),
            None => false,
        })
    }

    /// `Recover`: resume on `caught` when the runtime forgets the arena's own `OutOfSpace`.
    pub(super) fn recover(&mut self, arena: ValueId, caught: EdgeId, failed: EdgeId) -> Emitted {
        let entry = self.runtime(RuntimeEntry::ArenaRecover)?;
        let status = self.builder.call(entry, &[self.context, self.get(arena)?]);
        let zero = self.module.const_int(self.module.int_type(8), 0);
        let recovered = self.builder.icmp(IntPredicate::Ne, status, zero);
        let caught = self.edge_block(caught)?;
        let failed = self.edge_block(failed)?;
        self.builder.cond_br(recovered, caught, failed);
        Ok(())
    }
}
