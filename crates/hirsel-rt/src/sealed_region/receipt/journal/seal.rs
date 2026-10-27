//! Journal custody exchange through the existing supplied-plan transaction.
//! The complete typed physical plan is still an independent caller obligation;
//! this bridge supplies semantic charge and exact source-holder retirement.
use super::*;
use crate::region::Region;
use crate::sealed_region::{Plan, SealedRegion, transaction};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SealError {
    Semantic(semantic::Error),
    ChargeMismatch,
    Transaction(crate::sealed_region::SealError),
}

impl<'ctx> Journal<'ctx> {
    /// Consume this entered operation's journal on successful publication.
    /// Any failure returns the SAME journal/custody for entered-body cleanup;
    /// no source holder was retired and no public owner was produced.
    ///
    /// This does not build or validate complete physical graph coverage. The
    /// charge comes from maintained semantic inventory/root, never physical
    /// block deduplication or a caller-supplied total alone.
    ///
    /// # Safety
    /// prepare_commit's complete source/type/physical plan contract applies.
    /// `arena` is the exact live physical guard paired with this journal; its
    /// cleanup does not also own journal credits. `root` is the original paired
    /// root metadata and plan.root owns precisely the entered root's ordinary
    /// credits. Caller disarms root cleanup only on success. All adopted and
    /// evacuated sources, including dead/elided retained payload, have exact
    /// coverage, live source custody and complete Shared multiplicity. The
    /// returned owner is affine and outlives no run. Rejected admission belongs
    /// to the surrounding call and MUST NOT invoke this entered transition.
    pub unsafe fn seal(
        self,
        arena: *mut Region,
        root: &Receipt<Record>,
        plan: &Plan,
    ) -> Result<*mut SealedRegion, (SealError, Self)> {
        unsafe { self.seal_with(arena, root, plan, &mut |_| Ok(())) }
    }

    pub(in crate::sealed_region) unsafe fn seal_with(
        mut self,
        arena: *mut Region,
        root: &Receipt<Record>,
        plan: &Plan,
        checkpoint: &mut impl FnMut(transaction::Event) -> Result<(), crate::sealed_region::SealError>,
    ) -> Result<*mut SealedRegion, (SealError, Self)> {
        let charge = match self.inventory.seal_charge(root) {
            Ok(charge) => charge,
            Err(error) => return Err((SealError::Semantic(error), self)),
        };
        if charge != plan.charge {
            return Err((SealError::ChargeMismatch, self));
        }
        // Disjoint fields: the context is used by the transaction, while the
        // custody adapter only snapshots/clears holder batches. It never reads
        // ctx through an overlapping mutable journal borrow.
        let result = unsafe {
            transaction::run_with_journal(
                self.ctx.as_ptr(),
                arena,
                plan,
                checkpoint,
                Some(&mut self.custody),
            )
        };
        match result {
            Ok(owner) => Ok(owner),
            Err(error) => Err((SealError::Transaction(error), self)),
        }
        // On success self drops empty source custody and private metadata;
        // transaction already retired the exact source snapshot after publish.
    }
}
