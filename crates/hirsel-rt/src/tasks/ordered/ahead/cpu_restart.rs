//! An oldest job can revoke younger allocation attempts, as in wound/wait.
//! A checked inert input permits a checkpoint before the first allocation.
//! Attempt pages never mix with canonical owners, and publication follows
//! re-execution at the head. The canonical claim and its slot never change.
use super::super::engine;
use super::native::Native;
use crate::TaskCtx;
use crate::budget::stretch::Stretch;
use crate::pool::cpu_scoped::ledger::{HeapReservation, Ledger};
use std::mem::MaybeUninit;
use std::sync::Arc;

struct Checkpoint {
    sp: *mut u8,
    stack: Vec<MaybeUninit<u8>>,
    request: super::cpu_allocation::Request,
    growth: *mut super::cpu_handoff::Growth,
    fuel: i64,
    private_fuel: super::cpu_fuel::Fuel,
    freed: u64,
    charges: crate::budget::stretch_trace::Local,
    before: Option<Stretch>,
    grant: Option<u64>,
    ledger: Arc<Ledger>,
    owner: crate::pool::cpu_scoped::ledger::blocks::Owner,
    _reservation: HeapReservation,
}

pub(super) struct State {
    eligible: bool,
    started: bool,
    heap: *mut crate::alloc::Heap,
    registry: *const crate::heaps::HeapRegistry,
    checkpoint: Option<Checkpoint>,
}
impl State {
    pub(super) fn new(preclaimed: bool) -> Self {
        Self {
            eligible: preclaimed,
            started: false,
            heap: std::ptr::null_mut(),
            registry: std::ptr::null(),
            checkpoint: None,
        }
    }
}
impl State {
    pub(super) fn discard(&mut self, arena: &super::cpu_arena::Arena) {
        if !self.heap.is_null() {
            // SAFETY: the native owner has drained. No attempt value escaped.
            unsafe {
                discard_storage(self.checkpoint.as_ref().unwrap(), self.heap, arena);
                (*self.registry).return_private(self.heap);
            }
            self.heap = std::ptr::null_mut();
            self.checkpoint = None;
        }
    }
}

unsafe fn discard_storage(
    checkpoint: &Checkpoint,
    heap: *mut crate::alloc::Heap,
    arena: &super::cpu_arena::Arena,
) {
    // Bare shipping system-allocator blocks have no heap list. Their checked
    // generation inventory is complete because checkpointing precedes backing.
    #[cfg(all(
        feature = "system-allocator",
        not(any(feature = "diagnostic", feature = "measurement"))
    ))]
    {
        let blocks = checkpoint
            .ledger
            .blocks
            .lock()
            .unwrap()
            .owned(checkpoint.owner);
        // A traced free can end logical custody while its dead bare block
        // stays in the arena cache. Bare blocks have no heap prefix/list.
        let pointers: std::collections::BTreeSet<_> = blocks
            .into_iter()
            .map(|block| block.pointer().addr())
            .chain(
                arena
                    .entries
                    .iter()
                    .filter(|entry| !entry.pointer.is_null() && entry.live == 0)
                    .map(|entry| entry.pointer as usize),
            )
            .collect();
        for pointer in pointers {
            let pointer = std::ptr::with_exposed_provenance_mut::<crate::Header>(pointer);
            if unsafe { (*pointer).kind() } == crate::alloc::KIND_SMALL {
                unsafe { (*heap).free_cached(pointer) };
            }
        }
    }
    #[cfg(not(all(
        feature = "system-allocator",
        not(any(feature = "diagnostic", feature = "measurement"))
    )))]
    let _ = (heap, arena);
    checkpoint
        .ledger
        .blocks
        .lock()
        .unwrap()
        .discard(checkpoint.owner);
}

pub(crate) fn heap(ctx: &TaskCtx) -> *mut crate::alloc::Heap {
    let native = ctx.ahead_physical.cast::<Native>();
    if native.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the exclusively dispatched context owns this native record.
    unsafe { (*native).restart.heap }
}

pub(crate) fn active(ctx: *mut TaskCtx) -> bool {
    // SAFETY: the caller owns this context's dispatch token.
    unsafe { !heap(&*ctx).is_null() }
}

/// Idle and role-owned, before the first physical mapper allocation. Version
/// 6 proves the direct input inert and the mapper capture immutable. There is
/// no earlier mutable owner to restore. Owning legacy inputs keep head reserve.
/// # Safety
/// The saved native stack is readable through `ctx` and exclusively owned.
pub(super) unsafe fn prepare(ctx: *mut TaskCtx, native: *mut Native, sp: *mut u8) {
    unsafe { prepare_at(ctx, native, sp, ctx.cast()) }
}

unsafe fn prepare_at(ctx: *mut TaskCtx, native: *mut Native, sp: *mut u8, top: *mut u8) {
    unsafe {
        if (*native).restart.started
            || !(*native).restart.eligible
            || (*native).cpu_phase != 2
            || (!(*native).allocation.pending() && (*native).growth.is_null())
        {
            return;
        }
        (*native).restart.started = true;
        let task = engine::current(ctx);
        if engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().head() == Some(task)) {
            return;
        }
        let Some(size) = top.addr().checked_sub(sp.addr()).filter(|&n| n <= 65536) else {
            return;
        };
        let Some(ledger) = (*ctx).heap_ref().meter.cpu.get().cloned() else {
            return;
        };
        let Some(owner) = (*native).cpu_owner() else {
            return;
        };
        if !ledger.blocks.lock().unwrap().owned(owner).is_empty() {
            return;
        }
        let Ok(reservation) = ledger.reserve_heap(size as u64) else {
            return;
        };
        let (before, grant) = engine::with_driver(ctx, |d| {
            assert!(!d.cpu_allocation_leases.contains_key(&task));
            (d.stretches.get(&task).cloned(), d.saved_grant(task))
        });
        let mut stack = vec![MaybeUninit::uninit(); size];
        // SAFETY: copy padding as MaybeUninit; Rust never interprets it.
        std::ptr::copy_nonoverlapping(sp.cast(), stack.as_mut_ptr(), size);
        let checkpoint = Checkpoint {
            sp,
            stack,
            request: (*native).allocation,
            growth: (*native).growth,
            fuel: (*ctx).view.compiled.fuel,
            private_fuel: (*native).cpu_fuel.clone(),
            freed: (*ctx).ordered_freed.get(),
            charges: (*ctx).ordered_charge_trace.clone(),
            before,
            grant,
            ledger,
            owner,
            _reservation: reservation,
        };
        let registry = &*(*ctx).run_ref().heaps as *const crate::heaps::HeapRegistry;
        let private = (*registry).private_heap();
        (*ctx).heap().unbind();
        (*native).restart.registry = registry;
        (*native).restart.heap = private;
        (*native).restart.checkpoint = Some(checkpoint);
        (*private).bind(&raw mut (*ctx).view);
        (*private).set_fuel((*native).restart.checkpoint.as_ref().unwrap().fuel);
    }
}

/// No private result reaches the single-use canonical publication slot.
pub(super) unsafe fn must_revoke(ctx: *mut TaskCtx, native: *mut Native) -> bool {
    if !active(ctx) {
        return false;
    }
    // SAFETY: idle owns this image and the role prevents head movement.
    unsafe {
        let task = engine::current(ctx);
        !(*native).summary.is_null()
            || (*native).physical.reason & (4 | 16) != 0
            || engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().head() == Some(task))
    }
}

/// Revoke before physical save acknowledgement or canonical admission. The
/// restored allocation call later executes at the head with its original input.
/// # Safety
/// Idle owns the complete pure attempt, its stack and the scheduler role.
pub(super) unsafe fn rollback(ctx: *mut TaskCtx, native: *mut Native) {
    unsafe {
        let Some(checkpoint) = (*native).restart.checkpoint.take() else {
            return;
        };
        let task = engine::current(ctx);
        engine::with_driver(ctx, |d| {
            d.cpu_allocation_leases.remove(&task);
            d.frontier.as_mut().unwrap().discard_cpu_attempt(task);
            d.stretches.remove(&task);
            if let Some(before) = checkpoint.before.clone() {
                d.stretches.insert(task, before);
            }
            d.release_grant(task);
            if let Some(grant) = checkpoint.grant {
                d.grant(task, grant);
            }
        });
        let private = (*native).restart.heap;
        discard_storage(&checkpoint, private, &(*native).arena);
        (*private).unbind();
        (*native).restart.heap = std::ptr::null_mut();
        (*(*native).restart.registry).return_private(private);
        std::ptr::copy_nonoverlapping(
            checkpoint.stack.as_ptr(),
            checkpoint.sp.cast(),
            checkpoint.stack.len(),
        );
        (*ctx).ordered_freed.set(checkpoint.freed);
        (*ctx).ordered_charge_trace = checkpoint.charges.clone();
        (*ctx).heap().bind(&raw mut (*ctx).view);
        (*ctx).heap().set_fuel(checkpoint.fuel);
        (*ctx).task_sp = checkpoint.sp;
        (*native).physical.saved_sp = checkpoint.sp;
        (*native).physical.points_left = 1024;
        (*native).physical.reason = super::cpu_allocation::REASON;
        (*native).cpu_fuel = checkpoint.private_fuel;
        (*native).pending_refill = false;
        (*native).allocation = checkpoint.request;
        (*native).growth = checkpoint.growth;
        (*native).growth_status = 0;
        (*native).growth_plan = None;
        (*native).summary = std::ptr::null_mut();
        (*native).summary_status = 0;
        (*native).summary_blocks.clear();
        (*native).saved_roots = None;
        (*native).arena = super::cpu_arena::Arena::new();
        (*native).restart.eligible = false;
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests;
