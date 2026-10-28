//! Private physical frames, disjoint from the logical task table.
//!
//! The parent's owned D command names a reservation; only retirement installs
//! its row. Tokio's exclusive Running token remains the physical owner. Like
//! Rayon join's unwind path (ee0a00bd, join/mod.rs:180-196), adoption waits for
//! that owner to save before transferring the storage it may still reference.

use super::super::registry::TaskKey;
use super::super::{Logical, Scheduler, Task, handle};
use crate::continuation::{Custody, PrivateSave, SavedFiber, SavedPc};
use crate::{TaskCtx, budget::stretch::Stretch};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) mod cpu_allocation;
pub(crate) mod cpu_arena;
pub mod cpu_handoff;
pub(crate) mod cpu_roots;
pub(crate) mod cpu_withdrawal;
pub(crate) mod cpu_worker;
pub(crate) mod host_return;
pub(crate) mod native;

pub(crate) const MAX_RESERVATIONS: usize = 8;

/// Owner-local counters are touched only by the running image. The committer
/// writes only the two atomic handoff flags until idle acknowledges the save.
pub(crate) struct Local {
    pub(crate) admitted: AtomicBool,
    pub(crate) save_requested: AtomicBool,
    grant: Cell<u64>,
    fuel: Cell<u64>,
    custody: Cell<Custody>,
    pub(crate) native: Option<Box<native::Native>>,
}

impl Local {
    pub(crate) fn new(admitted: bool) -> Self {
        Self {
            admitted: AtomicBool::new(admitted),
            save_requested: AtomicBool::new(false),
            grant: Cell::new(0),
            fuel: Cell::new(0),
            custody: Cell::new(Custody::Logical),
            native: None,
        }
    }

    /// Before publication: the body may now start privately.
    pub(crate) fn reserve(&self, generation: u32) {
        self.custody.set(Custody::Private { generation });
        self.admitted.store(false, Ordering::Release);
    }

    pub(crate) fn custody(&self) -> Custody {
        self.custody.get()
    }

    /// Adopt the acknowledged save once: the logical task resumes at exactly
    /// this PC with this private fuel, never at its entry.
    pub(crate) fn adopt(&mut self, generation: u32, pc: *mut u8, fuel: u64) -> PrivateSave {
        let mut custody = self.custody.get();
        let save = custody.adopt(PrivateSave {
            pc: SavedPc::Fiber(SavedFiber { sp: pc }),
            generation,
            fuel,
            descriptor: None,
        });
        self.custody.set(custody);
        // The private owner has acknowledged this save. A later CPU backing
        // withdrawal reuses the same flag after logical admission.
        self.save_requested.store(false, Ordering::Release);
        if let Some(native) = self.native.as_mut() {
            native.adopt();
        }
        save
    }

    /// Idle only, generation-matching Saved owner; no RC operation or user code.
    pub(crate) unsafe fn restore(&mut self, generation: u32) {
        let mut custody = self.custody.get();
        let first = custody.withdraw(generation);
        self.custody.set(custody);
        if first && let Some(native) = self.native.as_mut() {
            unsafe { native.restore_mask() };
        }
    }
}

pub(crate) struct Reserved {
    pub(crate) task: Box<Task>,
}

pub(crate) fn private(ctx: *mut TaskCtx) -> bool {
    unsafe {
        (*ctx)
            .ahead
            .as_ref()
            .is_some_and(|local| !local.admitted.load(Ordering::Acquire))
    }
}

/// The role is held. The task `key` names for physical dispatch: its private
/// reservation, or its published row, either only at `key`'s generation
/// (slotmap `SlotMap::contains_key`/`get`, basic.rs:363-368, 700-705). A
/// stale key resolves to nothing; no lookup reaches Scheduler::task.
pub(crate) fn physical(s: &mut Scheduler, key: TaskKey) -> Option<&mut Task> {
    if let Logical::Ordered(driver) = &mut s.logical
        && let Some(reserved) = driver.reserved.get_mut(&key.slot)
    {
        return (u64::from(reserved.task.generation) == key.generation)
            .then_some(&mut *reserved.task);
    }
    s.tasks
        .get_mut(key.slot as usize)?
        .as_mut()
        .filter(|row| u64::from(row.generation) == key.generation)
}

/// The private reservation at `index`, which the caller knows is owned.
fn reservation(s: &mut Scheduler, index: u32) -> &mut Task {
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!("ahead custody belongs to Ordered execution")
    };
    &mut driver
        .reserved
        .get_mut(&index)
        .expect("an owned reservation")
        .task
}

/// A queued private notification can outlive promotion. It is then merely a
/// physical wake, not permission to start a logical turn. The role is held;
/// no saved-image or notification transition may race this decision.
pub(crate) fn dispatch_ready(ctx: *mut TaskCtx) -> bool {
    unsafe {
        if (*ctx)
            .ahead
            .as_ref()
            .is_none_or(|local| !local.admitted.load(Ordering::Acquire))
        {
            return true;
        }
        let s = (*(*ctx).run_ref().tasks.get()).as_deref_mut().unwrap();
        let index = (*ctx).task_id;
        let id = handle(index, s.task(index).generation);
        let Logical::Ordered(driver) = &s.logical else {
            unreachable!("ahead custody belongs to Ordered execution")
        };
        driver.dispatch_owned(id)
    }
}

/// Quiescent teardown, before any run heap is reclaimed. Keep the reservation
/// table private to Ordered; the parent scheduler delegates this bounded walk.
pub(crate) fn restore_reserved_custody(scheduler: &mut Scheduler) {
    if let Logical::Ordered(driver) = &mut scheduler.logical {
        for reserved in driver.reserved.values_mut() {
            reserved.task.restore_ahead_custody();
        }
    }
}

/// Request acknowledgement, without touching the running owner's fuel word.
pub(crate) fn can_promote(s: &mut Scheduler, index: u32) -> bool {
    let row = reservation(s, index);
    if row.dispatch.as_ref().is_none_or(|frame| frame.is_saved()) {
        return true;
    }
    unsafe {
        (*row.ctx)
            .ahead
            .as_ref()
            .expect("a privately running reservation")
            .save_requested
            .store(true, Ordering::Release);
    }
    false
}

/// Adopt once, after physical save. No result, completion rank or child count
/// is published here; the ordinary spawn retirement performs those mutations.
pub(crate) fn take(s: &mut Scheduler, index: u32) -> Task {
    assert!(can_promote(s, index));
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let reserved = driver.reserved.remove(&index).expect("owned reservation");
    let id = handle(index, reserved.task.generation);
    unsafe {
        let pc = (*reserved.task.ctx).task_sp;
        let local = (*reserved.task.ctx).ahead.as_mut().unwrap();
        let fuel = local.fuel.replace(0);
        if fuel != 0 {
            driver
                .stretches
                .entry(id)
                .or_insert_with(|| Stretch::new(driver.budgets))
                .fuel_points(fuel);
        }
        local.adopt(reserved.task.generation, pc, fuel);
        crate::ahead_trace::adopted(index, reserved.task.generation, pc);
        local.admitted.store(true, Ordering::Release);
    }
    *reserved.task
}

/// Idle only, after the exclusive physical owner acknowledged its save.
pub(crate) fn after_save(
    s: &mut Scheduler,
    key: TaskKey,
    completed: bool,
    stopped: bool,
) -> Option<Vec<(TaskKey, crate::pool::Owner)>> {
    let index = key.slot;
    if !matches!(&s.logical, Logical::Ordered(d) if d.reserved.contains_key(&index)) {
        return None;
    }
    assert!(
        !completed || stopped,
        "an unpublished child cannot complete logically"
    );
    if let Some(frame) = &physical(s, key)
        .unwrap_or_else(|| crate::fault("a saved reservation's key is stale"))
        .dispatch
    {
        frame.mark_saved();
    }
    if stopped {
        return Some(Vec::new());
    }
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    driver.awaiting_save = false;
    let selected = driver.select();
    Some(
        selected
            .into_iter()
            .map(|id| {
                let key = TaskKey::from_word(id);
                let row = physical(s, key)
                    .unwrap_or_else(|| crate::fault("a selected task's key is stale"));
                (key, row.owner)
            })
            .collect(),
    )
}

pub(crate) fn fuel_in(ctx: *mut TaskCtx) {
    unsafe {
        let local = (*ctx).ahead.as_ref().unwrap();
        local.grant.set(crate::budget::FUEL_CHUNK);
        (*ctx).view.private();
        (*ctx).heap().set_fuel(crate::budget::FUEL_CHUNK as i64);
        (*ctx)
            .ordered_charge_trace
            .grant(crate::budget::FUEL_CHUNK as i64);
    }
}

pub(crate) fn fuel_out(ctx: *mut TaskCtx) {
    unsafe {
        let word = (*ctx).view.compiled.fuel;
        (*ctx).ordered_charge_trace.capture(word);
        let local = (*ctx).ahead.as_ref().unwrap();
        let consumed = local
            .grant
            .get()
            .saturating_sub(word.max(0) as u64)
            .saturating_add(u64::from(word < 0));
        local.fuel.set(local.fuel.get().saturating_add(consumed));
        local.grant.set(0);
        (*ctx).heap().set_fuel(0);
    }
}

/// Before an R/E/H boundary, allocation or unknown work, a reservation waits
/// without a logical yield. Its stack and context remain run-owned throughout.
pub(crate) unsafe fn barrier(ctx: *mut TaskCtx) {
    while private(ctx) {
        unsafe {
            super::super::role::enter(ctx);
            crate::pool::free::suspend_current(ctx, false);
        }
    }
}

/// A physical refill adds neither a cancellation observation nor a logical
/// point. The same compiled fuel points charge only when a budget is active.
pub(crate) unsafe fn refill(ctx: *mut TaskCtx) {
    fuel_out(ctx);
    let pause = unsafe {
        let local = (*ctx).ahead.as_ref().unwrap();
        local.save_requested.load(Ordering::Acquire)
            || local.fuel.get() > (*ctx).run_ref().fuel.remaining()
    };
    if pause {
        unsafe { barrier(ctx) };
    }
    if private(ctx) {
        fuel_in(ctx);
    } else {
        super::engine::fuel_in(ctx);
    }
}

/// Additive, immutable alternate entry metadata. The source closure and its
/// ordinary entry are unchanged; an unsupported record is a typed refusal.
#[repr(C)]
pub struct SpawnAheadInfo {
    pub version: u32,
    pub flags: u32,
    pub entry: Option<super::super::TaskCode>,
}
const _: () = {
    assert!(size_of::<SpawnAheadInfo>() == 16);
    assert!(align_of::<SpawnAheadInfo>() == 8);
    assert!(std::mem::offset_of!(SpawnAheadInfo, version) == 0);
    assert!(std::mem::offset_of!(SpawnAheadInfo, flags) == 4);
    assert!(std::mem::offset_of!(SpawnAheadInfo, entry) == 8);
};

/// # Safety
/// The ordinary TaskSpawn contract; non-null info is a live immutable record.
#[cfg_attr(not(feature = "operation-latency"), unsafe(no_mangle))]
pub unsafe extern "C" fn hirsel_rt_v2_task_spawn_ahead(
    ctx: *mut TaskCtx,
    caller_site: u64,
    operation_site: u64,
    destination: *mut u8,
    out: *mut u64,
    scope: *const u64,
    code: super::super::TaskCode,
    environment: *mut crate::Header,
    flags: u64,
    info: *const SpawnAheadInfo,
) -> u8 {
    let version = if info.is_null() {
        0
    } else {
        let version = unsafe { std::ptr::addr_of!((*info).version).read() };
        let flags = unsafe { std::ptr::addr_of!((*info).flags).read() };
        if !matches!(version, 1..=6) || flags != 0 {
            unsafe {
                crate::stop::raise(
                    ctx,
                    crate::stop::Stop::Refused(
                        "unsupported spawn-ahead metadata version or flags".into(),
                    ),
                )
            };
        }
        version
    };
    if super::engine::on(ctx) {
        let _role = unsafe { super::super::role::Guard::new(ctx) };
        // Validate the prefix in every mode, but inspect a certificate only
        // when this run can use it. W1 and strict execution start ordinary code.
        let enabled = version != 0
            && crate::pool::cpu_scoped::width(ctx).unwrap_or_else(|| unsafe {
                (*ctx).run_ref().control.width.load(Ordering::Acquire)
            }) > 1
            && super::engine::with_driver(ctx, |d| d.private_enabled);
        let (entry, certificate) = if !enabled {
            (None, None)
        } else if version == 1 {
            (unsafe { (*info).entry }, None)
        } else {
            #[cfg(all(target_arch = "x86_64", not(feature = "tsan")))]
            let certificate = unsafe {
                if version == 6 {
                    cpu_worker::validate_preclaimed(ctx, environment, info.cast())
                } else if version == 5 {
                    cpu_worker::validate(ctx, environment, info.cast())
                } else if version == 4 {
                    host_return::validate_resumable(ctx, environment, info.cast())
                } else if version == 3 {
                    host_return::validate(ctx, environment, info.cast())
                } else {
                    native::validate(ctx, environment, info.cast())
                }
            };
            #[cfg(all(target_arch = "x86_64", feature = "tsan"))]
            let certificate = unsafe {
                match version {
                    6 => cpu_worker::validate_preclaimed(ctx, environment, info.cast()),
                    5 => cpu_worker::validate(ctx, environment, info.cast()),
                    _ => None,
                }
            };
            #[cfg(not(target_arch = "x86_64"))]
            let certificate: Option<native::Certificate> = None;
            (certificate.as_ref().map(|c| c.entry), certificate)
        };
        return unsafe {
            crate::stop::check(
                ctx,
                super::lifecycle::spawn_certified(
                    ctx,
                    out,
                    scope,
                    code,
                    environment,
                    operation_site,
                    entry,
                    certificate,
                ),
            )
        };
    }
    unsafe {
        super::super::hirsel_rt_v2_task_spawn(
            ctx,
            caller_site,
            operation_site,
            destination,
            out,
            scope,
            code,
            environment,
            flags,
        )
    }
}

/// # Safety
/// The running ahead body owns its context, and no loan crosses this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ahead_barrier(ctx: *mut TaskCtx) {
    unsafe { barrier(ctx) };
}

#[cfg(test)]
mod qualification;

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::tasks::*;
    use std::sync::{Arc, atomic::AtomicU32};

    #[derive(Default)]
    struct Probe {
        entered: AtomicU32,
        private: AtomicU32,
        scoped: bool,
        caller: std::sync::Mutex<Option<std::thread::ThreadId>>,
    }

    unsafe extern "C" fn child(
        ctx: *mut TaskCtx,
        environment: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        let [address, ordinal] =
            unsafe { crate::alloc::payload(environment).cast::<[u64; 2]>().read() };
        let probe = unsafe { &*(address as *const Probe) };
        if probe.scoped && ordinal == 0 {
            assert_eq!(
                Some(std::thread::current().id()),
                *probe.caller.lock().unwrap()
            );
        } else if probe.scoped {
            assert!(
                std::thread::current()
                    .name()
                    .is_some_and(|name| name.starts_with("hirsel-cpu-"))
            );
        }
        if private(ctx) {
            // Observer only: this is not a language-visible shared operation.
            unsafe { role::enter(ctx) };
            let s = scheduler(ctx);
            assert!(s.tasks[unsafe { (*ctx).task_id } as usize].is_none());
            probe.private.fetch_add(1, Ordering::Release);
            unsafe { role::release(ctx) };
        }
        probe.entered.fetch_or(1 << ordinal, Ordering::AcqRel);
        if ordinal == 0 {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while probe.entered.load(Ordering::Acquire) & 2 == 0 {
                assert!(
                    std::time::Instant::now() < until,
                    "a second private segment never started"
                );
                std::thread::yield_now();
            }
        }
        crate::STATUS_OK
    }

    #[test]
    pub(crate) fn a_second_real_frame_runs_before_its_spawn_is_admitted_and_is_adopted_once() {
        private_frames_case(2, false);
        private_frames_case(4, false);
    }

    #[test]
    fn scoped_private_frames_keep_their_addressed_workers_through_adoption() {
        // Protocol coverage, like the other BodyRun dispatcher fixtures. No
        // claim about on-CPU overlap comes from this synthetic manager grant.
        let previous = crate::pool::compute_permits().0;
        crate::pool::set_compute_grant(4);
        private_frames_case(4, true);
        crate::pool::set_compute_grant(previous);
    }

    fn private_frames_case(width: u32, scoped: bool) {
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&layout as *const crate::Layout];
        {
            let probe = Arc::new(Probe {
                scoped,
                ..Default::default()
            });
            let seen = probe.clone();
            let mut run = crate::host::BodyRun::new(if scoped { 1 } else { width }, false);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    super::super::engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    )
                },
                move |ctx| unsafe {
                    if scoped {
                        *seen.caller.lock().unwrap() = Some(std::thread::current().id());
                        let _role = role::Guard::new(ctx);
                        crate::pool::cpu_scoped::enter(ctx, width).unwrap();
                    }
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        crate::STATUS_OK
                    );
                    for ordinal in 0..4 {
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            16,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env)
                            .cast::<[u64; 2]>()
                            .write([Arc::as_ptr(&seen) as u64, ordinal]);
                        let mut task = 0;
                        let _role = role::Guard::new(ctx);
                        assert_eq!(
                            hirsel_rt_v2_task_spawn_ahead(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut task,
                                &scope,
                                child,
                                env,
                                0,
                                &SpawnAheadInfo {
                                    version: 1,
                                    flags: 0,
                                    entry: Some(child),
                                },
                            ),
                            crate::STATUS_OK
                        );
                    }
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        crate::STATUS_OK
                    );
                    if scoped {
                        let _role = role::Guard::new(ctx);
                        crate::pool::cpu_scoped::leave(ctx);
                    }
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
            assert_eq!(probe.entered.load(Ordering::Acquire), 15);
            assert!(probe.private.load(Ordering::Acquire) > 0);
        }
    }

    struct FuelProbe {
        wide: bool,
        points: u64,
        hold_until_save: bool,
        second_computed: AtomicBool,
        private_refills: AtomicU32,
        remaining: std::sync::atomic::AtomicU64,
    }

    pub(crate) const POINTS: u64 = crate::budget::FUEL_CHUNK + 4;

    unsafe extern "C" fn fuel_child(
        ctx: *mut TaskCtx,
        environment: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        let [address, ordinal] =
            unsafe { crate::alloc::payload(environment).cast::<[u64; 2]>().read() };
        let probe = unsafe { &*(address as *const FuelProbe) };
        if ordinal == 0 && probe.wide {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !probe.second_computed.load(Ordering::Acquire) {
                assert!(
                    std::time::Instant::now() < until,
                    "private checkpoint tail did not run"
                );
                std::thread::yield_now();
            }
        }
        let points = probe.points;
        for _ in 0..points {
            unsafe {
                (*ctx).view.compiled.fuel -= 1;
                if (*ctx).view.compiled.fuel < 0 {
                    if private(ctx) {
                        probe.private_refills.fetch_add(1, Ordering::Release);
                    }
                    crate::budget::hirsel_rt_v2_fuel_refill(ctx);
                }
            }
        }
        if ordinal == 1 {
            probe.second_computed.store(true, Ordering::Release);
            // Force the legal interleaving in which this private owner is
            // still Running when its spawn becomes the parent's head op.
            // This reads only physical protocol state, never language state.
            if probe.hold_until_save && private(ctx) {
                let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while private(ctx) {
                    assert!(
                        std::time::Instant::now() < until,
                        "requested private save was never adopted"
                    );
                    unsafe {
                        (*ctx).view.compiled.fuel -= 1;
                        if (*ctx).view.compiled.fuel < 0 {
                            probe.private_refills.fetch_add(1, Ordering::Release);
                            crate::budget::hirsel_rt_v2_fuel_refill(ctx);
                        }
                    }
                }
            }
        }
        crate::STATUS_OK
    }

    #[test]
    fn private_checkpoint_fuel_is_adopted_once_and_matches_width_one() {
        for width in [1, 2, 4] {
            checkpoint_case(width, 4, false, POINTS, Boundary::Exit);
        }
    }

    #[test]
    fn head_work_waits_for_running_reservation_save_before_scope_exit() {
        checkpoint_case(1, 2, true, 1, Boundary::Exit);
        eprintln!("width-one two-child control completed successfully");
        checkpoint_case(2, 2, true, 1, Boundary::Exit);
    }

    #[test]
    fn head_work_waits_for_running_reservation_save_before_scope_exit_full_chunk() {
        checkpoint_case(2, 2, true, POINTS, Boundary::Exit);
    }

    #[derive(Clone, Copy)]
    pub(crate) enum Boundary {
        Exit,
        Submit,
        Refill,
        Allocation,
    }

    #[test]
    fn submit_waits_for_running_reservation_save_before_new_head_operation() {
        checkpoint_case(2, 2, true, 1, Boundary::Submit);
    }

    #[test]
    fn refill_waits_for_running_reservation_save_before_head_settlement() {
        checkpoint_case(2, 2, true, 1, Boundary::Refill);
    }

    #[test]
    fn allocation_waits_for_running_reservation_save_before_head_settlement() {
        checkpoint_case(2, 2, true, 1, Boundary::Allocation);
    }

    pub(crate) fn checkpoint_case(
        width: u32,
        jobs: u64,
        hold_until_save: bool,
        points: u64,
        boundary: Boundary,
    ) {
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&layout as *const crate::Layout];
        {
            let probe = Arc::new(FuelProbe {
                wide: width != 1,
                points,
                hold_until_save,
                second_computed: AtomicBool::new(false),
                private_refills: AtomicU32::new(0),
                remaining: std::sync::atomic::AtomicU64::new(u64::MAX),
            });
            let seen = probe.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            run.fuel = (!hold_until_save).then_some(1_000_000);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    if matches!(boundary, Boundary::Allocation) {
                        (*ctx).set_budgets(None, Some(1_000_000));
                    }
                    super::super::engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    );
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        crate::STATUS_OK
                    );
                    for ordinal in 0..jobs {
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            16,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env)
                            .cast::<[u64; 2]>()
                            .write([Arc::as_ptr(&seen) as u64, ordinal]);
                        let mut task = 0;
                        let _role = role::Guard::new(ctx);
                        assert_eq!(
                            super::super::lifecycle::spawn_with_entry(
                                ctx,
                                &mut task,
                                &scope,
                                fuel_child,
                                env,
                                0,
                                Some(fuel_child)
                            ),
                            Ok(crate::STATUS_OK)
                        );
                    }
                    if !matches!(boundary, Boundary::Exit) {
                        // Save without absorbing: a busy wait would retain the
                        // parent's permit while child zero waits for child one.
                        // Head-only dispatch resumes this exact native frame
                        // with its spawn log still owned by Frontier.
                        let _role = role::Guard::new(ctx);
                        let task = super::super::engine::current(ctx);
                        crate::tasks::ordered::engine::with_driver(ctx, |d| {
                            d.suspending(task, true)
                        });
                        crate::pool::free::suspend_current(ctx, false);
                        let driver = super::super::engine::driver(ctx);
                        driver.enter(task, crate::pool::compute_index().unwrap_or(0));
                        assert_eq!(driver.frontier.as_ref().unwrap().head(), Some(task));
                        assert!(!driver.frontier.as_ref().unwrap().head_ready(task));
                        assert!(!driver.reserved.is_empty());
                        assert!(seen.second_computed.load(Ordering::Acquire));
                    }
                    match boundary {
                        Boundary::Exit => {}
                        Boundary::Submit => {
                            let mut inner = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_scope_enter(
                                    ctx,
                                    std::ptr::null_mut(),
                                    &mut inner,
                                    &scope
                                ),
                                crate::STATUS_OK
                            );
                            assert_eq!(
                                hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &inner, 0),
                                crate::STATUS_OK
                            );
                        }
                        Boundary::Refill => {
                            let _role = role::Guard::new(ctx);
                            (*ctx).view.compiled.fuel -= 1;
                            super::super::engine::fuel_refill(ctx).unwrap();
                        }
                        Boundary::Allocation => {
                            let allocation = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                16,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            assert_eq!(
                                crate::release::hirsel_rt_v2_release(
                                    ctx,
                                    allocation,
                                    2,
                                    std::ptr::null(),
                                    0,
                                    0
                                ),
                                crate::STATUS_OK
                            );
                        }
                    }
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        crate::STATUS_OK
                    );
                    let _role = role::Guard::new(ctx);
                    super::super::engine::at_head(ctx).unwrap();
                    seen.remaining.store(
                        super::super::engine::driver(ctx)
                            .frontier
                            .as_ref()
                            .unwrap()
                            .settled
                            .fuel,
                        Ordering::Release,
                    );
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
            if !hold_until_save {
                assert_eq!(
                    probe.remaining.load(Ordering::Acquire),
                    1_000_000 - jobs * POINTS,
                    "width {width}"
                );
            }
            if width != 1 {
                assert!(probe.private_refills.load(Ordering::Acquire) > 0);
            }
        }
    }

    struct AdoptProbe {
        entered: [AtomicU32; 2],
        private_seen: AtomicBool,
        resumed: AtomicU32,
        infinite: bool,
        adopted: std::sync::Mutex<Option<(usize, u32, u64, Option<u32>, u64, usize, usize, u32)>>,
    }

    /// Ordinal 1 starts privately and spends fuel until its spawn's head
    /// requests its save. After adoption it must continue in this same frame.
    unsafe extern "C" fn adopt_child(
        ctx: *mut TaskCtx,
        environment: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        let [address, ordinal] =
            unsafe { crate::alloc::payload(environment).cast::<[u64; 2]>().read() };
        let probe = unsafe { &*(address as *const AdoptProbe) };
        probe.entered[ordinal as usize].fetch_add(1, Ordering::AcqRel);
        let marker = 0u64;
        let frame = std::hint::black_box(&raw const marker) as usize;
        if ordinal == 0 {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !probe.private_seen.load(Ordering::Acquire) {
                assert!(std::time::Instant::now() < until, "no private start");
                std::thread::yield_now();
            }
            if probe.infinite {
                loop {
                    unsafe {
                        (*ctx).view.compiled.fuel -= 1;
                        if (*ctx).view.compiled.fuel < 0 {
                            crate::budget::hirsel_rt_v2_fuel_refill(ctx);
                        }
                    }
                }
            }
            return crate::STATUS_OK;
        }
        if !private(ctx) {
            return crate::STATUS_OK;
        }
        probe.private_seen.store(true, Ordering::Release);
        let mut expected = 0;
        while private(ctx) {
            unsafe {
                (*ctx).view.compiled.fuel -= 1;
                if (*ctx).view.compiled.fuel < 0 {
                    let local = (*ctx).ahead.as_ref().unwrap();
                    let word = (*ctx).view.compiled.fuel;
                    expected = local.fuel.get().saturating_add(
                        local
                            .grant
                            .get()
                            .saturating_sub(word.max(0) as u64)
                            .saturating_add(u64::from(word < 0)),
                    );
                    crate::budget::hirsel_rt_v2_fuel_refill(ctx);
                }
            }
        }
        probe.resumed.fetch_add(1, Ordering::AcqRel);
        let Custody::Adopted(save) = (unsafe { (*ctx).ahead.as_ref().unwrap().custody() }) else {
            panic!("a promoted private body runs without adopted custody");
        };
        let generation = unsafe {
            role::enter(ctx);
            let generation = scheduler(ctx).task((*ctx).task_id).generation;
            role::release(ctx);
            generation
        };
        *probe.adopted.lock().unwrap() = Some((
            save.pc.sp().expect("a stackful private save") as usize,
            save.generation,
            save.fuel,
            save.descriptor,
            expected,
            frame,
            ctx as usize,
            generation,
        ));
        crate::STATUS_OK
    }

    fn adopt_case(width: u32, infinite: bool) -> (Arc<AdoptProbe>, crate::host::BodyOutcome) {
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&layout as *const crate::Layout];
        let probe = Arc::new(AdoptProbe {
            entered: Default::default(),
            private_seen: AtomicBool::new(false),
            resumed: AtomicU32::new(0),
            infinite,
            adopted: Default::default(),
        });
        let seen = probe.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        run.fuel = infinite.then_some(4 * crate::budget::FUEL_CHUNK);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                super::super::engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                )
            },
            move |ctx| unsafe {
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    crate::STATUS_OK
                );
                for ordinal in 0..2u64 {
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        16,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(env)
                        .cast::<[u64; 2]>()
                        .write([Arc::as_ptr(&seen) as u64, ordinal]);
                    let mut task = 0;
                    let _role = role::Guard::new(ctx);
                    assert_eq!(
                        super::super::lifecycle::spawn_with_entry(
                            ctx,
                            &mut task,
                            &scope,
                            adopt_child,
                            env,
                            0,
                            Some(adopt_child)
                        ),
                        Ok(crate::STATUS_OK)
                    );
                }
                hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0)
            },
        );
        (probe, outcome)
    }

    #[test]
    fn runtime50_private_save_adopt_no_restart() {
        // Promotion: the requested save is adopted once, at the exact saved PC
        // inside the child's own stack, with exactly the fuel spent privately.
        for width in [2, 4] {
            let (probe, outcome) = adopt_case(width, false);
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
            for entered in &probe.entered {
                assert_eq!(
                    entered.load(Ordering::Acquire),
                    1,
                    "an executed body restarted"
                );
            }
            assert_eq!(probe.resumed.load(Ordering::Acquire), 1);
            let (pc, saved_generation, fuel, descriptor, expected, frame, ctx, generation) = probe
                .adopted
                .lock()
                .unwrap()
                .take()
                .expect("adoption record");
            assert_eq!(fuel, expected, "private fuel adopted exactly");
            assert_ne!(fuel, 0);
            assert_eq!(saved_generation, generation);
            assert_eq!(descriptor, None);
            // The context lies at the top of the task's own stack: the saved PC
            // is below it, above the frame that resumed, and inside the mapping.
            assert!(
                pc < ctx && pc + crate::fiber::STACK_BYTES > ctx,
                "PC outside the task stack"
            );
            assert!(
                frame > pc,
                "the adopted PC is not inside the resumed frame's callees"
            );
        }
        // Stop while private: the reservation is withdrawn, never resumed or rerun.
        let (probe, outcome) = adopt_case(2, true);
        assert!(
            matches!(outcome.ended, Err(crate::stop::Stop::BudgetExceeded(_))),
            "{:?}",
            outcome.ended
        );
        assert!(probe.private_seen.load(Ordering::Acquire));
        assert_eq!(probe.entered[1].load(Ordering::Acquire), 1);
        assert_eq!(probe.resumed.load(Ordering::Acquire), 0);
        assert!(probe.adopted.lock().unwrap().is_none());
        // A host-return segment adopts its selected original descriptor once.
        super::host_return::tests::selected_descriptor_admission();
    }
}
