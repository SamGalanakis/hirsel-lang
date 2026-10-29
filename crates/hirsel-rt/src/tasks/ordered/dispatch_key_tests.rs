//! A late notification with a retired task's key never reaches the row that
//! reuses its slot: it neither activates that row nor exposes, creates or
//! notifies its frame, under Free and Ordered physical dispatch.
use super::values::{Field, Node, Program, Tag};
use super::{engine, events};
use crate::pool::Owner;
use crate::tasks::registry::TaskKey;
use crate::tasks::{self, role, scheduler};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};

const RESULT: u32 = 3;

#[derive(Default)]
struct Probe {
    entered: AtomicU32,
    running: AtomicBool,
    go: AtomicBool,
}

fn layout(size: u64) -> crate::Layout {
    crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size,
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

/// Counts its activation, then spins until released: while it runs, a
/// notification would be remembered on its frame rather than lost.
unsafe extern "C" fn child(
    ctx: *mut crate::TaskCtx,
    env: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    let [probe, result] = unsafe { crate::alloc::payload(env).cast::<[u64; 2]>().read() };
    let probe = unsafe { &*(probe as *const Probe) };
    probe.entered.fetch_add(1, SeqCst);
    probe.running.store(true, SeqCst);
    while !probe.go.load(SeqCst) {
        std::hint::spin_loop();
    }
    let mut image = [1u64, 42];
    unsafe {
        tasks::hirsel_rt_v2_task_finish(
            ctx,
            image.as_mut_ptr().cast(),
            [0u64; 2].as_ptr().cast(),
            16,
            result as u32,
        )
    }
}

unsafe fn spawn(ctx: *mut crate::TaskCtx, scope: &u64, probe: &Probe, result: u32) -> TaskKey {
    unsafe {
        let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(env)
            .cast::<[u64; 2]>()
            .write([probe as *const Probe as u64, u64::from(result)]);
        let mut task = 0;
        tasks::hirsel_rt_v2_task_spawn(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            &mut task,
            scope,
            child,
            env,
            0,
        );
        TaskKey::from_word(task)
    }
}

unsafe fn join(ctx: *mut crate::TaskCtx, scope: &u64, task: TaskKey) {
    let mut word = task.word();
    let mut image = [0u64; 2];
    let status = unsafe {
        tasks::hirsel_rt_v2_task_join(ctx, 0, 0, scope, &mut word, image.as_mut_ptr().cast(), 16)
    };
    assert_eq!((status, image), (crate::STATUS_OK, [1, 42]));
}

/// A completes and is joined; B reuses its slot; the root then wakes A's key
/// through both policy entry points while B is live.
unsafe fn body(ctx: *mut crate::TaskCtx, probe: &Probe, result: u32, free: bool) -> u8 {
    unsafe {
        let mut scope = 0;
        tasks::hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        probe.go.store(true, SeqCst);
        let old = spawn(ctx, &scope, probe, result);
        join(ctx, &scope, old);
        // Join publishes logical completion. Older idle-save continuations
        // can still hold frame leases, which deliberately postpone slot reuse.
        loop {
            role::enter(ctx);
            let ready = scheduler(ctx).task_slots.reuse_ready(old.slot);
            role::release(ctx);
            if ready {
                break;
            }
            std::thread::yield_now();
        }
        // Free: B spins until released, so a wake would latch on its frame.
        probe.running.store(false, SeqCst);
        probe.go.store(!free, SeqCst);
        let new = spawn(ctx, &scope, probe, result);
        assert_eq!(new.slot, old.slot, "B reuses A's retired slot");
        assert!(new.generation > old.generation);
        while free && !probe.running.load(SeqCst) {
            std::hint::spin_loop();
        }
        role::enter(ctx);
        let run = (*ctx).run_ptr();
        let before = scheduler(ctx).task(new.slot).dispatch.clone();
        assert!(
            tasks::dispatch_frame(run, old).is_none(),
            "a stale key exposed a frame"
        );
        crate::pool::free::schedule_saved(run, old, Owner::Any);
        drop({
            let mut batch = crate::pool::free::ReadyBatch::new(run);
            batch.schedule(old, Owner::Any);
            batch
        });
        let after = scheduler(ctx).task(new.slot).dispatch.clone();
        assert_eq!(
            before.as_ref().map(Arc::as_ptr),
            after.as_ref().map(Arc::as_ptr),
            "a stale wake created or replaced the occupant's frame"
        );
        if free {
            let frame = after.expect("running B has a frame");
            assert!(
                frame.quiet(true),
                "a stale wake latched on the running occupant"
            );
            // The live key does reach it: the observation is sensitive.
            crate::pool::free::schedule_saved(run, new, Owner::Any);
            assert!(!frame.quiet(true));
        }
        drop(before);
        role::release(ctx);
        probe.go.store(true, SeqCst);
        join(ctx, &scope, new);
        tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
        // Each task was activated once: the stale wakes started nothing.
        assert_eq!(probe.entered.load(SeqCst), 2);
        crate::STATUS_OK
    }
}

#[test]
fn a_late_free_wake_with_a_reused_slots_old_key_is_dropped() {
    for width in [2, 4] {
        let environment = layout(16);
        let layouts = [&raw const environment];
        let probe = Arc::new(Probe::default());
        let mut run = crate::host::BodyRun::new(width, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let inner = probe.clone();
        let outcome = run.run(
            |ctx| unsafe { crate::tasks::free::enable(ctx) },
            move |ctx| unsafe { body(ctx, &inner, u32::MAX, true) },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK), "width {width}");
        assert_eq!(outcome.pending, 0);
    }
}

#[test]
fn a_late_ordered_wake_with_a_reused_slots_old_key_is_dropped() {
    for width in [1, 2, 4] {
        let environment = layout(16);
        let result = layout(16);
        let layouts = [&raw const environment, &raw const result];
        let probe = Arc::new(Probe::default());
        let mut run = crate::host::BodyRun::new(width, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 2;
        unsafe { run.use_physical_dispatch() };
        let inner = probe.clone();
        let outcome = run.run(
            |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
                events::enable(ctx);
                events::install_value(
                    ctx,
                    RESULT,
                    Program::new(
                        vec![
                            Node::Variant {
                                extent: 16,
                                tag: Tag::Ordinal { offset: 0 },
                                arms: vec![vec![], vec![Field { offset: 8, node: 1 }]],
                            },
                            Node::Bits(8),
                        ],
                        0,
                    )
                    .unwrap(),
                );
            },
            move |ctx| unsafe { body(ctx, &inner, RESULT, false) },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK), "width {width}");
        assert_eq!(outcome.pending, 0);
    }
}
