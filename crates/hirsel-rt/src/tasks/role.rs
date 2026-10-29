//! Scheduler-role ownership, separate from a worker's compute permit.
//!
//! An atomic owner word publishes the scheduler tables across role transfers.
//! A task's stable context address preserves ownership across migration. Home
//! serves synchronous bridges while waiting on the same event as role release.

use crate::parking::Event;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{TaskCtx, context::RunShared};

use super::Scheduler;

// Task contexts are aligned pointers; temporary claims use a u32 task ID.
// This marker is used only by acquire_idle, which never permits re-entry.
// The marker is even but not aligned like TaskCtx, and claims are odd.
const IDLE_OWNER: usize = usize::MAX - 1;

pub(crate) mod cell;
mod owner;
use owner::Owner;

pub(crate) struct Role {
    enabled: AtomicBool,
    held: Owner,
    available: Event,
    home_waiting: AtomicBool,
    /// Ownership transitions into a task, claim or idle owner. Re-entry by
    /// the current owner is not an acquisition.
    acquisitions: AtomicU64,
}

#[cfg(test)]
thread_local! {
    static THREAD_ACQUISITIONS: Cell<u64> = const { Cell::new(0) };
}

/// The calling thread's scheduler-role acquisitions, for tests that tell an
/// operation's own role use from its wakee's dispatch on another worker.
#[cfg(test)]
pub(crate) fn thread_acquisitions() -> u64 {
    THREAD_ACQUISITIONS.with(Cell::get)
}

impl Default for Role {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            held: Owner::default(),
            available: Event::default(),
            home_waiting: AtomicBool::new(false),
            acquisitions: AtomicU64::new(0),
        }
    }
}

impl Role {
    /// Admission only, before any worker can execute Hirsel code.
    pub(crate) fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    /// The scheduler-role acquisitions so far, for headroom counters and tests.
    pub(crate) fn acquisitions(&self) -> u64 {
        self.acquisitions.load(Ordering::Relaxed)
    }

    fn acquired(&self) {
        self.acquisitions.fetch_add(1, Ordering::Relaxed);
        #[cfg(test)]
        THREAD_ACQUISITIONS.with(|count| count.set(count.get() + 1));
    }

    #[inline(always)]
    fn reentered(&self, task: usize) -> bool {
        self.held.is(task)
    }

    fn try_acquire(&self, task: usize) -> bool {
        if !self.held.try_acquire(task) {
            return false;
        }
        self.acquired();
        true
    }

    fn acquire(&self, task: usize) {
        if !self.reentered(task) {
            self.acquire_unowned(task);
        }
    }

    fn acquire_unowned(&self, task: usize) {
        loop {
            let ticket = self.available.ticket();
            for _ in 0..64 {
                if self.try_acquire(task) {
                    return;
                }
                std::hint::spin_loop();
            }
            self.available.wait(ticket);
        }
    }

    fn acquire_idle(&self) {
        self.acquire_unowned(IDLE_OWNER);
    }

    unsafe fn acquire_home(&self, run: *mut RunShared, task: usize) {
        if task != IDLE_OWNER && self.reentered(task) {
            return;
        }
        self.home_waiting.store(true, Ordering::Relaxed);
        loop {
            let ticket = self.available.ticket();
            if self.try_acquire(task) {
                self.home_waiting.store(false, Ordering::Relaxed);
                return;
            }
            if let Some(call) = unsafe { crate::pool::take_home_call(run) } {
                // No role or adapter borrow crosses this callback.
                unsafe { crate::pool::run_call(call) };
            } else {
                self.available.wait(ticket);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn home_waiting(&self) -> bool {
        self.home_waiting.load(Ordering::Relaxed)
    }

    /// Queue publication precedes the event, so home cannot miss a bridge
    /// between its last queue check and parking, even without owning the role.
    pub(crate) fn notify_home_call(&self) {
        if self.enabled.load(Ordering::Acquire) {
            self.available.notify();
        }
    }

    fn try_acquire_idle(&self) -> bool {
        self.try_acquire(IDLE_OWNER)
    }

    fn release(&self, task: usize) {
        self.release_with(task, || {});
    }

    fn release_with(&self, task: usize, before: impl FnOnce()) -> bool {
        if !self.reentered(task) {
            return false;
        }
        before();
        self.held.release();
        self.available.notify();
        true
    }
}

/// The deterministic runtime driver's adapter: one thread fires every
/// transition, so an acquisition that would wait is a driver error, never a
/// schedule. Each call is the production predicate transition.
#[cfg(test)]
impl Role {
    pub(crate) const IDLE: usize = IDLE_OWNER;

    pub(crate) fn driven_owner(&self) -> Option<usize> {
        self.held.current()
    }

    /// A task's semantic entry (`enter`), or its owner's re-entry.
    pub(crate) fn driven_enter(&self, task: usize) {
        let owner = self.driven_owner();
        assert!(
            owner.is_none_or(|owner| owner == task),
            "{task:#x} would wait for {owner:?}"
        );
        self.acquire(task);
    }

    /// The idle callback's guard (`IdleGuard::new`).
    pub(crate) fn driven_idle(&self) {
        assert_eq!(self.driven_owner(), None, "an idle callback would wait");
        self.acquire_idle();
    }

    /// Home's nonblocking idle claim (`IdleGuard::try_new`).
    pub(crate) fn driven_try_idle(&self) -> bool {
        self.try_acquire_idle()
    }

    pub(crate) fn driven_release(&self, owner: usize) -> bool {
        self.release_with(owner, || {})
    }
}

thread_local! {
    /// Whether this thread is inside a [`with_role`] closure. A table borrow
    /// is lexical and non-suspending: every host, user, home-call and fiber
    /// switch boundary checks that none is live.
    static LENT: Cell<bool> = const { Cell::new(false) };
}

/// The role-protected task/scope tables, lent to one [`with_role`] closure.
/// It is neither Send nor storable: the borrow ends when the closure
/// returns, before any suspension, user code, home call or role release.
pub(crate) struct RoleTables<'a> {
    tables: &'a mut Scheduler,
    _local: std::marker::PhantomData<*mut ()>,
}

impl std::ops::Deref for RoleTables<'_> {
    type Target = Scheduler;
    fn deref(&self) -> &Scheduler {
        self.tables
    }
}

impl std::ops::DerefMut for RoleTables<'_> {
    fn deref_mut(&mut self) -> &mut Scheduler {
        self.tables
    }
}

struct Lent;

impl Lent {
    #[inline(always)]
    fn new() -> Self {
        if LENT.with(|lent| lent.replace(true)) {
            spanned("another role table borrow");
        }
        Self
    }
}

impl Drop for Lent {
    #[inline(always)]
    fn drop(&mut self) {
        LENT.with(|lent| lent.set(false));
    }
}

/// Run `f` with the role's tables, entering the role for `ctx`'s task as a
/// semantic entry does. The borrow cannot outlive `f`; a nested borrow, or a
/// boundary reached inside `f`, is an invariant fault.
pub(crate) fn with_role<T>(ctx: *mut TaskCtx, f: impl FnOnce(&mut RoleTables<'_>) -> T) -> T {
    let tables = super::role_tables(ctx);
    let _lent = Lent::new();
    f(&mut RoleTables {
        tables,
        _local: std::marker::PhantomData,
    })
}

/// Enter the role and observe a stop as [`with_role`] does, without lending
/// the tables: for an operation whose table accesses all lie in its section.
pub(crate) fn admit(ctx: *mut TaskCtx) {
    super::role_tables(ctx);
}

/// As [`with_role`], for a caller that already holds the role and must not
/// observe a stop here: inside a recording section, whose owner ends normally
/// before any stop or suspension, or after the operation's own entry.
/// # Safety
/// `ctx`'s task holds the role, and the run's tables exist.
#[inline(always)]
pub(crate) unsafe fn with_held<T>(
    ctx: *mut TaskCtx,
    f: impl FnOnce(&mut RoleTables<'_>) -> T,
) -> T {
    let tables = unsafe { super::held_tables(ctx) };
    let _lent = Lent::new();
    f(&mut RoleTables {
        tables,
        _local: std::marker::PhantomData,
    })
}

/// No role table borrow may span `boundary` on this thread. Like every
/// runtime panic, this aborts at the compiled-code boundary.
#[inline(always)]
pub(crate) fn unlent(boundary: &str) {
    if LENT.with(Cell::get) {
        spanned(boundary);
    }
}

#[cold]
#[inline(never)]
fn spanned(boundary: &str) -> ! {
    panic!("a role table borrow spans {boundary}")
}

#[cfg(test)]
pub(crate) fn lent() -> bool {
    LENT.with(Cell::get)
}

/// Role ownership for an idle callback, independent of any task context.
/// Its Drop may run after the callback vacates a completed task's row/stack.
/// No task identity or scheduler.current is installed by this guard.
pub(crate) struct IdleGuard {
    run: *mut RunShared,
    // A guard must stay on its idle thread and never cross a fiber switch.
    _local: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl IdleGuard {
    /// Nonblocking idle claim for the home loop, which must keep serving a
    /// worker's synchronous bridge while that worker owns the role.
    /// # Safety
    /// As for new; a failed claim acquires no runtime ownership.
    pub(crate) unsafe fn try_new(run: *mut RunShared) -> Option<Self> {
        let role = unsafe { &(*run).role };
        let enabled = role.enabled.load(Ordering::Acquire);
        if enabled && !role.try_acquire_idle() {
            return None;
        }
        Some(Self {
            run: if enabled { run } else { std::ptr::null_mut() },
            _local: std::marker::PhantomData,
        })
    }

    /// Acquire on admission after releasing every adapter borrow. While a
    /// worker owns the role, its synchronous home calls must still be served.
    /// # Safety
    /// As for new, on the binding's home thread with no adapter borrow.
    pub(crate) unsafe fn home(run: *mut RunShared) -> Self {
        let role = unsafe { &(*run).role };
        let enabled = role.enabled.load(Ordering::Acquire);
        if enabled {
            unsafe { role.acquire_home(run, IDLE_OWNER) };
        }
        Self {
            run: if enabled { run } else { std::ptr::null_mut() },
            _local: std::marker::PhantomData,
        }
    }

    /// An idle callback's claim from whichever stack is idle. Home acquires
    /// as [`home`](Self::home): a role owner may be inside a synchronous home
    /// loan, which only home can serve, so a blocking idle wait there never
    /// returns.
    /// # Safety
    /// As for new, with no adapter borrow.
    pub(crate) unsafe fn idle(run: *mut RunShared) -> Self {
        if unsafe { crate::pool::on_home(run) } {
            unsafe { Self::home(run) }
        } else {
            unsafe { Self::new(run) }
        }
    }

    /// # Safety
    /// The caller is on an idle stack and holds no role or runtime borrow.
    /// `run` outlives the guard. Enabled callbacks must raw-project role-owned
    /// fields and cannot re-enter through scheduler(ctx), release the role,
    /// call a home bridge, or switch fibers while this guard is live.
    pub(crate) unsafe fn new(run: *mut RunShared) -> Self {
        // SAFETY: caller retains the run for the callback's entire lifetime.
        let role = unsafe { &(*run).role };
        let enabled = role.enabled.load(Ordering::Acquire);
        if enabled {
            role.acquire_idle();
        }
        Self {
            run: if enabled { run } else { std::ptr::null_mut() },
            _local: std::marker::PhantomData,
        }
    }
}

impl Drop for IdleGuard {
    fn drop(&mut self) {
        if !self.run.is_null() {
            // SAFETY: the callback retains RunShared, even if its task is gone.
            unsafe { (*self.run).role.release(IDLE_OWNER) };
        }
    }
}

/// Enable scheduler-role guards for a concurrent driver. Admission alone may
/// call this, after installing guarded dispatch and before starting any worker.
///
/// # Safety
/// `ctx` is the live root context before execution. The driver must obey the
/// documented enter/release and saved-fiber publication contracts.
pub unsafe fn enable(ctx: *mut TaskCtx) {
    // SAFETY: caller contract.
    unsafe { (*(*ctx).run_ptr()).role.enable() };
}

/// Claim a suspended task by id, before even borrowing the moving task table.
/// The temporary owner is replaced while the claim owns the role, without a
/// gap in which another worker can mutate the table.
///
/// # Safety
/// The run and task are live, suspended, and claimed by exactly one worker.
pub(crate) unsafe fn claim(run: *mut crate::context::RunShared, task: u32) -> *mut TaskCtx {
    // SAFETY: caller contract.
    unsafe {
        let role = &(*run).role;
        let claim = ((task as usize) << 1) | 1;
        if role.enabled.load(Ordering::Acquire) {
            if crate::pool::on_home(run) {
                role.acquire_home(run, claim);
            } else {
                role.acquire(claim);
            }
        }
        let ctx = (*(*run).tasks.get())
            .as_deref_mut()
            .expect("a run with tasks")
            .task(task)
            .ctx;
        if role.enabled.load(Ordering::Acquire) {
            role.held.replace(claim, ctx.addr());
        }
        ctx
    }
}

/// Re-enter a suspended runtime continuation without adding another semantic
/// boundary. Setting current is protected by role ownership, not a TLS cache.
///
/// # Safety
/// `ctx` belongs to the running fiber; no scheduler borrow is retained while
/// acquiring. The run outlives every caller.
pub(crate) unsafe fn enter(ctx: *mut TaskCtx) {
    // SAFETY: caller contract; these immutable fields outlive every worker.
    let run = unsafe { (*ctx).run_ptr() };
    let role = unsafe { &(*run).role };
    if !role.enabled.load(Ordering::Acquire) {
        // W1's run token owns the tables.
        unsafe { super::free::objects::retire(ctx) };
        return;
    }
    if role.reentered(ctx.addr()) {
        // This task already installed current and its shield when it acquired
        // the role. A switch or compiled-code segment releases ownership;
        // shield changes publish themselves through shield_changed.
        unsafe { super::free::objects::retire(ctx) };
        return;
    }
    // A home foreign callback can wait for a child's R while that child
    // synchronously loans runtime work to home. No scheduler or adapter
    // borrow survives entry, so serve that loan before claiming our own R.
    if unsafe { crate::pool::on_home(run) } {
        unsafe { role.acquire_home(run, ctx.addr()) };
    } else {
        role.acquire(ctx.addr());
    }
    // SAFETY: the role exclusively owns the scheduler. This intentionally
    // avoids scheduler(), which itself checks the running task's stop word.
    unsafe {
        if let Some(scheduler) = (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut() {
            scheduler.current = (*ctx).task_id;
            super::snapshot_shield(scheduler, ctx);
        }
        super::free::objects::retire(ctx);
    }
}

/// Leave before a segment, foreign call, host wait or fiber switch. The
/// continuation re-enters before touching the scheduler again. Nested guards
/// stay on their own task's stack; none owns a thread's lock guard.
///
/// # Safety
/// `ctx` is live. No scheduler/object borrow survives this call.
pub(crate) unsafe fn release(ctx: *mut TaskCtx) {
    unsafe { release_owned(ctx) };
}

/// Return the actual released ownership under the same predicate lock. Idle
/// task accounting may own the role without opening a semantic entry guard.
unsafe fn release_owned(ctx: *mut TaskCtx) -> bool {
    unlent("a role release");
    // SAFETY: caller contract.
    let role = unsafe { &(*(*ctx).run_ptr()).role };
    if role.enabled.load(Ordering::Acquire) {
        role.release_with(ctx.addr(), || {
            // SAFETY: release_with still owns the role. Completed tasks may
            // already have vacated their row, so snapshot only a matching row.
            unsafe {
                if let Some(scheduler) = (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut() {
                    super::snapshot_shield(scheduler, ctx);
                }
            }
        })
    } else {
        false
    }
}

/// One semantic runtime entry. Nested implementation calls do not leave the
/// role until the outermost entry returns to compiled code.
pub(crate) struct Guard(*mut TaskCtx);

impl Guard {
    /// # Safety
    /// As for enter. The guard stays on this task's own stack.
    pub(crate) unsafe fn new(ctx: *mut TaskCtx) -> Self {
        unsafe { super::ordered::ahead::barrier(ctx) };
        unsafe { Self::physical(ctx) }
    }

    /// # Safety
    /// Physical dispatch/fuel only; this does not authorize a semantic op.
    pub(crate) unsafe fn physical(ctx: *mut TaskCtx) -> Self {
        // SAFETY: caller contract.
        unsafe {
            if !(*(*ctx).run_ptr()).role.enabled.load(Ordering::Acquire) {
                return Self(std::ptr::null_mut());
            }
            enter(ctx);
            (*ctx).role_depth = (*ctx)
                .role_depth
                .checked_add(1)
                .expect("runtime entry depth");
        }
        Self(ctx)
    }
}

/// A semantic entry whose work is the running task's own state: it waits at
/// the ahead barrier as [`Guard::new`] does, without entering the role. Any
/// lifecycle work inside it opens its own guard.
/// # Safety
/// As for [`Guard::new`].
#[inline(always)]
pub(crate) unsafe fn task_owned(ctx: *mut TaskCtx) {
    unsafe { super::ordered::ahead::barrier(ctx) };
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        // SAFETY: this guard's task is running again, on whichever worker.
        unsafe {
            (*self.0).role_depth -= 1;
            if (*self.0).role_depth == 0 {
                release(self.0);
            }
        }
    }
}

/// Runtime calls compiled user code (a finalizer, callback, scope body, or
/// task entry). That code's operations have independent guards, even while
/// its outer runtime continuation remains on the same fiber's stack.
pub(crate) struct Segment {
    ctx: *mut TaskCtx,
    depth: u32,
    owned: bool,
}

impl Segment {
    /// # Safety
    /// The caller holds no scheduler borrow across the compiled code call.
    pub(crate) unsafe fn new(ctx: *mut TaskCtx) -> Self {
        unlent("compiled user code");
        // SAFETY: caller contract.
        unsafe {
            let depth = std::mem::take(&mut (*ctx).role_depth);
            let owned = release_owned(ctx);
            Self { ctx, depth, owned }
        }
    }
}

impl Drop for Segment {
    fn drop(&mut self) {
        // SAFETY: the user call returned on this task's fiber.
        unsafe {
            assert_eq!(
                (*self.ctx).role_depth,
                0,
                "user code left a runtime entry open"
            );
            if self.owned {
                enter(self.ctx);
            }
            (*self.ctx).role_depth = self.depth;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn home_idle_claim_never_waits_for_a_worker_bridge_or_another_idle_callback() {
        let role = Role::default();
        role.enable();
        role.acquire(8);
        assert!(!role.try_acquire_idle());
        role.release(8);
        assert!(role.try_acquire_idle());
        assert!(!role.try_acquire_idle());
        role.release(IDLE_OWNER);
        assert!(role.try_acquire_idle());
        role.release(IDLE_OWNER);
    }

    struct Run(*mut TaskCtx);
    impl Run {
        fn new(enabled: bool) -> Self {
            let mut ctx = std::ptr::null_mut();
            // SAFETY: fresh context, no workers started.
            unsafe {
                assert_eq!(
                    crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
                    0
                );
                if enabled {
                    enable(ctx);
                }
            }
            Self(ctx)
        }
        fn shared(&self) -> *mut RunShared {
            // SAFETY: test retains the context until every scoped thread joins.
            unsafe { (*self.0).run_ptr() }
        }
    }
    impl Drop for Run {
        fn drop(&mut self) {
            // SAFETY: the test has joined all workers and released all guards.
            unsafe { crate::context::hirsel_rt_v2_ctx_destroy(self.0) };
        }
    }

    #[test]
    fn local_base64_decode_preserves_role_on_success_and_rejection() {
        use crate::text::{allocate_bytes, hirsel_rt_v2_bytes_base64_decode};
        use crate::views::Slice;
        use std::ptr;

        for flags in [0, crate::context::CONFIG_DIAGNOSTIC] {
            let mut ctx = ptr::null_mut();
            unsafe {
                assert_eq!(
                    crate::context::hirsel_rt_v2_ctx_create(
                        &crate::Config {
                            flags,
                            ..crate::Config::new()
                        },
                        &mut ctx,
                    ),
                    0
                );
            }
            let run = Run(ctx);
            unsafe { enable(ctx) };
            for alphabet in [0, 1] {
                for padding in [0, 1] {
                    let tail = if padding == 0 {
                        b"TWE=".as_slice()
                    } else {
                        b"TWE"
                    };
                    let bad_tail = if padding == 0 {
                        b"AB==".as_slice()
                    } else {
                        b"AB"
                    };
                    let mut bad_block = vec![b'A'; 256];
                    bad_block[128] = b'!';
                    for (input, expected) in [
                        (b"".as_slice(), Some(b"".as_slice())),
                        (b"TWFu".as_slice(), Some(b"Man".as_slice())),
                        (tail, Some(b"Ma".as_slice())),
                        (b"!".as_slice(), None), // Length rejection, before allocation.
                        (b"AAAA!AAA".as_slice(), None), // Rejection after allocation.
                        (bad_tail, None),        // Noncanonical tail bits.
                        (bad_block.as_slice(), None), // SIMD block rejection.
                    ] {
                        unsafe {
                            let source = allocate_bytes(ctx, input);
                            let mut output = Slice {
                                owner: ptr::null_mut(),
                                start: 0,
                                length: 0,
                            };
                            // A Local leaf must preserve the driver's role ownership.
                            // A nested semantic release would enter and then release it.
                            enter(ctx);
                            assert_eq!(
                                hirsel_rt_v2_bytes_base64_decode(
                                    ctx,
                                    ptr::null_mut(),
                                    &mut output,
                                    source.owner,
                                    source.start,
                                    source.length,
                                    &alphabet,
                                    &padding,
                                ),
                                crate::STATUS_OK
                            );
                            assert_eq!((*run.shared()).role.driven_owner(), Some(ctx.addr()));
                            assert_eq!((*ctx).role_depth, 0);
                            assert_eq!((*source.owner).count, 1);
                            let shown = |s: Slice| {
                                std::slice::from_raw_parts(
                                    crate::alloc::payload(s.owner).add(8 + s.start as usize),
                                    s.length as usize,
                                )
                            };
                            assert_eq!(shown(source), input);
                            if let Some(expected) = expected {
                                assert!(!output.owner.is_null());
                                assert_ne!(output.owner, source.owner);
                                assert_eq!(shown(output), expected);
                                assert_eq!((*ctx).stats().live_blocks, 2);
                                (*ctx).heap().free(output.owner);
                            } else {
                                assert!(output.owner.is_null());
                                assert_eq!((output.start, output.length), (0, 0));
                                assert_eq!((*ctx).stats().live_blocks, 1);
                            }
                            (*ctx).heap().free(source.owner);
                            assert_eq!((*ctx).stats().live_blocks, 0);
                            release(ctx);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn idle_callbacks_exclude_each_other_and_task_owners() {
        let run = Run::new(true);
        let address = run.shared().addr();
        let inside = std::sync::atomic::AtomicUsize::new(0);
        let start = std::sync::Barrier::new(8);
        std::thread::scope(|threads| {
            for worker in 0..8 {
                let (inside, start) = (&inside, &start);
                threads.spawn(move || {
                    let shared = address as *mut RunShared;
                    start.wait();
                    for _ in 0..1000 {
                        // SAFETY: the scoped test retains the run; these idle
                        // threads hold no ctx, service borrow, or other guard.
                        let idle = if worker < 4 {
                            Some(unsafe { IdleGuard::new(shared) })
                        } else {
                            unsafe { (*shared).role.acquire((worker + 1) * 2) };
                            None
                        };
                        assert_eq!(inside.fetch_add(1, Ordering::SeqCst), 0);
                        std::thread::yield_now();
                        assert_eq!(inside.fetch_sub(1, Ordering::SeqCst), 1);
                        if idle.is_none() {
                            unsafe { (*shared).role.release((worker + 1) * 2) };
                        }
                        drop(idle);
                    }
                });
            }
        });
        assert_eq!(unsafe { (*run.shared()).role.driven_owner() }, None);
    }

    #[test]
    fn zero_depth_segment_restores_only_the_actual_idle_task_owner() {
        for held in [false, true] {
            let run = Run::new(true);
            assert_eq!(unsafe { (*run.0).role_depth }, 0);
            if held {
                // Native idle accounting enters the role without opening a
                // semantic runtime guard on the saved task's compiled stack.
                unsafe { enter(run.0) };
            }
            {
                let _segment = unsafe { Segment::new(run.0) };
                assert_eq!(unsafe { (*run.shared()).role.driven_owner() }, None);
                assert_eq!(unsafe { (*run.0).role_depth }, 0);
            }
            let restored = unsafe { (*run.shared()).role.driven_owner() };
            unsafe { release(run.0) };
            assert_eq!(restored, held.then_some(run.0.addr()));
            assert_eq!(unsafe { (*run.0).role_depth }, 0);
        }
    }

    #[test]
    fn idle_guard_can_drop_after_its_callback_vacates_task_metadata() {
        let run = Run::new(true);
        super::super::scheduler(run.0);
        unsafe { release(run.0) };
        let guard = unsafe { IdleGuard::new(run.shared()) };
        // No ctx-dependent guard may remain when completion vacates a row.
        unsafe {
            (*(*run.shared()).tasks.get())
                .as_deref_mut()
                .unwrap()
                .tasks
                .clear();
        }
        drop(guard);
        assert_eq!(unsafe { (*run.shared()).role.driven_owner() }, None);
    }

    #[test]
    fn finalizer_shield_is_published_with_and_without_role_guards() {
        for enabled in [false, true] {
            let run = Run::new(enabled);
            let s = super::super::scheduler(run.0);
            s.cancel(0, super::super::CANCEL_REQUESTED);
            unsafe { release(run.0) };
            assert_eq!(super::super::task_canceled(run.0, 0), Some(0));
            unsafe {
                crate::finalize::hirsel_rt_v2_finalize_begin(run.0, 0);
                assert_eq!(super::super::task_canceled(run.0, 0), None);
                crate::finalize::hirsel_rt_v2_finalize_begin(run.0, 0);
                crate::finalize::hirsel_rt_v2_finalize_end(run.0, 0);
                assert_eq!(super::super::task_canceled(run.0, 0), None);
                crate::finalize::hirsel_rt_v2_finalize_end(run.0, 0);
                assert_eq!(super::super::task_canceled(run.0, 0), Some(0));
                release(run.0);
            }
        }
    }

    #[test]
    fn remote_shield_reads_use_the_snapshot_while_local_state_changes() {
        let run = Run::new(true);
        let s = super::super::scheduler(run.0);
        s.cancel(0, super::super::CANCEL_REQUESTED);
        unsafe {
            (*run.0).shield = 1;
            release(run.0);
            // Stand in for an owner's task-local entry with exclusive ctx
            // access. Remote metadata must retain the published value.
            (*run.0).shield = 0;
            let _idle = IdleGuard::new(run.shared());
            let s = (*(*run.shared()).tasks.get()).as_deref_mut().unwrap();
            assert_eq!(s.task(0).shield(), 1);
            assert_eq!(
                s.canceled_task(0, s.tasks[0].as_ref().unwrap().shield()),
                None
            );
        }
    }

    #[test]
    fn owner_predicate_serializes_threads_and_reenters_for_the_same_task() {
        let role = Arc::new(Role::default());
        role.enable();
        let in_role = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        std::thread::scope(|threads| {
            for task in 1..=8 {
                let role = Arc::clone(&role);
                let in_role = Arc::clone(&in_role);
                threads.spawn(move || {
                    for _ in 0..1000 {
                        role.acquire(task);
                        role.acquire(task);
                        assert_eq!(in_role.fetch_add(1, Ordering::SeqCst), 0);
                        std::thread::yield_now();
                        assert_eq!(in_role.fetch_sub(1, Ordering::SeqCst), 1);
                        role.release(task);
                    }
                });
            }
        });
        assert_eq!(role.driven_owner(), None);
    }
}
