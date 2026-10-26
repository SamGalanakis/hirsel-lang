use super::*;
use crate::release::{Cursor, FinalizerPoll, Layout, Step};
use std::ptr;

unsafe extern "C" fn opaque(_: *mut TaskCtx, _: *mut u8, _: u64, _: u64) -> u8 {
    panic!("certified dispatcher must not restart the ordinary finalizer")
}

unsafe extern "C" fn dispatch(
    ctx: *mut TaskCtx,
    value: *mut u8,
    anchor: u64,
    site: u64,
    frame: *mut u8,
) -> u8 {
    unsafe {
        assert_eq!((*ctx).shield, 1);
        assert_eq!((*ctx).cleanup_invocations.len(), 1);
        assert_eq!((anchor, site), (31, 41));
        let counter = value.cast::<u64>();
        counter.write(counter.read() + 1);
        let pc = frame.cast::<u32>();
        match pc.read() {
            0 => {
                pc.write(1);
                STATUS_SUSPENDED
            }
            1 => {
                pc.write(2);
                if frame.add(8).read() != 0 {
                    crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 17, anchor, site);
                    STATUS_FAILURE
                } else {
                    STATUS_OK
                }
            }
            _ => panic!("finalizer dispatcher repeated a completed PC"),
        }
    }
}

fn descriptor(steps: &[Step]) -> Layout {
    Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 8,
        align: 8,
        finalizer: Some(opaque),
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

fn ending() -> Step {
    Step {
        kind: crate::release::STEP_END_FINALIZE,
        field: crate::release::WHOLE_VALUE,
        variant: 1,
        span: 0,
        offset: 0,
        stride: 0,
        count: 0,
        count_offset: crate::release::NO_OFFSET,
        layout: ptr::null(),
    }
}

unsafe fn cursor(ctx: *mut TaskCtx) -> (Box<Cursor>, Finalizer) {
    unsafe {
        let root = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(root).cast::<u64>().write(0);
        let base = (*ctx).work;
        crate::release::push(&mut *ctx, root, true);
        let mut cursor = Box::new(Cursor::new(base, root, ptr::null(), false, 31, 41));
        let FinalizerPoll::Finalizer(token) = cursor.poll_finalizers(&mut *ctx) else {
            panic!("original finalizer handoff")
        };
        (cursor, token)
    }
}

fn run(body: impl FnMut(*mut TaskCtx) -> u8 + Send + 'static) {
    run_ended(Ok(STATUS_OK), body);
}

fn run_ended(
    expected: Result<u8, crate::stop::Stop>,
    body: impl FnMut(*mut TaskCtx) -> u8 + Send + 'static,
) {
    let mut steps = [ending()];
    let resource = descriptor(&steps);
    steps[0].layout = &resource;
    let layouts = [&raw const resource];
    let mut run = crate::host::BodyRun::new(1, false);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    let result = run.run(
        |ctx| unsafe {
            let mut scope = 0;
            assert_eq!(
                crate::tasks::hirsel_rt_v2_task_scope_enter(ctx, ptr::null_mut(), &mut scope, &0),
                STATUS_OK
            );
        },
        body,
    );
    assert_eq!(result.ended, expected);
}

#[test]
fn pending_finalizer_retains_one_admission_and_shield_until_original_body_finishes() {
    for fail in [false, true] {
        run(move |ctx| unsafe {
            let (mut cursor, token) = cursor(ctx);
            let mut frame = crate::continuation::OwnedResumeFrame::new(16);
            frame.as_mut_ptr().add(8).write(u8::from(fail));
            let mut driver = Driver::new(
                crate::tasks::running_key(ctx),
                token,
                dispatch,
                frame.as_mut_ptr(),
                16,
            );
            assert!(matches!(driver.poll(ctx), Ok(Poll::Pending)));
            assert_eq!(token.value.cast::<u64>().read(), 1);
            assert_eq!((*ctx).activations.live(), (0, 1));
            assert_eq!((*ctx).shield, 1);
            assert!(
                matches!(cursor.poll_finalizers(&mut *ctx), FinalizerPoll::Finalizer(t) if t == token)
            );
            let Ok(Poll::Complete(status)) = driver.poll(ctx) else {
                panic!("original finalizer completion")
            };
            assert_eq!(status, if fail { STATUS_FAILURE } else { STATUS_OK });
            assert_eq!(token.value.cast::<u64>().read(), 2);
            assert_eq!((*ctx).shield, 0);
            assert_eq!(
                (*ctx).activations.live(),
                (0, 1),
                "fields retain original activation"
            );
            cursor.finalized(&mut *ctx, token, status);
            assert!(
                matches!(cursor.poll_finalizers(&mut *ctx), FinalizerPoll::Complete(done) if done.status == status)
            );
            assert_eq!((*ctx).activations.live(), (0, 0));
            assert_eq!((*ctx).cleanup_invocations.len(), 0);
            if fail {
                crate::failure::hirsel_rt_v2_failure_clear(ctx);
            }
            STATUS_OK
        });
    }
}

#[test]
fn sixty_fifth_finalizer_refuses_before_body_and_retires_the_rejected_field_scope() {
    run(|ctx| unsafe {
        let mut existing = Vec::new();
        for _ in 0..64 {
            existing.push((*ctx).activations.admit_finalizer().unwrap());
        }
        let (mut cursor, token) = cursor(ctx);
        let mut frame = crate::continuation::OwnedResumeFrame::new(16);
        let mut driver = Driver::new(
            crate::tasks::running_key(ctx),
            token,
            dispatch,
            frame.as_mut_ptr(),
            16,
        );
        assert!(matches!(
            driver.poll(ctx),
            Ok(Poll::Complete(STATUS_FAILURE))
        ));
        assert_eq!(token.value.cast::<u64>().read(), 0);
        assert_eq!((*ctx).shield, 0);
        assert_eq!((*ctx).cleanup_rejections, 1);
        cursor.finalized(&mut *ctx, token, STATUS_FAILURE);
        assert!(matches!(
            cursor.poll_finalizers(&mut *ctx),
            FinalizerPoll::Complete(_)
        ));
        assert_eq!((*ctx).cleanup_rejections, 0);
        assert_eq!((*ctx).activations.live(), (0, 64));
        for activation in existing {
            (*ctx).activations.retire(activation);
        }
        crate::failure::hirsel_rt_v2_failure_clear(ctx);
        STATUS_OK
    });
}

#[test]
fn host_stop_does_not_deliver_a_pending_finalizer_or_lower_its_shield_as_cleanup() {
    run_ended(Err(crate::stop::Stop::Requested), |ctx| unsafe {
        let (cursor, token) = cursor(ctx);
        let mut frame = crate::continuation::OwnedResumeFrame::new(16);
        let mut driver = Driver::new(
            crate::tasks::running_key(ctx),
            token,
            dispatch,
            frame.as_mut_ptr(),
            16,
        );
        assert!(matches!(driver.poll(ctx), Ok(Poll::Pending)));
        (*ctx)
            .run_ref()
            .control
            .request(crate::stop::Stop::Requested);
        assert!(matches!(
            driver.poll(ctx),
            Err(crate::stop::Stop::Requested)
        ));
        assert_eq!(token.value.cast::<u64>().read(), 1);
        assert_eq!((*ctx).shield, 1);
        drop(driver);
        drop(cursor);
        (*ctx).abandon();
        assert_eq!((*ctx).shield, 0);
        STATUS_OK
    });
}
