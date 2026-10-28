//! Spawn optimizations are physical choices, never replay instructions.
//! Like Tokio's task harness (runtime/task/harness.rs:160-229 at ce685ee6),
//! running ownership excludes publication. Here that ownership can temporarily
//! describe a child whose continuation is nested on its parent's native stack.
use super::super::*;

/// Compiler proof: the next operation on the normal path awaits this child.
pub(crate) const IMMEDIATE: u64 = 1;
/// Compiler proof: the user closure has no operations, including destruction;
/// its result needs no destruction. Administrative TaskFinish is excluded.
pub(crate) const OP_FREE: u64 = 2;

pub(super) fn allowed(ctx: *mut TaskCtx, flags: u64) -> bool {
    !super::replay::on(ctx)
        && flags & (IMMEDIATE | OP_FREE) != 0
        && role::with_role(ctx, |s| s.task(unsafe { (*ctx).task_id }).owner) == Owner::Any
        && (flags & IMMEDIATE != 0 || !super::record::on(ctx))
        // Replay always retains independent fibers; recorded immediate children
        // retain per-task logs while borrowing a fiber until replay.
        && unsafe { (*ctx).run_ref().free.is_none() }
}

/// A promised adjacent await did not happen (for example failure cleanup).
/// Materialize and publish before the next op, with the original identity.
/// # Safety
/// The current task holds the role, before any operation's effect.
pub(super) unsafe fn flush(ctx: *mut TaskCtx) {
    let Some(child) = (unsafe { (*ctx).free_ops.deferred.take() }) else {
        return;
    };
    role::admit(ctx);
    let stack = unsafe { (*ctx).worker().idle_stacks.take() };
    let target = stack.context();
    let key = unsafe {
        role::with_held(ctx, |s| {
            let key = s.key(child);
            let row = s.task(child);
            let owned = row
                .inline_context
                .take()
                .expect("unpublished child context");
            target.write((*owned).into_inner());
            (*target).task_sp =
                stack.prepare(task_entry, target.cast(), key.word() as usize as *mut u8);
            row.ctx = target;
            row.stack = Some(stack);
            key
        })
    };
    unsafe { crate::pool::free::schedule_saved((*ctx).run_ptr(), key, Owner::Any) };
}

/// # Safety
/// The role owns this unpublished child. The parent's stack and context remain
/// alive until this nested call returns, including across child migrations.
pub(super) unsafe fn run(
    ctx: *mut TaskCtx,
    child: u32,
    result: Option<(*mut u8, u64)>,
) -> Stopped<()> {
    let (parent, parent_key, child_key, target, code, environment, site) =
        role::with_role(ctx, |s| {
            let parent = s.current;
            let parent_key = s.key(parent);
            let child_key = s.key(child);
            let frame = s.task(parent).dispatch.clone();
            s.task(parent).state = State::Parked;
            if let Some((slot, size)) = result {
                s.task(parent).wait = Some(Wait::Join { child, slot, size });
            }
            let row = s.task(child);
            row.inline_result = result;
            #[cfg(feature = "ownership-trace")]
            {
                row.ownership_inline_parent = Some(parent);
            }
            row.inline_running = true;
            row.dispatch = frame;
            let (code, environment, site) = row.code.take().expect("an unstarted fast child");
            (
                parent,
                parent_key,
                child_key,
                row.ctx,
                code,
                environment,
                site,
            )
        });
    // No runtime borrow crosses user code. The physical image changes before
    // the child can suspend; a saved child never publishes the parent twice.
    unsafe {
        (*ctx).switch_out();
        role::release(ctx);
        crate::pool::free::lend(target, child_key);
        TaskCtx::switch_in(target, (*ctx).worker_ptr());
        role::enter(target);
        (*target).role_depth = 1;
    }
    let status = unsafe {
        let _segment = role::Segment::new(target);
        let mut status = code(target, environment, site, site);
        status |=
            crate::release::release_block(&mut *target, environment, std::ptr::null(), site, site);
        status
    };
    // A cut during the child's end abandons the child's running image, never
    // the parent's currently unbound context below it on the lent stack.
    unsafe { crate::stop::check(target, super::lifecycle::completion(target, child, status)) };
    role::with_role(target, |s| {
        let row = s.task(child);
        row.finishing = false;
        row.inline_running = false;
        #[cfg(feature = "ownership-trace")]
        {
            row.ownership_inline_parent = None;
        }
        row.state = State::Done;
        row.dispatch = None;
    });
    // The child may have migrated; restore onto its actual worker, not the
    // worker on which the parent originally entered this optimization.
    unsafe {
        (*target).role_depth = 0;
        let worker = (*target).worker_ptr();
        (*target).switch_out();
        role::release(target);
        crate::pool::free::lend(ctx, parent_key);
        TaskCtx::switch_in(ctx, worker);
        role::enter(ctx);
    }
    role::with_role(ctx, |s| {
        let row = s.task(parent);
        row.wait = None;
        row.state = State::Ready;
    });
    Ok(())
}

/// TaskFinish while nested in a lent fiber. The wrapper's ordinary cleanup
/// still executes on the child's context, with the result moved to its owner.
/// # Safety
/// The live result and none are size-byte Option images.
pub(super) unsafe fn finish(
    ctx: *mut TaskCtx,
    slot: *mut u8,
    none: *const u8,
    size: u64,
) -> Stopped<bool> {
    let (me, inline) = role::with_role(ctx, |s| {
        let me = s.current;
        (me, s.task(me).inline_running)
    });
    if !inline {
        return Ok(false);
    }
    // The physical stack loan does not remove TaskFinish's result-ready op.
    // Replay uses independent fibers and must reach this same child ordinal
    // before its distinct cleanup/completion op.
    super::record::end(ctx, || unsafe {
        role::with_held(ctx, |s| {
            s.task(me).ended = true;
            s.order(me);
        })
    })?;
    #[cfg(feature = "ownership-trace")]
    let mut delivered_parent = None;
    unsafe {
        role::with_held(ctx, |s| {
            let row = s.task(me);
            if let Some((out, expected)) = row.inline_result.take() {
                assert_eq!(expected, size);
                std::ptr::copy_nonoverlapping(slot, out, size as usize);
                #[cfg(feature = "ownership-trace")]
                {
                    delivered_parent = row.ownership_inline_parent;
                    if delivered_parent.is_none()
                        && let Some(held) = &row.ownership_held_result
                    {
                        held.record.lost();
                    }
                }
            } else {
                // Only the OP_FREE proof permits this branch: no result finalizer
                // or root is lost when a scope discards the raw image.
                let size = usize::try_from(size).expect("result image fits address space");
                let mut image = vec![0u128; size.div_ceil(16)];
                std::ptr::copy_nonoverlapping(slot, image.as_mut_ptr().cast(), size);
                row.inline_value = Some((image, size));
                #[cfg(feature = "ownership-trace")]
                if let Some(held) = &row.ownership_held_result {
                    held.record.lost();
                }
            }
            cpu_job_event(row, "transfer");
        })
    };
    unsafe { std::ptr::copy_nonoverlapping(none, slot, size as usize) };
    #[cfg(feature = "ownership-trace")]
    if let Some(parent) = delivered_parent {
        unsafe { role::with_held(ctx, |s| ownership_result_transferred(s, parent, me)) };
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    const NONE: [u64; 2] = [0; 2];

    unsafe extern "C" fn child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
        let mode = unsafe { env.add(1).cast::<u64>().read() };
        assert_ne!(unsafe { (*ctx).task_id }, 0);
        if mode == 1 {
            // Exhaust the cooperative op budget so the lent fiber actually saves.
            for _ in 0..260 {
                unsafe { yield_now(&mut *ctx) };
            }
        }
        let mut result = [1u64, 42];
        unsafe {
            hirsel_rt_v2_task_finish(
                ctx,
                result.as_mut_ptr().cast(),
                NONE.as_ptr().cast(),
                16,
                u32::MAX,
            )
        };
        assert_eq!(result, NONE, "the child cleans up a moved result");
        STATUS_OK
    }

    fn layout() -> crate::Layout {
        crate::Layout {
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
        }
    }

    fn exercise(flags: u64, fallback: bool, recording: bool) {
        let descriptor = layout();
        let layouts = [&descriptor as *const crate::Layout];
        let mut run = crate::host::BodyRun::new(2, true);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let recorder =
            recording.then(|| super::super::record::Recording::new(b"spawn fast", false));
        let install = recorder.clone();
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                if let Some(install) = install {
                    install.root(ctx);
                }
            },
            move |ctx| unsafe {
                let mut scope = 0;
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                for _ in 0..8 {
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    env.add(1)
                        .cast::<u64>()
                        .write(u64::from(flags & OP_FREE == 0));
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
                            env,
                            flags
                        ),
                        STATUS_OK
                    );
                    if flags == IMMEDIATE && !fallback {
                        let _role = role::Guard::new(ctx);
                        let s = scheduler(ctx);
                        assert_eq!((*ctx).free_ops.deferred, Some(task as u32));
                        assert!(s.task(task as u32).stack.is_none());
                    }
                    if fallback {
                        // Any other op materializes before acting; preserves identity.
                        let mut canceled = 99;
                        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
                        assert_eq!(canceled, 0);
                    }
                    let mut slot = NONE;
                    assert_eq!(
                        hirsel_rt_v2_task_join(
                            ctx,
                            0,
                            0,
                            &scope,
                            &mut task,
                            slot.as_mut_ptr().cast(),
                            16
                        ),
                        STATUS_OK
                    );
                    assert_eq!(slot, [1, 42]);
                }
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        if let Some(recorder) = recorder {
            recorder.recorder.stop("returned", None).unwrap().unwrap();
        }
    }

    #[test]
    fn immediate_children_lend_save_and_migrate_the_parent_fiber() {
        exercise(IMMEDIATE, false, false);
    }
    #[test]
    fn an_intervening_operation_materializes_the_deferred_child() {
        exercise(IMMEDIATE, true, false);
    }
    #[test]
    fn op_free_children_return_owned_result_images_without_stacks() {
        exercise(OP_FREE, false, false);
    }
    #[test]
    fn immediate_recorded_children_keep_their_own_logs() {
        exercise(IMMEDIATE, false, true);
    }
}

#[cfg(test)]
mod store_load_gate {
    use super::*;
    use hirsel_free::projection::{Event, Observed, Projection};
    use hirsel_free::replay::Replayer;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct Shared {
        loaded: AtomicBool,
    }

    unsafe extern "C" fn store(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
        let (word, shared) = unsafe { env.add(1).cast::<(u64, Arc<Shared>)>().read() };
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !shared.loaded.load(Ordering::Acquire) {
            assert!(
                std::time::Instant::now() < until,
                "immediate C must pass runnable B"
            );
            std::thread::yield_now();
        }
        drop(shared);
        unsafe { hirsel_rt_v2_atomic_store(ctx, &word, (&1u64 as *const u64).cast(), 8) }
    }
    unsafe extern "C" fn load(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
        let (word, shared) = unsafe { env.add(1).cast::<(u64, Arc<Shared>)>().read() };
        let mut value = 99u64;
        unsafe { hirsel_rt_v2_atomic_load(ctx, &word, (&raw mut value).cast(), 8) };
        assert_eq!(value, 0);
        shared.loaded.store(true, Ordering::Release);
        drop(shared);
        let mut slot = [1u64, value];
        let none = [0u64; 2];
        unsafe {
            hirsel_rt_v2_task_finish(
                ctx,
                slot.as_mut_ptr().cast(),
                none.as_ptr().cast(),
                16,
                u32::MAX,
            )
        }
    }

    #[test]
    fn recorded_immediate_child_load_passes_a_runnable_sibling_and_replays_zero() {
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
        let recording = super::super::record::Recording::new(b"L2 store/load", false);
        let install = recording.clone();
        let mut run = crate::host::BodyRun::new(2, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let mut scope = 0;
                let mut word = 0;
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                hirsel_rt_v2_atomic_create(
                    ctx,
                    std::ptr::null_mut(),
                    &mut word,
                    (&0u64 as *const u64).cast(),
                    8,
                    0,
                    8,
                );
                let shared = Arc::new(Shared {
                    loaded: AtomicBool::new(false),
                });
                let mut handles = Vec::new();
                for (code, flags) in [(store as TaskCode, 0), (load as TaskCode, IMMEDIATE)] {
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        16,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    env.add(1)
                        .cast::<(u64, Arc<Shared>)>()
                        .write((word, shared.clone()));
                    let mut task = 0;
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut task,
                        &scope,
                        code,
                        env,
                        flags,
                    );
                    handles.push(task);
                }
                let mut slot = [0u64; 2];
                hirsel_rt_v2_task_join(
                    ctx,
                    0,
                    0,
                    &scope,
                    &mut handles[1],
                    slot.as_mut_ptr().cast(),
                    16,
                );
                assert_eq!(slot, [1, 0]);
                slot = [0; 2];
                hirsel_rt_v2_task_join(
                    ctx,
                    0,
                    0,
                    &scope,
                    &mut handles[0],
                    slot.as_mut_ptr().cast(),
                    16,
                );
                hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        recording.recorder.stop("returned", None).unwrap().unwrap();
        let trace = recording.trace();
        let marked = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink();
        let tape = hirsel_free::tape::parse(&marked.bytes).unwrap();
        let mut replay = Replayer::new(&tape).unwrap();
        let loads: Vec<_> = trace
            .iter()
            .filter_map(|event| match event.event {
                Event::Observed(Observed::Load(value)) => {
                    Some((event.task.clone(), event.progress.op, value))
                }
                _ => None,
            })
            .collect();
        assert_eq!(loads.len(), 1);
        for (task, ordinal, value) in loads {
            assert_eq!(task, [1]);
            assert_eq!(value, 0);
            assert_eq!(replay.load(&task, ordinal).unwrap(), value);
            replay
                .publish(
                    &task,
                    hirsel_free::phase::Progress::new(ordinal, Phase::Done),
                )
                .unwrap();
        }
        assert_eq!(
            replay.finish(&tape.cut).unwrap(),
            Projection::new("returned", &tape.cut, &trace)
        );
    }
}

#[cfg(test)]
mod benchmark {
    use super::*;
}

#[cfg(test)]
mod cut_tests {
    use super::*;
    use std::sync::{Arc, Mutex, mpsc};

    struct Handshake {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    unsafe extern "C" fn cut_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
        let handshake = unsafe { &*env.add(1).cast::<*const Handshake>().read() };
        // Save a started lent fiber before the cutter races its next boundary.
        for _ in 0..260 {
            unsafe { yield_now(&mut *ctx) };
        }
        handshake.entered.send(()).unwrap();
        handshake.release.lock().unwrap().recv().unwrap();
        let mut canceled = 0;
        unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) };
        panic!("a cut child must not return into its unbound parent")
    }

    #[test]
    fn a_cutter_and_host_stop_leave_the_suspended_immediate_child_image() {
        let (entered, wait) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let handshake = Arc::new(Handshake {
            entered,
            release: Mutex::new(resume),
        });
        let recording = super::super::record::Recording::new(b"L2 borrowed cut", false);
        let install = recording.clone();
        let cutter = recording.clone();
        let layout = crate::Layout {
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
        let layouts = [&layout as *const crate::Layout];
        let mut run = crate::host::BodyRun::new(2, true);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let control = run.control();
        let stop = std::thread::spawn(move || {
            wait.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            cutter.recorder.flush().unwrap().unwrap();
            let cut = cutter.recorder.stop("cut", None).unwrap().unwrap().0;
            assert!(cut.progress.contains_key(&vec![0]));
            control.stop();
            release.send(()).unwrap();
        });
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let mut scope = 0;
                let mut task = 0;
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                env.add(1)
                    .cast::<*const Handshake>()
                    .write(Arc::as_ptr(&handshake));
                hirsel_rt_v2_task_spawn(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut task,
                    &scope,
                    cut_child,
                    env,
                    IMMEDIATE,
                );
                let mut slot = [0u64; 2];
                hirsel_rt_v2_task_join(ctx, 0, 0, &scope, &mut task, slot.as_mut_ptr().cast(), 16);
                panic!("stopped join must not resume")
            },
        );
        stop.join().unwrap();
        assert!(
            matches!(outcome.ended, Err(crate::stop::Stop::Requested)),
            "{outcome:?}"
        );
    }
}

#[cfg(test)]
#[test]
fn spawn_flag_bits_match_the_compiler_abi() {
    assert_eq!(IMMEDIATE, hirsel_lir::runtime_abi::SPAWN_IMMEDIATE_AWAIT);
    assert_eq!(OP_FREE, hirsel_lir::runtime_abi::SPAWN_OP_FREE);
}
