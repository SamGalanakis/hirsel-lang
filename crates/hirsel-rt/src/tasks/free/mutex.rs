//! Live Free mutex effects run under a per-object lock outside the scheduler
//! role. FIFO permit state and payload movement share that lock. A notification
//! is installed after reacquiring the role, even if the waiter has not parked
//! yet; park's first saved-frame poll consumes it, avoiding a lost wake.
//!
//! Recorded mutex phases are a subsequent admission, not silently unrecorded.

use super::super::*;
use hirsel_free::mutex::Acquired;

pub(crate) fn live(ctx: *mut TaskCtx) -> bool {
    if !super::enabled(ctx) {
        return false;
    }
    assert!(
        !super::record::on(ctx),
        "native mutex recording not admitted"
    );
    true
}

fn notify(ctx: *mut TaskCtx, task: u32) {
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
    crate::roots::apply_pending(ctx);
    let me = scheduler(ctx).current;
    scheduler(ctx).task(me).retried = false;
    let mut queued = false;
    loop {
        let cell = scheduler(ctx).sync.mutex(word);
        unsafe { role::release(ctx) };
        let mut state = cell.lock();
        let result = state.permit.acquire(me, &mut queued);
        if result == Acquired::Payload {
            unsafe { cell.take(&mut state, payload) };
        }
        drop(state);
        drop(cell);
        unsafe { role::enter(ctx) };
        match result {
            Acquired::Pending => {
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
    crate::roots::apply_pending(ctx);
    let cell = scheduler(ctx).sync.mutex(word);
    unsafe { role::release(ctx) };
    let mut state = cell.lock();
    let next = unsafe { cell.put(&mut state, payload) };
    drop(state);
    drop(cell);
    unsafe { role::enter(ctx) };
    if let Some(next) = next {
        notify(ctx, next);
    }
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// A failed mutex body holds the role and owns the empty cell.
pub(crate) unsafe fn kill(ctx: *mut TaskCtx, word: u64, chain: Vec<Chained>) -> Stopped<()> {
    crate::roots::apply_pending(ctx);
    // Avoid stop-aware scheduler() while this frame owns a failure chain and
    // the detached wake list. The caller already owns the role.
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let waiters = s.sync.kill_waiters(word, chain);
    for task in waiters {
        s.task(task).retried = true;
        s.wake(task);
    }
    unsafe { try_yield(&mut *ctx) }
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

    fn layout() -> crate::Layout {
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
                            kill(ctx, word, Vec::new()).unwrap();
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
