//! Structured lifecycle through the task ABI, over per-slot generations and
//! save/lease-gated row reuse: unawaited children, detach, a failing
//! cleanup, completion ranks that predate a TaskSet, and failures ordered by
//! spawn path rather than by time. Children are Rust stand-ins for compiled
//! `$native.task_main` code whose environment holds their ID.

use crate::support;

use std::cell::RefCell;
use std::mem::MaybeUninit;

use hirsel_rt::failure::{hirsel_rt_v2_failure_begin, hirsel_rt_v2_failure_primary};
use hirsel_rt::tasks::{
    TaskCode, hirsel_rt_v2_task_add, hirsel_rt_v2_task_await_any, hirsel_rt_v2_task_detach,
    hirsel_rt_v2_task_finish, hirsel_rt_v2_task_join, hirsel_rt_v2_task_scope_enter,
    hirsel_rt_v2_task_scope_exit, hirsel_rt_v2_task_set, hirsel_rt_v2_task_set_take,
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
const TASKS: u64 = 7;

fn environment() -> &'static hirsel_rt::Layout {
    cached_layout("runtime50-environment", 0, || leaf(8))
}

/// Hold `Some(id * 100)`; returns the slot's tag after resumption: 0 when a
/// join took the result, 1 when it was discarded and this frame drops it.
unsafe fn finish(ctx: *mut TaskCtx, id: u64) -> u64 {
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
    slot[0]
}

/// Traces `id1`, yields, traces `id2`, then finishes.
unsafe extern "C" fn yielding(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let id = read_u64(env, 0);
    trace(id * 10 + 1);
    unsafe { yield_now(&mut *ctx) };
    trace(id * 10 + 2);
    unsafe { finish(ctx, id) };
    0
}

/// Finishes at once, without a yield of its own.
unsafe extern "C" fn quick(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let id = read_u64(env, 0);
    trace(id * 10 + 1);
    unsafe { finish(ctx, id) };
    0
}

/// Fails with rule `id` at once.
unsafe extern "C" fn failing(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let id = read_u64(env, 0);
    trace(id * 10 + 1);
    unsafe { hirsel_rt_v2_failure_begin(ctx, 0, id as u32, id, id) }
}

/// Yields three times, then fails with rule `id`: in queue order it fails
/// after a sibling spawned after it that fails at once.
unsafe extern "C" fn failing_late(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let id = read_u64(env, 0);
    trace(id * 10 + 1);
    for _ in 0..3 {
        unsafe { yield_now(&mut *ctx) };
    }
    trace(id * 10 + 2);
    unsafe { hirsel_rt_v2_failure_begin(ctx, 0, id as u32, id, id) }
}

/// Holds its result; when it is discarded, dropping it fails with rule `id`.
unsafe extern "C" fn cleanup_fails(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let id = read_u64(env, 0);
    trace(id * 10 + 1);
    if unsafe { finish(ctx, id) } == 1 {
        return unsafe { hirsel_rt_v2_failure_begin(ctx, 0, id as u32, id, id) };
    }
    0
}

fn spawn(ctx: *mut TaskCtx, scope: u64, code: TaskCode, id: u64) -> u64 {
    let env = alloc_block(ctx, environment(), 8);
    write_u64(env, 0, id);
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
            env,
            0,
        )
    };
    assert_eq!(status, 0);
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
    let status =
        unsafe { hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &TASKS) };
    assert_eq!(status, 0);
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

fn task_set(ctx: *mut TaskCtx, scope: u64) -> u64 {
    let mut set = 0;
    assert_eq!(
        unsafe { hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope) },
        0
    );
    set
}

fn add(ctx: *mut TaskCtx, scope: u64, mut set: u64, mut task: u64) {
    assert_eq!(
        unsafe { hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut task) },
        0
    );
}

/// Take member `index` of `set`: its Task word.
fn take(ctx: *mut TaskCtx, scope: u64, mut set: u64, index: u64) -> u64 {
    let mut task = 0;
    let status = unsafe {
        hirsel_rt_v2_task_set_take(
            ctx,
            std::ptr::null_mut(),
            &mut task,
            &scope,
            &mut set,
            index,
        )
    };
    assert_eq!(status, 0);
    task
}

/// `task.await_any`'s `Option[u64]` add-order index.
fn await_any(ctx: *mut TaskCtx, scope: u64, set: u64) -> Option<u64> {
    let mut out = [0u64; 2];
    let status = unsafe {
        hirsel_rt_v2_task_await_any(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            out.as_mut_ptr().cast(),
            &scope,
            &set,
        )
    };
    assert_eq!(status, 0);
    (out[0] as u32 == 1).then_some(out[1])
}

#[test]
fn runtime50_scope_cleanup_and_late_taskset() {
    // Unawaited children: scope exit drops their held results, last spawned
    // first, after both finish; their rows and environments are reclaimed.
    {
        let ctx = Ctx::diagnostic();
        index(ctx.0, environment());
        for round in 0..3 {
            let scope = enter(ctx.0);
            spawn(ctx.0, scope, yielding, 1);
            spawn(ctx.0, scope, yielding, 2);
            trace(90);
            assert_eq!(exit(ctx.0, scope), 0, "round {round}");
            assert_eq!(take_trace(), [11, 12, 21, 90, 22, 231, 131]);
        }
        assert_eq!(ctx.stats().live_blocks, 0);
    }
    // Detach: a running detached child drops its own result; the scope exit
    // still waits for it, and the reused row never receives a stale join.
    {
        let ctx = Ctx::diagnostic();
        index(ctx.0, environment());
        let scope = enter(ctx.0);
        let mut detached = spawn(ctx.0, scope, yielding, 3);
        assert_eq!(
            unsafe { hirsel_rt_v2_task_detach(ctx.0, &scope, &mut detached) },
            0
        );
        let joined = spawn(ctx.0, scope, yielding, 4);
        assert_ne!(joined, detached, "a live detached child keeps its identity");
        assert_eq!(join(ctx.0, scope, joined), (0, [1, 400]));
        assert_eq!(exit(ctx.0, scope), 0);
        assert_eq!(take_trace(), [31, 32, 331, 41, 42, 430]);
        assert_eq!(ctx.stats().live_blocks, 0);
    }
    // A failing cleanup: discarding the held result runs the child's own drop,
    // whose failure fails the scope exit.
    {
        let ctx = Ctx::diagnostic();
        index(ctx.0, environment());
        let scope = enter(ctx.0);
        spawn(ctx.0, scope, cleanup_fails, 5);
        spawn(ctx.0, scope, quick, 6);
        assert_eq!(exit(ctx.0, scope), 1);
        assert_eq!(primary(ctx.0).map(|r| r.rule), Some(5));
        assert_eq!(take_trace(), [51, 61, 631, 531]);
        assert_eq!(ctx.stats().live_blocks, 0);
    }
    // Completion ranks belong to the scope: children that completed before
    // the TaskSet existed, added in the reverse order, are awaited in
    // completion order; once every member is taken the set is empty.
    {
        let ctx = Ctx::diagnostic();
        index(ctx.0, environment());
        let scope = enter(ctx.0);
        let first = spawn(ctx.0, scope, quick, 7);
        let second = spawn(ctx.0, scope, quick, 8);
        assert_eq!(take_trace(), [71, 81]);
        let set = task_set(ctx.0, scope);
        add(ctx.0, scope, set, second);
        add(ctx.0, scope, set, first);
        assert_eq!(await_any(ctx.0, scope, set), Some(1));
        let earlier = take(ctx.0, scope, set, 1);
        assert_eq!(earlier, first);
        assert_eq!(join(ctx.0, scope, earlier), (0, [1, 700]));
        assert_eq!(await_any(ctx.0, scope, set), Some(0));
        let later = take(ctx.0, scope, set, 0);
        assert_eq!(later, second);
        assert_eq!(join(ctx.0, scope, later), (0, [1, 800]));
        assert_eq!(await_any(ctx.0, scope, set), None);
        assert_eq!(exit(ctx.0, scope), 0);
        assert_eq!(take_trace(), [730, 830]);
        assert_eq!(ctx.stats().live_blocks, 0);
    }
    // Spawn-ordered failures: the later sibling fails first in time, yet the
    // earlier spawn path leads the scope's chain (FAIL-2).
    {
        let ctx = Ctx::diagnostic();
        index(ctx.0, environment());
        let scope = enter(ctx.0);
        spawn(ctx.0, scope, failing_late, 1);
        spawn(ctx.0, scope, failing, 2);
        trace(90);
        assert_eq!(exit(ctx.0, scope), 1);
        assert_eq!(take_trace(), [11, 21, 90, 12]);
        assert_eq!(primary(ctx.0).map(|r| r.rule), Some(1));
    }
}
