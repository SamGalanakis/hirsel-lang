//! Bounded arenas (MEM-6). An arena record's `remaining` word is its logical budget,
//! `u64::MAX` when unbounded. Every arena construction tests its charge against it and
//! lowers it inline; an excess records the arena's own `OutOfSpace` and takes the
//! construction's failure edge. A program with no `Recover` opens no bounded arena, so
//! its failure codes carry no MEM-6 and its constructions are not charged.
use super::*;
use hirsel_lir::emission::{BOUNDED_RULE, FailureCode};

impl<'m, 'a> FnEmitter<'m, 'a> {
    /// MEM-6's failure code, present when the program opens a bounded arena.
    pub(super) fn bounded_code(&self) -> Option<FailureCode> {
        self.inputs.failures.codes.get(BOUNDED_RULE).copied()
    }

    /// A block that records the arena `region`'s own `OutOfSpace` at `anchor`, disposes
    /// `failure`'s tokens and enters its cleanup.
    pub(super) fn exhausted_block(
        &mut self,
        code: FailureCode,
        region: Value<'m>,
        anchor: SourceAnchorId,
        failure: FailureId,
    ) -> Emitted<BasicBlock<'m>> {
        let edge = self.failure_edge(failure)?;
        let here = self.builder.current();
        let block = self.module.append_block(self.function);
        self.builder.position(block);
        let site = self.anchor_code(anchor)?;
        let exhausted = self.runtime(RuntimeEntry::ArenaExhausted)?;
        let i32_ty = self.module.int_type(32);
        self.builder.call(
            exhausted,
            &[
                self.context,
                region,
                self.module.const_int(i32_ty, u128::from(code.kind)),
                self.module.const_int(i32_ty, u128::from(code.rule)),
                self.const_u64(site),
            ],
        );
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
    pub(super) fn charge(
        &mut self,
        region: Value<'m>,
        bytes: u64,
        anchor: SourceAnchorId,
        failure: FailureId,
    ) -> Emitted {
        let Some(code) = self.bounded_code() else {
            return Ok(());
        };
        if bytes == 0 {
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
        let exhausted = self.exhausted_block(code, region, anchor, failure)?;
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
        let Some(code) = self.bounded_code() else {
            return self.status_guard(status, failure);
        };
        let zero = self.module.const_int(self.module.int_type(8), 0);
        let failed = self.builder.icmp(IntPredicate::Ne, status, zero);
        let exhausted = self.exhausted_block(code, destination, anchor, failure)?;
        let next = self.module.append_block(self.function);
        self.builder.cold_cond_br(failed, exhausted, next);
        self.builder.position(next);
        Ok(())
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
