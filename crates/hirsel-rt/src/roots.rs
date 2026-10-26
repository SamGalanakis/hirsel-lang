//! `Shared` root counts (MEM-11, S-4, S-5; design §6 "`Shared` roots"): exact counts,
//! immediate increments, decrements by the run's mode.
//!
//! A root block's header count is its root count, read and written only atomically and only
//! here. It never saturates: at the run's finite maximum the excess moves into the run's
//! overflow table (an exact `u64`) and the header holds [`ROOT_OVERFLOW`], which no finite
//! count reaches. The table is entered and left under its lock, by a CAS on the header, so a
//! count is always either the header's or the table's.
//!
//! - **Increment.** Compiled code adds a root with an inline CAS below the context's dup
//!   bound; anything else calls [`hirsel_rt_v2_root_dup`]. In free mode a dup that reaches the
//!   runtime cancels one of the task's pending releases of the root instead of adding.
//! - **Decrement.** Every root release calls [`hirsel_rt_v2_root_release`] or reaches
//!   [`consume`] from a release walk's `STEP_RELEASE_ROOT`. At width 1, and at the head in
//!   ordered mode, it applies at once. A non-head turn in ordered mode appends it to the
//!   task's deferred list, which its retirement applies in program order ([`retire`]). In
//!   free mode a release that finds the count at 1 is the sole holder's and frees the root at
//!   once (its walk follows the same rule for nested roots); any other joins the task's
//!   pending list, coalescing per root, which the entered phase of its next op applies
//!   ([`apply_pending`]). A walk of a root that phase frees applies its nested releases at
//!   once, as the phase's tail (design §5 Amendment F0-1).
//! - **Sole holder.** A count of 1 read with acquire ordering is the reader's alone: every
//!   other holder's release happened before it, and nobody else can add one.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::alloc::Header;
use crate::context::TaskCtx;
use crate::count::check_live_block;
use crate::fault;

/// The header count of a root whose exact count lives in the run's overflow table. A root is
/// never immortal, so this value of [`crate::COUNT_IMMORTAL`]'s bits names nothing else on a
/// root block, and every inline dup bound lies below it.
pub const ROOT_OVERFLOW: u32 = u32::MAX;

/// The exact counts of overflowed roots, by block address.
#[derive(Default)]
pub(crate) struct Overflow(Mutex<HashMap<usize, u64>>);

impl Overflow {
    /// Hold admission contended without changing a root, for runtime protocol tests.
    #[cfg(test)]
    pub(crate) fn test_hold(&self) -> std::sync::MutexGuard<'_, HashMap<usize, u64>> {
        self.lock()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<usize, u64>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A task's root releases that have not taken effect.
#[derive(Default)]
pub(crate) struct TaskRoots {
    /// Free mode: releases deferred to the task's next entered phase, per root.
    pending: hirsel_free::pending::Pending<usize>,
    /// Ordered mode: releases of a non-head turn, applied at its retirement in program order.
    deferred: Vec<*mut Header>,
    /// Ordered mode: the running turn is not the head, so its releases defer. Only the ordered
    /// executor sets it.
    pub(crate) speculative: bool,
    /// Free mode: an entered or cleanup phase is applying releases, whose walks apply nested
    /// releases at once.
    in_phase: bool,
    pub(crate) trace_creator: crate::ordered::root_ids::Creator,
    /// Only trace admission records decrements, including immediate nested tails.
    pub(crate) trace_releases: Vec<Option<(crate::ordered::root_ids::SharedRootId, bool)>>,
    /// Native replay: roots this task's releases left at count 1, whose thaw
    /// waits its phase publication wakes.
    pub(crate) sole: Vec<usize>,
}

impl TaskRoots {
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty() || !self.deferred.is_empty()
    }

    #[cfg(all(test, feature = "diagnostic"))]
    pub(crate) fn deferred_for_test(&self) -> &[*mut Header] {
        &self.deferred
    }
}

#[inline(always)]
unsafe fn word<'a>(block: *mut Header) -> &'a AtomicU32 {
    // SAFETY: a live root block's count word is only accessed atomically.
    unsafe { AtomicU32::from_ptr(&raw mut (*block).count) }
}

#[cold]
unsafe fn check(ctx: &TaskCtx, block: *mut Header) {
    // SAFETY: caller passes a block reference.
    unsafe {
        check_live_block(ctx, block);
        if !crate::shared::is_root(ctx, block) {
            fault("a root count operation on a block that is not a Shared root");
        }
    }
}

#[inline(always)]
fn measure(ctx: &mut TaskCtx, name: &'static str) {
    #[cfg(feature = "measurement")]
    ctx.heap().measurement.record(name, 1);
    #[cfg(not(feature = "measurement"))]
    let _ = (ctx, name);
}

/// The exact root count of `block` (`--trace-roots` reads it).
///
/// # Safety
/// `block` is a live root block of `ctx`'s run.
pub unsafe fn count(ctx: &TaskCtx, block: *mut Header) -> u64 {
    // SAFETY: caller contract.
    let count = unsafe { word(block) }.load(Ordering::Acquire);
    if count != ROOT_OVERFLOW {
        return u64::from(count);
    }
    let table = ctx.run_ref().root_overflow.lock();
    // The lock orders this read after any exit from the table.
    match table.get(&block.addr()) {
        Some(&count) => count,
        // SAFETY: as above; a root that left the table stored its count before unlocking.
        None => u64::from(unsafe { word(block) }.load(Ordering::Acquire)),
    }
}

/// Whether the caller holds the only root of `block`, with every other holder's release
/// acquired.
///
/// # Safety
/// `block` is a live root block of `ctx`'s run whose root the caller holds.
pub(crate) unsafe fn sole(block: *mut Header) -> bool {
    // SAFETY: caller contract.
    unsafe { word(block) }.load(Ordering::Acquire) == 1
}

/// Add one root (T03 Dup on a root): exact, through the overflow table past the run's finite
/// maximum.
unsafe fn add(ctx: &mut TaskCtx, block: *mut Header) {
    let max = ctx.run_ref().count_max;
    // SAFETY: the caller holds a root of the live block.
    let word = unsafe { word(block) };
    loop {
        let count = word.load(Ordering::Relaxed);
        if count != ROOT_OVERFLOW && count < max {
            if word
                .compare_exchange_weak(count, count + 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                measure(ctx, "root_atomics");
                return;
            }
            continue;
        }
        let mut table = ctx.run_ref().root_overflow.lock();
        let count = word.load(Ordering::Relaxed);
        if count == ROOT_OVERFLOW {
            *table
                .get_mut(&block.addr())
                .unwrap_or_else(|| fault("an overflowed root has no exact count")) += 1;
            return;
        }
        if count < max {
            continue;
        }
        if word
            .compare_exchange(count, ROOT_OVERFLOW, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            table.insert(block.addr(), u64::from(count) + 1);
            drop(table);
            measure(ctx, "root_overflow_entries");
            return;
        }
    }
}

/// A release left `block` at count 1: in native replay, a thaw may wait for it.
#[cold]
fn left_sole(ctx: &mut TaskCtx, block: *mut Header) {
    if ctx.run_ref().free_replay {
        ctx.roots.sole.push(block.addr());
    }
}

/// Give up one root at once: whether it was the last, after which the caller walks the root
/// (its count stays 1, the dying block's alone).
unsafe fn take(ctx: &mut TaskCtx, block: *mut Header) -> bool {
    let registry = ctx.run_ref().ordered_roots.get();
    let identity = registry.and_then(|registry| registry.get(block));
    let last = unsafe { take_inner(ctx, block) };
    if let Some(identity) = identity {
        ctx.roots.trace_releases.push(Some((identity, last)));
        if last {
            ctx.run_ref().ordered_roots.get().unwrap().retire(block);
        }
    } else if ctx.run_ref().ordered_roots.get().is_some()
        && unsafe { (*ctx.descriptor((*block).index())).flags } & crate::LAYOUT_MUTEX_ROOT == 0
    {
        ctx.roots.trace_releases.push(None);
    }
    last
}

unsafe fn take_inner(ctx: &mut TaskCtx, block: *mut Header) -> bool {
    let max = u64::from(ctx.run_ref().count_max);
    // SAFETY: the caller holds a root of the live block.
    let word = unsafe { word(block) };
    loop {
        let count = word.load(Ordering::Acquire);
        if count == 1 {
            return true;
        }
        if count == ROOT_OVERFLOW {
            let mut table = ctx.run_ref().root_overflow.lock();
            if word.load(Ordering::Relaxed) != ROOT_OVERFLOW {
                continue;
            }
            let exact = table
                .get_mut(&block.addr())
                .unwrap_or_else(|| fault("an overflowed root has no exact count"));
            *exact -= 1;
            if *exact <= max {
                let back = *exact as u32;
                table.remove(&block.addr());
                word.store(back, Ordering::Release);
                drop(table);
                measure(ctx, "root_overflow_exits");
                if back == 1 {
                    left_sole(ctx, block);
                }
            }
            return false;
        }
        if word
            .compare_exchange_weak(count, count - 1, Ordering::Release, Ordering::Relaxed)
            .is_ok()
        {
            measure(ctx, "root_atomics");
            if count == 2 {
                left_sole(ctx, block);
            }
            return false;
        }
    }
}

/// Whether releases of the running task wait for its next entered phase: free mode, outside
/// a phase, including cleanup after a Holding child's result-ready op.
fn defers(ctx: *mut TaskCtx) -> bool {
    // SAFETY: the context is live.
    if unsafe { (*ctx).roots.in_phase } || !crate::free::on(ctx) {
        return false;
    }
    // A returned Holding rest has a distinct actual-completion op in Free
    // mode; releases stay pending until its entry, just as before TaskFinish.
    true
}

/// Consume one root of `block` by the run's mode: whether it died now, so the caller walks it.
///
/// # Safety
/// `ctx` is live and the caller owns one root of the live root block `block`.
pub(crate) unsafe fn consume(ctx: &mut TaskCtx, block: *mut Header) -> bool {
    // SAFETY: caller contract.
    unsafe {
        if ctx.heap_ref().diagnostic {
            check(ctx, block);
        }
        #[cfg(feature = "measurement")]
        {
            ctx.heap().measurement.record("count_decrements", 1);
            ctx.heap().measurement.count_decrements += 1;
        }
        if ctx.roots.in_phase {
            return take(ctx, block);
        }
        if ctx.run_ref().tasks.is_some() && crate::tasks::ordered::engine::on(ctx) {
            let ctx = std::ptr::from_mut(ctx);
            if (*(*ctx).descriptor((*block).index())).flags & crate::LAYOUT_MUTEX_ROOT != 0 {
                // A mutex's yielding finalizers belong to its original owner.
                return crate::tasks::ordered::drops::root_release(ctx, block, || {
                    take(&mut *ctx, block)
                });
            }
            crate::tasks::ordered::drops::shared_release(ctx, block);
            return false;
        }
        // A mutex may own finalizers. Its last release belongs to this task's
        // cleanup, so it cannot be deferred to an infallible Shared phase.
        if (*ctx.descriptor((*block).index())).flags & crate::LAYOUT_MUTEX_ROOT != 0 {
            return take(ctx, block);
        }
        if ctx.roots.speculative {
            ctx.roots.deferred.push(block);
            measure(ctx, "root_deferrals");
            return false;
        }
        if defers(ctx) {
            if sole(block) {
                return true;
            }
            ctx.roots.pending.defer(block.addr());
            measure(ctx, "root_deferrals");
            return false;
        }
        take(ctx, block)
    }
}

/// Walk a root whose last credit was consumed: thaw its value and release it.
unsafe fn walk(ctx: &mut TaskCtx, block: *mut Header) {
    // A frozen graph holds no finalizer and no handle (MEM-11), so its walk cannot fail.
    // SAFETY: the caller consumed the last root of the live block.
    if unsafe { crate::release::drop_last(ctx, block, std::ptr::null(), 0, 0) } != crate::STATUS_OK
    {
        fault("a Shared root's release walk failed");
    }
}

/// Apply an owned Shared-root credit during ordered retirement. Frozen graphs
/// have no finalizers or handles, so the walk cannot suspend. Nested releases
/// belong to this same action, as they do in a Free entered-phase tail.
///
/// # Safety
/// The caller owns one root credit and the scheduler role; `block` is a Shared
/// root, never a mutex root. No heap or scheduler borrow spans this call.
pub(crate) unsafe fn retire_shared(ctx: *mut TaskCtx, block: *mut Header) {
    let ctx = unsafe { &mut *ctx };
    assert_eq!(
        unsafe { (*ctx.descriptor((*block).index())).flags } & crate::LAYOUT_MUTEX_ROOT,
        0,
        "a mutex root can run yielding finalizers"
    );
    let phase = std::mem::replace(&mut ctx.roots.in_phase, true);
    unsafe {
        if take(ctx, block) {
            walk(ctx, block);
        }
    }
    ctx.roots.in_phase = phase;
}

/// Nested decrements of a non-suspending retirement/phase cleanup apply now.
/// # Safety
/// `ctx` is exclusively running under the role. `action` runs no user code,
/// performs no host operation and cannot suspend or publish the phase early.
pub(crate) unsafe fn immediate<T>(ctx: *mut TaskCtx, action: impl FnOnce() -> T) -> T {
    let phase = unsafe { std::mem::replace(&mut (*ctx).roots.in_phase, true) };
    let result = action();
    unsafe { (*ctx).roots.in_phase = phase };
    result
}

/// Free mode: an entered phase of the running task's op applies its pending releases,
/// walking the roots they free and applying those walks' nested releases at once.
pub(crate) fn apply_pending(ctx: *mut TaskCtx) {
    // SAFETY: the context is live and its task is running.
    let ctx = unsafe { &mut *ctx };
    if ctx.roots.pending.is_empty() {
        return;
    }
    let pending = ctx.roots.pending.take();
    ctx.roots.in_phase = true;
    for (root, n) in pending {
        let block = root as *mut Header;
        for _ in 0..n {
            // SAFETY: a pending release is a root the task still owned.
            unsafe {
                if take(ctx, block) {
                    walk(ctx, block);
                }
            }
        }
    }
    ctx.roots.in_phase = false;
}

/// Storage prepared before gate admission. Root credits stay on the task's
/// pending list until an admitted prefix acquires the overflow table without
/// waiting. A rejected prefix has no effect and can release its gate slot.
pub(crate) struct PhaseRoots {
    pending: Vec<(usize, u32)>,
    dead: Vec<*mut Header>,
}

impl PhaseRoots {
    pub(crate) fn prepare(ctx: *mut TaskCtx) -> Self {
        Self {
            pending: Vec::new(),
            dead: Vec::with_capacity(unsafe { (*ctx).roots.pending.len() }),
        }
    }

    /// Apply only the prepared primary decrements under cutter exclusion.
    /// No overflow lock wait, traversal, allocation or deallocation occurs.
    /// Dead roots retain their last credit until finish runs the phase tail.
    pub(crate) fn apply(&mut self, ctx: *mut TaskCtx) -> bool {
        let ctx = unsafe { &mut *ctx };
        if ctx.roots.pending.is_empty() {
            return true;
        }
        // Project the run's disjoint table before borrowing the task's heap.
        let overflow = unsafe { &(*ctx.run_ptr()).root_overflow };
        let mut table = match overflow.0.try_lock() {
            Ok(table) => table,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return false,
        };
        self.pending = ctx.roots.pending.take();
        for &(root, n) in &self.pending {
            let block = root as *mut Header;
            for _ in 0..n {
                let word = unsafe { word(block) };
                loop {
                    let count = word.load(Ordering::Acquire);
                    if count == 1 {
                        self.dead.push(block);
                        break;
                    }
                    if count == ROOT_OVERFLOW {
                        // Entering/exiting overflow requires this same lock;
                        // inline finite-count increments cannot change it.
                        let exact = table.get_mut(&root).expect("an overflowed pending root");
                        *exact -= 1;
                        if *exact <= u64::from(ctx.run_ref().count_max) {
                            let back = *exact as u32;
                            table.remove(&root);
                            word.store(back, Ordering::Release);
                            measure(ctx, "root_overflow_exits");
                            if back == 1 {
                                left_sole(ctx, block);
                            }
                        }
                        break;
                    }
                    if word
                        .compare_exchange_weak(
                            count,
                            count - 1,
                            Ordering::Release,
                            Ordering::Relaxed,
                        )
                        .is_ok()
                    {
                        measure(ctx, "root_atomics");
                        if count == 2 {
                            left_sole(ctx, block);
                        }
                        break;
                    }
                }
            }
        }
        drop(table);
        true
    }

    /// Complete a published entered phase's deterministic tail, outside the
    /// bounded section. Replay runs this whole tail whenever it admits that
    /// entry, even if the cut preceded its live traversal (F0-1). Frozen graphs
    /// cannot run user code or hold handles; nested releases apply immediately.
    /// The owner finishes the tail before any op action or end Done can follow.
    pub(crate) fn finish(&self, ctx: *mut TaskCtx) {
        let ctx = unsafe { &mut *ctx };
        let previous = std::mem::replace(&mut ctx.roots.in_phase, true);
        for &block in &self.dead {
            unsafe { walk(ctx, block) };
        }
        ctx.roots.in_phase = previous;
    }
}

/// Wait outside gate admission after a failed try-lock. Never retain this
/// lock while attempting admission: a cutter may be waiting for an older tail.
pub(crate) fn wait_phase(ctx: *mut TaskCtx) {
    let overflow = unsafe { &(*(*ctx).run_ptr()).root_overflow };
    drop(overflow.lock());
}

/// Ordered mode: the running turn retires, so its deferred releases apply in program order.
/// The ordered executor calls it once the turn is the head.
#[allow(dead_code)]
pub(crate) fn retire(ctx: &mut TaskCtx) {
    ctx.roots.speculative = false;
    for block in std::mem::take(&mut ctx.roots.deferred) {
        // SAFETY: a deferred release is a root the task still owned.
        unsafe {
            if take(ctx, block) {
                walk(ctx, block);
            }
        }
    }
}

/// Add one root of `block` by `ctx`'s task (T03 Dup on a root), past the inline CAS's bound:
/// exact, never saturating. In free mode it first cancels one of the task's pending releases
/// of the root, which leaves the count as the pair of operations would.
///
/// # Safety
/// `ctx` is live and the caller holds a root of the live root block `block`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_root_dup(ctx: *mut TaskCtx, block: *mut Header) {
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *ctx;
        if ctx.heap_ref().diagnostic {
            check(ctx, block);
        }
        #[cfg(feature = "measurement")]
        {
            ctx.heap().measurement.record("count_increments", 1);
            ctx.heap().measurement.count_increments += 1;
        }
        if ctx.roots.pending.dup(&block.addr()) {
            measure(ctx, "root_cancellations");
            return;
        }
        add(ctx, block);
    }
}

/// Give up one root of `block` (T04 Drop on a root) by the run's mode, walking the root when
/// it was the last and the release applies now. Never fails: a frozen graph runs no finalizer.
///
/// # Safety
/// `ctx` is live and the caller owns one root of the live root block `block`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_root_release(ctx: *mut TaskCtx, block: *mut Header) {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    // SAFETY: caller contract.
    unsafe {
        let ctx = &mut *ctx;
        if consume(ctx, block) {
            walk(ctx, block);
        }
    }
}

/// Add one holder of any counted block a runtime walk copies: a root through the root
/// contract, anything else through the ordinary count.
///
/// # Safety
/// `ctx` is live and the caller holds a credit of the live block `block`.
pub(crate) unsafe fn dup_holder(ctx: *mut TaskCtx, block: *mut Header) {
    // SAFETY: caller contract.
    unsafe {
        if (*block).domain() == crate::alloc::DOMAIN_HEAP && crate::shared::is_root(&*ctx, block) {
            hirsel_rt_v2_root_dup(ctx, block);
        } else {
            crate::count::hirsel_rt_v2_dup(ctx, block);
        }
    }
}

/// Drop an exact atomic root, propagating a mutex payload's destruction failure.
/// Shared roots retain their existing deferred-release semantics.
///
/// # Safety
/// `ctx` is live and the caller owns one credit of the live root `block`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_root_drop(
    ctx: *mut TaskCtx,
    block: *mut Header,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if let Some(status) = unsafe { crate::tasks::free::tail::invoke_root_drop(ctx, block) } {
        return status;
    }
    // SAFETY: caller owns a credit; consume handles exact atomic counts.
    unsafe {
        if consume(&mut *ctx, block) {
            crate::release::drop_last(&mut *ctx, block, std::ptr::null(), anchor, operation_site)
        } else {
            crate::STATUS_OK
        }
    }
}
