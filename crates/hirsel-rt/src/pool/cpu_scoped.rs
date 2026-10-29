//! All-or-nothing scoped reservations under the existing process permit owner.
//! Like Tokio's batch_semaphore acquire, a reservation owns every requested
//! credit before handing any to workers. Unlike its wait list, CPU admission
//! refuses immediately and leaves existing waiters their capacity.
use super::{Exec, Execs, Kind, Owner, Permits, Post, RunPtr, RunShared, TaskCtx, Worker};
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

pub(crate) mod ledger;
mod placement;
pub(super) use placement::State as Placement;
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
        if state.waiting != 0 {
            self.permits.freed.notify_all();
        }
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
    cpu: Option<usize>,
    placement: Option<Arc<Placement>>,
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
        if state.waiting != 0 {
            self.freed.notify_all();
        }
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
            cpu: None,
            placement: None,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self.permits.state.lock().unwrap();
        state.held -= self.unassigned;
        if state.waiting != 0 {
            self.permits.freed.notify_all();
        }
    }
}

impl Assigned {
    pub(super) unsafe fn pin(&mut self, run: *mut RunShared) -> Result<(), Refusal> {
        if let Some(cpu) = self.cpu {
            let pin = Arc::clone(&self.exec.scoped_placement);
            pin.pin(cpu)?;
            if let Some(probe) = unsafe { (*run).cpu_probe.as_ref() } {
                probe.placement(self.exec.scoped_index().unwrap(), pin.tid(), cpu);
            }
            self.placement = Some(pin);
        }
        Ok(())
    }
}

impl Drop for Assigned {
    fn drop(&mut self) {
        // The worker has exited compiled code. Its idle transitions did not
        // return this credit to the process, even after capacity contracted.
        if let Some(placement) = self.placement.take() {
            placement.restore();
        }
        let mut state = self.permits.state.lock().unwrap();
        assert!(self.exec.reserved.swap(false, Ordering::AcqRel));
        self.exec.held.store(false, Ordering::Release);
        if !matches!(self.exec.kind, Kind::Scoped(_)) {
            self.exec.scoped_slot.store(u32::MAX, Ordering::Release);
        }
        state.held -= 1;
        if self.exec.lost.swap(false, Ordering::AcqRel) {
            state.lost -= 1;
        }
        if state.waiting != 0 {
            self.permits.freed.notify_all();
        }
    }
}

/// Native handles belong to the run, never to a fiber a stop can abandon.
/// Scoped frames use addressed Compute owners and the existing dispatcher.
/// These workers cannot claim ordinary work from its shared ready queue.
pub(super) struct Workers {
    execs: Vec<Arc<Exec>>,
    threads: Vec<std::thread::JoinHandle<()>>,
    pending: Vec<Option<Assigned>>,
    calling: Option<Assigned>,
    cursor: usize,
    caller: Option<Owner>,
    controllers: Option<Controllers>,
    _topology: Option<Arc<ledger::Topology>>,
    contraction: u64,
    domain: u64,
    caller_task: u32,
}

impl Workers {
    fn request_exit(&mut self) {
        // Dormant destinations own no native work. Return their credits even
        // when a stopped mapper keeps the run and its own guard alive.
        self.pending.clear();
        for exec in self.execs.iter().skip(usize::from(self.calling.is_some())) {
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

/// Reserve every destination before preparation, but start a native thread
/// only when it has work. Go 1.25 runtime/proc.go:5144-5157 publishes runnable
/// work before wakep, which starts an idle processor at :3203-3242.
/// Hirsel keeps all W credits and backing reserved until the scoped drain.
/// # Safety
/// The caller holds the scheduler role for this live run, before publishing
/// any scoped frame. Worker zero runs preparation; mappers keep their owners.
#[cfg(test)]
pub(super) unsafe fn start(
    run: *mut RunShared,
    reservation: Reservation,
) -> Result<Vec<super::Owner>, Refusal> {
    unsafe { start_at(run, reservation, None, None) }
}

/// One claim-start record, shared by all acknowledgments. Workers retain it
/// through notification, so reaching zero cannot free a live parking word.
#[derive(Default)]
pub(super) struct Startup {
    pending: AtomicU32,
    ready: crate::parking::Event,
}
impl Startup {
    pub(super) fn acknowledge(&self) {
        self.pending.fetch_sub(1, Ordering::Release);
        self.ready.notify();
    }
    fn wait(&self) {
        loop {
            let ticket = self.ready.ticket();
            if self.pending.load(Ordering::Acquire) == 0 {
                return;
            }
            self.ready.wait(ticket);
        }
    }
}

impl Workers {
    fn start_pending(
        &mut self,
        run: *mut RunShared,
        indices: impl IntoIterator<Item = usize> + Clone,
        fail_at: Option<u32>,
    ) -> Result<(), Refusal> {
        let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
        let ready = Arc::new(Startup::default());
        // Prepare independent heaps before spawning. The native starts then
        // run back to back, with no role work or heap construction between.
        for index in indices.clone() {
            if self.pending[index].is_some() {
                let exec = &self.execs[index];
                assert!(exec.worker.get().is_null());
                exec.worker.set(unsafe { Worker::create_scoped(run) });
            }
        }
        for index in indices {
            let Some(assigned) = self.pending[index].take() else {
                continue;
            };
            let exec = &self.execs[index];
            // The role owns attachment, and this dormant exec has no thread
            // that can reach its cell. Spawn publishes the initialized worker.
            if let Some(probe) = unsafe { (*run).cpu_probe.as_ref() } {
                probe.event(
                    "start",
                    self.domain,
                    Arc::as_ptr(exec) as usize,
                    index as u64,
                );
            }
            execs.holders.count.fetch_add(1, Ordering::AcqRel);
            ready.pending.fetch_add(1, Ordering::Relaxed);
            let thread = if fail_at == Some(index as u32) {
                drop(assigned);
                Err(std::io::Error::other("injected CPU worker startup failure"))
            } else {
                std::thread::Builder::new()
                    .name(format!("hirsel-cpu-{index}"))
                    .stack_size(2 * 1024 * 1024)
                    .spawn({
                        let (run, exec, holders, ready) = (
                            RunPtr(run),
                            Arc::clone(exec),
                            Arc::clone(&execs.holders),
                            ready.clone(),
                        );
                        move || {
                            super::worker_main_reserved(
                                run,
                                exec,
                                holders,
                                Some(assigned),
                                Some(ready),
                            )
                        }
                    })
            };
            let Ok(thread) = thread else {
                execs.holders.count.fetch_sub(1, Ordering::AcqRel);
                return Err(Refusal::WorkerStartFailed);
            };
            self.threads.push(thread);
        }
        ready.wait();
        Ok(())
    }
    fn start_jobs(
        &mut self,
        run: *mut RunShared,
        count: u64,
        fail_at: Option<u32>,
    ) -> Result<(), Refusal> {
        let width = self.execs.len();
        let cursor = self.cursor;
        self.start_pending(
            run,
            (0..count.min(width as u64) as usize).map(|i| (cursor + i) % width),
            fail_at,
        )
    }
}

static CLEANUP: super::ScopedCleanup = super::ScopedCleanup {
    request_exit,
    shutdown,
};

unsafe fn start_at(
    run: *mut RunShared,
    mut reservation: Reservation,
    fail_at: Option<u32>,
    calling: Option<Arc<Exec>>,
) -> Result<Vec<super::Owner>, Refusal> {
    let execs = unsafe {
        (*(*run).execs.get())
            .as_deref()
            .expect("scoped run executors")
    };
    execs.scoped_cleanup.get_or_init(|| &CLEANUP);
    let mut slot = execs.scoped.lock().unwrap();
    if slot.is_some() {
        return Err(Refusal::Busy);
    }
    let width = reservation.unassigned;
    let cpus = unsafe { (*run).cpu_grant.as_ref() }
        .map(|_| placement::cpus())
        .transpose()?;
    let topology = match unsafe { (*run).cpu_ledger.as_ref() } {
        Some(ledger) => Some(ledger.topology(width).ok_or(Refusal::ResourceLimit)?),
        None => None,
    };
    let mut workers = Workers {
        execs: Vec::with_capacity(width as usize),
        threads: Vec::with_capacity(width as usize),
        pending: Vec::with_capacity(width as usize),
        calling: None,
        cursor: 0,
        caller: None,
        controllers: None,
        _topology: topology.clone(),
        contraction: execs.permits.state.lock().unwrap().contraction,
        domain: unsafe { (*run).cpu_probe.as_ref().map_or(0, |p| p.domain()) },
        caller_task: u32::MAX,
    };
    for index in 0..width {
        let exec = if index == 0 {
            calling
                .clone()
                .unwrap_or_else(|| Exec::new(Kind::Scoped(0), std::ptr::null_mut(), false))
        } else {
            Exec::new(Kind::Scoped(index as u16), std::ptr::null_mut(), false)
        };
        let mut assigned = reservation.assign(Arc::clone(&exec)).unwrap();
        assigned.topology = topology.clone();
        assigned.cpu = cpus.as_ref().map(|cpus| cpus[index as usize % cpus.len()]);
        if index == 0 && calling.is_some() {
            exec.scoped_slot.store(0, Ordering::Release);
            unsafe { assigned.pin(run) }?;
            workers.calling = Some(assigned);
            workers.pending.push(None);
            if let Some(probe) = unsafe { (*run).cpu_probe.as_ref() } {
                probe.event("caller", workers.domain, Arc::as_ptr(&exec) as usize, 0);
            }
        } else {
            execs
                .holders
                .workers
                .lock()
                .unwrap()
                .push(Arc::downgrade(&exec));
            workers.pending.push(Some(assigned));
        }
        workers.execs.push(exec);
    }
    // Fault injection also exercises rollback of every eagerly started prefix.
    let count = if fail_at.is_some() { width as usize } else { 1 };
    if let Err(error) = workers.start_pending(run, 0..count, fail_at) {
        workers.join();
        for exec in workers
            .execs
            .iter()
            .skip(usize::from(workers.calling.is_some()))
        {
            let worker = exec.worker.replace(std::ptr::null_mut());
            if worker.is_null() {
                continue;
            }
            unsafe { Worker::retire(worker) };
            if let Some(probe) = unsafe { (*run).cpu_probe.as_ref() } {
                probe.event("retire", workers.domain, Arc::as_ptr(exec) as usize, 0);
            }
        }
        return Err(error);
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
    if let Some(workers) = &mut *execs.scoped.lock().unwrap() {
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
        workers
            .execs
            .iter()
            .skip(usize::from(workers.calling.is_some()))
            .filter(|exec| exec.busy())
            .count()
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
                .skip(usize::from(workers.calling.is_some()))
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
        for exec in workers
            .execs
            .iter()
            .skip(usize::from(workers.calling.is_some()))
        {
            assert!(!exec.busy());
            let worker = exec.worker.replace(std::ptr::null_mut());
            if worker.is_null() {
                continue;
            }
            unsafe { Worker::retire(worker) };
            if let Some(probe) = unsafe { (*run).cpu_probe.as_ref() } {
                probe.event("retire", workers.domain, Arc::as_ptr(exec) as usize, 0);
            }
        }
        if let Some(probe) = unsafe { (*run).cpu_probe.as_ref() } {
            probe.event("shutdown", workers.domain, 0, workers.execs.len() as u64);
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
            workers
                .execs
                .iter()
                .skip(usize::from(workers.calling.is_some()))
                .map(|exec| exec.worker.get())
                .filter(|worker| !worker.is_null())
                .collect()
        })
}

/// Like rayon-core 1.13 join/mod.rs:132-174, the caller runs the first job
/// and drains before returning. Saved frames keep the existing task identities.
/// Enter and leave use the ordinary saved-frame owner protocol.
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
    let address = super::current(execs);
    let calling = if address == Arc::as_ptr(&execs.home) {
        Arc::clone(&execs.home)
    } else {
        execs
            .holders
            .workers
            .lock()
            .unwrap()
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .find(|exec| Arc::as_ptr(exec) == address)
            .expect("the calling executor belongs to this run")
    };
    let mut controllers = Vec::new();
    let mut caller_is_controller = false;
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
                caller_is_controller |= Arc::ptr_eq(&calling, &exec);
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
        Ok((reservation, controllers)) => unsafe {
            start_at(
                run,
                reservation,
                None,
                (!caller_is_controller).then_some(calling),
            )
        }
        .map(|owners| (owners, controllers)),
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
    // Direct runtime fixtures can publish without the typed bounded-batch
    // action. Production has already started precisely these owners there.
    workers
        .start_pending(unsafe { (*ctx).run_ptr() }, [index], None)
        .unwrap_or_else(|_| crate::environmental("a scoped CPU worker could not start"));
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
    {
        let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
        super::acquire_permit(ctx);
    }
}

fn capacity_lost(ctx: *mut TaskCtx) -> bool {
    let execs = super::execs(ctx);
    let slot = execs.scoped.lock().unwrap();
    let Some(workers) = slot.as_ref() else {
        return false;
    };
    // Refresh before taking permit state: refresh also inspects process permits.
    let host_lost = unsafe { (*ctx).run_ref().capacity.as_ref() }
        .is_some_and(|capacity| !capacity.permits(workers.execs.len() as u32));
    let state = execs.permits.state.lock().unwrap();
    state.contraction != workers.contraction
        || state.limit < workers.execs.len() as u32
        || host_lost
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
    // Observations never enter the scheduler, settle a CPU prefix, or yield.
    // The independently checked worker certificate permits these two calls
    // on a private native frame. Its immutable job handle was installed at spawn.
    if matches!(action, 6 | 7) {
        let native =
            unsafe { (*ctx).ahead_physical }.cast::<crate::tasks::ordered::ahead::native::Native>();
        if !native.is_null() {
            if let Some(job) = unsafe { (*native).cpu_job.as_ref() } {
                if action == 6 {
                    job.enter(width, count);
                } else {
                    job.event("claim");
                }
            }
        } else if unsafe { (*ctx).run_ref().cpu_probe.is_some() } {
            let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
            if action == 6 {
                crate::tasks::cpu_job_entry(ctx, width, count);
            } else {
                crate::tasks::cpu_job_claim(ctx);
            }
        }
        unsafe { out.write(0) };
        return crate::STATUS_OK;
    }
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
                if width > grant.max_workers {
                    return 4;
                }
                crate::managed_cores::admit_capacity(ctx);
                if (*ctx).run_ref().capacity.as_ref().map_or_else(
                    || std::thread::available_parallelism().is_ok_and(|n| n.get() < width as usize),
                    |capacity| !capacity.permits(width),
                ) {
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
                } else if action == 2 {
                    let execs = super::execs(ctx);
                    let mut slot = execs.scoped.lock().unwrap();
                    match slot
                        .as_mut()
                        .unwrap()
                        .start_jobs((*ctx).run_ptr(), count, None)
                    {
                        Ok(()) => 0,
                        Err(Refusal::WorkerStartFailed) => 7,
                        Err(_) => unreachable!(),
                    }
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
            && let Some(worker) = unsafe { (*super::own_exec()).scoped_index() }
        {
            unsafe { probe::begin(ctx, workers.domain, worker) };
        }
    }
    if let Some(probe) = unsafe { (*ctx).run_ref().cpu_probe.as_ref() } {
        probe.control(
            crate::tasks::running_key(ctx).word(),
            width,
            action,
            count,
            value,
        );
    }
    #[cfg(all(feature = "tsan", target_arch = "x86_64"))]
    if action == 3 && value == 0 {
        unsafe { super::free::double_claim::claimed(ctx) };
    }
    unsafe { out.write(value) };
    crate::STATUS_OK
}

#[cfg(all(feature = "tsan", target_arch = "x86_64"))]
pub(super) unsafe fn tsan_job(ctx: *mut TaskCtx) -> bool {
    let execs = super::execs(ctx);
    let slot = execs.scoped.lock().unwrap();
    slot.as_ref()
        .is_some_and(|workers| workers.caller_task != unsafe { (*ctx).task_id })
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
    fn retained_calling_credit_cannot_restore_a_tid_after_its_native_executor_exits() {
        unsafe {
            let mut ctx = std::ptr::null_mut();
            assert_eq!(
                crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
                0
            );
            let run = (*ctx).run_ptr();
            let execs = super::super::init_execs(ctx);
            let permits = Arc::new(Permits::new(1));
            let exec = Exec::new(Kind::Scoped(0), std::ptr::null_mut(), false);
            let mut assigned = permits
                .try_reserve(1)
                .unwrap()
                .assign(exec.clone())
                .unwrap();
            assigned.cpu = Some(placement::cpus().unwrap()[0]);
            let retained = Arc::new(std::sync::Mutex::new(Some(assigned)));
            exec.post(Post::Exit, None);
            execs.holders.count.fetch_add(1, Ordering::AcqRel);
            let thread = std::thread::spawn({
                let run = RunPtr(run);
                let exec = exec.clone();
                let holders = execs.holders.clone();
                let retained = retained.clone();
                move || {
                    retained
                        .lock()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .pin(run.0)
                        .unwrap();
                    // The credit stays outside this native worker, as it does
                    // for a stopped run's original calling executor.
                    super::super::worker_main_reserved(run, exec, holders, None, None);
                }
            });
            thread.join().unwrap();
            assert!(exec.exited.load(Ordering::Acquire));
            assert_eq!(
                permits.available(),
                0,
                "the retained domain still owns its credit"
            );
            drop(retained.lock().unwrap().take());
            assert_eq!(permits.available(), 1);
            assert_eq!(execs.holders.count.load(Ordering::Acquire), 1);
            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
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
                    let owners = start_at(run, reservation, fail_at, None);
                    if fail_at.is_some() {
                        assert_eq!(owners, Err(Refusal::WorkerStartFailed));
                        assert!(workers(execs).is_empty());
                        assert_eq!(permits.available(), width);
                        assert_eq!(execs.holders.count.load(Ordering::Acquire), 1);
                        continue;
                    }
                    let owners = owners.unwrap();
                    assert_eq!(owners.len(), width as usize);
                    execs
                        .scoped
                        .lock()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .start_jobs(run, width.into(), None)
                        .unwrap();
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
            // A failure after preparation rolls back the started prefix and
            // every dormant credit before this run can admit another batch.
            let permits = Arc::new(Permits::new(4));
            let owners = start(run, permits.try_reserve(4).unwrap()).unwrap();
            let deferred =
                execs
                    .scoped
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .start_jobs(run, 4, Some(2));
            assert_eq!(deferred, Err(Refusal::WorkerStartFailed));
            drop(owners);
            finish(run);
            assert_eq!(permits.available(), 4);
            assert!(workers(execs).is_empty());
            assert_eq!(execs.holders.count.load(Ordering::Acquire), 1);
            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
        }
    }

    #[test]
    fn caller_runs_scoped_width_one_and_returns_its_credit_after_each_call() {
        #[cfg(feature = "tsan")]
        let switches = crate::fiber::TSAN_SWITCHES.load(Ordering::Relaxed);
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
                    assert_eq!(std::thread::current().id(), original);
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
        #[cfg(feature = "tsan")]
        assert!(crate::fiber::TSAN_SWITCHES.load(Ordering::Relaxed) > switches);
    }

    #[test]
    fn ordered_caller_migrates_on_its_saved_head_and_retires_each_control_once() {
        #[cfg(feature = "tsan")]
        let switches = crate::fiber::TSAN_SWITCHES.load(Ordering::Relaxed);
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
                    assert_eq!(std::thread::current().id(), original);
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
        #[cfg(feature = "tsan")]
        assert!(crate::fiber::TSAN_SWITCHES.load(Ordering::Relaxed) > switches);
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
                assert!(!matches!((*super::super::own_exec()).kind, Kind::Scoped(_)));
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
