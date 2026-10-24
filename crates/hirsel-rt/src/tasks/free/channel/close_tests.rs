use super::*;

unsafe extern "C" fn drop_message(ctx: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    let [tag, id, mut sender] = unsafe { value.cast::<[u64; 3]>().read() };
    assert_eq!(tag, 1);
    assert_eq!(unsafe { crate::finalize::hirsel_rt_v2_shielded(ctx) }, 1);
    // Force an idle save in each finalizer while its receiver's detached
    // image ring and the close frame remain live on this same native stack.
    for _ in 0..130 {
        unsafe {
            let _role = role::Guard::new(ctx);
            try_yield(&mut *ctx).unwrap();
        }
    }
    let mut message = [1u64, id];
    let mut status = 99;
    unsafe {
        hirsel_rt_v2_channel_send(ctx, &mut status, &mut sender, message.as_mut_ptr().cast());
    }
    assert_eq!(status, crate::channels::DONE);
    assert_eq!(message, [0, 0]);
    STATUS_OK
}

#[test]
fn receiver_close_detaches_fifo_images_before_their_yielding_finalizers() {
    let mut run = crate::host::BodyRun::new(2, false);
    run.free = true;
    unsafe { run.use_physical_dispatch() };
    let plain = tests::layout();
    // A hand-written runtime test thunk has no emitted finalizer scope marker.
    let image = crate::Layout {
        size: 24,
        finalizer: Some(drop_message),
        flags: crate::release::LAYOUT_IMPLEMENTATION_FINALIZER,
        ..tests::layout()
    };
    let layouts = [&raw const image, &raw const plain];
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 2;
    let outcome = run.run(
        |ctx| unsafe { super::super::enable(ctx) },
        |ctx| unsafe {
            let mut dropped = [0u64; 2];
            let mut results = [0u64; 2];
            let none = [0u64; 3];
            hirsel_rt_v2_channel_open(
                ctx,
                std::ptr::null_mut(),
                dropped.as_mut_ptr(),
                &0,
                none.as_ptr().cast(),
                2,
            );
            hirsel_rt_v2_channel_open(
                ctx,
                std::ptr::null_mut(),
                results.as_mut_ptr(),
                &0,
                none.as_ptr().cast(),
                3,
            );
            for id in 1..=8 {
                let mut message = [1u64, id, results[0]];
                let mut status = 99;
                hirsel_rt_v2_channel_send(
                    ctx,
                    &mut status,
                    &mut dropped[0],
                    message.as_mut_ptr().cast(),
                );
                assert_eq!(status, crate::channels::DONE);
                assert_eq!(message, none);
            }
            {
                let _role = role::Guard::new(ctx);
                scheduler(ctx).cancel(ROOT, CANCEL_REQUESTED);
                assert_eq!(close_endpoint(ctx, dropped[1], 0, 0), STATUS_OK);
            }
            // Destruction suppressed cancellation while sending all results.
            // Shield these observations too, then restore the context.
            {
                let _role = role::Guard::new(ctx);
                (*ctx).shield = 1;
            }
            for id in 1..=8 {
                let mut message = [0u64; 2];
                let mut status = 99;
                hirsel_rt_v2_channel_receive(
                    ctx,
                    0,
                    0,
                    &mut status,
                    &mut results[1],
                    message.as_mut_ptr().cast(),
                );
                assert_eq!(status, crate::channels::DONE);
                assert_eq!(message, [1, id]);
            }
            {
                let _role = role::Guard::new(ctx);
                (*ctx).shield = 0;
                assert_eq!(close_endpoint(ctx, dropped[0], 0, 0), STATUS_OK);
                assert_eq!(close_endpoint(ctx, results[0], 0, 0), STATUS_OK);
                assert_eq!(close_endpoint(ctx, results[1], 0, 0), STATUS_OK);
            }
            STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(STATUS_OK));
    assert_eq!(outcome.pending, 0);
}
