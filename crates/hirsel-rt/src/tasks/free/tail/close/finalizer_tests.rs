use super::*;
use crate::tasks::free::objects::tests::{plain, wrapper};

unsafe extern "C" fn opaque(_: *mut TaskCtx, _: *mut u8, _: u64, _: u64) -> u8 {
    panic!("certified finalizer must use its retained dispatcher")
}

unsafe extern "C" fn dispatch(
    ctx: *mut TaskCtx,
    value: *mut u8,
    anchor: u64,
    operation_site: u64,
    frame: *mut u8,
) -> u8 {
    unsafe {
        assert_eq!((*ctx).shield, 2, "close and finalizer shields persist");
        assert_eq!((*ctx).cleanup_invocations.len(), 1);
        assert_eq!((anchor, operation_site), (31, 41));
        let pc = frame.cast::<u32>();
        let words = value.cast::<u64>();
        assert_eq!(words.read(), 1);
        if pc.read() == 0 {
            assert_eq!(words.add(1).read(), 7);
            words.add(1).write(8);
            pc.write(1);
            crate::STATUS_SUSPENDED
        } else {
            assert_eq!(pc.read(), 1);
            assert_eq!(words.add(1).read(), 8, "first segment never repeats");
            words.add(1).write(9);
            pc.write(2);
            crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 17, anchor, operation_site);
            STATUS_FAILURE
        }
    }
}

unsafe fn resolve(_: *mut TaskCtx, token: Finalizer, retained: usize) -> Option<ResolvedFinalizer> {
    let frame = retained as *mut u8;
    let expected = unsafe { frame.add(16).cast::<*const crate::Layout>().read() };
    assert_eq!(token.layout, expected);
    Some(ResolvedFinalizer {
        dispatch,
        frame: unsafe { frame.add(32) },
        bytes: 16,
    })
}

#[test]
fn a_close_retains_its_original_child_through_nested_finalizer_pending_and_failure() {
    for durable in [false, true] {
        let scalar = crate::tasks::free::finalized(crate::Layout {
            finalizer: Some(opaque),
            ..plain(16)
        });
        let close_step = Box::leak(Box::new(crate::Step {
            kind: crate::STEP_CLOSE_HANDLE,
            field: crate::WHOLE_VALUE,
            variant: 0,
            span: 0,
            offset: 0,
            stride: 0,
            count: 0,
            count_offset: crate::NO_OFFSET,
            layout: ptr::null(),
        }));
        let wrapper = crate::Layout {
            steps: close_step,
            step_count: 1,
            ..wrapper()
        };
        let layouts = [scalar as *const crate::Layout, &wrapper];
        let mut run = crate::host::BodyRun::new(1, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 2;
        run.free = true;
        let recording = record::Recording::new(b"entered finalizer nested in close", durable);
        let outcome = run.run(
            move |ctx| unsafe {
                crate::tasks::free::enable(ctx);
                recording.root(ctx);
            },
            move |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                let mut word = 0;
                let mut image = [1u64, 7];
                crate::tasks::hirsel_rt_v2_mutex_create(
                    ctx,
                    ptr::null_mut(),
                    &mut word,
                    image.as_mut_ptr().cast(),
                    [0u64; 2].as_ptr().cast(),
                    2,
                    3,
                );
                let block = word as *mut Header;
                let mut frame = crate::continuation::OwnedResumeFrame::new(64);
                frame
                    .as_mut_ptr()
                    .add(16)
                    .cast::<*const crate::Layout>()
                    .write((*ctx).descriptor(2));
                let cursor = Box::new(Cursor::new((*ctx).work, block, ptr::null(), false, 31, 41));
                let index = attach_finalizers(
                    ctx,
                    cursor,
                    31,
                    41,
                    FinalizerResolver {
                        resolve,
                        retained: frame.as_mut_ptr() as usize,
                    },
                );
                assert!(crate::roots::consume(&mut *ctx, block));
                crate::release::push(&mut *ctx, block, true);
                assert!(
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poll(ctx, index)))
                        .is_err(),
                    "ordinary polling must not erase a nested wait disposition"
                );
                assert!(matches!(
                    poll_finalizers(ctx, index),
                    Ok(CleanupPoll::Pending(CleanupPending::Nested))
                ));
                assert_eq!((*ctx).shield, 2);
                assert_eq!((*ctx).cleanup_invocations.len(), 1);
                assert_eq!((*ctx).detached_images.len(), 1);
                assert!(!(*ctx).work.is_null());
                assert_eq!(frame.as_mut_ptr().add(32).cast::<u32>().read(), 1);
                assert!(matches!(
                    poll_finalizers(ctx, index),
                    Ok(CleanupPoll::Ready(STATUS_FAILURE))
                ));
                assert!((*ctx).free_ops.tails.drivers.is_empty());
                assert_eq!((*ctx).shield, 0);
                assert_eq!((*ctx).cleanup_invocations.len(), 0);
                assert_eq!((*ctx).detached_images.len(), 0);
                assert!((*ctx).work.is_null());
                assert_eq!(frame.as_mut_ptr().add(32).cast::<u32>().read(), 2);
                crate::failure::hirsel_rt_v2_failure_clear(ctx);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
    }
}
