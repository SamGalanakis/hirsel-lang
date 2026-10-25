//! The former cooperative Free fairness vector, through production dispatch.
use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};

static CHILD_RAN: AtomicBool = AtomicBool::new(false);
const NONE: [u64; 2] = [0, 0];

unsafe extern "C" fn child(ctx: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
    CHILD_RAN.store(true, SeqCst);
    let mut slot = [1_u64, 42];
    unsafe {
        hirsel_rt_v2_task_finish(
            ctx,
            slot.as_mut_ptr().cast(),
            NONE.as_ptr().cast(),
            16,
            u32::MAX,
        )
    }
}

/// Whether this process runs the named test's body: the first call reruns
/// it alone in a child process, so its one-permit grant is process-wide.
fn isolated(test: &str) -> bool {
    const CHILD: &str = "HIRSEL_FREE_QUANTUM_ONE_PERMIT";
    if std::env::var_os(CHILD).is_some() {
        // A wider production topology can run the child immediately on
        // another permit. One permit makes the 128-op fairness vectors exact.
        crate::pool::set_compute_grant(1);
        return true;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("tasks::free::quantum_tests::{test}"),
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    false
}

const LAYOUT: crate::Layout = crate::Layout {
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
};

#[test]
fn free_immediate_ops_keep_their_worker_for_at_most_128_semantic_ops() {
    if !isolated("free_immediate_ops_keep_their_worker_for_at_most_128_semantic_ops") {
        return;
    }
    let layout = LAYOUT;
    let layouts = [&layout as *const crate::Layout];
    for width in [1, 8] {
        CHILD_RAN.store(false, SeqCst);
        let mut run = crate::host::BodyRun::new(width, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        run.free = true;
        let metrics = run.worker_metrics();
        // SAFETY: the Free policy owns every operation and saved-frame wake.
        unsafe { run.use_physical_dispatch() };
        let saves = Arc::new(AtomicU32::new(0));
        let observed = saves.clone();
        let outcome = run.run(
            |ctx| unsafe { enable(ctx) },
            move |ctx| unsafe {
                assert!(enabled(ctx));
                let observed = observed.clone();
                crate::pool::free::test_before_save(ctx, move |task, completed| {
                    if task == 0 && !completed {
                        observed.fetch_add(1, SeqCst);
                    }
                });
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &7),
                    crate::STATUS_OK
                ); // Op 1.
                let environment =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                let mut task = 0;
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut task,
                        &scope,
                        child,
                        environment,
                        0,
                    ),
                    crate::STATUS_OK
                ); // Op 2.
                assert!(!CHILD_RAN.load(SeqCst), "spawn's switch is elided");
                assert_eq!(saves.load(SeqCst), 0);
                for _ in 0..125 {
                    yield_now(&mut *ctx);
                    assert!(!CHILD_RAN.load(SeqCst));
                    assert_eq!(saves.load(SeqCst), 0);
                }
                yield_now(&mut *ctx); // Op 128 gives up this worker.
                assert_eq!(saves.load(SeqCst), 1, "op 128 physically saves the root");
                // The child is injected while the yielded root is local.
                // Production Free promises bounded global service, not FIFO
                // selection of that child at the root's first quantum end.
                let bound = usize::from(hirsel_free::schedule::OP_BUDGET)
                    * hirsel_free::work::GLOBAL_INTERVAL as usize;
                for _ in 0..bound {
                    if CHILD_RAN.load(SeqCst) {
                        break;
                    }
                    yield_now(&mut *ctx);
                }
                assert!(
                    CHILD_RAN.load(SeqCst),
                    "the injected child made bounded progress"
                );
                let mut slot = NONE;
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut task,
                        slot.as_mut_ptr().cast(),
                        16,
                    ),
                    crate::STATUS_OK
                );
                assert_eq!(slot, [1, 42]);
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    crate::STATUS_OK
                );
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK), "width {width}");
        assert_eq!(outcome.pending, 0);
        assert!(metrics.elisions() >= 127);
    }
}

/// A spawn the compiler pairs with its join (`IMMEDIATE`) elides its switch,
/// but is still an op: as op 128 it gives up the worker, and the quantum's
/// exhaustion is not charged twice into a fresh one.
#[test]
fn an_immediate_spawn_as_op_128_gives_up_its_worker() {
    if !isolated("an_immediate_spawn_as_op_128_gives_up_its_worker") {
        return;
    }
    let layout = LAYOUT;
    let layouts = [&layout as *const crate::Layout];
    for width in [1, 8] {
        let mut run = crate::host::BodyRun::new(width, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        run.free = true;
        // SAFETY: the Free policy owns every operation and saved-frame wake.
        unsafe { run.use_physical_dispatch() };
        let saves = Arc::new(AtomicU32::new(0));
        let observed = saves.clone();
        let outcome = run.run(
            |ctx| unsafe { enable(ctx) },
            move |ctx| unsafe {
                let observed = observed.clone();
                crate::pool::free::test_before_save(ctx, move |task, completed| {
                    if task == 0 && !completed {
                        observed.fetch_add(1, SeqCst);
                    }
                });
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &7),
                    crate::STATUS_OK
                ); // Op 1.
                for _ in 0..126 {
                    yield_now(&mut *ctx);
                }
                assert_eq!(saves.load(SeqCst), 0);
                let environment =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                let mut task = 0;
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut task,
                        &scope,
                        child,
                        environment,
                        spawn::IMMEDIATE,
                    ),
                    crate::STATUS_OK
                ); // Op 128.
                assert_eq!(saves.load(SeqCst), 1, "op 128 physically saves the root");
                let mut slot = NONE;
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut task,
                        slot.as_mut_ptr().cast(),
                        16,
                    ),
                    crate::STATUS_OK
                );
                assert_eq!(slot, [1, 42]);
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    crate::STATUS_OK
                );
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK), "width {width}");
        assert_eq!(outcome.pending, 0);
    }
}
