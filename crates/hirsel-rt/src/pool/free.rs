//! Physical dispatch of saved segments, independent of their logical policy.
//!
//! Notification/Queued/Running use the Tokio task-harness ownership protocol
//! in hirsel-free::frame. Only an idle continuation publishes a started stack.
//! The scheduler role still owns task rows and operation effects. Admission
//! must explicitly enable this dispatcher; the W1 run-token path is unchanged.

use super::*;
use hirsel_free::frame::{self, Notification, Placement, Queued, Running};
use hirsel_free::schedule::Budget;
use hirsel_free::work::{Ready, Source, Worker as ReadyWorker};

use crate::tasks::registry::TaskKey;

struct Image {
    ctx: *mut TaskCtx,
    task: TaskKey,
    sp: *mut u8,
}

// SAFETY: the image is accessed only by its exclusive Running owner. Queued
// transfers it after the idle stack saved sp. Notifications never inspect it.
unsafe impl Send for Image {}

pub(crate) struct Frame {
    notification: Notification<Image>,
    /// The owner word and, under forced migration, the exec a compact frame left.
    placement: Placement,
    saved: AtomicBool,
    // A test stop channel's observer; it never reads a queued image.
    stop_before_start: bool,
}

impl Frame {
    pub(crate) fn is_saved(&self) -> bool {
        self.saved.load(Ordering::Acquire)
    }

    /// Whether this frame lets the run be quiet: idle, or, for the task whose
    /// save callback asks (`saving`), running with no remembered wake. A
    /// role-free object wake queues a frame whose row is still parked.
    pub(crate) fn quiet(&self, saving: bool) -> bool {
        if saving {
            !self.notification.notified()
        } else {
            self.notification.idle()
        }
    }

    /// The idle callback owns the scheduler role before exposing slots or
    /// completion. Otherwise a join could retire this row before its callback.
    pub(crate) fn mark_saved(&self) {
        self.saved.store(true, Ordering::Release);
    }

    /// A frame never published to a worker, for registry ownership tests.
    #[cfg(test)]
    pub(crate) fn unpublished(task: TaskKey, saved: bool) -> Arc<Self> {
        let (notification, queued) = frame::prepared(Image {
            ctx: std::ptr::null_mut(),
            task,
            sp: std::ptr::null_mut(),
        });
        drop(queued);
        Arc::new(Self {
            notification,
            placement: Placement::new(0),
            saved: AtomicBool::new(saved),
            stop_before_start: false,
        })
    }
}

/// Whether the task's parked stack is available to the logical policy for
/// join delivery, receive slots, shield reads, and retirement. A row becoming
/// Parked or Holding alone is insufficient: its old worker may still be saving.
///
/// # Safety
/// The caller holds the scheduler role; `task` is a live published row,
/// which the caller has just read under that role.
pub(crate) unsafe fn is_saved(run: *mut RunShared, task: u32) -> bool {
    unsafe { crate::tasks::frame_saved(run, task) }
}

struct Count {
    live: AtomicU32,
    home: std::sync::Weak<Exec>,
}

struct Lease(Arc<Count>);
impl Lease {
    fn new(count: &Arc<Count>) -> Self {
        count.live.fetch_add(1, Ordering::AcqRel);
        Self(count.clone())
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.0.live.fetch_sub(1, Ordering::AcqRel) == 1
            && let Some(home) = self.0.home.upgrade()
        {
            wake(&home);
        }
    }
}

pub(super) struct Saved {
    queued: Queued<Image>,
    frame: Arc<Frame>,
    // Drop execution ownership before releasing the activity reservation.
    lease: Lease,
    /// Claimed from its worker's LIFO slot: it continues that worker's
    /// immediate chain and its operation budget rather than starting one.
    chained: bool,
}

pub(super) struct Active {
    running: Running<Image>,
    frame: Arc<Frame>,
    lease: Lease,
}

#[derive(Clone, Copy)]
pub(crate) enum Leave {
    Suspend {
        owner: Owner,
        yielded: bool,
    },
    /// A role-free object park (`tasks::free::park_registered`): the save
    /// callback parks the row and reads the owner under its own role.
    Park,
    /// A saved compact frame left its stack for good (`tasks::compact`).
    Compact,
    Complete,
}

pub(super) struct Dispatch {
    ready: Arc<Ready<Saved>>,
    locals: Mutex<Vec<ReadyWorker<Saved>>>,
    count: Arc<Count>,
    /// Reservations precede publication; claims consume one after removal.
    queued: AtomicUsize,
    queued_stop_witness: Option<std::path::PathBuf>,
    /// At most one worker is being woken to search (Go wakep / Tokio Idle).
    searcher: AtomicUsize,
    #[cfg(test)]
    after_search: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    before_save: std::sync::OnceLock<Box<dyn Fn(u32, bool) + Send + Sync>>,
    #[cfg(test)]
    before_idle: std::sync::OnceLock<Box<dyn Fn(u32) + Send + Sync>>,
    /// Observes each compact leave's released stack: its task, the stack's
    /// context slot, and whether the idle worker's pool now holds it.
    #[cfg(test)]
    after_compact: std::sync::OnceLock<CompactHook>,
}

/// Install a per-run idle-save observer before the root spawns children. It runs after
/// the worker released its permit and outside every scheduler-role borrow.
///
/// # Safety
/// The root owns this live run, physical dispatch is enabled, and no child
/// has started. The observer must retain no runtime borrow across its return.
#[cfg(test)]
pub(crate) unsafe fn test_before_save(
    ctx: *mut TaskCtx,
    hook: impl Fn(u32, bool) + Send + Sync + 'static,
) {
    assert!(
        execs(ctx)
            .dispatch
            .get()
            .unwrap()
            .before_save
            .set(Box::new(hook))
            .is_ok()
    );
}

/// Install a per-run observer between scheduler save publication and the
/// physical token's transition to idle. It runs outside the scheduler role.
/// # Safety
/// As for [`test_before_save`].
#[cfg(test)]
pub(crate) unsafe fn test_before_idle(
    ctx: *mut TaskCtx,
    hook: impl Fn(u32) + Send + Sync + 'static,
) {
    assert!(
        execs(ctx)
            .dispatch
            .get()
            .unwrap()
            .before_idle
            .set(Box::new(hook))
            .is_ok()
    );
}

#[cfg(test)]
type CompactHook = Box<dyn Fn(u32, usize, bool) + Send + Sync>;

/// Install a per-run observer of compact leaves, as for [`test_before_save`].
/// # Safety
/// As for [`test_before_save`].
#[cfg(test)]
pub(crate) unsafe fn test_after_compact(
    ctx: *mut TaskCtx,
    hook: impl Fn(u32, usize, bool) + Send + Sync + 'static,
) {
    assert!(
        execs(ctx)
            .dispatch
            .get()
            .unwrap()
            .after_compact
            .set(Box::new(hook))
            .is_ok()
    );
}

fn owner_word(owner: Owner) -> usize {
    match owner {
        Owner::Any => 0,
        Owner::Home => 1,
        Owner::Pool(slot) => (usize::from(slot) << 2) | 2,
        Owner::Compute(address) => {
            assert_eq!(address & 3, 0);
            address | 3
        }
    }
}
fn word_owner(owner: usize) -> Owner {
    match owner & 3 {
        0 => {
            assert_eq!(owner, 0);
            Owner::Any
        }
        1 => {
            assert_eq!(owner, 1);
            Owner::Home
        }
        2 => Owner::Pool((owner >> 2) as u16),
        3 => Owner::Compute(owner & !3),
        _ => unreachable!(),
    }
}

/// Admission-only opt-in. Logical policies must route every ready/park/end
/// through this module and obey saved-frame gating of parked stack slots.
///
/// # Safety
/// No task has started; topology is installed, on its admission thread. All
/// service lifetime and scheduler role contracts must hold before activation.
pub(crate) unsafe fn enable(ctx: *mut TaskCtx) {
    let execs = execs(ctx);
    unsafe { crate::tasks::install_cancel_metrics(ctx) };
    let metrics = unsafe { (*ctx).run_ref() }
        .worker_metrics
        .as_ref()
        .map(|m| m.ready_metrics());
    let (ready, locals) = hirsel_free::work::workers(MAX_WIDTH, metrics);
    // As with HIRSEL_RT_STOP_AFTER_MS, only a test host requests this stop.
    // Read the channel once at admission, outside scheduler paths.
    let queued_stop_witness = unsafe { (*ctx).run_ref() }
        .free_production
        .then(|| std::env::var_os("HIRSEL_RT_FREE_STOP_BEFORE_FIRST_CHILD"))
        .flatten()
        .map(std::path::PathBuf::from);
    if queued_stop_witness.is_some() {
        unsafe { (*ctx).run_ref() }.control.host();
    }
    assert!(
        execs
            .dispatch
            .set(Dispatch {
                ready,
                locals: Mutex::new(locals),
                queued: AtomicUsize::new(0),
                queued_stop_witness,
                searcher: AtomicUsize::new(0),
                #[cfg(test)]
                after_search: Mutex::new(None),
                #[cfg(test)]
                before_save: Default::default(),
                #[cfg(test)]
                before_idle: Default::default(),
                #[cfg(test)]
                after_compact: Default::default(),
                count: Arc::new(Count {
                    live: AtomicU32::new(0),
                    home: Arc::downgrade(&execs.home),
                }),
            })
            .is_ok(),
        "physical dispatch enabled twice"
    );
    set_width(execs, execs.compute.lock().unwrap().active.len());
    // SAFETY: admission, before the first task starts.
    unsafe { crate::tasks::role::enable(ctx) };
    // Workers started before opt-in may still be waiting in W1's inbox-only
    // predicate. A real post moves each to the physical ready predicate.
    for exec in &execs.compute.lock().unwrap().active {
        exec.post(Post::Wake, None);
    }
}

/// Size injection refills by the live compute membership. Only the run-token
/// holder changes membership; the refill share is a physical tuning input.
pub(super) fn set_width(execs: &Execs, active: usize) {
    if let Some(dispatch) = execs.dispatch.get() {
        dispatch
            .ready
            .set_width(active.clamp(1, MAX_WIDTH as usize) as u32);
    }
}

pub(crate) fn enabled(run: &RunShared) -> bool {
    run.execs
        .as_deref()
        .is_some_and(|execs| execs.dispatch.get().is_some())
}

/// Whether every suspension publishes its frame to another worker (the
/// forced-migration test seam: `Host::with_workers` and `BodyRun::new`).
pub(crate) fn migrating(run: &RunShared) -> bool {
    run.execs.as_deref().is_some_and(|execs| execs.migrate)
}

pub(super) fn pending(execs: &Execs) -> bool {
    execs
        .dispatch
        .get()
        .is_some_and(|dispatch| dispatch.count.live.load(Ordering::Acquire) != 0)
}

/// # Safety
/// Idle only, after notifications were invalidated and the context's values
/// were consumed. No pointer into this stack may be accessed again.
pub(crate) unsafe fn recycle_stack(stack: Stack) {
    let me = own_exec();
    assert!(!me.is_null(), "stack retirement requires its idle worker");
    // SAFETY: this worker belongs exclusively to this idle thread.
    unsafe { (*(*me).worker.get()).idle_stacks.put(stack) };
}

/// Lend the running physical image to an immediate child's stable context.
/// The parent's continuation remains below it on the same native stack. A
/// child may save/migrate that stack; its return restores the parent's image.
/// # Safety
/// The caller owns the role and current image; neither row is published elsewhere.
pub(crate) unsafe fn lend(ctx: *mut TaskCtx, task: TaskKey) {
    unsafe {
        let me = own_exec();
        assert!(!me.is_null());
        let mut active = (*me).active.take().expect("running image to lend");
        let image = active.running.value_mut();
        image.ctx = ctx;
        image.task = task;
        (*me).active.set(Some(active));
    }
}

/// Notify a task selected by the logical policy. A running notification
/// never publishes: its idle continuation does that after saving the stack.
/// A stale `task` (its row retired, its slot perhaps reused) is dropped.
///
/// # Safety
/// The caller holds the run's scheduler role (or is admitting its root). A
/// live row's stack stays allocated until completion.
pub(crate) unsafe fn schedule_saved(run: *mut RunShared, task: TaskKey, owner: Owner) {
    let execs = unsafe { (*(*run).execs.get()).as_deref().expect("physical dispatch") };
    if let Some(saved) = unsafe { admit(run, execs, task, owner) } {
        // A notifier can keep executing indefinitely after this op. Work
        // cannot remain in its private LIFO slot until its next idle turn.
        publish(execs, saved, false, false);
    }
}

/// Transfer one notification into queue ownership, or latch it on a running
/// image. `None` means the task's idle save, or an earlier wake, publishes,
/// or that `task` is stale: a generation mismatch neither notifies nor
/// exposes the slot's current frame (slotmap `SlotMap::get`, basic.rs:700-705).
/// # Safety
/// As for [`schedule_saved`].
unsafe fn admit(run: *mut RunShared, execs: &Execs, task: TaskKey, owner: Owner) -> Option<Saved> {
    // SAFETY: the role owns this task lookup. No borrow survives publication.
    let (slot, ctx) = unsafe { crate::tasks::dispatch_frame(run, task) }?;
    let dispatch = execs.dispatch.get().expect("physical dispatch enabled");
    unsafe {
        if let Some(frame) = slot {
            let owner = route(execs, frame, owner);
            frame.placement.set_owner(owner_word(owner));
            frame.notification.notify().map(|queued| Saved {
                queued,
                frame: frame.clone(),
                lease: Lease::new(&dispatch.count),
                chained: false,
            })
        } else {
            let (notification, queued) = frame::prepared(Image {
                ctx,
                task,
                sp: (*ctx).task_sp,
            });
            assert!(!(*ctx).task_sp.is_null(), "a prepared task has a stack");
            let frame = Arc::new(Frame {
                notification,
                placement: Placement::new(owner_word(owner)),
                saved: AtomicBool::new(true),
                stop_before_start: task.slot == 1 && dispatch.queued_stop_witness.is_some(),
            });
            *slot = Some(frame.clone());
            if crate::tasks::ordered::engine::on(ctx) {
                crate::tasks::ordered::cpu::bind_frame(run, task, &frame);
            }
            Some(Saved {
                queued,
                frame,
                lease: Lease::new(&dispatch.count),
                chained: false,
            })
        }
    }
}

/// A role-free wake for one registered object waiter (S2 `NotifyToken`).
/// It holds the waiter's frame and the owner its park leaves it with, both
/// captured under the role at registration. Its notification runs only the
/// frame's own latch: a running waiter remembers it until its save, and a
/// stale token can only coalesce, wake a frame that has completed, or wake
/// one its dispatch returns unstarted (`tasks::resumed`).
///
/// The owner is the one admit() would read from the row at wake time: a
/// parked row's owner never changes (only a running task pins or unpins,
/// and its park leaves it unpinned), and a token is taken only for a task
/// still parked in, or entering, the wait it registered for. A task's own
/// token ([`own_token`]) has none: its role-free park's save sets the owner
/// before any wake can publish the frame.
pub(crate) struct Token {
    frame: Arc<Frame>,
    owner: Option<usize>,
}

impl Clone for Token {
    fn clone(&self) -> Self {
        Self {
            frame: self.frame.clone(),
            owner: self.owner,
        }
    }
}

/// The running task's wake token, or `None` without physical dispatch.
/// # Safety
/// The caller holds the run's scheduler role; `task` is its running task.
pub(crate) unsafe fn token(run: *mut RunShared, task: TaskKey, owner: Owner) -> Option<Token> {
    if !enabled(unsafe { &*run }) {
        return None;
    }
    // SAFETY: the role owns this task lookup.
    let (slot, _) = unsafe { crate::tasks::dispatch_frame(run, task) }
        .unwrap_or_else(|| fault("a wake token for a stale task key"));
    slot.as_ref().map(|frame| Token {
        frame: frame.clone(),
        owner: Some(owner_word(owner)),
    })
}

/// The running task's own wake token, which it keeps for its role-free
/// object parks (`Leave::Park`), or `None` without physical dispatch. Its
/// frame is the task's for life: a lent inline child's is its parent's.
/// # Safety
/// As for [`token`].
pub(crate) unsafe fn own_token(run: *mut RunShared, task: TaskKey) -> Option<Token> {
    if !enabled(unsafe { &*run }) {
        return None;
    }
    // SAFETY: the role owns this task lookup.
    let (slot, _) = unsafe { crate::tasks::dispatch_frame(run, task) }
        .unwrap_or_else(|| fault("a wake token for a stale task key"));
    slot.as_ref().map(|frame| Token {
        frame: frame.clone(),
        owner: None,
    })
}

/// Wake the token's task without the scheduler role, as admit() and
/// publish() do for a policy wake. Its dispatch leaves the logical wait under
/// the role it takes to start the task (`tasks::resumed`); its frame's latch
/// keeps the run from a quiet verdict meanwhile (`Frame::quiet`).
/// # Safety
/// The run's execs are initialized and its physical dispatch is enabled.
pub(crate) unsafe fn notify(run: *mut RunShared, token: &Token) {
    let execs = unsafe { (*(*run).execs.get()).as_deref().expect("physical dispatch") };
    notify_frame(execs, token);
}

fn notify_frame(execs: &Execs, token: &Token) {
    let dispatch = execs.dispatch.get().expect("physical dispatch enabled");
    if let Some(owner) = token.owner {
        let owner = route(execs, &token.frame, word_owner(owner));
        token.frame.placement.set_owner(owner_word(owner));
    } else if token.frame.placement.left() != 0 {
        let owner = route(
            execs,
            &token.frame,
            word_owner(token.frame.placement.owner()),
        );
        token.frame.placement.set_owner(owner_word(owner));
    }
    if let Some(queued) = token.frame.notification.notify() {
        publish(
            execs,
            Saved {
                queued,
                frame: token.frame.clone(),
                lease: Lease::new(&dispatch.count),
                chained: false,
            },
            false,
            false,
        );
    }
}

/// The most saved continuations a [`ReadyBatch`] holds before it flushes.
pub(crate) const BATCH: usize = 32;

/// Bounded owned ready publication (S2 `ReadyBatch`). A policy that makes
/// several saved continuations ready in one runtime entry publishes all of
/// its Any-owned entries with one injection and one wake decision; a woken
/// searcher that claims work wakes the next (Tokio `worker.rs:647-651`), so
/// one wake suffices. Dropping flushes, so no admitted entry survives the
/// entry that made it ready: nothing hides behind a park, a foreign call or
/// a return to potentially nonterminating user code. Batching is transport
/// only; entries keep their policy order and every ordinal and wake position.
pub(crate) struct ReadyBatch {
    run: *mut RunShared,
    execs: *const Execs,
    /// Exactly the first `len` entries are initialized; no slot is cleared
    /// or dropped beyond them.
    entries: [std::mem::MaybeUninit<Saved>; BATCH],
    len: usize,
}

impl ReadyBatch {
    /// # Safety
    /// The caller holds the run's scheduler role (or admits its root) until
    /// this batch drops, and the run has physical dispatch enabled.
    pub(crate) unsafe fn new(run: *mut RunShared) -> Self {
        let execs = unsafe { (*(*run).execs.get()).as_deref().expect("physical dispatch") };
        Self {
            run,
            execs,
            entries: [const { std::mem::MaybeUninit::uninit() }; BATCH],
            len: 0,
        }
    }

    /// Notify a task selected by the logical policy; publication waits for
    /// this batch's flush or drop.
    ///
    /// # Safety
    /// As for [`schedule_saved`].
    pub(crate) unsafe fn schedule(&mut self, task: TaskKey, owner: Owner) {
        let execs = unsafe { &*self.execs };
        if let Some(saved) = unsafe { admit(self.run, execs, task, owner) } {
            self.push(saved);
        }
    }

    fn push(&mut self, saved: Saved) {
        if word_owner(saved.frame.placement.owner()) != Owner::Any {
            // An addressed owner has its own inbox; there is no shared wake.
            publish(unsafe { &*self.execs }, saved, false, false);
            return;
        }
        self.entries[self.len].write(saved);
        self.len += 1;
        if self.len == BATCH {
            self.flush();
        }
    }

    /// Publish every held entry, then make one wake decision. A notifier can
    /// keep executing indefinitely after its op, so non-idle publication never
    /// uses its private LIFO slot.
    pub(crate) fn flush(&mut self) {
        let len = std::mem::take(&mut self.len);
        if len == 0 {
            return;
        }
        // SAFETY: the batch's creator retains the run's topology.
        let execs = unsafe { &*self.execs };
        inject(
            execs,
            self.entries[..len].iter().map(|entry| {
                // SAFETY: entries below the taken len were written by push
                // and are read exactly once; len is already zero.
                unsafe { entry.assume_init_read() }
            }),
        );
    }
}

impl Drop for ReadyBatch {
    fn drop(&mut self) {
        self.flush();
    }
}

/// The one shared transport for Any-owned continuations: a single
/// injection and a single wake decision for every entry.
fn inject(execs: &Execs, entries: impl ExactSizeIterator<Item = Saved>) {
    if !execs.demand && home_computes(execs) {
        for saved in entries {
            execs.home.post(Post::Saved(saved), None);
        }
        return;
    }
    let dispatch = execs.dispatch.get().expect("physical dispatch enabled");
    dispatch.queued.fetch_add(entries.len(), Ordering::SeqCst);
    dispatch.ready.inject(entries);
    wake_ready(execs);
}

fn publish(execs: &Execs, saved: Saved, yielded: bool, idle: bool) {
    match word_owner(saved.frame.placement.owner()) {
        Owner::Any => {
            let me = own_exec();
            // SAFETY: an exec's local state belongs only to its OS thread.
            // Tokio's LIFO slot is private until the current poll returns.
            // A Hirsel pure segment need not return, so only an idle
            // continuation may use the slot: it searches again immediately.
            // Home may retain a local after a width-one interval. Once
            // wider, it serves host work rather than searching that local.
            let local = if idle
                && !execs.migrate
                && !home_computes(execs)
                && !me.is_null()
                && matches!(unsafe { (*me).kind }, Kind::Compute(_))
            {
                unsafe { (*me).ready.take() }
            } else {
                None
            };
            let Some(mut local) = local else {
                // A non-idle publisher may enter a pure interval indefinitely.
                return inject(execs, std::iter::once(saved));
            };
            let dispatch = execs.dispatch.get().expect("physical dispatch enabled");
            dispatch.queued.fetch_add(1, Ordering::SeqCst);
            local.push(&dispatch.ready, saved, yielded);
            unsafe { (*me).ready.set(Some(local)) };
            // This idle worker will claim its own sole continuation next.
            if dispatch.queued.load(Ordering::SeqCst) > 1 {
                wake_ready(execs);
            }
        }
        owner => {
            let to = match owner {
                Owner::Home => Arc::as_ptr(&execs.home),
                Owner::Pool(slot) => pool_exec(execs, slot),
                Owner::Compute(address) => std::ptr::with_exposed_provenance(address),
                Owner::Any => unreachable!(),
            };
            // SAFETY: topology retains every destination until shutdown.
            unsafe { (*to).post(Post::Saved(saved), None) };
        }
    }
}

fn wake_ready(execs: &Execs) {
    let Some(dispatch) = execs.dispatch.get() else {
        return;
    };
    if dispatch.queued.load(Ordering::SeqCst) == 0 || dispatch.searcher.load(Ordering::SeqCst) != 0
    {
        return;
    }
    let mut compute = execs.compute.lock().unwrap();
    // Go 1.25 runtime/proc.go:5144-5157 readies work before wakep.
    // Membership grows only if every existing worker is occupied. The
    // requested width remains in control; this is just native thread lifetime.
    if execs.demand
        && !execs.home.inbox.lock().unwrap().closed
        && compute.active.iter().all(|exec| exec.busy())
    {
        let width = unsafe { (&(*execs.run.0).control).width.load(Ordering::Acquire) };
        let active = compute.active.len();
        if active < width as usize {
            grow(execs.run.0, execs, &mut compute, active as u32 + 1);
            set_width(execs, compute.active.len());
        }
    }
    let width = compute.active.len();
    if width == 0 {
        return;
    }
    let first = execs.cursor.fetch_add(1, Relaxed);
    for offset in 0..width {
        let exec = &compute.active[(first + offset) % width];
        if !exec.busy() {
            if exec.kind == Kind::Home {
                // Home also services host posts; its inbox predicate needs
                // an actual post, rather than a compute-search epoch alone.
                exec.post(Post::Wake, None);
                return;
            }
            // Reserve before notifying, so repeated publications do not wake
            // more workers before this searcher can observe the queue. Like
            // Tokio idle.rs:51-127 and Go proc.go:3232-3272, the last searcher
            // releases its reservation before a final work check.
            if dispatch
                .searcher
                .compare_exchange(
                    0,
                    Arc::as_ptr(exec) as usize,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_err()
            {
                return;
            }
            // Publish before locking the sleep predicate: a search that missed
            // this queue either sees the epoch change or receives the bell.
            exec.ready_epoch.fetch_add(1, Ordering::Release);
            exec.bell.notify();
            return;
        }
    }
}

/// Suspend a segment. The continuation returns with the scheduler role;
/// runtime/user callers must release it at their usual semantic boundary.
///
/// # Safety
/// The caller is the running task, holding the role. No scheduler, object,
/// worker, or run borrow may survive the switch. Its stack stays allocated.
pub(crate) unsafe fn suspend_current(ctx: *mut TaskCtx, yielded: bool) {
    let owner = crate::tasks::dispatch_owner(ctx);
    // SAFETY: caller contract; only this thread touches its Exec cells.
    unsafe {
        (*ctx).switch_out();
        let execs = execs(ctx);
        let me = current(execs);
        assert!(
            (*me)
                .leaving
                .replace(Some(Leave::Suspend { owner, yielded }))
                .is_none()
        );
        let idle = idle_of(ctx, execs, me);
        crate::tasks::role::release(ctx);
        fiber::switch(&raw mut (*ctx).task_sp, idle);
        crate::tasks::role::enter(ctx);
    }
}

/// Suspend a segment parked on an object wait it registered without the
/// role. The idle continuation parks its row and sets its owner under the
/// save callback's role, before its notification state settles: a wake that
/// beat the save republishes the frame (`saved`). It returns without the role.
///
/// # Safety
/// The caller is the running task at role depth zero, on a compute or home
/// worker. No scheduler, object, worker, or run borrow may survive the switch.
pub(crate) unsafe fn park_current(ctx: *mut TaskCtx) {
    // SAFETY: caller contract; only this thread touches its Exec cells.
    unsafe {
        (*ctx).switch_out();
        let execs = execs(ctx);
        let me = current(execs);
        assert!((*me).leaving.replace(Some(Leave::Park)).is_none());
        let idle = idle_of(ctx, execs, me);
        fiber::switch(&raw mut (*ctx).task_sp, idle);
    }
}

/// Abandon completed execution ownership. The idle continuation invalidates
/// notifications before releasing its activity reservation or reusing a stack.
///
/// # Safety
/// As for suspend_current; no task-owned Rust value requiring Drop may remain
/// on this abandoned stack. The caller settled its logical completion.
pub(crate) unsafe fn finish_current(ctx: *mut TaskCtx) -> ! {
    unsafe {
        (*ctx).switch_out();
        let execs = execs(ctx);
        let me = current(execs);
        assert!((*me).leaving.replace(Some(Leave::Complete)).is_none());
        let idle = idle_of(ctx, execs, me);
        crate::tasks::role::release(ctx);
        let mut abandoned = std::ptr::null_mut();
        fiber::switch(&raw mut abandoned, idle);
    }
    fault("a completed physical segment was resumed")
}

/// Leave a saved compact frame's stack for good. The idle continuation
/// gives the stack back to its worker and publishes the frame, which any
/// worker activates on a stack of its own (`tasks::compact`).
///
/// # Safety
/// The running task holds the role at depth one from its resume entry, and
/// `tasks::compact::leave` accepted this resume point: no runtime borrow,
/// wait or address into this stack is live, and no frame on it owns a value
/// requiring Drop.
pub(crate) unsafe fn leave_compact(
    ctx: *mut TaskCtx,
    compact: Box<crate::continuation::Compact>,
) -> ! {
    // SAFETY: caller contract; only this thread touches its Exec cells.
    unsafe {
        assert!((*ctx).compact.replace(compact).is_none());
        (*ctx).switch_out();
        let execs = execs(ctx);
        let me = current(execs);
        assert!((*me).leaving.replace(Some(Leave::Compact)).is_none());
        let idle = idle_of(ctx, execs, me);
        // The abandoned frames' guards never drop: the lent stack's entry
        // takes the role afresh.
        (*ctx).role_depth = 0;
        crate::tasks::role::release(ctx);
        let mut abandoned = std::ptr::null_mut();
        fiber::switch(&raw mut abandoned, idle);
    }
    fault("a left compact stack was resumed")
}

/// The owner a notification publishes `frame` to: under forced migration, a
/// compact frame goes to a compute worker other than the one it left, when
/// the run has one; every other frame keeps `owner`.
fn route(execs: &Execs, frame: &Frame, owner: Owner) -> Owner {
    let left = frame.placement.left();
    if owner != Owner::Any || left == 0 {
        return owner;
    }
    let compute = execs.compute.lock().unwrap();
    let width = compute.active.len();
    let here = compute
        .active
        .iter()
        .position(|other| Arc::as_ptr(other).addr() == left);
    let next = &compute.active[here.map_or(0, |here| (here + 1) % width.max(1))];
    if Arc::as_ptr(next).addr() == left || next.kind == Kind::Home {
        return owner;
    }
    Owner::Compute(Arc::as_ptr(next).expose_provenance())
}

/// Idle only: the switch has saved task_sp before this becomes reachable.
pub(super) unsafe fn saved(run: *mut RunShared, exec: &Exec) {
    let Some(leaving) = exec.leaving.take() else {
        return;
    };
    let Active {
        mut running,
        frame,
        lease,
    } = exec.active.take().expect("running physical frame");
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    execs.permits.release(exec);
    crate::stop::set_running(std::ptr::null_mut());
    crate::foreign::restore(crate::foreign::ForeignScope::NONE);
    #[cfg(test)]
    if let Some(hook) = execs
        .dispatch
        .get()
        .and_then(|dispatch| dispatch.before_save.get())
    {
        hook(
            running.value().task.slot,
            matches!(leaving, Leave::Complete),
        );
    }
    match leaving {
        Leave::Complete => {
            let task = running.value().task;
            drop(running);
            // The save callback exposes completion to joiners. Release the
            // executor's frame reference first. Older idle saves and wake
            // tokens can still retain their own references until they leave.
            drop(frame);
            // Keep busy while acquiring the role: a stop must retain this
            // worker's run, and no activity lock may precede the role lock.
            unsafe { crate::tasks::after_save(run, task, true) };
            let _activity = execs.activity.lock().unwrap();
            exec.busy.store(false, Ordering::SeqCst);
            drop(lease);
        }
        Leave::Compact => {
            let task = running.value().task;
            // SAFETY: the leave switched away from its stack for good.
            let (ctx, stack) = unsafe { crate::tasks::compact::saved(run, task) };
            let released = stack.context() as usize;
            // SAFETY: this idle worker's own pool; no frame runs on the stack.
            unsafe { (*exec.worker.get()).idle_stacks.put(stack) };
            #[cfg(any(test, feature = "diagnostic"))]
            {
                // SAFETY: as above; the pool is this idle thread's.
                let pooled = unsafe { &*exec.worker.get() }
                    .idle_stacks
                    .iter()
                    .any(|stack| stack.context() as usize == released);
                #[cfg(test)]
                if let Some(hook) = execs
                    .dispatch
                    .get()
                    .and_then(|dispatch| dispatch.after_compact.get())
                {
                    hook(task.slot, released, pooled);
                }
                #[cfg(feature = "diagnostic")]
                // SAFETY: the idle worker owns this saved image exclusively.
                crate::compact_trace::left(
                    task.slot,
                    unsafe { (*ctx).compact.as_ref() }.map_or(0, |compact| compact.state()),
                    pooled,
                );
            }
            #[cfg(not(any(test, feature = "diagnostic")))]
            let _ = released;
            let image = running.value_mut();
            image.ctx = ctx;
            image.sp = std::ptr::null_mut();
            let left = if execs.migrate {
                (exec as *const Exec).addr()
            } else {
                0
            };
            let queued = frame.placement.leave(running, left, || unsafe {
                crate::tasks::after_save(run, task, false)
            });
            if queued.is_none() {
                unsafe { crate::tasks::saved_idle(run) };
            }
            if queued.is_some() {
                let owner = route(execs, &frame, word_owner(frame.placement.owner()));
                frame.placement.set_owner(owner_word(owner));
            }
            let _activity = execs.activity.lock().unwrap();
            exec.busy.store(false, Ordering::SeqCst);
            if let Some(queued) = queued {
                publish(
                    execs,
                    Saved {
                        queued,
                        frame,
                        lease,
                        chained: false,
                    },
                    false,
                    true,
                );
            }
        }
        Leave::Suspend { .. } | Leave::Park => {
            let image = running.value_mut();
            image.sp = unsafe { (*image.ctx).task_sp };
            let task = image.task;
            #[cfg(feature = "diagnostic")]
            crate::compact_trace::held(task.slot);
            let yielded = match leaving {
                Leave::Suspend { owner, yielded } => {
                    frame.placement.set_owner(owner_word(owner));
                    unsafe { crate::tasks::after_save(run, task, false) };
                    yielded
                }
                _ => {
                    // The row parks before its save can take a wake.
                    if let Some(owner) = unsafe { crate::tasks::after_park(run, task, image.ctx) } {
                        frame.placement.set_owner(owner_word(owner));
                    }
                    false
                }
            };
            #[cfg(test)]
            if let Some(hook) = execs
                .dispatch
                .get()
                .and_then(|dispatch| dispatch.before_idle.get())
            {
                hook(task.slot);
            }
            let queued = running.save(yielded);
            if queued.is_none() {
                unsafe { crate::tasks::saved_idle(run) };
            }
            let _activity = execs.activity.lock().unwrap();
            exec.busy.store(false, Ordering::SeqCst);
            if let Some(queued) = queued {
                publish(
                    execs,
                    Saved {
                        queued,
                        frame,
                        lease,
                        chained: false,
                    },
                    yielded,
                    true,
                );
            }
        }
    }
}

pub(super) fn next(execs: &Execs, exec: &Exec) -> Option<Saved> {
    if exec.kind == Kind::Home && !home_computes(execs) {
        // The idle owner releases its old width-one queue after growth.
        // Resize cannot touch this cell from another OS thread.
        retire(execs, exec);
        return None;
    }
    if !matches!(exec.kind, Kind::Compute(_) | Kind::Home) {
        return None;
    }
    let dispatch = execs.dispatch.get()?;
    let mut local = exec.ready.take().or_else(|| {
        let mut local = dispatch.locals.lock().unwrap().pop()?;
        local.prepare();
        Some(local)
    })?;
    let mut saved = if dispatch.queued.load(Ordering::SeqCst) != 0 {
        local.next(&dispatch.ready).map(chain)
    } else {
        None
    };
    #[cfg(test)]
    if saved.is_none() {
        let hook = dispatch.after_search.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
    }
    if dispatch
        .searcher
        .compare_exchange(
            exec as *const Exec as usize,
            0,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
        && saved.is_none()
        && dispatch.queued.load(Ordering::SeqCst) != 0
    {
        // Publication either precedes this last check, or observes no
        // searcher and notifies a worker. Never park on the earlier miss.
        saved = local.next(&dispatch.ready).map(chain);
    }
    if saved.is_some() {
        assert_ne!(dispatch.queued.fetch_sub(1, Ordering::SeqCst), 0);
    }
    exec.ready.set(Some(local));
    if saved.is_some() {
        // An injection/steal can leave more work behind while the selected
        // segment never yields again. Mark this claimant before choosing a
        // sleeper, so a burst of notifications cannot keep waking it instead
        // of a worker that can start the remaining work. Tokio similarly
        // notifies after searching (worker.rs:647-651,1207-1226 at ce685ee6).
        exec.busy.store(true, Ordering::SeqCst);
        wake_ready(execs);
    }
    saved
}

fn chain((mut saved, source): (Saved, Source)) -> Saved {
    saved.chained = source == Source::Lifo;
    saved
}

/// Spend one completed immediate operation from the calling worker's chain
/// budget; false when the chain must yield its worker. Tokio's LIFO tasks
/// inherit their parent's coop budget (`worker.rs:680-714`): a handoff through
/// the LIFO slot continues this budget, so an immediate chain cannot renew
/// its quantum on every save. Any other dispatch starts a fresh budget.
/// # Safety
/// The running task's context, on the exec that runs it.
pub(crate) unsafe fn proceed(ctx: *mut TaskCtx) -> bool {
    // SAFETY: only this exec's own thread touches its chain cell.
    let exec = unsafe { &*current(execs(ctx)) };
    let mut budget = exec.chain.take();
    let more = budget.proceed();
    exec.chain.set(budget);
    more
}

/// Start the calling worker's chain budget afresh.
/// # Safety
/// As for proceed.
#[cfg(test)]
pub(crate) unsafe fn renew(ctx: *mut TaskCtx) {
    // SAFETY: only this exec's own thread touches its chain cell.
    unsafe { &*current(execs(ctx)) }
        .chain
        .set(Budget::default());
}

/// An addressed post can supersede a requested queue search. Hand its
/// reservation on before running that post, which might never yield.
pub(super) fn cancel_search(execs: &Execs, exec: &Exec) {
    if let Some(dispatch) = execs.dispatch.get()
        && dispatch
            .searcher
            .compare_exchange(
                exec as *const Exec as usize,
                0,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    {
        wake_ready(execs);
    }
}

pub(super) fn retire(execs: &Execs, exec: &Exec) {
    if let Some(mut local) = exec.ready.take() {
        let dispatch = execs.dispatch.get().unwrap();
        local.drain(&dispatch.ready);
        dispatch.locals.lock().unwrap().push(local);
        wake_ready(execs);
    }
}

pub(super) unsafe fn resume(run: *mut RunShared, exec: &Exec, saved: Saved) {
    let Saved {
        queued,
        frame,
        lease,
        chained,
    } = saved;
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    if exec.kind == Kind::Home
        && word_owner(frame.placement.owner()) == Owner::Any
        && !home_computes(execs)
    {
        publish(
            execs,
            Saved {
                queued,
                frame,
                lease,
                chained: false,
            },
            false,
            false,
        );
        return;
    }
    if frame.stop_before_start {
        // Immutable metadata was copied while preparing the first child under
        // the role. The idle claimant owns its actual Queued token, but must
        // neither start it nor inspect Image to observe this boundary.
        let control = unsafe { &(*run).control };
        let witness = serde_json::json!({
            "first_child": true,
            "before_token_start": true,
            "requested": control.width.load(Ordering::Acquire),
            "effective": control.live_width.load(Ordering::Acquire),
        });
        let path = unsafe { (*(*run).execs.get()).as_deref().unwrap() }
            .dispatch
            .get()
            .unwrap()
            .queued_stop_witness
            .as_ref()
            .unwrap();
        std::fs::write(path, witness.to_string()).expect("queued stop witness");
        control.request(crate::stop::Stop::Requested);
        // Queued::drop invalidates future notifications without source entry.
        drop(queued);
        drop(lease);
        wake(&execs.home);
        return;
    }
    #[cfg(all(feature = "tsan", target_arch = "x86_64"))]
    if exec.scoped_index().is_some() {
        // The mutant copies a queued image before start's exclusive claim/take.
        unsafe { double_claim::dispatch(&queued) };
    }
    let running = queued.start();
    let image = running.value();
    let (ctx, task, sp) = (image.ctx, image.task, image.sp);
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    exec.busy.store(true, Ordering::SeqCst);
    if unsafe { (*run).control.stopped().is_some() } {
        drop(running);
        drop(lease);
        exec.busy.store(false, Ordering::SeqCst);
        wake(&execs.home);
        return;
    }
    // The exclusive saved image includes callback depth: Home/Pool C entry
    // needs no permit, but a callback continuation must reacquire its own.
    let permit = super::resumes_into_hirsel(ctx, exec);
    if permit
        && execs
            .permits
            .acquire(exec, Some(unsafe { &(*run).control }), unsafe {
                (*run).worker_metrics.as_deref()
            })
            .is_err()
    {
        drop(running);
        drop(lease);
        exec.busy.store(false, Ordering::SeqCst);
        wake(&execs.home);
        return;
    }
    // SAFETY: token ownership precedes every access to this saved task. Role
    // acquisition protects moving metadata; the continuation acquires it again.
    unsafe {
        crate::tasks::role::enter(ctx);
        debug_assert_eq!((*ctx).task_id, task.slot);
        if !crate::tasks::resumed(run, task) || !crate::tasks::ordered::ahead::dispatch_ready(ctx) {
            // Promotion may precede the claim of the reservation's original
            // physical queue token. It must not claim an unselected logical
            // row; nor may a stale object token start a row parked on another
            // wait, nor an image whose key no longer names its slot's row. No task code or context switch has run: return the exact
            // image to Saved, without a semantic save callback. The role
            // excludes selection until the token's save is published.
            let returned = running.return_unstarted();
            crate::tasks::role::release(ctx);
            execs.permits.release(exec);
            exec.busy.store(false, Ordering::SeqCst);
            match returned {
                frame::Returned::Requeued(queued) => publish(
                    execs,
                    Saved {
                        queued,
                        frame,
                        lease,
                        chained: false,
                    },
                    false,
                    true,
                ),
                frame::Returned::Idle(quiet) => {
                    quiet.take(|| crate::tasks::saved_idle(run));
                    drop(lease);
                }
            }
            wake(&execs.home);
            return;
        }
        crate::tasks::record_dispatch(run, task, compute_index());
        if !chained {
            exec.chain.set(Budget::default());
        }
        frame.saved.store(false, Ordering::Release);
        frame.placement.activated();
        // A compact frame owns no stack: this worker lends it one.
        let sp = if sp.is_null() {
            let stack = (*exec.worker.get()).idle_stacks.take();
            #[cfg(feature = "diagnostic")]
            crate::compact_trace::activated(
                task.slot,
                (*ctx).compact.as_ref().map_or(0, |compact| compact.state()),
            );
            crate::tasks::compact::activate(run, task, stack)
        } else {
            #[cfg(feature = "diagnostic")]
            crate::compact_trace::resumed(task.slot);
            sp
        };
        TaskCtx::switch_in(ctx, exec.worker.get());
        crate::tasks::role::release(ctx);
        assert!(
            exec.active
                .replace(Some(Active {
                    running,
                    frame,
                    lease
                }))
                .is_none()
        );
        if !(*ctx).ahead_physical.is_null() {
            crate::tasks::ordered::ahead::native::run(ctx, exec, sp);
        } else {
            #[cfg(not(all(feature = "tsan", target_arch = "x86_64")))]
            fiber::switch(exec.idle.as_ptr(), sp);
            #[cfg(all(feature = "tsan", target_arch = "x86_64"))]
            {
                let mut sp = sp;
                loop {
                    fiber::switch(exec.idle.as_ptr(), sp);
                    if let Some(saved) = double_claim::ordinary_saved(ctx) {
                        sp = saved;
                        continue;
                    }
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_wake_waits_for_idle_save_and_the_root_drains_every_dispatch() {
        let mut run = crate::host::BodyRun::new(8, false);
        // SAFETY: this body uses only the explicit suspend/wake seams below;
        // it never borrows another task's stack or a home-owned service.
        unsafe { run.use_physical_dispatch() };
        let count = Arc::new(AtomicU32::new(0));
        let output = count.clone();
        let outcome = run.run(
            |_| {},
            move |ctx| {
                for _ in 0..1024 {
                    // These stand in for a logical frontier selecting this same
                    // segment while it is still executing. The saved-image CAS
                    // must remember, rather than publish, that notification.
                    unsafe {
                        crate::tasks::role::enter(ctx);
                        assert!(!is_saved((*ctx).run_ptr(), (*ctx).task_id));
                        schedule_saved(
                            (*ctx).run_ptr(),
                            crate::tasks::scheduler(ctx).key((*ctx).task_id),
                            Owner::Any,
                        );
                        assert!(!is_saved((*ctx).run_ptr(), (*ctx).task_id));
                        suspend_current(ctx, false);
                        assert!(!is_saved((*ctx).run_ptr(), (*ctx).task_id));
                        crate::tasks::role::release(ctx);
                    }
                    output.fetch_add(1, Relaxed);
                }
                crate::STATUS_OK
            },
        );
        assert!(matches!(outcome.ended, Ok(crate::STATUS_OK)), "{outcome:?}");
        assert_eq!(outcome.pending, 0);
        assert_eq!(count.load(Relaxed), 1024);
    }
}

#[cfg(test)]
mod dispatch_tests;

#[cfg(test)]
#[path = "free/modules.rs"]
mod audit_modules;

#[cfg(all(feature = "tsan", target_arch = "x86_64"))]
pub(crate) mod double_claim;
