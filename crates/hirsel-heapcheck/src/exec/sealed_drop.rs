//! Sealed release uses the ordinary cell decrement/drain protocol. The owner
//! row remains Dropping until adopted storage and its private control are dead.
use super::*;

impl<S: Stage> Machine<'_, S> {
    pub(super) fn release_sealed(
        &mut self,
        at: Ptr,
        layout: LayoutId,
        work: &mut Vec<Work>,
    ) -> Checked<()> {
        if !matches!(self.record(layout)?.kind, LayoutKind::SealedRegion { .. }) {
            return fail(
                V::TypeConfusion,
                "sealed release has no sealed owner layout",
            );
        }
        let Cell::Data {
            ptr: control,
            counted: true,
            borrow: None,
        } = self.read_cell(at)?
        else {
            return fail(
                V::CreditUnderflow,
                "sealed release requires the actual owning control cell",
            );
        };
        if self.ledger.check(control)?.layout != Some(layout) {
            return fail(
                V::TypeConfusion,
                "sealed release control has another owner layout",
            );
        }
        let owner = self.ledger.sealed_control(control)?;
        let allocations = self.ledger.begin_sealed_drop(owner)?;
        let word = self.word;
        self.ledger
            .get_mut(at.alloc)
            .extent
            .take(at.offset, word, FREE_POISON, Some((&[], &[])));
        if !self.count_down(control)? {
            return fail(
                V::CreditUnderflow,
                "sealed control was not affine at release",
            );
        }
        let mut pending = Vec::new();
        for alloc in &allocations {
            let allocation = self.ledger.get(*alloc);
            let base = Ptr {
                alloc: *alloc,
                generation: allocation.generation,
                offset: 0,
                path: 0,
            };
            let layout = allocation
                .layout
                .ok_or_else(|| err(V::TypeConfusion, "sealed storage lost its layout"))?;
            for (&offset, cell) in &allocation.extent.cells {
                if matches!(cell, Cell::Data { .. }) {
                    pending.push(Work::Step {
                        base,
                        logical_base: base,
                        layout,
                        step: DropStep::Release { offset },
                        dead: Vec::new(),
                        child_mask: None,
                    });
                }
            }
        }
        pending.extend(allocations.into_iter().map(Work::SealedStorageDeath));
        pending.push(Work::SealedOwnerDone { owner, control });
        work.extend(pending.into_iter().rev());
        Ok(())
    }
}
