//! Source-holder custody for demanded semantic arena allocations.
//!
//! This is separate from immutable metadata: taking a snapshot never acquires
//! a language credit. Successful source events transfer actual credits to this
//! journal INSTEAD OF scheduling the same credits on physical arena cleanup.
//! Existing region::sealed_cleanup::Credits performs source retirement, using
//! the ordinary release and Shared-root deferral policy. No new release engine
//! or count interpretation is introduced.
//!
//! This Rust-only component is not yet a demanded guard, a complete typed Plan
//! producer or a native ABI. A caller must still prove complete source-event
//! correspondence, pairing/lifetimes and exclusive ownership independently.
use super::semantic::{self, AllocationId, GrantId, Inventory, Record};
use super::{MetadataRun, Receipt};
use crate::alloc::Header;
use crate::context::TaskCtx;
use crate::region::sealed_cleanup::Credits;
use std::collections::BTreeMap;

/// Proposed next metadata plus only the NEW source credits for an event.
/// On rejection this entire input is returned and its source custody stays
/// with the caller. Raw words do not release credits merely by being dropped.
pub struct Update {
    pub value: Receipt<Record>,
    pub additions: Vec<*mut Header>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Semantic(semantic::Error),
    InvalidHolder,
    InvalidRetention,
}

pub type Rejected = (Error, Update);

mod seal;
pub use seal::SealError;

/// One open arena's private source credits and retained semantic metadata.
/// The source task context stays borrowed until cleanup; no such context is
/// stored inside the immutable snapshots or a published sealed owner.
/// There is no remove-on-last-use operation and no Clone/Send implementation.
pub struct Journal<'ctx> {
    ctx: &'ctx mut TaskCtx,
    inventory: Inventory,
    holders: BTreeMap<AllocationId, Vec<*mut Header>>,
}
impl<'ctx> Journal<'ctx> {
    pub fn new(ctx: &'ctx mut TaskCtx, run: MetadataRun) -> Self {
        Self {
            ctx,
            inventory: Inventory::new(run),
            holders: BTreeMap::new(),
        }
    }
    pub fn grant(&self) -> GrantId {
        self.inventory.grant()
    }
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }
    /// Exact actual holder multiplicity, independently of semantic paths and
    /// physical relocation deduplication. These words grant no new credit.
    pub fn holders(&self, id: AllocationId) -> Option<&[*mut Header]> {
        self.holders.get(&id).map(Vec::as_slice)
    }

    /// Record a committed construction or COW allocation. All additions become
    /// journal-owned on success, with no retain of ordinary or Shared holders.
    /// Failure returns input unchanged and acquires/retires no source credits.
    ///
    /// # Safety
    /// Null additions are rejected before transfer. Each nonnull addition is
    /// one actual owned, live, resource-free holder in ctx's
    /// run and remains live until retired here. Caller removes its former
    /// cleanup obligation on success, including physical region cleanup. Each
    /// repeated pointer represents a separate credit. Source semantics and C
    /// prove this is the exact successful event, not a metadata-only clone.
    pub unsafe fn allocate(&mut self, input: Update) -> Result<AllocationId, Rejected> {
        if input.additions.iter().any(|holder| holder.is_null()) {
            return Err((Error::InvalidHolder, input));
        }
        match self.inventory.allocate(input.value.clone()) {
            Ok(id) => {
                self.holders.insert(id, input.additions);
                Ok(id)
            }
            Err(error) => Err((Error::Semantic(error), input)),
        }
    }

    /// Commit a unique mutation/growth on the SAME semantic allocation.
    /// `keep` lists increasing ordinals of existing actual credits transferred
    /// to the new payload. No retain/release is performed for those holders.
    /// Additions transfer as in allocate; all omitted old holders retire only
    /// after both new metadata and custody have been installed. COW must use
    /// allocate, with its independently acquired credits, retaining old entry.
    /// The old snapshot returned is descriptive metadata, no storage authority.
    ///
    /// # Safety
    /// allocate's credit/event contract applies to additions. Every kept holder
    /// is exactly the old source obligation transferred by this successful
    /// mutation. No language failure or suspension may intervene between the
    /// source payload mutation and this paired journal commit. Removed holders
    /// have no resource finalizers; ordinary root deferral remains unchanged.
    pub unsafe fn replace(
        &mut self,
        id: AllocationId,
        keep: &[usize],
        input: Update,
    ) -> Result<Receipt<Record>, Rejected> {
        let Some(old) = self.holders.get(&id) else {
            return Err((Error::Semantic(semantic::Error::UnknownAllocation), input));
        };
        if keep.windows(2).any(|pair| pair[0] >= pair[1])
            || keep.last().is_some_and(|&index| index >= old.len())
        {
            return Err((Error::InvalidRetention, input));
        }
        if input.additions.iter().any(|holder| holder.is_null()) {
            return Err((Error::InvalidHolder, input));
        }
        // Reserve both new custody and retirement vectors before committing
        // metadata. These are raw snapshots, acquiring no language holders.
        let mut next = Vec::with_capacity(keep.len() + input.additions.len());
        next.extend(keep.iter().map(|&index| old[index]));
        next.extend_from_slice(&input.additions);
        let mut removed = Vec::with_capacity(old.len() - keep.len());
        let mut kept = keep.iter().copied().peekable();
        for (index, &holder) in old.iter().enumerate() {
            if kept.peek() == Some(&index) {
                kept.next();
            } else {
                removed.push(holder);
            }
        }
        let previous = match self.inventory.replace(id, input.value.clone()) {
            Ok(previous) => previous,
            Err(error) => return Err((Error::Semantic(error), input)),
        };
        self.holders.insert(id, next);
        // Both state components now describe the committed payload. The old
        // raw holder vector drops without releasing transferred credits.
        unsafe { Credits::from_owned(removed).release(self.ctx) };
        Ok(previous)
    }
}

impl Drop for Journal<'_> {
    fn drop(&mut self) {
        // Reuse each retained batch's allocation; do not allocate a flattened
        // list at scope cleanup. Actual source release keeps existing runtime
        // scratch/root-deferral behavior; only metadata drop promises NoAlloc.
        for holders in self.holders.values_mut() {
            let owned = std::mem::take(holders);
            // SAFETY: successful events transferred these exact credits, and
            // retained updates moved them rather than duplicating ownership.
            unsafe { Credits::from_owned(owned).release(self.ctx) };
        }
    }
}

#[cfg(all(test, feature = "diagnostic"))]
mod tests;
