//! Demand-only lexical guard; ordinary region::Region remains its exact64 bytes.
//! The separately approved80-byte record adds private journal/disarm words.
//! Native entry shares these same initialization, publication and cleanup paths.
//! Source grant, complete typed Plan and invocation entry remain independently
//! checked caller obligations. No source context is retained by the sealed owner.
use super::receipt::journal::{self, Journal, Update};
use super::receipt::semantic::{AllocationId, GrantId, Inventory, Record};
use super::receipt::{MetadataRun, Receipt};
use super::{Plan, SealedRegion};
use crate::STATUS_OK;
use crate::context::TaskCtx;
use crate::region::{self, Region};
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::ptr::{self, NonNull};

pub mod abi;
mod bits;

/// Exact demand-only extension, with the ordinary base fields/offsets intact.
/// The trailing owner word records consumption ONLY. It may be dangling after
/// affine owner drop and MUST NEVER be dereferenced for access or cleanup.
#[repr(C)]
pub struct ReceiptArenaGuard<'ctx> {
    base: Region,
    inventory: *mut Journal<'ctx>,
    sealed_owner: *mut SealedRegion,
}
const _: () = {
    assert!(size_of::<Region>() == 64);
    assert!(size_of::<ReceiptArenaGuard<'static>>() == 80);
    assert!(std::mem::offset_of!(ReceiptArenaGuard<'static>, base) == 0);
    assert!(std::mem::offset_of!(ReceiptArenaGuard<'static>, inventory) == 64);
    assert!(std::mem::offset_of!(ReceiptArenaGuard<'static>, sealed_owner) == 72);
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Consumed,
    InventoryRequired,
    RootLayout,
    RootImage,
    Journal(journal::Error),
    Seal(journal::SealError),
}

/// Pinned Rust ownership of the lexical guard and its live task identity.
/// This wrapper is not the emitted/C layout. The pinned record survives moving
/// the wrapper and sealing an outer arena while an inner guard is active.
pub struct DemandedGuard<'ctx> {
    record: Pin<Box<UnsafeCell<MaybeUninit<ReceiptArenaGuard<'ctx>>>>>,
    ctx: NonNull<TaskCtx>,
    closed: bool,
    lifetime: PhantomData<&'ctx mut TaskCtx>,
}
impl<'ctx> DemandedGuard<'ctx> {
    pub fn new(ctx: &'ctx mut TaskCtx, run: MetadataRun) -> Self {
        // SAFETY: task lifetime is bounded by this exclusive source borrow.
        unsafe { Self::from_raw(ctx, run) }
    }
    /// # Safety
    /// ctx stays live/current for 'ctx. Native calls use only transient borrows
    /// and never run concurrently. Guards close in lexical nesting order; all
    /// actual values/loans/source cleanup satisfy their original C contracts.
    pub(crate) unsafe fn from_raw(ctx: *mut TaskCtx, run: MetadataRun) -> Self {
        let ctx = NonNull::new(ctx).unwrap_or_else(|| crate::fault("null demanded guard context"));
        let allocation = Box::pin(UnsafeCell::new(
            MaybeUninit::<ReceiptArenaGuard<'ctx>>::uninit(),
        ));
        let raw = allocation
            .as_ref()
            .get_ref()
            .get()
            .cast::<ReceiptArenaGuard<'ctx>>();
        // SAFETY: the boxed record never moves; base enter initializes exactly
        // the unchanged prefix. Every remaining field is initialized below.
        unsafe {
            enter_record(ctx.as_ptr(), raw, run);
            Self {
                record: allocation,
                ctx,
                closed: false,
                lifetime: PhantomData,
            }
        }
    }
    fn raw(&self) -> *mut ReceiptArenaGuard<'ctx> {
        // UnsafeCell is essential: native arena operations mutate this pinned
        // record through raw pointers, including while only the Rust handle
        // is borrowed. No pointer is cast from a shared record reference.
        self.record.as_ref().get_ref().get().cast()
    }
    pub fn context(&self) -> *mut TaskCtx {
        self.ctx.as_ptr()
    }
    /// Stable ordinary prefix, for existing runtime operations only. Obtaining
    /// this word does not mint a source grant or permission to mutate storage.
    pub fn base(&self) -> *mut Region {
        // SAFETY: only form a field pointer; the pin keeps it stable.
        unsafe { ptr::addr_of_mut!((*self.raw()).base) }
    }
    pub fn was_sealed(&self) -> bool {
        // SAFETY: compare the disarm identity; NEVER follow the owner pointer.
        unsafe { !(*self.raw()).sealed_owner.is_null() }
    }
    fn journal(&self) -> Result<&Journal<'ctx>, Error> {
        // SAFETY: open state owns this exact allocation until take at seal.
        unsafe { (*self.raw()).inventory.as_ref().ok_or(Error::Consumed) }
    }
    fn journal_mut(&mut self) -> Result<&mut Journal<'ctx>, Error> {
        // SAFETY: exclusive wrapper borrow, pinned live owned journal.
        unsafe { (*self.raw()).inventory.as_mut().ok_or(Error::Consumed) }
    }
    pub fn grant(&self) -> Result<GrantId, Error> {
        Ok(self.journal()?.grant())
    }
    pub fn inventory(&self) -> Result<&Inventory, Error> {
        Ok(self.journal()?.inventory())
    }
    /// # Safety
    /// Journal::allocate's exact source event/credit transfer contract applies.
    pub unsafe fn allocate(&mut self, input: Update) -> Result<AllocationId, (Error, Update)> {
        let journal = match self.journal_mut() {
            Ok(journal) => journal,
            Err(error) => return Err((error, input)),
        };
        unsafe { journal.allocate(input) }.map_err(|(error, input)| (Error::Journal(error), input))
    }
    /// # Safety
    /// Journal::replace's successful mutation/pairing/credit contract applies.
    pub unsafe fn replace(
        &mut self,
        id: AllocationId,
        keep: &[usize],
        input: Update,
    ) -> Result<Receipt<Record>, (Error, Update)> {
        let journal = match self.journal_mut() {
            Ok(journal) => journal,
            Err(error) => return Err((error, input)),
        };
        unsafe { journal.replace(id, keep, input) }
            .map_err(|(error, input)| (Error::Journal(error), input))
    }
    /// # Safety
    /// Journal::replace_transfer's source mutation, returned ordinary ownership
    /// and cleanup contract applies. Consumed guards return input unchanged.
    pub unsafe fn replace_transfer(
        &mut self,
        id: AllocationId,
        keep: &[usize],
        take: &[usize],
        input: Update,
    ) -> Result<journal::Replacement, (Error, Update)> {
        let journal = match self.journal_mut() {
            Ok(journal) => journal,
            Err(error) => return Err((error, input)),
        };
        unsafe { journal.replace_transfer(id, keep, take, input) }
            .map_err(|(error, input)| (Error::Journal(error), input))
    }

    /// # Safety
    /// Original source scope entry authorizes setting this MEM-6 bound once.
    pub unsafe fn bound(&mut self, capacity: u64) -> Result<(), Error> {
        self.journal()?;
        unsafe { region::hirsel_rt_v2_arena_bound(self.context(), self.base(), capacity) };
        Ok(())
    }
    /// Test the original bounded failure identity even after storage transfer.
    /// # Safety
    /// Original bounded invocation authorizes recovery at this exact guard.
    pub unsafe fn recover(&mut self) -> bool {
        unsafe { region::hirsel_rt_v2_arena_recover(self.context(), self.base()) != 0 }
    }
    /// # Safety
    /// Journal::seal's entered-operation, paired root and complete independently
    /// typed Plan contract applies. Source root cleanup disarms only on Ok.
    pub unsafe fn seal(
        &mut self,
        root: &Receipt<Record>,
        plan: &Plan,
    ) -> Result<*mut SealedRegion, Error> {
        unsafe { seal_record(self.context(), self.raw(), root, plan) }
    }
    fn close_inner(&mut self, anchor: u64, operation_site: u64) -> u8 {
        if self.closed {
            return STATUS_OK;
        }
        self.closed = true;
        unsafe { close_record(self.context(), self.raw(), anchor, operation_site) }
    }
    /// Close lexical storage/callback cleanup; a transferred arena has no
    /// storage or source credits here. The affine sealed owner is independent.
    pub fn close(mut self, anchor: u64, operation_site: u64) -> u8 {
        self.close_inner(anchor, operation_site)
    }
}

unsafe fn enter_record<'ctx>(
    ctx: *mut TaskCtx,
    raw: *mut ReceiptArenaGuard<'ctx>,
    run: MetadataRun,
) {
    unsafe {
        region::hirsel_rt_v2_region_enter(ctx, ptr::addr_of_mut!((*raw).base));
        let journal = Box::new(Journal::from_raw(ctx, run));
        ptr::addr_of_mut!((*raw).inventory).write(Box::into_raw(journal));
        ptr::addr_of_mut!((*raw).sealed_owner).write(ptr::null_mut());
    }
}

unsafe fn close_record(
    ctx: *mut TaskCtx,
    raw: *mut ReceiptArenaGuard<'_>,
    anchor: u64,
    site: u64,
) -> u8 {
    unsafe {
        let journal = ptr::replace(ptr::addr_of_mut!((*raw).inventory), ptr::null_mut());
        if !journal.is_null() {
            drop(Box::from_raw(journal));
        }
        region::hirsel_rt_v2_region_exit(ctx, ptr::addr_of_mut!((*raw).base), anchor, site)
    }
}

unsafe fn seal_record<'ctx>(
    _ctx: *mut TaskCtx,
    raw: *mut ReceiptArenaGuard<'ctx>,
    root: &Receipt<Record>,
    plan: &Plan,
) -> Result<*mut SealedRegion, Error> {
    // SAFETY: take exactly the owned journal allocation. Its slot stays
    // allocated, allowing failure restoration without allocating again.
    let journal = unsafe { ptr::replace(ptr::addr_of_mut!((*raw).inventory), ptr::null_mut()) };
    if journal.is_null() {
        return Err(Error::Consumed);
    }
    let slot = JournalSlot(journal.cast::<MaybeUninit<Journal<'ctx>>>());
    let value = unsafe { journal.read() };
    match unsafe { value.seal(ptr::addr_of_mut!((*raw).base), root, plan) } {
        Ok(owner) => {
            unsafe { (*raw).sealed_owner = owner };
            // self-drop already consumed private metadata/empty batches.
            drop(slot);
            Ok(owner)
        }
        Err((error, value)) => {
            unsafe {
                journal.write(value);
                (*raw).inventory = journal
            };
            // The same Box allocation and original source custody remain.
            std::mem::forget(slot);
            Err(Error::Seal(error))
        }
    }
}
impl Drop for DemandedGuard<'_> {
    fn drop(&mut self) {
        // Runtime failures are already recorded by ordinary scope cleanup;
        // explicit close returns status to the library body's failure edge.
        self.close_inner(0, 0);
    }
}

/// Own only the temporary UNINITIALIZED Box storage while Journal is moved
/// into entered seal. Unwind frees the slot; restoration keeps its allocation.
struct JournalSlot<'ctx>(*mut MaybeUninit<Journal<'ctx>>);
impl Drop for JournalSlot<'_> {
    fn drop(&mut self) {
        // SAFETY: exact Box allocation/layout, with Journal moved out. It must
        // not run Journal::drop again, whether seal succeeded or unwound.
        unsafe { drop(Box::from_raw(self.0)) };
    }
}

#[cfg(all(test, feature = "diagnostic"))]
mod tests;
