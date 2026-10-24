//! Whole-runtime stop, capacity and lifetime acceptance for production Free
//! (r6 §3 and §9, SEAMS S9): cooperative shrink and regrowth while tasks run,
//! stopped pure loops whose permits stay lost until they leave, a stop while a
//! home extern is in C, a stopped blocking call that keeps its worker until the
//! call actually returns, and home-affine state destroyed only on home. Each
//! case runs at widths 2 and 8, unrecorded and recorded, through the stable
//! task/foreign entries rather than scheduler internals.
use crate::foreign::*;
use crate::host::BodyRun;
use crate::io::{Adapter, AdapterError, Completion, Request};
use crate::stop::Stop;
use crate::tasks::free::record::Recording;
use crate::tasks::{
    hirsel_rt_v2_task_canceled, hirsel_rt_v2_task_join, hirsel_rt_v2_task_scope_enter,
    hirsel_rt_v2_task_scope_exit, hirsel_rt_v2_task_spawn,
};
use crate::{Header, STATUS_OK, TaskCtx};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

const CHILD: &str = "HIRSEL_RUNTIME50_STOP_CAPACITY";
const WIDTHS: [u32; 2] = [2, 8];

fn eventually(what: &str, condition: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < until, "{what} did not happen");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Non-Send home state. Its destructor records the thread that ran it.
struct Home {
    dropped: Arc<Mutex<Option<ThreadId>>>,
    _affine: std::rc::Rc<()>,
}

impl Home {
    fn new() -> (Self, Arc<Mutex<Option<ThreadId>>>) {
        let dropped = Arc::new(Mutex::new(None));
        let home = Home {
            dropped: dropped.clone(),
            _affine: std::rc::Rc::new(()),
        };
        (home, dropped)
    }
}

impl Adapter for Home {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
        Err(AdapterError::ReplayMismatch(0))
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        *self.dropped.lock().unwrap() = Some(std::thread::current().id());
    }
}

/// A child's environment: one scalar word, descriptor index 2.
fn layouts() -> &'static [*const crate::Layout] {
    let environment = Box::leak(Box::new(crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 8,
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
    }));
    Box::leak(vec![&raw const *environment].into_boxed_slice())
}

/// A production Free run, with the home adapter and optional recorder.
fn free_run(width: u32, recorded: Option<bool>) -> (BodyRun, Option<Arc<Recording>>) {
    let mut run = BodyRun::new(width, true);
    let layouts = layouts();
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    run.free = true;
    unsafe { run.use_physical_dispatch() };
    let recording = recorded.map(|durable| Recording::new(b"runtime50 stop capacity", durable));
    (run, recording)
}

fn setup(recording: Option<Arc<Recording>>) -> impl FnOnce(*mut TaskCtx) {
    move |ctx| unsafe {
        crate::tasks::free::enable(ctx);
        if let Some(recording) = &recording {
            recording.root(ctx);
        }
    }
}

fn op(ctx: *mut TaskCtx) {
    let mut canceled = 99;
    assert_eq!(
        unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) },
        STATUS_OK
    );
    assert_eq!(canceled, 0);
}

static CHILD_OPS: AtomicU64 = AtomicU64::new(0);

/// More than two 128-operation budgets, so every child gives up its worker.
unsafe extern "C" fn busy_child(ctx: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
    for _ in 0..300 {
        op(ctx);
        CHILD_OPS.fetch_add(1, SeqCst);
    }
    STATUS_OK
}

/// Children keep running through a contraction to one worker and a regrowth;
/// retiring workers return their queued and started tasks rather than lose them.
fn cooperative_shrink(width: u32, recorded: Option<bool>) {
    const CHILDREN: u64 = 16;
    CHILD_OPS.store(0, SeqCst);
    let (run, recording) = free_run(width, recorded);
    let control = run.control();
    let (home, dropped) = Home::new();
    let mut run = run;
    run.adapter = Some(Box::new(home));
    let widths = Arc::new(Mutex::new(Vec::new()));
    let observed = widths.clone();
    let outcome = run.run(setup(recording.clone()), move |ctx| unsafe {
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        let mut children = Vec::new();
        for _ in 0..CHILDREN {
            let mut child = 0;
            hirsel_rt_v2_task_spawn(
                ctx,
                0,
                0,
                std::ptr::null_mut(),
                &mut child,
                &scope,
                busy_child,
                crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0),
                0,
            );
            children.push(child);
        }
        for target in [1, width, 1] {
            control.resize(target).unwrap();
            let until = Instant::now() + Duration::from_secs(10);
            while control.worker_width() != target {
                assert!(
                    Instant::now() < until,
                    "width {} did not reach {target}",
                    control.worker_width()
                );
                op(ctx);
            }
            observed.lock().unwrap().push(control.worker_width());
        }
        for mut child in children {
            let mut slot = [0u64; 2];
            assert_eq!(
                hirsel_rt_v2_task_join(ctx, 0, 0, &scope, &mut child, slot.as_mut_ptr().cast(), 0),
                STATUS_OK
            );
        }
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
        STATUS_OK
    });
    assert_eq!(outcome.ended, Ok(STATUS_OK), "shrink width {width}");
    assert_eq!(outcome.pending, 0);
    assert_eq!(*widths.lock().unwrap(), [1, width, 1]);
    assert_eq!(
        CHILD_OPS.load(SeqCst),
        CHILDREN * 300,
        "every child op ran once"
    );
    assert_eq!(*dropped.lock().unwrap(), Some(std::thread::current().id()));
    finish(recording, "returned");
}

/// A stopped non-fuel pure loop returns the run at once. Its worker keeps its
/// permit, reported as lost, until the loop exits; no replacement is admitted.
fn lost_permit(width: u32, recorded: Option<bool>) {
    let looping = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let (mut run, recording) = free_run(width, recorded);
    let (home, dropped) = Home::new();
    run.adapter = Some(Box::new(home));
    let control = run.control();
    let stopper = std::thread::spawn({
        let (control, looping) = (control.clone(), looping.clone());
        move || {
            eventually("the pure loop", || looping.load(SeqCst));
            control.stop();
        }
    });
    let held = crate::pool::compute_permits().1;
    let outcome = run.run(setup(recording.clone()), {
        let (looping, release, finished) = (looping.clone(), release.clone(), finished.clone());
        move |ctx| {
            op(ctx);
            looping.store(true, SeqCst);
            while !release.load(SeqCst) {
                std::hint::spin_loop();
            }
            finished.store(true, SeqCst);
            STATUS_OK
        }
    });
    stopper.join().unwrap();
    assert_eq!(
        outcome.ended,
        Err(Stop::Requested),
        "lost permit width {width}"
    );
    assert!(
        outcome.pending >= 1,
        "the loop's worker still holds the run"
    );
    let status = control.status();
    assert!(status.stop_requested && status.finished && !status.home_busy);
    assert!(status.reclamation_pending >= 1);
    assert_eq!(status.lost_permits, 1, "{status:?}");
    assert_eq!(crate::pool::lost_compute_permits(), 1);
    assert_eq!(crate::pool::compute_permits().1, held + 1);
    assert_eq!(
        *dropped.lock().unwrap(),
        Some(std::thread::current().id()),
        "home state dies on home while a stopped worker still runs"
    );
    assert!(!finished.load(SeqCst));
    release.store(true, SeqCst);
    eventually("the loop's end", || finished.load(SeqCst));
    eventually("the stopped run's reclamation", || {
        control.status().reclamation_pending == 0
    });
    assert_eq!(control.status().lost_permits, 0);
    assert_eq!(crate::pool::lost_compute_permits(), 0);
    assert_eq!(crate::pool::compute_permits().1, held);
    finish(recording, "stopped");
}

/// A stop while a home extern is in C: home_busy, wait waits, and home state
/// survives until C returns and the interval's leave ends the run on home.
fn home_busy_stop(width: u32, recorded: Option<bool>) {
    let in_c = Arc::new(AtomicBool::new(false));
    let ret = Arc::new(AtomicBool::new(false));
    let after = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = std::sync::mpsc::channel();
    let admission = std::thread::spawn({
        let (in_c, ret, after) = (in_c.clone(), ret.clone(), after.clone());
        move || {
            let (mut run, recording) = free_run(width, recorded);
            let (home, dropped) = Home::new();
            run.adapter = Some(Box::new(home));
            sender.send((run.control(), dropped.clone())).unwrap();
            let home = std::thread::current().id();
            let outcome = run.run(setup(recording.clone()), move |ctx| unsafe {
                let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
                hirsel_rt_v2_foreign_enter(
                    ctx,
                    0,
                    interval.as_mut_ptr().cast(),
                    FOREIGN_THREADS_HOME | FOREIGN_BLOCKING,
                );
                assert_eq!(std::thread::current().id(), home, "home extern on home");
                in_c.store(true, SeqCst);
                while !ret.load(SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
                after.store(true, SeqCst);
                STATUS_OK
            });
            assert_eq!(*dropped.lock().unwrap(), Some(home));
            finish(recording, "stopped");
            (outcome, home)
        }
    });
    let (control, dropped) = receiver.recv().unwrap();
    eventually("the home call", || in_c.load(SeqCst));
    control.stop();
    std::thread::sleep(Duration::from_millis(20));
    let status = control.status();
    assert!(
        status.stop_requested && status.home_busy && !status.finished,
        "home busy width {width}: {status:?}"
    );
    assert_eq!(*dropped.lock().unwrap(), None, "no home destructor ran");
    assert!(!admission.is_finished(), "wait waits for the home call");
    ret.store(true, SeqCst);
    let (outcome, home) = admission.join().unwrap();
    assert_eq!(outcome.ended, Err(Stop::Requested));
    assert!(!after.load(SeqCst), "the interval's leave ended the run");
    assert_eq!(*dropped.lock().unwrap(), Some(home));
    eventually("home busy run reclamation", || {
        control.status().reclamation_pending == 0
    });
    let status = control.status();
    assert!(status.finished && !status.home_busy && status.lost_permits == 0);
}

/// A stop while a blocking call is in C on a compute worker: the run returns,
/// C holds no compute permit, and the worker retains the stopped run until the
/// call actually returns. No Hirsel code runs after that return.
fn blocking_return(width: u32, recorded: Option<bool>) {
    let in_c = Arc::new(AtomicBool::new(false));
    let ret = Arc::new(AtomicBool::new(false));
    let returned = Arc::new(AtomicBool::new(false));
    let after = Arc::new(AtomicBool::new(false));
    let (mut run, recording) = free_run(width, recorded);
    let (home, dropped) = Home::new();
    run.adapter = Some(Box::new(home));
    let control = run.control();
    let held = crate::pool::compute_permits().1;
    let stopper = std::thread::spawn({
        let (control, in_c) = (control.clone(), in_c.clone());
        move || {
            eventually("the blocking call", || in_c.load(SeqCst));
            control.stop();
        }
    });
    let admission = std::thread::current().id();
    let outcome = run.run(setup(recording.clone()), {
        let (in_c, ret, returned, after) =
            (in_c.clone(), ret.clone(), returned.clone(), after.clone());
        move |ctx| unsafe {
            op(ctx);
            let mut interval = [0u64; FOREIGN_SCOPE_BYTES / 8];
            hirsel_rt_v2_foreign_enter(
                ctx,
                0,
                interval.as_mut_ptr().cast(),
                FOREIGN_THREADS_ANY | FOREIGN_BLOCKING,
            );
            assert_ne!(
                std::thread::current().id(),
                admission,
                "blocking C off home"
            );
            assert_eq!(
                crate::pool::compute_permits().1,
                held,
                "blocking C holds no compute permit"
            );
            in_c.store(true, SeqCst);
            while !ret.load(SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            returned.store(true, SeqCst);
            hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
            after.store(true, SeqCst);
            STATUS_OK
        }
    });
    stopper.join().unwrap();
    assert_eq!(
        outcome.ended,
        Err(Stop::Requested),
        "blocking width {width}"
    );
    assert!(
        outcome.pending >= 1,
        "the blocked worker still holds the run"
    );
    let status = control.status();
    assert!(status.stop_requested && status.finished && !status.home_busy);
    assert!(status.reclamation_pending >= 1, "{status:?}");
    assert!(!returned.load(SeqCst));
    assert_eq!(*dropped.lock().unwrap(), Some(admission));
    ret.store(true, SeqCst);
    eventually("the blocking call's return", || returned.load(SeqCst));
    eventually("the blocked run's reclamation", || {
        control.status().reclamation_pending == 0
    });
    assert!(!after.load(SeqCst), "no Hirsel code after a stopped return");
    assert_eq!(control.status().lost_permits, 0);
    assert_eq!(crate::pool::compute_permits().1, held);
    finish(recording, "stopped");
}

/// The recorder closes and its last owner is the test once workers have left.
fn finish(recording: Option<Arc<Recording>>, outcome: &str) {
    let Some(recording) = recording else {
        return;
    };
    let _ = recording.recorder.stop(outcome, None);
    eventually("recording reclamation", || {
        Arc::strong_count(&recording) == 1
    });
    assert!(Arc::try_unwrap(recording).is_ok());
}

#[test]
fn runtime50_end_to_end_stop_capacity_lifetime() {
    // The process permit grant and lost-permit totals are process-wide.
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "free_launch::runtime50_tests::runtime50_end_to_end_stop_capacity_lifetime",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("1 passed"), "{output:?}");
        assert!(
            stdout.contains("runtime50 stop capacity cases 24"),
            "{output:?}"
        );
        return;
    }
    crate::pool::set_compute_grant(8);
    let mut cases = 0;
    for width in WIDTHS {
        for recorded in [None, Some(false), Some(true)] {
            cooperative_shrink(width, recorded);
            lost_permit(width, recorded);
            home_busy_stop(width, recorded);
            blocking_return(width, recorded);
            cases += 4;
        }
    }
    println!("runtime50 stop capacity cases {cases}");
}
