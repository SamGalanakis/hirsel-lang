use super::*;
use crate::alloc::{DOMAIN_HEAP, hirsel_rt_v2_alloc};
use crate::context::{Config, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy};

fn step(kind: u32, offset: u64) -> Step {
    Step {
        kind,
        field: WHOLE_VALUE,
        variant: 0,
        span: 0,
        offset,
        stride: 0,
        count: 0,
        count_offset: NO_OFFSET,
        layout: ptr::null(),
    }
}
fn layout(size: u64, steps: &[Step]) -> Layout {
    Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size,
        align: 8,
        finalizer: None,
        steps: steps.as_ptr(),
        step_count: steps.len() as u64,
        field_count: 0,
        flags: 0,
        logical: 0,
        logical_count: ptr::null(),
        scan: ptr::null(),
        scan_words: u64::MAX,
    }
}
struct Context(*mut TaskCtx);
impl Context {
    fn new(layouts: &[&Layout]) -> Self {
        let table: Vec<_> = layouts.iter().map(|l| *l as *const Layout).collect();
        let mut ctx = ptr::null_mut();
        let config = Config {
            layouts: table.as_ptr(),
            layout_count: table.len() as u32,
            ..Config::new()
        };
        assert_eq!(unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) }, 0);
        Self(ctx)
    }
    fn alloc(&self, descriptor: u32, size: u64) -> *mut Header {
        unsafe { hirsel_rt_v2_alloc(self.0, descriptor, size, 8, DOMAIN_HEAP, 0) }
    }
    fn live(&self) -> u64 {
        unsafe { (*self.0).stats().live_blocks }
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        unsafe { hirsel_rt_v2_ctx_destroy(self.0) };
    }
}

#[test]
fn good_close_poll_retains_all_reversal_ancestors_loop_pc_and_failure_suffix() {
    let parent_steps = [step(STEP_RELEASE, 0), step(STEP_CLOSE_HANDLE, 8)];
    let loop_step = Step {
        span: 1,
        stride: 8,
        count: 3,
        ..step(STEP_LOOP, 0)
    };
    let leaf_steps = [
        loop_step,
        step(STEP_CLOSE_HANDLE, 0),
        step(STEP_RELEASE_LAST, 24),
    ];
    let parent_layout = layout(16, &parent_steps);
    let leaf_layout = Layout {
        flags: LAYOUT_WIDE_CURSOR,
        ..layout(32, &leaf_steps)
    };
    let plain = layout(8, &[]);
    let ctx = Context::new(&[&parent_layout, &leaf_layout, &plain]);
    unsafe {
        let root = ctx.alloc(2, 16);
        let middle = ctx.alloc(2, 16);
        let leaf = ctx.alloc(3, 32);
        let resource = ctx.alloc(4, 8);
        root.add(1).cast::<u64>().write(middle as u64);
        root.add(1).cast::<u64>().add(1).write(9);
        middle.add(1).cast::<u64>().write(leaf as u64);
        middle.add(1).cast::<u64>().add(1).write(8);
        for (i, value) in [1, 2, 3, resource as u64].into_iter().enumerate() {
            leaf.add(1).cast::<u64>().add(i).write(value);
        }
        push(&mut *ctx.0, root, true);
        let mut cursor = Box::new(Cursor::new(
            ptr::null_mut(),
            root,
            ptr::null(),
            false,
            31,
            41,
        ));
        let mut order = Vec::new();
        loop {
            match cursor.poll(&mut *ctx.0) {
                ReleasePoll::Close(handle) => {
                    order.push(handle);
                    let before = ctx.live();
                    // A duplicate poll cannot run a close or decrement again.
                    assert!(
                        matches!(cursor.poll(&mut *ctx.0), ReleasePoll::Close(h) if h == handle)
                    );
                    assert_eq!(ctx.live(), before);
                    if handle <= 3 {
                        assert_eq!(cursor.parent, middle);
                        assert_eq!(cursor.root, root);
                    }
                    if handle == 3 {
                        crate::failure::hirsel_rt_v2_failure_begin(ctx.0, 1, 17, 31, 41);
                        cursor.closed(STATUS_FAILURE);
                    } else {
                        cursor.closed(STATUS_OK);
                    }
                }
                ReleasePoll::Complete(result) => {
                    assert_eq!(result.status, STATUS_FAILURE);
                    assert!(!result.kept_root);
                    break;
                }
            }
        }
        assert_eq!(order, [3, 2, 1, 8, 9]);
        assert_eq!(
            ctx.live(),
            0,
            "all children/siblings are released after failure"
        );
        assert!((*ctx.0).work.is_null());
        crate::failure::hirsel_rt_v2_failure_clear(ctx.0);
    }
}

#[test]
fn good_tail_free_root_is_cleared_before_its_child_can_suspend() {
    let parent_steps = [step(STEP_RELEASE_LAST, 0)];
    let child_steps = [step(STEP_CLOSE_HANDLE, 0)];
    let parent_layout = layout(8, &parent_steps);
    let child_layout = layout(8, &child_steps);
    let ctx = Context::new(&[&parent_layout, &child_layout]);
    unsafe {
        let root = ctx.alloc(2, 8);
        let child = ctx.alloc(3, 8);
        root.add(1).cast::<u64>().write(child as u64);
        child.add(1).cast::<u64>().write(7);
        push(&mut *ctx.0, root, true);
        let mut cursor = Cursor::new(ptr::null_mut(), root, ptr::null(), false, 31, 41);
        assert!(matches!(cursor.poll(&mut *ctx.0), ReleasePoll::Close(7)));
        assert!(
            cursor.root.is_null(),
            "no saved cursor retains a freed root"
        );
        assert!(cursor.parent.is_null());
        assert_eq!(ctx.live(), 1);
        cursor.closed(STATUS_OK);
        assert!(matches!(
            cursor.poll(&mut *ctx.0),
            ReleasePoll::Complete(Drained {
                status: STATUS_OK,
                ..
            })
        ));
        assert_eq!(ctx.live(), 0);
    }
}

#[test]
fn good_stable_mask_skips_inactive_fields_across_pending_close() {
    let steps = [
        Step {
            field: 0,
            ..step(STEP_CLOSE_HANDLE, 0)
        },
        Step {
            field: 1,
            ..step(STEP_CLOSE_HANDLE, 8)
        },
        Step {
            field: 2,
            ..step(STEP_CLOSE_HANDLE, 16)
        },
    ];
    let image = Layout {
        field_count: 3,
        ..layout(24, &steps)
    };
    let ctx = Context::new(&[&image]);
    let mask = Box::new(0b101u64);
    unsafe {
        let root = ctx.alloc(2, 24);
        for i in 0..3 {
            root.add(1).cast::<u64>().add(i).write(i as u64 + 1);
        }
        push(&mut *ctx.0, root, false);
        let mut cursor = Cursor::new(ptr::null_mut(), root, &*mask, false, 31, 41);
        for expected in [1, 3] {
            assert!(matches!(cursor.poll(&mut *ctx.0), ReleasePoll::Close(h) if h == expected));
            cursor.closed(STATUS_OK);
        }
        assert!(matches!(cursor.poll(&mut *ctx.0), ReleasePoll::Complete(_)));
        assert_eq!(ctx.live(), 0);
    }
}

#[test]
#[should_panic(expected = "pending is not a release failure bit")]
fn bad_pending_close_status_cannot_complete_a_release_step() {
    let mut cursor = Cursor::new(ptr::null_mut(), ptr::null_mut(), ptr::null(), false, 0, 0);
    cursor.close = Some(7);
    cursor.closed(crate::STATUS_SUSPENDED);
}

unsafe extern "C" fn entered_counter(
    ctx: *mut TaskCtx,
    value: *mut u8,
    anchor: u64,
    site: u64,
) -> u8 {
    unsafe {
        let counter = value.cast::<u64>();
        let before = counter.read();
        counter.write(before + 1);
        if before == 20 {
            crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 17, anchor, site);
            STATUS_FAILURE
        } else {
            STATUS_OK
        }
    }
}

#[test]
fn entered_finalizers_keep_original_fresh_and_loop_suffixes_without_repeating_admission() {
    let mut resource_steps = [Step {
        variant: 1,
        ..step(STEP_END_FINALIZE, 0)
    }];
    let resource = Layout {
        finalizer: Some(entered_counter),
        ..layout(16, &resource_steps)
    };
    resource_steps[0].layout = &resource;
    let end = Step {
        layout: &resource,
        ..step(STEP_END_FINALIZE, 0)
    };
    let loop_steps = [
        Step {
            span: 3,
            stride: 16,
            count: 3,
            ..step(STEP_LOOP, 0)
        },
        Step {
            layout: &resource,
            ..step(STEP_FINALIZE, 0)
        },
        step(STEP_CLOSE_HANDLE, 8),
        end,
    ];
    let elements = Layout {
        flags: LAYOUT_WIDE_CURSOR,
        ..layout(48, &loop_steps)
    };
    let mut root_steps = [
        step(STEP_RELEASE, 8),
        step(STEP_CLOSE_HANDLE, 16),
        Step {
            variant: 1,
            layout: &resource,
            ..end
        },
    ];
    let root_layout = Layout {
        finalizer: Some(entered_counter),
        ..layout(24, &root_steps)
    };
    root_steps[2].layout = &root_layout;
    let ctx = Context::new(&[&root_layout, &elements, &resource]);
    unsafe {
        let root = ctx.alloc(2, 24);
        let child = ctx.alloc(3, 48);
        let words = root.add(1).cast::<u64>();
        words.write(0);
        words.add(1).write(child as u64);
        words.add(2).write(9);
        for (i, word) in [10, 1, 20, 2, 30, 3].into_iter().enumerate() {
            child.add(1).cast::<u64>().add(i).write(word);
        }
        push(&mut *ctx.0, root, true);
        let mut cursor = Box::new(Cursor::new(
            ptr::null_mut(),
            root,
            ptr::null(),
            false,
            31,
            41,
        ));
        let mut finalizers = Vec::new();
        let mut closes = Vec::new();
        let mut previous = None;
        loop {
            match cursor.poll_finalizers(&mut *ctx.0) {
                FinalizerPoll::Finalizer(token) => {
                    let before = token.value.cast::<u64>().read();
                    let admissions = (*ctx.0).cleanup_invocations.len();
                    assert!(
                        matches!(cursor.poll_finalizers(&mut *ctx.0), FinalizerPoll::Finalizer(t) if t == token)
                    );
                    assert_eq!(token.value.cast::<u64>().read(), before);
                    assert_eq!((*ctx.0).cleanup_invocations.len(), admissions);
                    assert_eq!((token.anchor, token.operation_site), (31, 41));
                    if let Some(stale) = previous {
                        assert!(
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                cursor.finalized(&mut *ctx.0, stale, STATUS_OK)
                            }))
                            .is_err()
                        );
                    }
                    assert!(
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            cursor.finalized(&mut *ctx.0, token, crate::STATUS_SUSPENDED)
                        }))
                        .is_err()
                    );
                    finalizers.push(before);
                    let status = run_finalizer(
                        &mut *ctx.0,
                        token.layout,
                        token.value,
                        token.anchor,
                        token.operation_site,
                    );
                    assert_eq!((*ctx.0).cleanup_invocations.len(), admissions + 1);
                    assert_eq!((*ctx.0).shield, 0);
                    cursor.finalized(&mut *ctx.0, token, status);
                    assert_eq!(token.value.cast::<u64>().read(), before + 1);
                    previous = Some(token);
                }
                FinalizerPoll::Close(word) => {
                    closes.push(word);
                    assert!(
                        matches!(cursor.poll_finalizers(&mut *ctx.0), FinalizerPoll::Close(w) if w == word)
                    );
                    cursor.closed(STATUS_OK);
                }
                FinalizerPoll::Complete(result) => {
                    assert_eq!(result.status, STATUS_FAILURE);
                    break;
                }
            }
        }
        assert_eq!(finalizers, [0, 30, 20, 10]);
        assert_eq!(closes, [3, 2, 1, 9]);
        assert_eq!(ctx.live(), 0);
        assert!((*ctx.0).work.is_null());
        assert_eq!((*ctx.0).cleanup_invocations.len(), 0);
        assert_eq!((*ctx.0).shield, 0);
        crate::failure::hirsel_rt_v2_failure_clear(ctx.0);
    }
}

unsafe extern "C" fn abandoned_finalizer(_: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    unsafe {
        let observed = &*value.cast::<*const std::sync::atomic::AtomicUsize>().read();
        observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    STATUS_OK
}

#[test]
fn abandoning_an_exposed_finalizer_runs_no_body_or_language_cleanup() {
    let observed = Box::new(std::sync::atomic::AtomicUsize::new(0));
    let mut steps = [Step {
        variant: 1,
        ..step(STEP_END_FINALIZE, 0)
    }];
    let resource = Layout {
        finalizer: Some(abandoned_finalizer),
        ..layout(8, &steps)
    };
    steps[0].layout = &resource;
    let ctx = Context::new(&[&resource]);
    unsafe {
        let root = ctx.alloc(2, 8);
        root.add(1)
            .cast::<*const std::sync::atomic::AtomicUsize>()
            .write(&*observed);
        push(&mut *ctx.0, root, true);
        let mut cursor = Box::new(Cursor::new(
            ptr::null_mut(),
            root,
            ptr::null(),
            false,
            31,
            41,
        ));
        assert!(matches!(
            cursor.poll_finalizers(&mut *ctx.0),
            FinalizerPoll::Finalizer(_)
        ));
        drop(cursor);
        assert_eq!(ctx.live(), 1);
        assert_eq!((*ctx.0).cleanup_invocations.len(), 0);
        assert_eq!((*ctx.0).shield, 0);
    }
    // Host stop abandons the task's mechanical work before normal wholesale
    // teardown; normal successful exit correctly refuses a pending cursor.
    unsafe { (*ctx.0).abandon() };
    assert!(unsafe { (*ctx.0).work.is_null() });
    drop(ctx);
    assert_eq!(observed.load(std::sync::atomic::Ordering::Acquire), 0);
}
