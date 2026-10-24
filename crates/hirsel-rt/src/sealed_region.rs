//! Stable custody after typed evacuation, separate from the frame-resident
//! region guard. Preparation owns only private root bytes and newly acquired
//! Shared credits; commit adopts storage only after every fallible graph step.
//!
//! Uses the existing `region::detach_storage` allocator-home protocol and
//! `roots` MEM-11 operations. Aligned root storage follows Rust std::alloc's
//! Layout/alloc/dealloc pairing. This is owner/rollback support, not a graph
//! evacuator or the public seal_prepare_commit entry.
use crate::alloc::Header;
use crate::context::{RunShared, TaskCtx};
use crate::region::{DetachedStorage, Region, detach_storage};
use crate::{fault, roots};
use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU64, Ordering};

pub mod receipt;
mod transaction;
pub use transaction::{
    Block, BlockId, Charge, Error as SealError, Patch, Placement, Plan, Root, prepare_commit,
};

static GENERATION: AtomicU64 = AtomicU64::new(1);

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Preparing,
    Published,
    Dropping,
    Dead,
}

struct RootBytes {
    data: NonNull<u8>,
    allocation: Layout,
}
impl RootBytes {
    unsafe fn snapshot(source: *const u8, size: usize, align: usize) -> Self {
        let allocation = Layout::from_size_align(size.max(1), align)
            .unwrap_or_else(|_| fault("invalid sealed root extent/alignment"));
        // SAFETY: the checked Layout is nonzero; exhaustion is environmental.
        let data = NonNull::new(unsafe { alloc(allocation) })
            .unwrap_or_else(|| handle_alloc_error(allocation));
        if size != 0 {
            // SAFETY: caller supplies size readable bytes; allocation is fresh.
            unsafe { ptr::copy_nonoverlapping(source, data.as_ptr(), size) };
        }
        Self { data, allocation }
    }
}
impl Drop for RootBytes {
    fn drop(&mut self) {
        // SAFETY: this is the exact nonzero Layout used for this sole allocation.
        unsafe { dealloc(self.data.as_ptr(), self.allocation) };
    }
}

/// Opaque affine control allocation. It contains a run identity, never a source
/// task/region/frame address. Its graph must be destroyed before its run ends.
#[repr(C)]
pub struct SealedRegion {
    state: State,
    generation: u64,
    run: *const RunShared,
    root: RootBytes,
    storage: Option<DetachedStorage>,
    /// Private evacuation extensions, adopted at the same publication point.
    extensions: Option<DetachedStorage>,
    /// One entry per semantic root credit, including repeated equal pointers.
    shared: Vec<*mut Header>,
}

// SAFETY: published contents and segment custody move affinely within one live
// run. Shared credits use the existing atomic/ordered return protocol. Neither
// root bytes nor detached payload may be accessed concurrently by another owner.
unsafe impl Send for SealedRegion {}

/// Private transaction custody. Dropping preparation releases its acquisitions
/// and root scratch, leaving the original root and arena guard untouched.
#[must_use = "preparation must be committed or dropped to roll back"]
pub struct PreparedSeal<'ctx> {
    ctx: &'ctx mut TaskCtx,
    control: Option<Box<SealedRegion>>,
}
impl<'ctx> PreparedSeal<'ctx> {
    /// Snapshot the typed root into stable, correctly aligned private storage.
    /// No ordinary or Shared ownership moves merely by copying these bytes.
    ///
    /// # Safety
    /// `root` is readable for `size` bytes (or may be null for size zero).
    /// `ctx`'s run outlives this preparation and every resulting sealed owner.
    pub unsafe fn new(ctx: &'ctx mut TaskCtx, root: *const u8, size: usize, align: usize) -> Self {
        let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
        if generation == u64::MAX {
            fault("sealed owner generation exhausted");
        }
        let control = Box::new(SealedRegion {
            state: State::Preparing,
            generation,
            run: ctx.run_ref(),
            // SAFETY: caller's root/extent contract.
            root: unsafe { RootBytes::snapshot(root, size, align) },
            storage: None,
            extensions: None,
            shared: Vec::new(),
        });
        Self {
            ctx,
            control: Some(control),
        }
    }

    /// Private destination for the typed relocation planner. This does not
    /// publish a language loan or acquire a root's source credits.
    pub fn root_mut(&mut self) -> *mut u8 {
        self.control
            .as_mut()
            .expect("uncommitted preparation")
            .root
            .data
            .as_ptr()
    }

    /// Acquire one destination Shared credit before source cleanup. Repeated
    /// calls for one root preserve semantic occurrence multiplicity.
    ///
    /// # Safety
    /// `root` is a live Shared root of this run, kept live by source custody.
    pub unsafe fn retain_shared(&mut self, root: *mut Header) {
        let control = self.control.as_mut().expect("uncommitted preparation");
        control.shared.reserve(1);
        // SAFETY: source custody retains this live root across the acquisition.
        unsafe { roots::hirsel_rt_v2_root_dup(self.ctx, root) };
        control.shared.push(root);
    }

    /// Mechanical publication: no allocation, language call or suspension.
    /// The caller has discharged original held edges after completing private
    /// evacuation; detach_storage independently requires an empty cleanup list.
    ///
    /// # Safety
    /// The typed graph closure is complete: root/private bytes contain no
    /// ordinary outgoing heap/frame/foreign-arena edge; every retained Shared
    /// occurrence has exactly one entry acquired above. All external arena views
    /// are dead. `arena` is exclusively owned, live, and belongs to this run.
    /// After this call the returned pointer has exactly one affine owner, whose
    /// cleanup must call `drop_owner` once with a context of the same live run.
    pub unsafe fn commit(mut self, arena: *mut Region) -> *mut SealedRegion {
        let mut control = self.control.take().expect("uncommitted preparation");
        // SAFETY: graph closure and exclusive live arena are caller obligations.
        let storage = unsafe { detach_storage(arena) };
        unsafe { storage.transfer_diagnostic(self.ctx, arena) };
        control.storage = Some(storage);
        control.state = State::Published;
        Box::into_raw(control)
    }
}
impl Drop for PreparedSeal<'_> {
    fn drop(&mut self) {
        if let Some(control) = &mut self.control {
            // These are staging acquisitions only. Source payload/guard and its
            // credits have not transferred and remain with entered-body cleanup.
            while let Some(root) = control.shared.pop() {
                // SAFETY: this preparation still owns each recorded acquisition.
                unsafe { roots::hirsel_rt_v2_root_release(self.ctx, root) };
            }
        }
    }
}

/// Borrow the prepared typed address. Loan ownership is checked in LIR/H;
/// reading the root itself performs no owning increment or allocation.
///
/// # Safety
/// `owner` is live and Published, exclusively reserved by the current owner
/// loan, and outlives the complete read-only use of the returned address.
pub unsafe fn root(owner: *const SealedRegion) -> *const u8 {
    // SAFETY: caller holds the owner loan.
    let owner = unsafe { &*owner };
    if owner.state != State::Published {
        fault("sealed root read outside Published state");
    }
    owner.root.data.as_ptr()
}

/// Destroy all transferred obligations once, preserving repeated Shared credits.
/// No allocation or user finalizer is introduced by this owner protocol.
///
/// # Safety
/// `owner` is the sole Published owner, with no active loan/view. `ctx` is used
/// exclusively on its worker and belongs to the owner's still-live run.
pub unsafe fn drop_owner(ctx: *mut TaskCtx, owner: *mut SealedRegion) {
    // SAFETY: sole affine owner and exclusive current context are caller obligations.
    unsafe {
        let mut owner = Box::from_raw(owner);
        if owner.state != State::Published {
            fault("sealed owner drop outside Published state");
        }
        if !ptr::eq(owner.run, (*ctx).run_ref()) {
            fault("sealed owner dropped by another run");
        }
        owner.state = State::Dropping;
        while let Some(root) = owner.shared.pop() {
            roots::hirsel_rt_v2_root_release(ctx, root);
        }
        owner
            .storage
            .take()
            .expect("published detached storage")
            .release(ctx);
        if let Some(extensions) = owner.extensions.take() {
            extensions.release(ctx);
        }
        owner.state = State::Dead;
        // Box and RootBytes free only owned control/root scratch; no source task
        // pointer is consulted, including on a receiving worker after source exit.
    }
}

#[cfg(all(test, feature = "diagnostic"))]
mod tests;
