//! A replay wait leaves its stack only from the first op its compact frame
//! began since a resume point whose operation can leave (`RESUME_LEAVES`,
//! `compact::can_wait`): a library body that made an effect before the op
//! keeps its stack, since reissuing the body from its boundary would repeat
//! that effect, and so does an op with no such point to reissue from (one
//! before the task's first resume point, or under `task.isolate`'s).
use super::*;
use std::sync::Mutex;

/// `can_wait` after each op the child begins since its resume point.
static PROBES: Mutex<Vec<bool>> = Mutex::new(Vec::new());
const NONE: [u64; 2] = [0, 0];

unsafe extern "C" fn dispatcher(
    ctx: *mut TaskCtx,
    _: *mut Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    unsafe {
        // Before any resume point: no boundary call to reissue.
        record::begin(ctx);
        PROBES.lock().unwrap().push(compact::can_wait(ctx));
        frame.cast::<u32>().write(1);
        // Free proceeds in place: nothing waits at this boundary.
        assert_eq!(
            crate::continuation::hirsel_rt_v2_resume_point(ctx, frame, 1 | crate::RESUME_LEAVES),
            0
        );
        // The op's own begin: its wait may leave from the saved frame.
        record::begin(ctx);
        PROBES.lock().unwrap().push(compact::can_wait(ctx));
        // An effect began before this one in the same body: reissuing the
        // body would repeat it, so the wait keeps its stack.
        record::begin(ctx);
        PROBES.lock().unwrap().push(compact::can_wait(ctx));
        // A boundary whose operation cannot leave (`task.isolate`'s runs its
        // callback first): the callback's first op keeps its stack.
        frame.cast::<u32>().write(2);
        assert_eq!(
            crate::continuation::hirsel_rt_v2_resume_point(ctx, frame, 2),
            0
        );
        record::begin(ctx);
        PROBES.lock().unwrap().push(compact::can_wait(ctx));
        let mut slot = [1_u64, 42];
        hirsel_rt_v2_task_finish(
            ctx,
            slot.as_mut_ptr().cast(),
            NONE.as_ptr().cast(),
            16,
            u32::MAX,
        )
    }
}

unsafe extern "C" fn child(
    ctx: *mut TaskCtx,
    environment: *mut Header,
    caller: u64,
    site: u64,
) -> u8 {
    unsafe {
        crate::continuation::hirsel_rt_v2_resume_run(ctx, environment, caller, site, dispatcher, 8)
    }
}

#[test]
fn a_replay_wait_leaves_only_as_the_first_op_since_its_resume_point() {
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
    let mut run = crate::host::BodyRun::new(2, false);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    run.free = true;
    // SAFETY: the Free policy owns every operation and saved-frame wake.
    unsafe { run.use_physical_dispatch() };
    let outcome = run.run(
        |ctx| unsafe { enable(ctx) },
        |ctx| unsafe {
            let mut scope = 0;
            assert_eq!(
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &7),
                crate::STATUS_OK
            );
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
            );
            let mut slot = NONE;
            assert_eq!(
                hirsel_rt_v2_task_join(ctx, 0, 0, &scope, &mut task, slot.as_mut_ptr().cast(), 16),
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
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(*PROBES.lock().unwrap(), [false, true, false, false]);
}
