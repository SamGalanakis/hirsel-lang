//! Several compute workers beside the admission thread (design §6 Stops and worker lifetime,
//! lane W1): migration at every suspension, the run's stop word observed by every worker, a
//! stopped run that returns while a worker still computes, home foreign intervals, home
//! state destroyed on the admission thread, and fuel refills as stop points. Each root and
//! child is a Rust stand-in for compiled code, launched as `RunHandle::wait` launches a
//! program (`hirsel_rt::host::BodyRun`). Trace state is process-wide, since a task's thread
//! changes under it.

use crate::support::*;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use hirsel_rt::foreign::{
    FOREIGN_SCOPE_BYTES, FOREIGN_THREADS_HOME, hirsel_rt_v2_callback_context,
    hirsel_rt_v2_callback_failed, hirsel_rt_v2_foreign_enter, hirsel_rt_v2_foreign_leave,
};
use hirsel_rt::host::{BodyOutcome, BodyRun};
use hirsel_rt::io::{Adapter, AdapterError, Completion, Request};
use hirsel_rt::stop::Stop;
use hirsel_rt::tasks::{
    hirsel_rt_v2_task_join, hirsel_rt_v2_task_scope_enter, hirsel_rt_v2_task_scope_exit,
    hirsel_rt_v2_task_spawn, yield_now,
};
use hirsel_rt::{Config, Header, Layout, TaskCtx};
use hirsel_run_schema::BudgetKind;

/// Serialises this file's tests: their trace state is process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The threads that ran stand-in code, in order.
static THREADS: Mutex<Vec<ThreadId>> = Mutex::new(Vec::new());

fn ran_here() {
    THREADS.lock().unwrap().push(std::thread::current().id());
}

fn take_threads() -> Vec<ThreadId> {
    std::mem::take(&mut *THREADS.lock().unwrap())
}

/// A child's environment: one word, its ID.
fn environment() -> &'static Layout {
    cached_layout("workers-environment", 0, || leaf(8))
}

/// A configuration whose descriptor table holds `environment()` at index 2.
fn config() -> Config {
    let layouts: &'static [*const Layout] =
        Box::leak(vec![environment() as *const Layout].into_boxed_slice());
    Config {
        layouts: layouts.as_ptr(),
        layout_count: layouts.len() as u32,
        ..Config::new()
    }
}
const ENVIRONMENT: u32 = 2;
const TASKS: u64 = 7;
const NONE: [u64; 2] = [0, 0];

fn spawn(ctx: *mut TaskCtx, scope: u64, code: hirsel_rt::tasks::TaskCode, id: u64) -> u64 {
    let environment = unsafe {
        hirsel_rt::alloc::hirsel_rt_v2_alloc(ctx, ENVIRONMENT, 8, 8, hirsel_rt::DOMAIN_HEAP, 0)
    };
    write_u64(environment, 0, id);
    let mut task = 0;
    let status = unsafe {
        hirsel_rt_v2_task_spawn(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            &mut task,
            &scope,
            code,
            environment,
        )
    };
    assert_eq!(status, 0);
    task
}

fn enter(ctx: *mut TaskCtx) -> u64 {
    let mut scope = 0;
    let status =
        unsafe { hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &TASKS) };
    assert_eq!(status, 0);
    scope
}

fn join(ctx: *mut TaskCtx, scope: u64, mut task: u64) -> [u64; 2] {
    let mut slot = NONE;
    let status = unsafe {
        hirsel_rt_v2_task_join(ctx, 0, 0, &scope, &mut task, slot.as_mut_ptr().cast(), 16)
    };
    assert_eq!(status, 0);
    slot
}

fn exit(ctx: *mut TaskCtx, scope: u64) {
    assert_eq!(unsafe { hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0) }, 0);
}

/// Yields `id` times, recording its thread around each, then finishes with `Some(id)`.
unsafe extern "C" fn yielding(
    ctx: *mut TaskCtx,
    environment: *mut Header,
    _caller_site: u64,
    _operation_site: u64,
) -> u8 {
    let id = read_u64(environment, 0);
    for _ in 0..id {
        ran_here();
        unsafe { yield_now(&mut *ctx) };
    }
    ran_here();
    let mut slot = [1u64, id];
    unsafe {
        hirsel_rt::tasks::hirsel_rt_v2_task_finish(
            ctx,
            slot.as_mut_ptr().cast(),
            NONE.as_ptr().cast(),
            16,
        )
    }
}

/// One fuel point as compiled code charges it (`hirsel-llvm` `charge_fuel`).
fn charge(ctx: *mut TaskCtx) {
    unsafe {
        let word = ctx.cast::<u8>().add(hirsel_rt::CTX_FUEL_OFFSET).cast::<i64>();
        *word -= 1;
        if *word < 0 {
            hirsel_rt::budget::hirsel_rt_v2_fuel_refill(ctx);
        }
    }
}

/// Wait up to ten seconds for `condition`.
fn eventually(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// An adapter that records the thread its destructor runs on, holds an `Rc` (it is not
/// `Send`), and refuses every request as a replay mismatch.
struct Home {
    dropped: Arc<Mutex<Option<ThreadId>>>,
    _affine: std::rc::Rc<()>,
}
impl Home {
    fn new() -> (Self, Arc<Mutex<Option<ThreadId>>>) {
        let dropped = Arc::new(Mutex::new(None));
        (
            Home {
                dropped: Arc::clone(&dropped),
                _affine: std::rc::Rc::new(()),
            },
            dropped,
        )
    }
}
impl Adapter for Home {
    fn preflight(&self, _request: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        ran_here();
        let _ = request;
        Err(AdapterError::ReplayMismatch(0))
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        *self.dropped.lock().unwrap() = Some(std::thread::current().id());
    }
}

#[test]
fn migrated_tasks_run_only_on_compute_workers_and_return_their_results() {
    let _serial = serial();
    take_threads();
    let mut run = BodyRun::new(3, true);
    run.config = config();
    let metrics = run.worker_metrics();
    let ended = run.run(
        |_| {},
        |ctx| {
            ran_here();
            let scope = enter(ctx);
            let tasks: Vec<_> = (1..=4).map(|id| spawn(ctx, scope, yielding, id)).collect();
            for (task, id) in tasks.into_iter().zip(1..) {
                assert_eq!(join(ctx, scope, task), [1, id]);
            }
            exit(ctx, scope);
            ran_here();
            0
        },
    );
    assert_eq!(ended.ended, Ok(0));
    assert_eq!(ended.pending, 0);
    let threads = take_threads();
    let admission = std::thread::current().id();
    assert!(
        threads.iter().all(|&t| t != admission),
        "the admission thread ran stand-in code"
    );
    let distinct: std::collections::BTreeSet<_> = threads.iter().map(|t| format!("{t:?}")).collect();
    assert!(distinct.len() >= 2, "every suspension migrates: {distinct:?}");
    assert!(metrics.migrations() > 0);
    let values = metrics.values();
    for name in ["switches_per_op", "queue_ops_per_op", "spawn_ns", "migrations_per_s"] {
        assert!(
            values.iter().any(|(n, _)| *n == name),
            "{name} missing from {values:?}"
        );
    }
}

#[test]
fn width_one_runs_its_tasks_on_the_admission_thread() {
    let _serial = serial();
    take_threads();
    let mut run = BodyRun::new(1, false);
    run.config = config();
    let ended = run.run(
        |_| {},
        |ctx| {
            let scope = enter(ctx);
            let task = spawn(ctx, scope, yielding, 3);
            assert_eq!(join(ctx, scope, task), [1, 3]);
            exit(ctx, scope);
            0
        },
    );
    assert_eq!(ended.ended, Ok(0));
    let admission = std::thread::current().id();
    assert!(take_threads().iter().all(|&t| t == admission));
}

/// A child's request diverges from the replay on the home thread, which holds the binding:
/// the run ends Diverged with no worker left inside it.
#[test]
fn a_divergence_on_a_migrated_child_ends_the_run() {
    let _serial = serial();
    take_threads();
    unsafe extern "C" fn diverging(
        ctx: *mut TaskCtx,
        _environment: *mut Header,
        _caller_site: u64,
        _operation_site: u64,
    ) -> u8 {
        unsafe {
            yield_now(&mut *ctx);
            hirsel_rt::io::hirsel_rt_v2_io_close(ctx, 7, 0, 0);
        }
        unreachable!("a diverged request returned")
    }
    let (home, dropped) = Home::new();
    let mut run = BodyRun::new(3, true);
    run.config = config();
    run.adapter = Some(Box::new(home));
    let ended = run.run(
        |_| {},
        |ctx| {
            let scope = enter(ctx);
            let task = spawn(ctx, scope, diverging, 1);
            join(ctx, scope, task);
            unreachable!("the run diverged")
        },
    );
    assert!(
        matches!(ended.ended, Err(Stop::Diverged(_))),
        "{:?}",
        ended.ended
    );
    assert_eq!(ended.pending, 0);
    let admission = std::thread::current().id();
    // The request reached the adapter on the home thread, and the binding died there.
    assert_eq!(take_threads(), [admission]);
    assert_eq!(*dropped.lock().unwrap(), Some(admission));
}

/// Fuel spent inside cleanup (a finalizer a child's release runs, on a compute worker) ends
/// the run BudgetExceeded.
#[test]
fn budget_exhaustion_inside_cleanup_ends_the_run() {
    let _serial = serial();
    unsafe extern "C" fn spending(
        ctx: *mut TaskCtx,
        _value: *mut u8,
        _anchor: u64,
        _operation_site: u64,
    ) -> u8 {
        loop {
            charge(ctx);
        }
    }
    fn finalized() -> &'static Layout {
        cached_layout("workers-finalized", 0, || layout(8, 0, Some(spending), Vec::new()))
    }
    unsafe extern "C" fn cleaning(
        ctx: *mut TaskCtx,
        _environment: *mut Header,
        _caller_site: u64,
        _operation_site: u64,
    ) -> u8 {
        unsafe {
            yield_now(&mut *ctx);
            let block =
                hirsel_rt::alloc::hirsel_rt_v2_alloc(ctx, 3, 8, 8, hirsel_rt::DOMAIN_HEAP, 0);
            hirsel_rt::release::hirsel_rt_v2_release(ctx, block, 3, std::ptr::null(), 0, 0);
        }
        unreachable!("the cleanup's budget ran out")
    }
    let layouts: &'static [*const Layout] = Box::leak(
        vec![environment() as *const Layout, finalized() as *const Layout].into_boxed_slice(),
    );
    let mut run = BodyRun::new(2, true);
    run.config = Config {
        layouts: layouts.as_ptr(),
        layout_count: layouts.len() as u32,
        ..Config::new()
    };
    run.fuel = Some(3 * hirsel_rt::budget::FUEL_CHUNK + 11);
    let ended = run.run(
        |_| {},
        |ctx| {
            let scope = enter(ctx);
            let task = spawn(ctx, scope, cleaning, 1);
            join(ctx, scope, task);
            unreachable!("the run's budget ran out")
        },
    );
    assert_eq!(ended.ended, Err(Stop::BudgetExceeded(BudgetKind::Fuel)));
    assert_eq!(ended.pending, 0);
}

/// A host's stop while a migrated root runs a loop with no runtime entry: `wait` returns at
/// once, the binding dies on the admission thread then, the loop's worker still holds the
/// run, and once the loop ends the root finishes into its launch frame and the last thread
/// frees the run. The admission thread never ran the run's code.
#[test]
fn a_stop_during_a_pure_loop_returns_at_once_and_the_root_later_finishes_into_its_frame() {
    let _serial = serial();
    take_threads();
    static LOOPING: AtomicBool = AtomicBool::new(false);
    static RELEASE: AtomicBool = AtomicBool::new(false);
    static FINISHED: AtomicBool = AtomicBool::new(false);
    LOOPING.store(false, SeqCst);
    RELEASE.store(false, SeqCst);
    FINISHED.store(false, SeqCst);
    let (home, dropped) = Home::new();
    let mut run = BodyRun::new(2, true);
    run.config = config();
    run.adapter = Some(Box::new(home));
    let control = run.control();
    let stopper = std::thread::spawn({
        let control = control.clone();
        move || {
            eventually("the loop", || LOOPING.load(SeqCst));
            control.stop();
        }
    });
    let ended: BodyOutcome = run.run(
        |_| {},
        |ctx| {
            // The root migrates at each of these suspensions.
            for _ in 0..4 {
                ran_here();
                unsafe { yield_now(&mut *ctx) };
            }
            ran_here();
            LOOPING.store(true, SeqCst);
            while !RELEASE.load(SeqCst) {
                std::hint::spin_loop();
            }
            FINISHED.store(true, SeqCst);
            0
        },
    );
    stopper.join().unwrap();
    assert_eq!(ended.ended, Err(Stop::Requested));
    // The loop's worker still holds the run; the worker that handed the root to it may not
    // have gone idle yet.
    assert!(ended.pending >= 1, "the loop's worker still holds the run");
    let admission = std::thread::current().id();
    assert_eq!(*dropped.lock().unwrap(), Some(admission));
    let status = control.status();
    assert!(status.stop_requested && status.finished && !status.home_busy);
    assert!(status.reclamation_pending >= 1);
    assert!(!FINISHED.load(SeqCst));
    let threads = take_threads();
    assert!(threads.iter().all(|&t| t != admission));
    assert!(
        threads.windows(2).all(|w| w[0] != w[1]),
        "every suspension migrated: {threads:?}"
    );
    RELEASE.store(true, SeqCst);
    eventually("the root's end", || FINISHED.load(SeqCst));
    eventually("the run's reclamation", || {
        control.status().reclamation_pending == 0
    });
}

/// A stop while a home extern is in C: the status says the stop is requested and the home
/// thread busy, no home state is destroyed and `wait` waits; once C returns the interval's
/// leave ends the run, home state dies on the admission thread, and that thread exits
/// with nothing of the run's affine state left.
#[test]
fn a_stop_while_a_home_extern_is_in_c_waits_for_the_call() {
    let _serial = serial();
    take_threads();
    static IN_C: AtomicBool = AtomicBool::new(false);
    static RETURN: AtomicBool = AtomicBool::new(false);
    static AFTER: AtomicBool = AtomicBool::new(false);
    IN_C.store(false, SeqCst);
    RETURN.store(false, SeqCst);
    AFTER.store(false, SeqCst);
    let (sender, receiver) = std::sync::mpsc::channel();
    let admission = std::thread::spawn(move || {
        // The binding is not `Send`: it is made, bound and destroyed on this thread.
        let (home, dropped) = Home::new();
        let mut run = BodyRun::new(2, false);
        run.config = config();
        run.adapter = Some(Box::new(home));
        sender.send((run.control(), Arc::clone(&dropped))).unwrap();
        let ended = run.run(
            |_| {},
            |ctx| {
                let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                unsafe {
                    hirsel_rt_v2_foreign_enter(
                        ctx,
                        1,
                        interval.as_mut_ptr().cast(),
                        FOREIGN_THREADS_HOME,
                    )
                };
                // "C", on the home thread.
                ran_here();
                IN_C.store(true, SeqCst);
                while !RETURN.load(SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                unsafe { hirsel_rt_v2_foreign_leave(interval.as_ptr().cast()) };
                AFTER.store(true, SeqCst);
                0
            },
        );
        assert_eq!(
            *dropped.lock().unwrap(),
            Some(std::thread::current().id()),
            "home state dies on the admission thread before it exits"
        );
        (ended.ended, ended.pending, std::thread::current().id())
    });
    let (control, dropped) = receiver.recv().unwrap();
    eventually("the home call", || IN_C.load(SeqCst));
    control.stop();
    std::thread::sleep(Duration::from_millis(20));
    let status = control.status();
    assert!(status.stop_requested && status.home_busy && !status.finished, "{status:?}");
    assert_eq!(*dropped.lock().unwrap(), None, "no home destructor ran");
    assert!(!admission.is_finished(), "wait waits for the call");
    RETURN.store(true, SeqCst);
    let (ended, pending, thread) = admission.join().unwrap();
    assert_eq!(ended, Err(Stop::Requested));
    assert_eq!(pending, 0);
    assert!(!AFTER.load(SeqCst), "the interval's leave ended the run");
    assert_eq!(take_threads(), [thread], "the home call ran on the admission thread");
    assert_eq!(*dropped.lock().unwrap(), Some(thread));
    let status = control.status();
    assert!(status.finished && !status.home_busy && status.reclamation_pending == 0);
}

/// A host's stop races allocation and release on whichever worker runs: every run ends
/// Stopped, or returns when the stop came too late, and none faults.
#[test]
fn a_stop_racing_another_workers_allocation_ends_the_run() {
    let _serial = serial();
    unsafe extern "C" fn allocating(
        ctx: *mut TaskCtx,
        _environment: *mut Header,
        _caller_site: u64,
        _operation_site: u64,
    ) -> u8 {
        for _ in 0..2000 {
            for _ in 0..8 {
                let block = unsafe {
                    hirsel_rt::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        ENVIRONMENT,
                        8,
                        8,
                        hirsel_rt::DOMAIN_HEAP,
                        0,
                    )
                };
                write_u64(block, 0, 1);
                unsafe {
                    hirsel_rt::release::hirsel_rt_v2_release(
                        ctx,
                        block,
                        ENVIRONMENT,
                        std::ptr::null(),
                        0,
                        0,
                    )
                };
            }
            unsafe { yield_now(&mut *ctx) };
        }
        let mut slot = [1u64, 0];
        unsafe {
            hirsel_rt::tasks::hirsel_rt_v2_task_finish(
                ctx,
                slot.as_mut_ptr().cast(),
                NONE.as_ptr().cast(),
                16,
            )
        }
    }
    let mut stopped = 0;
    for round in 0..20u64 {
        let mut run = BodyRun::new(3, round % 2 == 0);
        run.config = config();
        let control = run.control();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_micros(200 * round));
            control.stop();
        });
        let ended = run.run(
            |_| {},
            |ctx| {
                let scope = enter(ctx);
                let tasks: Vec<_> = (0..3).map(|id| spawn(ctx, scope, allocating, id)).collect();
                for task in tasks {
                    join(ctx, scope, task);
                }
                exit(ctx, scope);
                0
            },
        );
        stopper.join().unwrap();
        match ended.ended {
            Err(Stop::Requested) => stopped += 1,
            Ok(0) => {}
            other => panic!("round {round}: {other:?}"),
        }
    }
    assert!(stopped > 0, "no round was stopped");
}

static CHARGES_AFTER_STOP: AtomicU64 = AtomicU64::new(0);

/// A loop with no runtime entry but fuel points: the run's stop ends it at its next refill,
/// within one chunk of fuel, at width 2 without a budget and at width 1 under a host's stop
/// handle with one.
#[test]
fn a_fuel_loop_observes_a_stop_within_one_chunk() {
    let _serial = serial();
    for (width, fuel) in [(2, None), (1, Some(u64::MAX / 2)), (2, Some(u64::MAX / 2))] {
        static STOP_SENT: AtomicBool = AtomicBool::new(false);
        static LOOPING: AtomicBool = AtomicBool::new(false);
        STOP_SENT.store(false, SeqCst);
        LOOPING.store(false, SeqCst);
        CHARGES_AFTER_STOP.store(0, SeqCst);
        let mut run = BodyRun::new(width, false);
        run.fuel = fuel;
        let control = run.control();
        let status = run.control();
        let stopper = std::thread::spawn(move || {
            eventually("the loop", || LOOPING.load(SeqCst));
            control.stop();
            STOP_SENT.store(true, SeqCst);
        });
        let ended = run.run(
            |_| {},
            |ctx| {
                LOOPING.store(true, SeqCst);
                loop {
                    charge(ctx);
                    if STOP_SENT.load(SeqCst) {
                        CHARGES_AFTER_STOP.fetch_add(1, SeqCst);
                    }
                }
            },
        );
        stopper.join().unwrap();
        assert_eq!(ended.ended, Err(Stop::Requested), "width {width}, fuel {fuel:?}");
        // A detached root leaves at its next refill too, before the next run counts.
        eventually("the root to leave", || {
            status.status().reclamation_pending == 0
        });
        assert!(
            CHARGES_AFTER_STOP.load(SeqCst) <= hirsel_rt::budget::FUEL_CHUNK,
            "width {width}: {} charges after the stop",
            CHARGES_AFTER_STOP.load(SeqCst)
        );
    }
}

/// R4 at width > 1: refills after the task migrated, then a concurrent stop at a refill.
#[test]
fn fuel_refills_after_migration_and_a_concurrent_stop() {
    let _serial = serial();
    static MIGRATED: AtomicBool = AtomicBool::new(false);
    MIGRATED.store(false, SeqCst);
    let mut run = BodyRun::new(2, true);
    run.fuel = Some(u64::MAX / 2);
    let metrics = run.worker_metrics();
    let control = run.control();
    let stopper = std::thread::spawn({
        let control = control.clone();
        move || {
            eventually("the refills after migration", || MIGRATED.load(SeqCst));
            control.stop();
        }
    });
    let ended = run.run(
        |_| {},
        move |ctx| {
            for _ in 0..3 {
                for _ in 0..3 * hirsel_rt::budget::FUEL_CHUNK {
                    charge(ctx);
                }
                unsafe { yield_now(&mut *ctx) };
            }
            MIGRATED.store(true, SeqCst);
            loop {
                charge(ctx);
            }
        },
    );
    stopper.join().unwrap();
    assert_eq!(ended.ended, Err(Stop::Requested));
    assert!(metrics.migrations() >= 3);
}

/// PROG-4: a callback's failure on a run with compute workers ends the process, as on one.
#[test]
fn a_callback_failure_on_a_wide_run_ends_the_process() {
    if in_child("a_callback_failure_on_a_wide_run_ends_the_process") {
        let mut run = BodyRun::new(2, true);
        run.config = config();
        run.run(
            |_| {},
            |ctx| {
                let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                unsafe {
                    hirsel_rt_v2_foreign_enter(
                        ctx,
                        1,
                        interval.as_mut_ptr().cast(),
                        FOREIGN_THREADS_HOME,
                    );
                    let callback = hirsel_rt_v2_callback_context();
                    hirsel_rt_v2_callback_failed(callback);
                }
                unreachable!("a failed callback returned")
            },
        );
        unreachable!("a failed callback's run returned");
    }
    assert_child_aborts(
        "a_callback_failure_on_a_wide_run_ends_the_process",
        "a callback failed",
    );
}
