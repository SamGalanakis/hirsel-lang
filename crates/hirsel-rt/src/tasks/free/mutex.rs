//! Live Free mutex effects run under a per-object lock outside the scheduler
//! role. FIFO permit state and payload movement share that lock. A notification
//! is installed after reacquiring the role, even if the waiter has not parked
//! yet; park's first saved-frame poll consumes it, avoiding a lost wake.
//! These are the role paths: resolution, waits, wakes and replay. An immediate
//! operation on a resolved lease runs in `objects` without the role.
//!
//! Recording publishes the action and its version inside that same lock.

use super::super::*;
use hirsel_free::mutex::Acquired;

#[cfg(test)]
mod close_tests;
#[cfg(test)]
mod death_tests;
#[cfg(test)]
mod record_tests;

pub(crate) fn live(ctx: *mut TaskCtx) -> bool {
    if !super::enabled(ctx) {
        return false;
    }
    true
}

pub(super) fn touched(
    ctx: *mut TaskCtx,
    phase: Phase,
    cell: &crate::sync::MutexCell,
    state: &mut crate::sync::MutexImage,
    replay: Option<&super::replay::Object>,
    action: impl FnOnce(&mut crate::sync::MutexImage) -> &'static str,
) -> Stopped<()> {
    super::record::track(ctx, cell.object.get());
    super::record::publish_with_replay(ctx, phase, replay.map(|object| object.token()), |writer| {
        let outcome = action(state);
        if let Some(writer) = writer {
            writer.touch(
                cell.object.get().expect("a recorded mutex identity"),
                state.permit.version(),
                0,
                outcome,
            );
        } else if let Some(replay) = replay {
            replay.touch(outcome)?;
        }
        Ok(())
    })?
}

/// # Safety
/// Creation's ABI carrier and descriptor contract; role is held.
pub(crate) unsafe fn create(
    ctx: *mut TaskCtx,
    out: *mut u64,
    payload: *mut u8,
    none: *const u8,
    descriptor: u32,
    wrapper: u32,
) -> Stopped<()> {
    // Allocation and prepared image capacity precede the op section. No
    // stop-aware scheduler accessor may abandon a Rust-owned prepared cell.
    let s = scheduler(ctx) as *mut Scheduler;
    let block = unsafe {
        crate::alloc::hirsel_rt_v2_alloc(ctx, wrapper, 8, 8, crate::alloc::DOMAIN_HEAP, 0)
    };
    let layout = unsafe { (*ctx).descriptor(descriptor) };
    let element = unsafe { crate::channels::Element::new(descriptor, layout, none) };
    let cell = crate::sync::Synchronizers::prepare_mutex(element, unsafe { &*layout });
    let reservation = unsafe { (*s).sync.reserve_mutex() };
    let word = reservation.word();
    super::record::begin(ctx);
    let object = super::record::object(ctx);
    let replay = super::replay::object(ctx, (crate::free::MUTEX, word));
    if let Some(object) = &object {
        cell.object.set(object.clone()).unwrap();
    }
    let publication = super::record::publish(ctx, Phase::Done, |writer| unsafe {
        if let Some(replay) = &replay {
            replay.create()?;
        }
        (*s).sync.initialize_mutex_cell(reservation, cell, payload);
        block.add(1).cast::<u64>().write(word);
        out.write(block as u64);
        if let Some(writer) = writer {
            writer.create(object.unwrap());
        }
        Ok(())
    });
    drop(replay);
    publication??;
    unsafe { try_yield(&mut *ctx) }
}

pub(super) fn notify(ctx: *mut TaskCtx, task: u32) {
    let s = scheduler(ctx);
    // Set this before testing Parked: a running queued waiter may still be
    // between releasing the object lock and registering its Retry wait.
    s.task(task).retried = true;
    s.wake(task);
}

/// # Safety
/// The role is held; payload is a writable None carrier and out writable.
pub(crate) unsafe fn take(
    ctx: *mut TaskCtx,
    word: u64,
    payload: *mut u8,
    out: *mut u32,
) -> Stopped<()> {
    super::record::begin(ctx);
    unsafe { super::replay::wait_object(ctx, (crate::free::MUTEX, word)) }?;
    let me = scheduler(ctx).current;
    scheduler(ctx).task(me).retried = false;
    let mut queued = false;
    let mut entered = false;
    loop {
        let replay = super::replay::object(ctx, (crate::free::MUTEX, word));
        let cell = scheduler(ctx).sync.mutex(word);
        super::objects::keep_mutex(ctx, word, &cell);
        unsafe { role::release(ctx) };
        let mut state = cell.lock();
        let mut result = Acquired::Pending;
        let publication = if state.permit.ready_for(me) {
            touched(
                ctx,
                Phase::Done,
                &cell,
                &mut state,
                replay.as_ref(),
                |state| {
                    result = state.permit.acquire(me, &mut queued);
                    if result == Acquired::Payload {
                        unsafe { cell.take(state, payload) };
                    }
                    "take"
                },
            )
        } else if !entered {
            state.permit.reserve_waiter();
            super::record::publish_with_replay(
                ctx,
                Phase::Entered,
                replay.as_ref().map(|object| object.token()),
                |_| {
                    result = state.permit.acquire(me, &mut queued);
                    assert_eq!(result, Acquired::Pending);
                },
            )
        } else {
            Ok(())
        };
        drop(state);
        drop(cell);
        drop(replay);
        unsafe { role::enter(ctx) };
        publication?;
        match result {
            Acquired::Pending => {
                entered = true;
                // Only primitives remain on this frame across the wait. The
                // returned stop unwinds helpers before the ABI entry raises it.
                unsafe { try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0) }?;
            }
            Acquired::Payload | Acquired::Dead => {
                unsafe { out.write(u32::from(result == Acquired::Dead)) };
                scheduler(ctx).task(me).retried = false;
                return unsafe { try_yield(&mut *ctx) };
            }
        }
    }
}

/// # Safety
/// The role is held; payload is a Some carrier held by this task.
pub(crate) unsafe fn put(ctx: *mut TaskCtx, word: u64, payload: *mut u8) -> Stopped<()> {
    super::record::begin(ctx);
    unsafe { super::replay::wait_object(ctx, (crate::free::MUTEX, word)) }?;
    let replay = super::replay::object(ctx, (crate::free::MUTEX, word));
    let cell = scheduler(ctx).sync.mutex(word);
    super::objects::keep_mutex(ctx, word, &cell);
    unsafe { role::release(ctx) };
    let mut state = cell.lock();
    let mut next = None;
    let publication = touched(
        ctx,
        Phase::Done,
        &cell,
        &mut state,
        replay.as_ref(),
        |state| {
            next = unsafe { cell.put(state, payload) };
            "put"
        },
    );
    drop(state);
    drop(cell);
    drop(replay);
    unsafe { role::enter(ctx) };
    publication?;
    if let Some(next) = next {
        notify(ctx, next);
    }
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// A failed mutex body holds the role and owns the empty cell.
pub(crate) unsafe fn kill(ctx: *mut TaskCtx, word: u64) -> Stopped<()> {
    super::record::begin(ctx);
    unsafe { super::replay::wait_object(ctx, (crate::free::MUTEX, word)) }?;
    let replay = super::replay::object(ctx, (crate::free::MUTEX, word));
    // Capture the chain only after the replay wait, then avoid stop-aware
    // accessors while this frame owns it or the detached wake list.
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let me = s.current;
    let chain = unsafe { (*ctx).failure.chain(&s.task(me).path) };
    let cell = s.sync.mutex(word);
    // The role also protects the failure-chain metadata through this phase.
    // Acquire the per-object lock before opening it; no syscall or user drop
    // runs in the action and no scheduler accessor observes a stop there.
    let mut state = cell.lock();
    let mut waiters = Vec::with_capacity(state.permit.waiting());
    touched(
        ctx,
        Phase::Done,
        &cell,
        &mut state,
        replay.as_ref(),
        |state| {
            state.permit.kill_into(&mut waiters);
            s.sync.set_dead(word, chain);
            "kill"
        },
    )?;
    drop(state);
    drop(cell);
    drop(replay);
    for task in waiters {
        s.task(task).retried = true;
        s.wake(task);
    }
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// As for close_mutex: this task owns the last wrapper and its role. Payload
/// destruction can yield, and must run after the versioned cell detaches.
pub(crate) unsafe fn close(
    ctx: *mut TaskCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> Stopped<u8> {
    let status = unsafe { close_inner(ctx, word, anchor, operation_site) }?;
    // No detached image or recorder owner survives a permanent cut save.
    super::replay::start(ctx);
    Ok(status)
}

/// # Safety
/// As for close; the caller checks the cut after temporary storage drops.
unsafe fn close_inner(
    ctx: *mut TaskCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> Stopped<u8> {
    super::record::begin(ctx);
    unsafe { super::replay::wait_object(ctx, (crate::free::MUTEX, word)) }?;
    let replay = super::replay::object(ctx, (crate::free::MUTEX, word));
    let s = scheduler(ctx) as *mut Scheduler;
    let cell = unsafe { (*s).sync.mutex(word) };
    let empty = crate::channels::Ring::new(cell.element.size, 1);
    let mut state = cell.lock();
    super::record::track(ctx, cell.object.get());
    let publication = super::record::publish_with_replay(
        ctx,
        Phase::Done,
        replay.as_ref().map(|object| object.token()),
        |writer| {
            let version = state.permit.observe();
            if let Some(writer) = writer {
                writer.touch(
                    cell.object.get().expect("a recorded mutex identity"),
                    version,
                    0,
                    "close mutex",
                );
            } else if let Some(replay) = &replay {
                replay.touch("close mutex")?;
            }
            Ok(unsafe { (*s).sync.close_locked(word, &cell, &mut state, empty) })
        },
    );
    drop(state);
    drop(cell);
    drop(replay);
    let Some((image, descriptor)) = publication??.into_left() else {
        return Ok(STATUS_OK);
    };
    let image = unsafe { (*ctx).detached_images.keep(image) };
    super::replay::start(ctx);
    let value = unsafe { (*ctx).detached_images.at(image, 0) };
    let mut continuation = std::mem::MaybeUninit::<crate::finalize::Cleanup>::uninit();
    let status = unsafe {
        crate::finalize::hirsel_rt_v2_finalize(
            ctx,
            descriptor,
            value,
            std::ptr::null(),
            continuation.as_mut_ptr(),
            anchor,
            operation_site,
        )
    };
    unsafe { (*ctx).detached_images.release(image) };
    Ok(status)
}

/// # Safety
/// The role is held and word names a dead mutex.
pub(crate) unsafe fn cause(ctx: *mut TaskCtx, word: u64) -> Stopped<()> {
    super::record::begin(ctx);
    unsafe { super::replay::wait_object(ctx, (crate::free::MUTEX, word)) }?;
    let replay = super::replay::object(ctx, (crate::free::MUTEX, word));
    let s = scheduler(ctx) as *mut Scheduler;
    let cell = unsafe { (*s).sync.mutex(word) };
    let chain = unsafe { (*s).sync.dead(word) }
        .expect("a dead mutex cause")
        .clone();
    let mut state = cell.lock();
    touched(
        ctx,
        Phase::Done,
        &cell,
        &mut state,
        replay.as_ref(),
        |state| {
            state.permit.observe();
            "cause"
        },
    )?;
    drop(state);
    drop(cell);
    drop(replay);
    unsafe { (*ctx).failure.append_chain(chain) };
    super::replay::start(ctx);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Run {
        word: u64,
        order: std::sync::Mutex<Vec<u32>>,
        finished: AtomicU32,
        kill: bool,
    }

    unsafe extern "C" fn child(ctx: *mut u8, data: *mut u8) -> ! {
        let ctx = ctx.cast::<TaskCtx>();
        let (run, id) = *unsafe { Box::from_raw(data.cast::<(Arc<Run>, u32)>()) };
        let mut payload = [0u64; 2];
        let mut status = 99;
        unsafe {
            let _role = role::Guard::new(ctx);
            take(ctx, run.word, payload.as_mut_ptr().cast(), &mut status).unwrap();
        }
        assert_eq!(status, u32::from(run.kill));
        if run.kill {
            assert_eq!(payload, [0, 0]);
        } else {
            assert_eq!(payload, [1, 7]);
            run.order.lock().unwrap().push(id);
            unsafe {
                let _role = role::Guard::new(ctx);
                put(ctx, run.word, payload.as_mut_ptr().cast()).unwrap();
            }
            assert_eq!(payload, [0, 0]);
        }
        run.finished.fetch_add(1, Ordering::Release);
        drop(run);
        unsafe {
            role::enter(ctx);
            crate::pool::free::finish_current(ctx);
        }
    }

    pub(super) fn layout() -> crate::Layout {
        crate::Layout {
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
        }
    }

    #[test]
    fn native_waiters_reacquire_in_fifo_order_and_death_wakes_them_all() {
        const CHILD: &str = "HIRSEL_F1_NATIVE_MUTEX";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tasks::free::mutex::tests::native_waiters_reacquire_in_fifo_order_and_death_wakes_them_all", "--nocapture"])
                .env(CHILD, "1")
                .output().unwrap();
            assert!(output.status.success(), "{output:?}");
            return;
        }
        crate::pool::set_compute_grant(4);
        for killed in [false, true] {
            let mut run = crate::host::BodyRun::new(4, false);
            run.free = true;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe { super::super::enable(ctx) },
                move |ctx| {
                    let mut initial = [1u64, 7];
                    let word = unsafe {
                        let _role = role::Guard::new(ctx);
                        scheduler(ctx).sync.create_mutex(
                            crate::channels::Element {
                                descriptor: 0,
                                size: 16,
                                inert: true,
                                none: vec![0; 16].into_boxed_slice(),
                            },
                            &layout(),
                            initial.as_mut_ptr().cast(),
                        )
                    };
                    let shared = Arc::new(Run {
                        word,
                        order: Default::default(),
                        finished: AtomicU32::new(0),
                        kill: killed,
                    });
                    let mut status = 99;
                    unsafe {
                        let _role = role::Guard::new(ctx);
                        take(ctx, word, initial.as_mut_ptr().cast(), &mut status).unwrap();
                    }
                    assert_eq!(status, 0);
                    for id in 0..7 {
                        let cell = unsafe {
                            let _role = role::Guard::new(ctx);
                            let stack = Stack::new();
                            let child_ctx = stack.context();
                            child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
                            let s = scheduler(ctx);
                            let task = s.tasks.len() as u32;
                            (*child_ctx).task_id = task;
                            (*child_ctx).task_sp = stack.prepare(
                                child,
                                child_ctx.cast(),
                                Box::into_raw(Box::new((shared.clone(), id))).cast(),
                            );
                            let mut row = Task::new(1, child_ctx, None, vec![id], None);
                            row.stack = Some(stack);
                            s.tasks.push(Some(row));
                            crate::pool::free::schedule_saved((*ctx).run_ptr(), task, Owner::Any);
                            scheduler(ctx).sync.mutex(word)
                        };
                        // Wait for queue registration, not stack save. A put
                        // or kill may therefore beat the task's logical park.
                        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                        while cell.lock().permit.waiting() != id as usize + 1 {
                            assert!(std::time::Instant::now() < until, "a waiter must register");
                            std::thread::yield_now();
                        }
                    }
                    unsafe {
                        let _role = role::Guard::new(ctx);
                        if killed {
                            kill(ctx, word).unwrap();
                        } else {
                            put(ctx, word, initial.as_mut_ptr().cast()).unwrap();
                        }
                    }
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    while shared.finished.load(Ordering::Acquire) != 7 {
                        assert!(
                            std::time::Instant::now() < until,
                            "all waiters must complete"
                        );
                        std::thread::yield_now();
                    }
                    if !killed {
                        assert_eq!(*shared.order.lock().unwrap(), (0..7).collect::<Vec<_>>());
                    }
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
        }
    }
}
