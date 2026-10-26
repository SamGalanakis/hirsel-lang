//! Source-holder custody for demanded semantic arena allocations.
//!
//! This is separate from immutable metadata: taking a snapshot never acquires
//! a language credit. Successful source events transfer actual credits to this
//! journal INSTEAD OF scheduling the same credits on physical arena cleanup.
//! Existing region::sealed_cleanup::Credits performs source retirement, using
//! the ordinary release and Shared-root deferral policy. No new release engine
//! or count interpretation is introduced.
//!
//! The native seal entry builds its plan from the typed bindings held here.
//! A caller must still prove complete source-event correspondence, pairing,
//! lifetimes and exclusive ownership independently.
use super::semantic::{self, AllocationId, GrantId, Inventory, Record};
use super::{MetadataRun, Receipt};
use crate::alloc::Header;
use crate::context::TaskCtx;
use crate::region::sealed_cleanup::Credits;
use crate::sealed_region::planner::Retained;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::ptr::NonNull;

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
    /// Private custody identity space exhausted; never a language budget error.
    CreditIdExhausted,
    PhysicalBinding,
}

pub type Rejected = (Error, Update);

/// Successful mutation output. Transferred words are actual source credits
/// now owned by the caller's ordinary result/slot, not by this metadata.
/// Dropping the raw vector itself retires none of them.
pub struct Replacement {
    pub previous: Receipt<Record>,
    pub transferred: Vec<*mut Header>,
}

mod custody;
mod seal;
use custody::Custody;
pub use seal::SealError;

/// One open arena's private source credits and retained semantic metadata.
/// A lifetime marker protects the source context through cleanup. Only a raw
/// task identity is retained, so source operations can enter runtime helpers
/// with transient borrows. No such pointer/reference is stored in immutable
/// snapshots or a published sealed owner.
/// There is no remove-on-last-use operation and no Clone/Send implementation.
pub struct Journal<'ctx> {
    ctx: NonNull<TaskCtx>,
    lifetime: PhantomData<&'ctx mut TaskCtx>,
    inventory: Inventory,
    custody: Custody,
    physical: BTreeMap<AllocationId, Retained>,
}
impl<'ctx> Journal<'ctx> {
    pub fn new(ctx: &'ctx mut TaskCtx, run: MetadataRun) -> Self {
        // SAFETY: the reference/lifetime protects this task through cleanup.
        unsafe { Self::from_raw(ctx, run) }
    }

    /// Construct task-scoped custody without retaining a Rust mutable reference
    /// across native runtime calls on the same context.
    ///
    /// # Safety
    /// The nonnull context stays live for 'ctx and is used only by its current
    /// task/worker role, with no concurrent access. Every context reference is
    /// a transient runtime-entry borrow. Nested journals can share the raw task
    /// identity, never simultaneous Rust mutable references to the task.
    pub(crate) unsafe fn from_raw(ctx: *mut TaskCtx, run: MetadataRun) -> Self {
        Self {
            ctx: NonNull::new(ctx).unwrap_or_else(|| crate::fault("null receipt journal context")),
            lifetime: PhantomData,
            inventory: Inventory::new(run),
            custody: Custody::new(),
            physical: BTreeMap::new(),
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
        self.custody.holders(id)
    }

    /// Bind a committed semantic allocation to its complete physical payload,
    /// or to its initialized image when the physical allocation was elided.
    /// This transfers the same source credits as allocate, without retaining them.
    ///
    /// # Safety
    /// allocate's contract applies. The descriptor and optional block describe
    /// this exact payload in the current run; the block stays live under this
    /// guard until replacement or seal. No source mutation/suspension may
    /// separate the ordinary construction from this journal publication.
    pub unsafe fn allocate_typed(
        &mut self,
        input: Update,
        descriptor: u32,
        block: Option<*mut Header>,
        extent: usize,
    ) -> Result<AllocationId, Rejected> {
        if input
            .value
            .bytes()
            .is_none_or(|bytes| bytes.len() != extent)
            || block.is_some_and(|block| block.is_null())
        {
            return Err((Error::PhysicalBinding, input));
        }
        let id = unsafe { self.allocate(input) }?;
        self.physical.insert(
            id,
            Retained {
                allocation: id,
                descriptor,
                block,
                extent,
            },
        );
        Ok(id)
    }

    /// Publish a new physical representation with a successful semantic
    /// replacement. A failed binding check leaves the old representation and
    /// every old source credit untouched. COW uses allocate_typed instead.
    ///
    /// # Safety
    /// replace_transfer's complete mutation/custody contract and
    /// allocate_typed's physical-binding contract apply to the new payload.
    pub unsafe fn replace_typed(
        &mut self,
        binding: Retained,
        keep: &[usize],
        take: &[usize],
        input: Update,
    ) -> Result<Replacement, Rejected> {
        if input
            .value
            .bytes()
            .is_none_or(|bytes| bytes.len() != binding.extent)
            || binding.block.is_some_and(|block| block.is_null())
        {
            return Err((Error::PhysicalBinding, input));
        }
        let result = unsafe { self.replace_transfer(binding.allocation, keep, take, input) }?;
        self.physical.insert(binding.allocation, binding);
        Ok(result)
    }

    /// Produce the native transaction plan from every retained allocation,
    /// including dead and elided payloads. Untyped entries cannot disappear.
    ///
    /// # Safety
    /// planner::plan's source custody, descriptor, initialized-image and root
    /// pairing contract applies. arena is this journal's exact live guard.
    pub(crate) unsafe fn plan(
        &self,
        arena: *mut crate::region::Region,
        descriptor: u32,
        value: *mut u8,
        root: &Receipt<Record>,
    ) -> Result<crate::sealed_region::Plan, crate::sealed_region::planner::Error> {
        let retained = self
            .physical
            .values()
            .map(|binding| Retained {
                allocation: binding.allocation,
                descriptor: binding.descriptor,
                block: binding.block,
                extent: binding.extent,
            })
            .collect::<Vec<_>>();
        // Physical storage without semantic history is never an empty scope.
        if self.inventory.entries().len() == 0
            && unsafe { !(*arena).segment.is_null() || !(*arena).cleanup.is_null() }
        {
            return Err(crate::sealed_region::planner::Error::Coverage);
        }
        unsafe {
            crate::sealed_region::planner::plan_inventory(
                self.ctx.as_ptr(),
                arena,
                &self.inventory,
                descriptor,
                value,
                root,
                &retained,
            )
        }
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
    /// repeated pointer represents a separate credit. Additions are in their
    /// original enrollment order (oldest first). Source semantics and C
    /// prove this is the exact successful event, not a metadata-only clone.
    pub unsafe fn allocate(&mut self, input: Update) -> Result<AllocationId, Rejected> {
        let change = match self.custody.allocation(&input.additions) {
            Ok(change) => change,
            Err(error) => return Err((error, input)),
        };
        match self.inventory.allocate(input.value.clone()) {
            Ok(id) => {
                self.custody.commit(id, change);
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
        unsafe { self.replace_transfer(id, keep, &[], input) }.map(|result| result.previous)
    }

    /// Commit a mutation that transfers some old holders into an ordinary
    /// result (for example pop), without a duplicate/release round trip.
    /// `keep` and `take` are increasing, disjoint old holder ordinals. Every
    /// omitted holder retires only after new metadata/custody are committed.
    /// On rejection the input and all old source custody remain unchanged.
    ///
    /// # Safety
    /// replace's event/pairing contract applies. Each taken credit transfers
    /// exactly once into the caller's actual owning result/slot on success.
    /// Caller supplies its ordinary cleanup and does not treat previous
    /// snapshot metadata or copied words as an additional owner. Already
    /// committed mutation/transfer is not rolled back on later entered failure.
    pub unsafe fn replace_transfer(
        &mut self,
        id: AllocationId,
        keep: &[usize],
        take: &[usize],
        input: Update,
    ) -> Result<Replacement, Rejected> {
        let change = match self.custody.replacement(id, keep, take, &input.additions) {
            Ok(change) => change,
            Err(error) => return Err((error, input)),
        };
        let previous = match self.inventory.replace(id, input.value.clone()) {
            Ok(previous) => previous,
            Err(error) => return Err((Error::Semantic(error), input)),
        };
        self.physical.remove(&id);
        let (transferred, removed) = self.custody.commit(id, change);
        // Kept and taken credits moved, so only omitted credits retire.
        unsafe { Credits::from_owned(removed).release(self.ctx.as_mut()) };
        Ok(Replacement {
            previous,
            transferred,
        })
    }
}

impl Drop for Journal<'_> {
    fn drop(&mut self) {
        // The global enrollment index, not allocation-map/holder ordinals,
        // owns cleanup order. Draining it needs no flattened allocation.
        // SAFETY: events transfer these exact credits once; keep moves their
        // enrollment identities and take removes custody without retirement.
        unsafe { self.custody.retire(self.ctx.as_mut()) };
    }
}

#[cfg(all(test, feature = "diagnostic"))]
mod tests;
