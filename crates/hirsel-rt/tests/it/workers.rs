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
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
            0,
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
    assert_eq!(
        unsafe { hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0) },
        0
    );
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
            u32::MAX,
        )
    }
}

/// One fuel point as compiled code charges it (`hirsel-llvm` `charge_fuel`).
fn charge(ctx: *mut TaskCtx) {
    unsafe {
        let word = ctx
            .cast::<u8>()
            .add(hirsel_rt::CTX_FUEL_OFFSET)
            .cast::<i64>();
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
fn role_guards_follow_saved_fibers_and_task_table_growth() {
    let _serial = serial();
    take_threads();
    for width in [1, 4] {
        let mut run = BodyRun::new(width, width > 1);
        run.config = config();
        let ended = run.run(
            |ctx| unsafe { hirsel_rt::tasks::role::enable(ctx) },
            |ctx| {
                let scope = enter(ctx);
                // Grow the task Vec several times while old continuations
                // retain save slots in stable TaskCtx storage.
                let tasks: Vec<_> = (1..=48).map(|id| spawn(ctx, scope, yielding, id)).collect();
                for (task, id) in tasks.into_iter().zip(1..) {
                    assert_eq!(join(ctx, scope, task), [1, id]);
                }
                exit(ctx, scope);
                0
            },
        );
        assert_eq!(ended.ended, Ok(0), "width {width}");
        assert_eq!(ended.pending, 0);
    }
    take_threads();
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
    let distinct: std::collections::BTreeSet<_> =
        threads.iter().map(|t| format!("{t:?}")).collect();
    assert!(
        distinct.len() >= 2,
        "every suspension migrates: {distinct:?}"
    );
    assert!(metrics.migrations() > 0);
    let values = metrics.values();
    for name in [
        "switches_per_op",
        "queue_ops_per_op",
        "spawn_ns",
        "migrations_per_s",
    ] {
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
    let control = run.control();
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
    // A stop returns immediately; pending is the snapshot at that boundary.
    // The bounded child must still retire and return every compute permit.
    eventually("the diverged child's reclamation", || {
        control.status().reclamation_pending == 0
    });
    let status = control.status();
    assert!(status.finished && status.stop_requested && !status.home_busy);
    assert_eq!(status.lost_permits, 0);
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
        cached_layout("workers-finalized", 0, || {
            layout(8, 0, Some(spending), Vec::new())
        })
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
    // A fast bounce back to the sender used to erase that worker's new busy
    // flag. Repeat the real stop/return interaction to exercise that race.
    for _ in 0..16 {
        stopped_migrated_loop();
    }
}

fn stopped_migrated_loop() {
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
    assert!(
        status.stop_requested && status.home_busy && !status.finished,
        "{status:?}"
    );
    assert_eq!(*dropped.lock().unwrap(), None, "no home destructor ran");
    assert!(!admission.is_finished(), "wait waits for the call");
    RETURN.store(true, SeqCst);
    let (ended, pending, thread) = admission.join().unwrap();
    assert_eq!(ended, Err(Stop::Requested));
    assert_eq!(pending, 0);
    assert!(!AFTER.load(SeqCst), "the interval's leave ended the run");
    assert_eq!(
        take_threads(),
        [thread],
        "the home call ran on the admission thread"
    );
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
                u32::MAX,
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
        assert_eq!(
            ended.ended,
            Err(Stop::Requested),
            "width {width}, fuel {fuel:?}"
        );
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

/// A second run cannot replace a stopped divergent worker's process permit.
/// Isolated in a subprocess because changing the manager's grant affects all tests.
#[test]
fn stopped_workers_keep_the_process_grant_across_runs() {
    const CHILD: &str = "HIRSEL_W1_PERMIT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "workers::stopped_workers_keep_the_process_grant_across_runs",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    hirsel_rt::pool::set_compute_grant(1);
    let looping = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let run = BodyRun::new(2, true);
    let control = run.control();
    let stopper = std::thread::spawn({
        let looping = looping.clone();
        let control = control.clone();
        move || {
            eventually("first root", || looping.load(SeqCst));
            control.stop();
        }
    });
    let ended = run.run(|_| {}, {
        let looping = looping.clone();
        let release = release.clone();
        move |_| {
            looping.store(true, SeqCst);
            while !release.load(SeqCst) {
                std::hint::spin_loop();
            }
            0
        }
    });
    stopper.join().unwrap();
    assert_eq!(ended.ended, Err(Stop::Requested));
    assert_eq!(control.status().lost_permits, 1);
    let entered = Arc::new(AtomicBool::new(false));
    let second = std::thread::spawn({
        let entered = entered.clone();
        move || {
            BodyRun::new(1, false).run(
                |_| {},
                move |_| {
                    entered.store(true, SeqCst);
                    0
                },
            )
        }
    });
    eventually("second run waiting for the process permit", || {
        hirsel_rt::pool::compute_permits().2 > 0
    });
    assert!(!entered.load(SeqCst));
    assert_eq!(hirsel_rt::pool::compute_permits().1, 1);
    release.store(true, SeqCst);
    assert_eq!(second.join().unwrap().ended, Ok(0));
    eventually("first root reclamation", || {
        control.status().reclamation_pending == 0
    });
    assert_eq!(control.status().lost_permits, 0);
    assert_eq!(hirsel_rt::pool::compute_permits().1, 0);
}

/// An external stop observed at a home callback's operation is a supported
/// abandonment boundary; it neither returns into C nor aborts the host process.
#[test]
fn a_home_callback_observes_a_host_stop_at_its_next_operation() {
    let _serial = serial();
    let (home, dropped) = Home::new();
    let entered = Arc::new(AtomicBool::new(false));
    let stopping = Arc::new(AtomicBool::new(false));
    let mut run = BodyRun::new(2, true);
    run.adapter = Some(Box::new(home));
    let control = run.control();
    let stopper = std::thread::spawn({
        let entered = entered.clone();
        let stopping = stopping.clone();
        move || {
            eventually("home callback", || entered.load(SeqCst));
            control.stop();
            stopping.store(true, SeqCst);
        }
    });
    let ended = run.run(
        |_| {},
        move |ctx| {
            let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
            unsafe {
                hirsel_rt_v2_foreign_enter(
                    ctx,
                    0,
                    interval.as_mut_ptr().cast(),
                    FOREIGN_THREADS_HOME,
                );
            }
            assert_eq!(hirsel_rt_v2_callback_context(), ctx);
            entered.store(true, SeqCst);
            while !stopping.load(SeqCst) {
                std::hint::spin_loop();
            }
            unsafe {
                yield_now(&mut *ctx);
            }
            panic!("a stopped callback returned into C")
        },
    );
    stopper.join().unwrap();
    assert_eq!(ended.ended, Err(Stop::Requested));
    assert_eq!(*dropped.lock().unwrap(), Some(std::thread::current().id()));
}

/// A callback remains on its foreign thread, while its non-Send adapter call
/// and destructor run on admission, at both widths.
#[test]
fn pool_callbacks_use_the_home_adapter_without_migrating_c_frames() {
    let _serial = serial();
    struct CloseHome {
        home: ThreadId,
        calls: Arc<AtomicU64>,
        _affine: std::rc::Rc<()>,
    }
    impl Adapter for CloseHome {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            assert_eq!(self.home, std::thread::current().id());
            assert!(matches!(request, Request::Close { handle: 99 }));
            self.calls.fetch_add(1, SeqCst);
            Ok(Completion::Unit(Ok(())))
        }
    }
    impl Drop for CloseHome {
        fn drop(&mut self) {
            assert_eq!(self.home, std::thread::current().id());
        }
    }
    for width in [1, 2] {
        let calls = Arc::new(AtomicU64::new(0));
        let mut run = BodyRun::new(width, true);
        run.adapter = Some(Box::new(CloseHome {
            home: std::thread::current().id(),
            calls: calls.clone(),
            _affine: std::rc::Rc::new(()),
        }));
        let ended = run.run(
            |_| {},
            |ctx| {
                let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                unsafe {
                    hirsel_rt_v2_foreign_enter(
                        ctx,
                        0,
                        interval.as_mut_ptr().cast(),
                        hirsel_rt::foreign::FOREIGN_THREADS_ANY
                            | hirsel_rt::foreign::FOREIGN_BLOCKING,
                    );
                    let thread = std::thread::current().id();
                    assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                    for _ in 0..32 {
                        assert_eq!(hirsel_rt::io::hirsel_rt_v2_io_close(ctx, 99, 0, 0), 0);
                    }
                    assert_eq!(thread, std::thread::current().id());
                    hirsel_rt::foreign::hirsel_rt_v2_callback_leave(ctx);
                    hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
                }
                0
            },
        );
        assert_eq!(ended.ended, Ok(0));
        assert_eq!(calls.load(SeqCst), 32);
    }
}

/// The real CONC-8 wait path must return the process permit to its caller,
/// rather than keeping it on the home service thread.
#[test]
fn an_idle_home_wait_reports_blocked_with_one_process_permit() {
    const CHILD: &str = "HIRSEL_W1_IDLE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "workers::an_idle_home_wait_reports_blocked_with_one_process_permit",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    hirsel_rt::pool::set_compute_grant(1);
    struct Waiting(ThreadId);
    impl Adapter for Waiting {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            assert_eq!(self.0, std::thread::current().id());
            assert!(matches!(request, Request::SleepUntil { .. }));
            Err(AdapterError::Blocked)
        }
        fn wait(&mut self) -> Result<bool, AdapterError> {
            assert_eq!(self.0, std::thread::current().id());
            assert_eq!(
                hirsel_rt::pool::compute_permits().1,
                0,
                "waiting on home holds no compute permit"
            );
            Ok(false)
        }
    }
    let mut run = BodyRun::new(2, true);
    run.adapter = Some(Box::new(Waiting(std::thread::current().id())));
    let outcome = run.run(
        |_| {},
        |ctx| {
            let mut out = std::mem::MaybeUninit::uninit();
            unsafe {
                hirsel_rt::io::hirsel_rt_v2_clock_sleep_until(
                    ctx,
                    std::ptr::null_mut(),
                    out.as_mut_ptr(),
                    &1,
                    &123,
                );
            }
            panic!("a permanently blocked wait returned")
        },
    );
    assert!(
        matches!(outcome.ended, Err(Stop::Blocked(_))),
        "{outcome:?}"
    );
    assert_eq!(outcome.pending, 0);
    assert_eq!(hirsel_rt::pool::compute_permits().1, 0);
}

#[test]
fn live_width_grows_shrinks_and_returns_tasks_from_retiring_workers() {
    let _serial = serial();
    let run = BodyRun::new(1, true);
    let control = run.control();
    control.resize(1).unwrap();
    let counts = Arc::new(Mutex::new(Vec::new()));
    let output = counts.clone();
    let result = run.run(
        |_| {},
        move |ctx| {
            for target in [2, 8, 1, 4, 1, 8, 2, 1] {
                control.resize(target).unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    unsafe { yield_now(&mut *ctx) };
                    if control.worker_width() == target {
                        break;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "width {} did not reach {target}",
                        control.worker_width()
                    );
                }
                ran_here();
                output.lock().unwrap().push(control.worker_width());
            }
            0
        },
    );
    assert_eq!(result.ended, Ok(0));
    assert_eq!(*counts.lock().unwrap(), [2, 8, 1, 4, 1, 8, 2, 1]);
    take_threads();
}

#[test]
fn repeated_stopped_runs_never_replace_lost_permits_and_only_manager_restores_capacity() {
    const CHILD: &str = "HIRSEL_K1_LOST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "workers::repeated_stopped_runs_never_replace_lost_permits_and_only_manager_restores_capacity", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    hirsel_rt::pool::set_compute_grant(3);
    let release = Arc::new(AtomicBool::new(false));
    let mut stopped = Vec::new();
    for lost in 1..=3 {
        let run = BodyRun::new(2, true);
        let control = run.control();
        let entered = Arc::new(AtomicBool::new(false));
        let stopper = std::thread::spawn({
            let (control, entered) = (control.clone(), entered.clone());
            move || {
                eventually("divergent root entry", || entered.load(SeqCst));
                control.stop();
            }
        });
        let outcome = run.run(|_| {}, {
            let release = release.clone();
            move |_| {
                entered.store(true, SeqCst);
                while !release.load(SeqCst) {
                    std::hint::spin_loop();
                }
                0
            }
        });
        stopper.join().unwrap();
        assert_eq!(outcome.ended, Err(Stop::Requested));
        assert_eq!(hirsel_rt::pool::lost_compute_permits(), lost);
        assert_eq!(hirsel_rt::pool::compute_permits().1, lost);
        stopped.push(control);
    }
    let entered = Arc::new(AtomicBool::new(false));
    let fourth = std::thread::spawn({
        let entered = entered.clone();
        move || {
            BodyRun::new(1, false).run(
                |_| {},
                move |_| {
                    entered.store(true, SeqCst);
                    0
                },
            )
        }
    });
    eventually("fourth run awaiting capacity", || {
        hirsel_rt::pool::compute_permits().2 > 0
    });
    assert!(!entered.load(SeqCst));
    hirsel_rt::pool::set_compute_grant(4);
    assert_eq!(fourth.join().unwrap().ended, Ok(0));
    assert_eq!(hirsel_rt::pool::lost_compute_permits(), 3);
    release.store(true, SeqCst);
    eventually("all stopped workers reclaimed", || {
        stopped.iter().all(|c| c.status().reclamation_pending == 0)
    });
    assert_eq!(hirsel_rt::pool::lost_compute_permits(), 0);
    assert_eq!(hirsel_rt::pool::compute_permits().1, 0);
}

unsafe extern "C" {
    fn hirsel_rt_v2_depth_enter_at_site(ctx: *mut TaskCtx, anchor: u64, operation_site: u64) -> u8;
    fn hirsel_rt_v2_depth_leave(ctx: *mut TaskCtx);
}

#[test]
fn callback_admission_31_exact_and_retirement() {
    let _serial = serial();
    let mut run = BodyRun::new(1, true);
    run.config = config();
    let ended = run.run(
        |_| {},
        |ctx| {
            let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
            // SAFETY: this stand-in owns the running task; the interval and all
            // admissions are paired before returning to the run launcher.
            unsafe {
                for _ in 1..hirsel_core_contract::depth::FOREGROUND_LIMIT {
                    assert_eq!(hirsel_rt_v2_depth_enter_at_site(ctx, 77, 88), 0);
                }
                hirsel_rt_v2_foreign_enter(
                    ctx,
                    77,
                    interval.as_mut_ptr().cast(),
                    FOREIGN_THREADS_HOME,
                );
                assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                hirsel_rt::foreign::hirsel_rt_v2_callback_leave(ctx);
                assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                hirsel_rt::foreign::hirsel_rt_v2_callback_leave(ctx);
                hirsel_rt_v2_foreign_leave(interval.as_mut_ptr().cast());
                for _ in 1..hirsel_core_contract::depth::FOREGROUND_LIMIT {
                    hirsel_rt_v2_depth_leave(ctx);
                }
            }
            0
        },
    );
    assert_eq!(ended.ended, Ok(0));
}

#[test]
fn callback_admission_31_over_preserves_foreign_fatal_boundary() {
    let name = "callback_admission_31_over_preserves_foreign_fatal_boundary";
    if in_child(name) {
        let mut run = BodyRun::new(1, true);
        run.config = config();
        run.run(
            |_| {},
            |ctx| {
                let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                // SAFETY: exclusively running task and live interval storage. The
                // rejected callback ends the process before any language body runs.
                unsafe {
                    for _ in 0..hirsel_core_contract::depth::FOREGROUND_LIMIT {
                        assert_eq!(hirsel_rt_v2_depth_enter_at_site(ctx, 77, 88), 0);
                    }
                    hirsel_rt_v2_foreign_enter(
                        ctx,
                        77,
                        interval.as_mut_ptr().cast(),
                        FOREIGN_THREADS_HOME,
                    );
                    hirsel_rt_v2_callback_context();
                }
                panic!("a rejected callback entered its body");
            },
        );
        panic!("a failed foreign callback returned to its run");
    }
    assert_child_aborts(name, "a callback failed");
}
