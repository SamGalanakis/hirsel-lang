//! The cooperative scheduler: the interpreter's queue order, result delivery through `Option`
//! slots, discards at scope exit, child failures and stack reuse. Each child is a Rust stand-in
//! for compiled `$native.task_main` code whose environment is a leaf block holding its ID.

use crate::support;

use std::cell::RefCell;
use std::mem::MaybeUninit;

use hirsel_rt::failure::{hirsel_rt_v2_failure_begin, hirsel_rt_v2_failure_primary};
use hirsel_rt::tasks::{
    hirsel_rt_v2_task_cancel, hirsel_rt_v2_task_detach, hirsel_rt_v2_task_finish,
    hirsel_rt_v2_task_join, hirsel_rt_v2_task_scope_enter, hirsel_rt_v2_task_scope_exit,
    hirsel_rt_v2_task_spawn, yield_now,
};
use hirsel_rt::{FailureRecord, Header, TaskCtx};
use support::*;

thread_local! { static TRACE: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) }; }

fn trace(event: u64) {
    TRACE.with(|t| t.borrow_mut().push(event));
}
fn take_trace() -> Vec<u64> {
    TRACE.with(|t| std::mem::take(&mut *t.borrow_mut()))
}

const NONE: [u64; 2] = [0, 0];

/// A child's environment: one word, its ID.
fn environment() -> &'static hirsel_rt::Layout {
    cached_layout("task-environment", 0, || leaf(8))
}
const TASKS: u64 = 7;

/// `Some(id * 100)` after one yield: traces `id1`, `id2` around the yield and `id3` with the
/// slot's tag once its result left (`id30` delivered, `id31` dropped).
unsafe extern "C" fn yielding(
    ctx: *mut TaskCtx,
    environment: *mut Header,
    _caller_site: u64,
    _operation_site: u64,
) -> u8 {
    let id = read_u64(environment, 0);
    trace(id * 10 + 1);
    unsafe { yield_now(&mut *ctx) };
    trace(id * 10 + 2);
    let mut slot = [1u64, id * 100];
    assert_eq!(
        unsafe {
            hirsel_rt_v2_task_finish(
                ctx,
                slot.as_mut_ptr().cast(),
                NONE.as_ptr().cast(),
                16,
                u32::MAX,
            )
        },
        0
    );
    trace(id * 100 + 30 + slot[0]);
    0
}

/// Fails with rule `id` at once.
unsafe extern "C" fn failing(
    ctx: *mut TaskCtx,
    environment: *mut Header,
    _caller_site: u64,
    _operation_site: u64,
) -> u8 {
    let id = read_u64(environment, 0);
    trace(id * 10 + 1);
    unsafe { hirsel_rt_v2_failure_begin(ctx, 0, id as u32, id, id) }
}

fn spawn(ctx: *mut TaskCtx, scope: u64, code: hirsel_rt::tasks::TaskCode, id: u64) -> u64 {
    let environment = alloc_block(ctx, environment(), 8);
    write_u64(environment, 0, id);
    let mut task = 0;
    assert_eq!(
        unsafe {
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
        },
        0
    );
    task
}

fn join(ctx: *mut TaskCtx, scope: u64, mut task: u64) -> (u8, [u64; 2]) {
    let mut slot = NONE;
    let status = unsafe {
        hirsel_rt_v2_task_join(ctx, 0, 0, &scope, &mut task, slot.as_mut_ptr().cast(), 16)
    };
    (status, slot)
}

fn enter(ctx: *mut TaskCtx) -> u64 {
    let mut scope = 0;
    assert_eq!(
        unsafe { hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &TASKS) },
        0
    );
    scope
}

fn exit(ctx: *mut TaskCtx, scope: u64) -> u8 {
    unsafe { hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0) }
}

fn primary(ctx: *mut TaskCtx) -> Option<FailureRecord> {
    let mut record = MaybeUninit::uninit();
    (unsafe { hirsel_rt_v2_failure_primary(ctx, record.as_mut_ptr()) } == 1)
        .then(|| unsafe { record.assume_init() })
}

#[test]
fn children_run_in_the_interpreters_queue_order_and_joins_move_their_results() {
    let ctx = Ctx::diagnostic();
    index(ctx.0, environment());
    let scope = enter(ctx.0);
    let one = spawn(ctx.0, scope, yielding, 1);
    let two = spawn(ctx.0, scope, yielding, 2);
    trace(90);
    assert_eq!(join(ctx.0, scope, one), (0, [1, 100]));
    trace(91);
    assert_eq!(join(ctx.0, scope, two), (0, [1, 200]));
    assert_eq!(exit(ctx.0, scope), 0);
    // The first spawn's yield starts its child, which yields back. The second spawn's yield
    // resumes the first child, which completes holding its result, then starts the second.
    // Each join hands the result over and lets the child finish before the joiner continues.
    assert_eq!(take_trace(), [11, 12, 21, 90, 130, 22, 91, 230]);
    assert_eq!(ctx.stats().live_blocks, 0, "every environment was released");
}

#[test]
fn a_join_waiting_for_a_running_child_receives_its_result_at_completion() {
    let ctx = Ctx::diagnostic();
    index(ctx.0, environment());
    let scope = enter(ctx.0);
    let one = spawn(ctx.0, scope, yielding, 1);
    // The child yielded back before finishing, so this join parks.
    assert_eq!(join(ctx.0, scope, one), (0, [1, 100]));
    assert_eq!(exit(ctx.0, scope), 0);
    assert_eq!(take_trace(), [11, 12, 130]);
}

#[test]
fn scope_exit_drops_unjoined_results_last_spawned_first() {
    let ctx = Ctx::diagnostic();
    index(ctx.0, environment());
    let scope = enter(ctx.0);
    spawn(ctx.0, scope, yielding, 1);
    spawn(ctx.0, scope, yielding, 2);
    trace(90);
    assert_eq!(exit(ctx.0, scope), 0);
    assert_eq!(take_trace(), [11, 12, 21, 90, 22, 231, 131]);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn a_detached_child_drops_its_own_result_and_a_completed_one_drops_it_at_once() {
    let ctx = Ctx::diagnostic();
    index(ctx.0, environment());
    let scope = enter(ctx.0);
    let mut one = spawn(ctx.0, scope, yielding, 1);
    // Child 1 yielded back: still running when detached.
    assert_eq!(
        unsafe { hirsel_rt_v2_task_detach(ctx.0, &scope, &mut one) },
        0
    );
    let mut two = spawn(ctx.0, scope, yielding, 2);
    trace(90);
    assert_eq!(exit(ctx.0, scope), 0);
    let _ = &mut two;
    assert_eq!(take_trace(), [11, 12, 131, 21, 90, 22, 231]);
}

#[test]
fn a_failed_child_fails_its_join_and_cancels_its_siblings() {
    let ctx = Ctx::diagnostic();
    index(ctx.0, environment());
    let scope = enter(ctx.0);
    let failed = spawn(ctx.0, scope, failing, 5);
    let sibling = spawn(ctx.0, scope, yielding, 6);
    let (status, slot) = join(ctx.0, scope, failed);
    assert_eq!((status, slot), (1, NONE));
    assert_eq!(primary(ctx.0).map(|r| r.rule), Some(5));
    // The failing scope exit cancels and joins the sibling and drops its result.
    assert_eq!(exit(ctx.0, scope), 1);
    assert_eq!(take_trace(), [51, 61, 62, 631]);
    assert_eq!(primary(ctx.0).map(|r| r.rule), Some(5));
    let _ = sibling;
}

#[test]
fn an_unjoined_failure_fails_the_scope_exit() {
    let ctx = Ctx::diagnostic();
    index(ctx.0, environment());
    let scope = enter(ctx.0);
    let other = spawn(ctx.0, scope, yielding, 1);
    spawn(ctx.0, scope, failing, 2);
    assert_eq!(join(ctx.0, scope, other), (0, [1, 100]));
    assert_eq!(exit(ctx.0, scope), 1);
    assert_eq!(primary(ctx.0).map(|r| r.rule), Some(2));
}

#[test]
fn cancel_names_a_child_of_its_scope_and_stacks_are_reused() {
    let ctx = Ctx::diagnostic();
    index(ctx.0, environment());
    for round in 0..64 {
        let scope = enter(ctx.0);
        let child = spawn(ctx.0, scope, yielding, round);
        assert_eq!(
            unsafe { hirsel_rt_v2_task_cancel(ctx.0, &scope, &child) },
            0
        );
        assert_eq!(join(ctx.0, scope, child), (0, [1, round * 100]));
        assert_eq!(exit(ctx.0, scope), 0);
    }
    take_trace();
    assert_eq!(ctx.stats().live_blocks, 0);
}

// The host is deliberately stalled until all three timers are parked. Advancing
// its clock is the blocking step before the first poll at which any timer is due.
#[test]
fn overdue_timers_resume_by_deadline_with_suspension_order_for_ties() {
    use hirsel_rt::io::{Adapter, AdapterError, Binding, Completion, Ready, Request, UnitResult};
    use std::{cell::Cell, rc::Rc};
    struct Clock(Rc<Cell<u64>>);
    impl Adapter for Clock {
        fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
            assert!(matches!(request, Request::SleepUntil { .. }));
            Ok(None)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            let Request::SleepUntil { deadline, .. } = request else {
                panic!("unexpected request: {request:?}")
            };
            if deadline <= self.0.get() {
                Ok(Completion::Wait(Ok(())))
            } else {
                Err(AdapterError::Blocked)
            }
        }
        fn ready(&mut self) -> Result<Ready, AdapterError> {
            Ok(if self.0.get() == 100 {
                Ready::Every
            } else {
                Ready::Tokens(vec![])
            })
        }
    }
    unsafe extern "C" fn sleeper(
        ctx: *mut TaskCtx,
        env: *mut Header,
        _caller_site: u64,
        _operation_site: u64,
    ) -> u8 {
        let id = read_u64(env, 0);
        let deadline = if id == 1 { 30 } else { 10 };
        let mut result = MaybeUninit::<UnitResult>::uninit();
        assert_eq!(
            unsafe {
                hirsel_rt::io::hirsel_rt_v2_clock_sleep_until(
                    ctx,
                    std::ptr::null_mut(),
                    result.as_mut_ptr(),
                    &2,
                    &deadline,
                )
            },
            0
        );
        assert_eq!(unsafe { result.assume_init() }.tag, 0);
        trace(id);
        let mut slot = [1u64, id];
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
    let ctx = Ctx::diagnostic();
    index(ctx.0, environment());
    let now = Rc::new(Cell::new(0));
    let mut binding = Box::new(Binding::new(Clock(now.clone())));
    unsafe { binding.bind(&mut *ctx.0) };
    let scope = enter(ctx.0);
    let late = spawn(ctx.0, scope, sleeper, 1);
    let early = spawn(ctx.0, scope, sleeper, 2);
    let tied = spawn(ctx.0, scope, sleeper, 3);
    // Let the last child reach its timer before making every deadline overdue.
    unsafe { yield_now(&mut *ctx.0) };
    assert!(take_trace().is_empty());
    now.set(100);
    assert_eq!(join(ctx.0, scope, late), (0, [1, 1]));
    assert_eq!(join(ctx.0, scope, early), (0, [1, 2]));
    assert_eq!(join(ctx.0, scope, tied), (0, [1, 3]));
    assert_eq!(exit(ctx.0, scope), 0);
    assert_eq!(take_trace(), [2, 3, 1]);
    unsafe { (*ctx.0).bind_io_data(std::ptr::null_mut()) };
    assert_eq!(ctx.stats().live_blocks, 0);
}

/// A counted wrapper's final drop closes the table cell. The Option descriptor
/// runs the resource finalizer only for Some, with its fields in descriptor order.
fn mutex_layouts(
    finalizer: hirsel_rt::DropThunk,
) -> (&'static hirsel_rt::Layout, &'static hirsel_rt::Layout) {
    let resource = cached_layout("mutex-resource", finalizer as usize as u64, || {
        layout(8, 1, Some(finalizer), Vec::new())
    });
    let image = cached_layout("mutex-option", finalizer as usize as u64, || {
        layout(
            16,
            1,
            None,
            vec![
                tag_step(0, 1, 2),
                finalize_step(0, 8, resource),
                end_finalize_step(0, 8, resource),
            ],
        )
    });
    let wrapper = cached_layout("mutex-wrapper", 0, || {
        let base = layout(8, 1, None, vec![step(hirsel_rt::STEP_CLOSE_HANDLE, 0, 0)]);
        leak(hirsel_rt::Layout {
            flags: hirsel_rt::LAYOUT_FROZEN | hirsel_rt::LAYOUT_MUTEX_ROOT,
            ..*base
        })
    });
    (image, wrapper)
}

fn mutex_create(ctx: &Ctx, image: &hirsel_rt::Layout, wrapper: &hirsel_rt::Layout, id: u64) -> u64 {
    let descriptor = index(ctx.0, image);
    let wrapper = index(ctx.0, wrapper);
    let mut payload = [1u64, id];
    let mut handle = 0;
    assert_eq!(
        unsafe {
            hirsel_rt::tasks::hirsel_rt_v2_mutex_create(
                ctx.0,
                std::ptr::null_mut(),
                &mut handle,
                payload.as_mut_ptr().cast(),
                NONE.as_ptr().cast(),
                descriptor,
                wrapper,
            )
        },
        0
    );
    assert_eq!(payload, NONE);
    handle
}

fn mutex_release(ctx: &Ctx, handle: u64) -> u8 {
    let block = handle as *mut Header;
    unsafe { hirsel_rt::roots::hirsel_rt_v2_root_drop(ctx.0, block, 0, 0) }
}

#[test]
fn mutex_images_move_without_allocations_and_finalize_at_last_handle() {
    let (image, wrapper) = mutex_layouts(log_drop);
    let ctx = Ctx::diagnostic();
    let handle = mutex_create(&ctx, image, wrapper, 41);
    unsafe { hirsel_rt::roots::hirsel_rt_v2_root_dup(ctx.0, handle as *mut Header) };
    take_events();
    assert_eq!(mutex_release(&ctx, handle), 0);
    assert!(
        take_events().is_empty(),
        "another handle still owns the cell"
    );
    let calls = host_calls();
    for _ in 0..1000 {
        let mut slot = NONE;
        let mut status = 99;
        assert_eq!(
            unsafe {
                hirsel_rt::tasks::hirsel_rt_v2_mutex_take(
                    ctx.0,
                    &mut status,
                    &handle,
                    slot.as_mut_ptr().cast(),
                )
            },
            0
        );
        assert_eq!((status, slot), (0, [1, 41]));
        assert_eq!(
            unsafe {
                hirsel_rt::tasks::hirsel_rt_v2_mutex_put(ctx.0, &handle, slot.as_mut_ptr().cast())
            },
            0
        );
        assert_eq!(slot, NONE);
    }
    assert_eq!(
        host_calls(),
        calls,
        "take/put must not allocate even host buffers"
    );
    assert_eq!(mutex_release(&ctx, handle), 0);
    assert_eq!(
        take_events().into_iter().map(|e| e.0).collect::<Vec<_>>(),
        [41]
    );
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn mutex_last_handle_propagates_payload_drop_failure_and_preserves_primary() {
    let (image, wrapper) = mutex_layouts(failing_drop);
    let ctx = Ctx::diagnostic();
    let handle = mutex_create(&ctx, image, wrapper, 42);
    unsafe { hirsel_rt_v2_failure_begin(ctx.0, 3, 900, 7, 7) };
    take_events();
    assert_eq!(mutex_release(&ctx, handle), 1);
    assert_eq!(
        take_events().into_iter().map(|e| e.0).collect::<Vec<_>>(),
        [42]
    );
    let mut primary = MaybeUninit::uninit();
    assert_eq!(
        unsafe { hirsel_rt_v2_failure_primary(ctx.0, primary.as_mut_ptr()) },
        1
    );
    assert_eq!(unsafe { primary.assume_init() }.rule, 900);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn mutex_kill_leaves_payload_with_the_body_and_last_close_does_not_drop_it_again() {
    let (image, wrapper) = mutex_layouts(log_drop);
    let ctx = Ctx::diagnostic();
    let handle = mutex_create(&ctx, image, wrapper, 43);
    let mut slot = NONE;
    let mut status = 99;
    assert_eq!(
        unsafe {
            hirsel_rt::tasks::hirsel_rt_v2_mutex_take(
                ctx.0,
                &mut status,
                &handle,
                slot.as_mut_ptr().cast(),
            )
        },
        0
    );
    assert_eq!(status, 0);
    unsafe { hirsel_rt_v2_failure_begin(ctx.0, 3, 901, 8, 8) };
    assert_eq!(
        unsafe { hirsel_rt::tasks::hirsel_rt_v2_mutex_kill(ctx.0, &handle) },
        0
    );
    let mut none = NONE;
    assert_eq!(
        unsafe {
            hirsel_rt::tasks::hirsel_rt_v2_mutex_take(
                ctx.0,
                &mut status,
                &handle,
                none.as_mut_ptr().cast(),
            )
        },
        0
    );
    assert_eq!((status, none), (1, NONE));
    take_events();
    let mut continuation = MaybeUninit::<hirsel_rt::Cleanup>::uninit();
    assert_eq!(
        unsafe {
            hirsel_rt::finalize::hirsel_rt_v2_finalize(
                ctx.0,
                index(ctx.0, image),
                slot.as_mut_ptr().cast(),
                std::ptr::null(),
                continuation.as_mut_ptr(),
                0,
                0,
            )
        },
        0
    );
    assert_eq!(mutex_release(&ctx, handle), 0);
    assert_eq!(
        take_events().into_iter().map(|e| e.0).collect::<Vec<_>>(),
        [43]
    );
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[cfg(feature = "ownership-trace")]
unsafe extern "C" fn ownership_child(ctx: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
    use hirsel_rt::ownership_trace::*;
    let frame = unsafe { hirsel_rt_v2_ownership_enter(ctx, ABI_VERSION, [7; 32].as_ptr()) };
    unsafe {
        hirsel_rt_v2_ownership_event(ctx, frame, 0, 0, 0, 0, 42, 7);
        yield_now(&mut *ctx);
        hirsel_rt_v2_ownership_event(ctx, frame, 1, 2, 0, 0, 43, 7);
        hirsel_rt_v2_ownership_leave(ctx, frame);
    }
    let mut slot = [1u64, 7u64];
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
#[cfg(feature = "ownership-trace")]
#[test]
fn ownership_runtime_child_paths_survive_yields_and_slot_reuse() {
    use hirsel_run_schema::{
        Site,
        ownership_trace::{TaskPath, native::*},
    };
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("tasks.json");
    let ctx = Ctx::diagnostic();
    index(ctx.0, environment());
    unsafe { hirsel_rt::ownership_trace::start(ctx.0, [49; 32], destination.clone(), 100) }
        .unwrap();
    let scope = enter(ctx.0);
    let first = spawn(ctx.0, scope, ownership_child, 0);
    let second = spawn(ctx.0, scope, ownership_child, 0);
    assert_eq!(join(ctx.0, scope, first), (0, [1, 7]));
    assert_eq!(join(ctx.0, scope, second), (0, [1, 7]));
    let third = spawn(ctx.0, scope, ownership_child, 0);
    assert!(
        third as u32 == first as u32 || third as u32 == second as u32,
        "retired child slot reused"
    );
    assert_eq!(join(ctx.0, scope, third), (0, [1, 7]));
    assert_eq!(exit(ctx.0, scope), 0);
    drop(ctx);
    let log: Log = serde_json::from_slice(&std::fs::read(destination).unwrap()).unwrap();
    let units = std::collections::BTreeMap::from([(
        [7; 32],
        (0..2)
            .map(|id| {
                (
                    id,
                    Box::new(Metadata {
                        action: if id == 0 { 0 } else { 2 },
                        channel: None,
                        empty_environment: false,
                        optional_carrier: false,
                        callee: None,
                        cleanup: None,
                        cleanup_paths: Default::default(),
                        additional_drops: vec![],
                        token: 0,
                        owner: 0,
                        token_path: None,
                        owner_path: None,
                        site: 42 + u64::from(id),
                        layout: 7,
                        source: Site {
                            definition: "runtime-child".into(),
                            file: "tasks.rs".into(),
                            line: 1,
                            column: 1,
                        },
                        value_type: "Token".into(),
                    }),
                )
            })
            .collect(),
    )]);
    let trace = decode_leaf(&log, [49; 32], &units, 100).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (3, 3, 0)
    );
    let paths = trace
        .events
        .iter()
        .map(|e| e.task.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        paths,
        [TaskPath(vec![0]), TaskPath(vec![1]), TaskPath(vec![2])].into()
    );
}
