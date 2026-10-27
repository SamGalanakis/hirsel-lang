//! All-or-nothing scoped reservations under the existing process permit owner.
//! Like Tokio's batch_semaphore acquire, a reservation owns every requested
//! credit before handing any to workers. Unlike its wait list, CPU admission
//! refuses immediately and leaves existing waiters their capacity.
use super::{Exec, Execs, Kind, Owner, Permits, Post, RunPtr, RunShared, TaskCtx, Worker};
use std::sync::{Arc, atomic::Ordering};

pub(crate) mod ledger;
pub(crate) mod probe;

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Refusal {
    InvalidWidth,
    WidthUnavailable { requested: u32, available: u32 },
    Busy,
    WorkerStartFailed,
    NestedPool,
    ResourceLimit,
}

/// Credits for the run's ordinary executors while scoped workers own theirs.
/// An active credit becomes ordinary again on drain; an idle one is returned.
struct Controllers {
    permits: Arc<Permits>,
    execs: Vec<Arc<Exec>>,
}

impl Drop for Controllers {
    fn drop(&mut self) {
        if self.execs.is_empty() {
            return;
        }
        let mut state = self.permits.state.lock().unwrap();
        for exec in &self.execs {
            assert!(exec.reserved.swap(false, Ordering::AcqRel));
            if !exec.held.load(Ordering::Acquire) {
                state.held -= 1;
                if exec.lost.swap(false, Ordering::AcqRel) {
                    state.lost -= 1;
                }
            }
        }
        self.permits.freed.notify_all();
    }
}

/// Unassigned credits. Each assigned credit has an independent lifetime, so a
/// stopped worker can outlive both startup rollback and its caller.
pub(super) struct Reservation {
    permits: Arc<Permits>,
    unassigned: u32,
}

/// Retained on the worker's native stack through its final exit, outside the
/// Hirsel fibers that a stop can abandon. Never store it on a task fiber.
pub(super) struct Assigned {
    permits: Arc<Permits>,
    exec: Arc<Exec>,
    topology: Option<Arc<ledger::Topology>>,
}

impl Permits {
    #[cfg(test)]
    pub(super) fn try_reserve(self: &Arc<Self>, width: u32) -> Result<Reservation, Refusal> {
        self.reserve_with_controllers(width, Vec::new())
            .map(|(reservation, _)| reservation)
    }

    fn reserve_with_controllers(
        self: &Arc<Self>,
        width: u32,
        execs: Vec<Arc<Exec>>,
    ) -> Result<(Reservation, Controllers), Refusal> {
        if !matches!(width, 1 | 4 | 8 | 16) {
            return Err(Refusal::InvalidWidth);
        }
        let mut state = self.state.lock().unwrap();
        if execs
            .iter()
            .any(|exec| exec.reserved.load(Ordering::Acquire))
        {
            return Err(Refusal::Busy);
        }
        let idle = execs
            .iter()
            .filter(|exec| !exec.held.load(Ordering::Acquire))
            .count() as u32;
        let requested = width.checked_add(idle).ok_or(Refusal::ResourceLimit)?;
        let available = state
            .limit
            .saturating_sub(state.held)
            .saturating_sub(state.waiting);
        if requested > available {
            return Err(Refusal::WidthUnavailable {
                requested,
                available,
            });
        }
        state.held += requested;
        for exec in &execs {
            exec.reserved.store(true, Ordering::Release);
        }
        self.freed.notify_all();
        Ok((
            Reservation {
                permits: Arc::clone(self),
                unassigned: width,
            },
            Controllers {
                permits: Arc::clone(self),
                execs,
            },
        ))
    }
}

impl Reservation {
    /// Bind a credit before starting this fresh worker. A failed OS spawn
    /// drops the returned guard; a successful spawn moves it into the thread.
    pub(super) fn assign(&mut self, exec: Arc<Exec>) -> Option<Assigned> {
        if self.unassigned == 0 {
            return None;
        }
        assert!(!exec.held.load(Ordering::Acquire));
        assert!(!exec.reserved.swap(true, Ordering::AcqRel));
        self.unassigned -= 1;
        Some(Assigned {
            permits: Arc::clone(&self.permits),
            exec,
            topology: None,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self.permits.state.lock().unwrap();
        state.held -= self.unassigned;
        self.permits.freed.notify_all();
    }
}

impl Drop for Assigned {
    fn drop(&mut self) {
        // The worker has exited compiled code. Its idle transitions did not
        // return this credit to the process, even after capacity contracted.
        let mut state = self.permits.state.lock().unwrap();
        assert!(self.exec.reserved.swap(false, Ordering::AcqRel));
        self.exec.held.store(false, Ordering::Release);
        state.held -= 1;
        if self.exec.lost.swap(false, Ordering::AcqRel) {
            state.lost -= 1;
        }
        self.permits.freed.notify_all();
    }
}

/// Native handles belong to the run, never to a fiber a stop can abandon.
/// Scoped frames use addressed Compute owners and the existing dispatcher.
/// These workers cannot claim ordinary work from its shared ready queue.
pub(super) struct Workers {
    execs: Vec<Arc<Exec>>,
    threads: Vec<std::thread::JoinHandle<()>>,
    cursor: usize,
    caller: Option<Owner>,
    controllers: Option<Controllers>,
    _topology: Option<Arc<ledger::Topology>>,
    contraction: u64,
    domain: u64,
    caller_task: u32,
    limits: Option<hirsel_run_config::managed_cores::LinuxLimits>,
}

impl Workers {
    fn request_exit(&self) {
        for exec in &self.execs {
            exec.post(Post::Exit, None);
        }
    }

    fn join(&mut self) {
        self.request_exit();
        for thread in self.threads.drain(..) {
            thread
                .join()
                .unwrap_or_else(|_| crate::fault("a scoped CPU worker panicked"));
        }
    }
}

/// Start exactly the reserved width, acknowledging every worker before return.
/// As in Rayon's explicit pool startup, a failed prefix is joined before refusal.
/// # Safety
/// The caller holds the scheduler role for this live run, before publishing
/// any scoped frame. No user code runs on these workers until it is addressed.
pub(super) unsafe fn start(
    run: *mut RunShared,
    reservation: Reservation,
) -> Result<Vec<super::Owner>, Refusal> {
    unsafe { start_at(run, reservation, None) }
}

unsafe fn start_at(
    run: *mut RunShared,
    mut reservation: Reservation,
    fail_at: Option<u32>,
) -> Result<Vec<super::Owner>, Refusal> {
    let execs = unsafe {
        (*(*run).execs.get())
            .as_deref()
            .expect("scoped run executors")
    };
    let mut slot = execs.scoped.lock().unwrap();
    if slot.is_some() {
        return Err(Refusal::Busy);
    }
    let width = reservation.unassigned;
    let topology = match unsafe { (*run).cpu_ledger.as_ref() } {
        Some(ledger) => Some(ledger.topology(width).ok_or(Refusal::ResourceLimit)?),
        None => None,
    };
    let (ready, acknowledgments) = std::sync::mpsc::sync_channel(width as usize);
    let mut workers = Workers {
        execs: Vec::with_capacity(width as usize),
        threads: Vec::with_capacity(width as usize),
        // The caller remains on worker zero while it publishes children.
        cursor: usize::from(!unsafe { (*run).free_mode }),
        caller: None,
        controllers: None,
        _topology: topology.clone(),
        contraction: execs.permits.state.lock().unwrap().contraction,
        domain: unsafe { (*run).cpu_probe.as_ref().map_or(0, |p| p.domain()) },
        caller_task: u32::MAX,
        limits: hirsel_run_config::managed_cores::LinuxLimits::capture().ok(),
    };
    for index in 0..width {
        // Sequential attachment is role-owned, like grow(). No scoped task
        // has been published, so the new worker cannot touch this heap yet.
        let worker = unsafe { Worker::create_scoped(run) };
        let exec = Exec::new(Kind::Scoped(index as u16), worker, false);
        let mut assigned = reservation.assign(Arc::clone(&exec)).unwrap();
        assigned.topology = topology.clone();
        execs
            .holders
            .workers
            .lock()
            .unwrap()
            .push(Arc::downgrade(&exec));
        execs.holders.count.fetch_add(1, Ordering::AcqRel);
        let thread = if fail_at == Some(index) {
            drop(assigned);
            Err(std::io::Error::other("injected CPU worker startup failure"))
        } else {
            std::thread::Builder::new()
                .name(format!("hirsel-cpu-{index}"))
                .stack_size(2 * 1024 * 1024)
                .spawn({
                    let (run, exec, holders, ready) = (
                        RunPtr(run),
                        Arc::clone(&exec),
                        Arc::clone(&execs.holders),
                        ready.clone(),
                    );
                    move || {
                        super::worker_main_reserved(run, exec, holders, Some(assigned), Some(ready))
                    }
                })
        };
        let Ok(thread) = thread else {
            execs.holders.count.fetch_sub(1, Ordering::AcqRel);
            unsafe { Worker::retire(worker) };
            workers.join();
            for exec in &workers.execs {
                unsafe { Worker::retire(exec.worker.replace(std::ptr::null_mut())) };
            }
            return Err(Refusal::WorkerStartFailed);
        };
        workers.execs.push(exec);
        workers.threads.push(thread);
    }
    drop(ready);
    for _ in 0..width {
        acknowledgments
            .recv()
            .expect("started scoped worker acknowledgement");
    }
    let owners = workers
        .execs
        .iter()
        .map(|exec| super::Owner::Compute(Arc::as_ptr(exec).expose_provenance()))
        .collect();
    *slot = Some(workers);
    Ok(owners)
}

pub(super) fn request_exit(execs: &Execs) {
    if let Some(workers) = &*execs.scoped.lock().unwrap() {
        workers.request_exit();
    }
}

pub(super) fn shutdown(execs: &Execs) {
    if let Some(workers) = &mut *execs.scoped.lock().unwrap() {
        workers.join();
    }
}

pub(super) fn busy(execs: &Execs) -> usize {
    execs.scoped.lock().unwrap().as_ref().map_or(0, |workers| {
        workers.execs.iter().filter(|exec| exec.busy()).count()
    })
}

pub(super) fn workers(execs: &Execs) -> Vec<*mut Worker> {
    execs
        .scoped
        .lock()
        .unwrap()
        .as_ref()
        .map_or_else(Vec::new, |workers| {
            workers
                .execs
                .iter()
                .map(|exec| exec.worker.get())
                .filter(|worker| !worker.is_null())
                .collect()
        })
}

/// After all scoped frames are saved and their results transferred. Orphan
/// heaps remain in the run registry for remote returns and result destruction.
pub(super) unsafe fn finish(run: *mut RunShared) {
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    if let Some(mut workers) = execs.scoped.lock().unwrap().take() {
        workers.join();
        for exec in &workers.execs {
            assert!(!exec.busy());
            unsafe { Worker::retire(exec.worker.replace(std::ptr::null_mut())) };
        }
    }
}

pub(super) fn forget(execs: &mut Execs) -> Vec<*mut Worker> {
    execs
        .scoped
        .get_mut()
        .unwrap()
        .take()
        .map_or_else(Vec::new, |workers| {
            // Detached reclaim runs only after the last native worker exited.
            workers.execs.iter().map(|exec| exec.worker.get()).collect()
        })
}

/// Enter and leave migrate through the ordinary saved-frame owner protocol.
/// The caller gives back its process credit before requesting the full batch.
/// # Safety
/// A running physical-dispatch task holds the scheduler role. Its selected
/// policy supports the scoped operation; replay uses its serial path instead.
pub(crate) unsafe fn enter(ctx: *mut TaskCtx, width: u32) -> Result<(), Refusal> {
    if !matches!(width, 1 | 4 | 8 | 16) {
        return Err(Refusal::InvalidWidth);
    }
    if crate::tasks::cpu_scoped(ctx) {
        return Err(Refusal::NestedPool);
    }
    let run = unsafe { (*ctx).run_ptr() };
    let execs = super::execs(ctx);
    if execs.scoped.lock().unwrap().is_some() {
        return Err(Refusal::Busy);
    }
    let mut controllers = Vec::new();
    if crate::tasks::ordered::engine::on(ctx) {
        for owner in crate::tasks::cpu_control_owners(ctx) {
            let candidates = match owner {
                Owner::Any => {
                    let compute = execs.compute.lock().unwrap();
                    if compute.active.is_empty() {
                        vec![Arc::clone(&execs.home)]
                    } else {
                        compute.active.clone()
                    }
                }
                Owner::Home => vec![Arc::clone(&execs.home)],
                Owner::Pool(index) => {
                    let address = super::pool_exec(execs, index);
                    execs
                        .holders
                        .workers
                        .lock()
                        .unwrap()
                        .iter()
                        .filter_map(std::sync::Weak::upgrade)
                        .filter(|exec| Arc::as_ptr(exec) == address)
                        .collect()
                }
                Owner::Compute(address) => execs
                    .holders
                    .workers
                    .lock()
                    .unwrap()
                    .iter()
                    .filter_map(std::sync::Weak::upgrade)
                    .filter(|exec| Arc::as_ptr(exec).addr() == address)
                    .collect(),
            };
            if candidates.is_empty() {
                return Err(Refusal::ResourceLimit);
            }
            for exec in candidates {
                if !controllers.iter().any(|old| Arc::ptr_eq(old, &exec)) {
                    controllers.push(exec);
                }
            }
        }
    }
    unsafe { (*ctx).frozen = Vec::new() };
    super::release_permit(ctx);
    let reservation = execs.permits.reserve_with_controllers(width, controllers);
    let owners = match reservation {
        Ok((reservation, controllers)) => {
            unsafe { start(run, reservation) }.map(|owners| (owners, controllers))
        }
        Err(error) => Err(error),
    };
    let (owners, controllers) = match owners {
        Ok(admitted) => admitted,
        Err(error) => {
            {
                let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
                super::acquire_permit(ctx);
            }
            return Err(error);
        }
    };
    let first = owners[0];
    drop(owners);
    let old = crate::tasks::cpu_owner(ctx, first, true);
    {
        let mut slot = execs.scoped.lock().unwrap();
        let workers = slot.as_mut().unwrap();
        workers.controllers = Some(controllers);
        workers.caller = Some(old);
        workers.caller_task = unsafe { (*ctx).task_id };
    }
    unsafe { migrate(ctx) };
    Ok(())
}

unsafe fn migrate(ctx: *mut TaskCtx) {
    if crate::tasks::ordered::engine::on(ctx) {
        unsafe { crate::stop::check(ctx, crate::tasks::ordered::engine::migrate(ctx)) };
        return;
    }
    let run = unsafe { (*ctx).run_ptr() };
    let key = crate::tasks::running_key(ctx);
    let owner = crate::tasks::dispatch_owner(ctx);
    // Latch a self-wake while Running. Only the idle save publishes the frame.
    unsafe { super::free::schedule_saved(run, key, owner) };
    unsafe { super::free::suspend_current(ctx, false) };
}

/// Assign mapper children in input order. Addressed posts are the existing
/// dispatcher's per-worker transport; there is no second shared ready queue.
pub(crate) fn next_owner(ctx: *mut TaskCtx) -> Owner {
    let execs = super::execs(ctx);
    let mut slot = execs.scoped.lock().unwrap();
    let workers = slot.as_mut().expect("live CPU batch");
    let index = workers.cursor % workers.execs.len();
    workers.cursor += 1;
    Owner::Compute(Arc::as_ptr(&workers.execs[index]).expose_provenance())
}

/// The scoped reservation replaces the ordinary run width while it is live.
pub(crate) fn width(ctx: *mut TaskCtx) -> Option<u32> {
    if !crate::tasks::cpu_scoped(ctx) {
        return None;
    }
    let execs = unsafe { (*(*ctx).run_ref().execs.get()).as_deref() }?;
    execs
        .scoped
        .lock()
        .unwrap()
        .as_ref()
        .map(|workers| workers.execs.len() as u32)
}

/// # Safety
/// All mapper children and their cleanup completed. The caller is the entering
/// task, holding the role, and no scoped child can publish another frame.
pub(crate) unsafe fn leave(ctx: *mut TaskCtx) {
    let run = unsafe { (*ctx).run_ptr() };
    let execs = super::execs(ctx);
    let old = execs
        .scoped
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .caller
        .take()
        .expect("scoped caller");
    crate::tasks::cpu_owner(ctx, old, false);
    // The CPU caller's freeze scratch belongs to this invocation, including
    // capacity retained by the existing graph walker.
    unsafe { (*ctx).frozen = Vec::new() };
    // Exit posts run only after the caller's idle save publishes its image to
    // the original topology. Its old CPU worker then returns its reserved
    // credit, allowing the original worker to reacquire without a W+1 holder.
    request_exit(execs);
    unsafe { migrate(ctx) };
    unsafe { finish(run) };
}

fn capacity_lost(ctx: *mut TaskCtx) -> bool {
    let execs = super::execs(ctx);
    let slot = execs.scoped.lock().unwrap();
    let Some(workers) = slot.as_ref() else {
        return false;
    };
    let state = execs.permits.state.lock().unwrap();
    state.contraction != workers.contraction
        || state.limit < workers.execs.len() as u32
        || workers.limits.as_ref().is_some_and(|reader| {
            reader.read().map_or(true, |limits| {
                limits
                    .admission_affinity
                    .intersection(&limits.effective_cpuset)
                    .len()
                    < workers.execs.len() as u64
                    || limits
                        .quota
                        .is_some_and(|quota| quota.floor() < workers.execs.len() as u64)
            })
        })
}

/// Private control for the typed cpu.map_scoped body. Preparation and mapper
/// callbacks remain ordinary generated calls and structured task children.
/// # Safety
/// The live task, Tasks word and output satisfy the Hirsel ABI. Only the
/// compiler's typed CPU body issues the start, bounded-batch and leave actions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_cpu_map_scoped(
    ctx: *mut TaskCtx,
    out: *mut u32,
    _tasks: *const u64,
    width: u32,
    action: u32,
    count: u64,
) -> u8 {
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if action > 5 {
        crate::fault("invalid scoped CPU action");
    }
    let live = || unsafe {
        match action {
            0 | 4 => {
                if !matches!(width, 1 | 4 | 8 | 16) {
                    return 1;
                }
                let Some(grant) = (*ctx).run_ref().cpu_grant.as_ref() else {
                    return 2;
                };
                if crate::tasks::cpu_scoped(ctx) {
                    return 5;
                }
                if !(*ctx).run_ref().free_mode && action != 4 {
                    return 11;
                }
                if let Some(cause) = crate::tasks::canceled(ctx) {
                    return 16 + cause;
                }
                if width > grant.max_workers
                    || std::thread::available_parallelism().is_ok_and(|n| n.get() < width as usize)
                {
                    return 4;
                }
                match enter(ctx, width) {
                    Ok(()) => {
                        if let Some(cause) = crate::tasks::canceled(ctx) {
                            leave(ctx);
                            16 + cause
                        } else {
                            0
                        }
                    }
                    Err(Refusal::InvalidWidth) => 1,
                    Err(Refusal::WidthUnavailable { .. }) => 4,
                    Err(Refusal::Busy) => 3,
                    Err(Refusal::NestedPool) => 5,
                    Err(Refusal::WorkerStartFailed) => 7,
                    Err(Refusal::ResourceLimit) => 10,
                }
            }
            1 => {
                let lost = capacity_lost(ctx);
                leave(ctx);
                if let Some(cause) = crate::tasks::canceled(ctx) {
                    16 + cause
                } else if lost {
                    8
                } else {
                    0
                }
            }
            2 | 3 | 5 => {
                if (action == 2 && count > u64::from(width) * 2)
                    || (matches!(action, 3 | 5) && count >= u64::from(width) * 2)
                {
                    6
                } else if capacity_lost(ctx) {
                    8
                } else if let Some(cause) = crate::tasks::canceled(ctx) {
                    16 + cause
                } else {
                    0
                }
            }
            _ => unreachable!(),
        }
    };
    let result = if crate::tasks::ordered::engine::on(ctx) {
        crate::tasks::ordered::cpu::control(ctx, width, action, count, live)
    } else {
        crate::tasks::free::record::cpu_control(ctx, width, action, count, live)
    };
    let value = unsafe { crate::stop::check(ctx, result) };
    if action == 3 && value == 0 && unsafe { (*ctx).run_ref().cpu_probe.is_some() } {
        let execs = super::execs(ctx);
        let slot = execs.scoped.lock().unwrap();
        if let Some(workers) = slot.as_ref()
            && workers.caller_task != unsafe { (*ctx).task_id }
            && let Kind::Scoped(worker) = unsafe { (*super::own_exec()).kind }
        {
            unsafe { probe::begin(ctx, workers.domain, u32::from(worker)) };
        }
    }
    unsafe { out.write(value) };
    crate::STATUS_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::Kind;

    #[test]
    fn waiting_controller_adopts_its_reserved_credit_once() {
        let permits = Arc::new(Permits::new(1));
        let blocker = Exec::new(Kind::Compute(0), std::ptr::null_mut(), false);
        let controller = Exec::new(Kind::Compute(1), std::ptr::null_mut(), false);
        assert!(permits.acquire(&blocker, None, None).unwrap());
        let (acquired, received) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let worker = std::thread::spawn({
            let permits = permits.clone();
            let controller = controller.clone();
            move || {
                assert!(permits.acquire(&controller, None, None).unwrap());
                acquired.send(()).unwrap();
                resume.recv().unwrap();
                permits.release(&controller);
            }
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while permits.state.lock().unwrap().waiting == 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        permits.state.lock().unwrap().limit = 4;
        let (reservation, controls) = permits
            .reserve_with_controllers(1, vec![controller])
            .unwrap();
        permits.release(&blocker);
        received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(permits.state.lock().unwrap().held, 2);
        drop(controls);
        drop(reservation);
        assert_eq!(permits.state.lock().unwrap().held, 1);
        release.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(permits.available(), 4);
    }

    #[test]
    fn ordinary_controller_credits_survive_idle_and_contraction_without_minting_capacity() {
        for active in [false, true] {
            let permits = Arc::new(Permits::new(5));
            let controller = Exec::new(Kind::Compute(0), std::ptr::null_mut(), false);
            assert!(permits.acquire(&controller, None, None).unwrap());
            let (reservation, controls) = permits
                .reserve_with_controllers(4, vec![controller.clone()])
                .unwrap();
            permits.release(&controller);
            assert_eq!(permits.available(), 0);
            permits.state.lock().unwrap().limit = 4;
            assert!(matches!(
                permits.try_reserve(1),
                Err(Refusal::WidthUnavailable { .. })
            ));
            if active {
                assert!(permits.acquire(&controller, None, None).unwrap());
            }
            drop(controls);
            drop(reservation);
            assert_eq!(permits.state.lock().unwrap().held, u32::from(active));
            if active {
                permits.release(&controller);
            }
            assert_eq!(permits.available(), 4);
            assert!(!controller.reserved.load(Ordering::Acquire));
        }
    }

    #[test]
    fn fresh_workers_acknowledge_and_every_failed_startup_prefix_returns_its_credits() {
        // Idle startup/rollback needs no physical CPU overlap. Physical-width
        // acceptance uses the exact-CPU harness, not this lifecycle test.
        unsafe {
            let mut ctx = std::ptr::null_mut();
            assert_eq!(
                crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
                0
            );
            let run = (*ctx).run_ptr();
            let execs = super::super::init_execs(ctx);
            for width in [1, 4, 8, 16] {
                let permits = Arc::new(Permits::new(width));
                for fail_at in (0..width).map(Some).chain([None]) {
                    let reservation = permits.try_reserve(width).unwrap();
                    let owners = start_at(run, reservation, fail_at);
                    if fail_at.is_some() {
                        assert_eq!(owners, Err(Refusal::WorkerStartFailed));
                        assert!(workers(execs).is_empty());
                        assert_eq!(permits.available(), width);
                        assert_eq!(execs.holders.count.load(Ordering::Acquire), 1);
                        continue;
                    }
                    let owners = owners.unwrap();
                    assert_eq!(owners.len(), width as usize);
                    assert_eq!(workers(execs).len(), width as usize);
                    assert_eq!(permits.available(), 0);
                    let other = Arc::new(Permits::new(1));
                    assert_eq!(
                        start(run, other.try_reserve(1).unwrap()),
                        Err(Refusal::Busy)
                    );
                    assert_eq!(other.available(), 1);
                    request_exit(execs);
                    shutdown(execs);
                    assert_eq!(permits.available(), width);
                    finish(run);
                    assert!(workers(execs).is_empty());
                }
            }
            assert_eq!(execs.holders.count.load(Ordering::Acquire), 1);
            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
        }
    }

    #[test]
    fn caller_yields_its_only_credit_and_migrates_back_after_fresh_width_one_calls() {
        let mut run = crate::host::BodyRun::new(1, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |_| {},
            |ctx| unsafe {
                let _role = crate::tasks::role::Guard::new(ctx);
                let original = std::thread::current().id();
                for _ in 0..2 {
                    enter(ctx, 1).unwrap();
                    assert!(matches!((*super::super::own_exec()).kind, Kind::Scoped(0)));
                    assert_ne!(std::thread::current().id(), original);
                    assert_eq!(enter(ctx, 1), Err(Refusal::NestedPool));
                    assert!(matches!(next_owner(ctx), Owner::Compute(_)));
                    leave(ctx);
                    assert_eq!(std::thread::current().id(), original);
                    assert!(!crate::tasks::cpu_scoped(ctx));
                    assert!(workers(super::super::execs(ctx)).is_empty());
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn ordered_caller_migrates_on_its_saved_head_and_retires_each_control_once() {
        let mut run = crate::host::BodyRun::new(1, false);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                crate::tasks::ordered::engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
            },
            |ctx| unsafe {
                let _role = crate::tasks::role::Guard::new(ctx);
                let original = std::thread::current().id();
                for _ in 0..2 {
                    let result = crate::tasks::ordered::cpu::control(ctx, 1, 0, 0, || {
                        enter(ctx, 1).unwrap();
                        0
                    });
                    assert_eq!(result, Ok(0));
                    assert!(matches!((*super::super::own_exec()).kind, Kind::Scoped(0)));
                    assert_ne!(std::thread::current().id(), original);
                    assert_eq!(enter(ctx, 1), Err(Refusal::NestedPool));
                    let result = crate::tasks::ordered::cpu::control(ctx, 1, 1, 0, || {
                        leave(ctx);
                        0
                    });
                    assert_eq!(result, Ok(0));
                    assert_eq!(std::thread::current().id(), original);
                    assert!(!crate::tasks::cpu_scoped(ctx));
                    assert!(workers(super::super::execs(ctx)).is_empty());
                }
                crate::stop::check(ctx, crate::tasks::ordered::engine::root_end(ctx, 0));
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn control_entry_refuses_before_startup_and_drains_a_bounded_batch() {
        let mut run = crate::host::BodyRun::new(1, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |_| {},
            |ctx| unsafe {
                let tasks = 0;
                let mut out = u32::MAX;
                assert_eq!(
                    hirsel_rt_v2_cpu_map_scoped(ctx, &mut out, &tasks, 0, 0, 0),
                    0
                );
                assert_eq!(out, 1);
                assert_eq!(
                    hirsel_rt_v2_cpu_map_scoped(ctx, &mut out, &tasks, 1, 0, 0),
                    0
                );
                assert_eq!(out, 2);
                (*ctx).run().cpu_grant = Some(hirsel_run_schema::CpuGrant {
                    provider: "cpu.scoped@1".into(),
                    max_workers: 1,
                    max_domains: 1,
                    max_physical_bytes: 1 << 28,
                    max_batch_bytes: 1 << 20,
                });
                let ledger = ledger::Ledger::new((*ctx).run_ref().cpu_grant.as_ref().unwrap());
                (*ctx)
                    .heap_ref()
                    .meter
                    .cpu
                    .set(ledger.clone())
                    .ok()
                    .unwrap();
                (*ctx).run().cpu_ledger = Some(ledger);
                assert_eq!(
                    hirsel_rt_v2_cpu_map_scoped(ctx, &mut out, &tasks, 4, 0, 0),
                    0
                );
                assert_eq!(out, 4);
                assert!(workers(super::super::execs(ctx)).is_empty());
                assert_eq!(
                    hirsel_rt_v2_cpu_map_scoped(ctx, &mut out, &tasks, 1, 0, 0),
                    0
                );
                assert_eq!(out, 0);
                assert!(matches!((*super::super::own_exec()).kind, Kind::Scoped(0)));
                assert_eq!(
                    hirsel_rt_v2_cpu_map_scoped(ctx, &mut out, &tasks, 1, 2, 3),
                    0
                );
                assert_eq!(out, 6);
                assert_eq!(
                    hirsel_rt_v2_cpu_map_scoped(ctx, &mut out, &tasks, 1, 2, 2),
                    0
                );
                assert_eq!(out, 0);
                assert_eq!(
                    hirsel_rt_v2_cpu_map_scoped(ctx, &mut out, &tasks, 1, 1, 0),
                    0
                );
                assert_eq!(out, 0);
                assert!(workers(super::super::execs(ctx)).is_empty());
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    fn exec() -> Arc<Exec> {
        Exec::new(Kind::Compute(0), std::ptr::null_mut(), false)
    }

    #[test]
    fn invalid_widths_and_insufficient_capacity_leave_no_partial_reservation() {
        for width in [0, 2, 3, 17, u32::MAX] {
            let permits = Arc::new(Permits::new(16));
            assert!(matches!(
                permits.try_reserve(width),
                Err(Refusal::InvalidWidth)
            ));
            assert_eq!(permits.available(), 16);
        }
        for width in [1, 4, 8, 16] {
            let permits = Arc::new(Permits::new(width - 1));
            assert!(
                matches!(permits.try_reserve(width), Err(Refusal::WidthUnavailable { requested, available }) if requested == width && available == width - 1)
            );
            assert_eq!(permits.state.lock().unwrap().held, 0);
            permits.state.lock().unwrap().limit = width;
            let reservation = permits.try_reserve(width).unwrap();
            assert_eq!(permits.available(), 0);
            drop(reservation);
            assert_eq!(permits.available(), width);
        }
    }

    #[test]
    fn parent_releases_before_reserving_and_can_resume_after_every_rollback_prefix() {
        for width in [1, 4, 8, 16] {
            for started in 0..=width {
                let permits = Arc::new(Permits::new(width));
                let parent = exec();
                assert!(permits.acquire(&parent, None, None).unwrap());
                assert!(matches!(
                    permits.try_reserve(width),
                    Err(Refusal::WidthUnavailable { .. })
                ));
                permits.release(&parent);
                let mut reservation = permits.try_reserve(width).unwrap();
                let workers: Vec<_> = (0..started)
                    .map(|_| reservation.assign(exec()).unwrap())
                    .collect();
                drop(reservation);
                assert_eq!(permits.state.lock().unwrap().held, started);
                drop(workers);
                assert!(permits.acquire(&parent, None, None).unwrap());
                permits.release(&parent);
                assert_eq!(permits.available(), width);
            }
        }
    }

    #[test]
    fn idle_worker_credit_stays_with_its_domain_until_actual_exit() {
        let permits = Arc::new(Permits::new(1));
        let mut reservation = permits.try_reserve(1).unwrap();
        let worker = exec();
        let assigned = reservation.assign(Arc::clone(&worker)).unwrap();
        assert!(reservation.assign(exec()).is_none());
        drop(reservation);
        for _ in 0..3 {
            assert!(permits.acquire(&worker, None, None).unwrap());
            assert!(!permits.acquire(&worker, None, None).unwrap());
            permits.release(&worker);
            permits.release(&worker);
            assert_eq!(permits.available(), 0);
            assert!(matches!(
                permits.try_reserve(1),
                Err(Refusal::WidthUnavailable { available: 0, .. })
            ));
        }
        drop(assigned);
        assert_eq!(permits.available(), 1);
    }

    #[test]
    fn contraction_and_lost_holder_do_not_mint_replacement_credits() {
        let permits = Arc::new(Permits::new(4));
        let mut reservation = permits.try_reserve(4).unwrap();
        let worker = exec();
        let assigned = reservation.assign(Arc::clone(&worker)).unwrap();
        permits.acquire(&worker, None, None).unwrap();
        {
            let mut state = permits.state.lock().unwrap();
            state.limit = 1;
            state.lost = 1;
            worker.lost.store(true, Ordering::Release);
        }
        drop(reservation);
        permits.release(&worker);
        assert_eq!(permits.available(), 0);
        assert_eq!(permits.state.lock().unwrap().lost, 1);
        assert!(matches!(
            permits.try_reserve(1),
            Err(Refusal::WidthUnavailable { .. })
        ));
        drop(assigned);
        assert_eq!(permits.state.lock().unwrap().lost, 0);
        assert_eq!(permits.available(), 1);
    }

    #[test]
    fn batch_reservations_do_not_barge_a_waiting_ordinary_worker() {
        let permits = Arc::new(Permits::new(4));
        permits.state.lock().unwrap().waiting = 1;
        assert!(matches!(
            permits.try_reserve(4),
            Err(Refusal::WidthUnavailable { available: 3, .. })
        ));
        assert_eq!(permits.state.lock().unwrap().held, 0);
        permits.state.lock().unwrap().waiting = 0;
        let reservation = permits.try_reserve(4).unwrap();
        drop(reservation);
        assert_eq!(permits.available(), 4);
    }

    #[test]
    fn competing_domains_cannot_each_reserve_the_same_four_credits() {
        let permits = Arc::new(Permits::new(4));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let outcomes = std::thread::scope(|scope| {
            let attempts: Vec<_> = (0..2)
                .map(|_| {
                    let permits = Arc::clone(&permits);
                    let barrier = Arc::clone(&barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        let reservation = permits.try_reserve(4);
                        // Keep the winning credits until the other admission
                        // finished; a release followed by success is legitimate.
                        barrier.wait();
                        reservation.is_ok()
                    })
                })
                .collect();
            attempts
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(outcomes.into_iter().filter(|ok| *ok).count(), 1);
        assert_eq!(permits.available(), 4);
    }
}

#[cfg(test)]
#[path = "cpu_scoped/race_tests.rs"]
mod race_tests;
