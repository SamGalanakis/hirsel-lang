//! Strict retirement boundaries for endpoint/mutex closes, roots and thaw.
//!
//! A close detaches its images in a bounded head callback. The owning task
//! then scans and finalizes them outside Frontier, so nested ops may suspend
//! without retaining policy or object borrows. Positional speculative-release
//! logs and allocation credit settlement are still a separate integration.

use super::super::*;
use super::engine;
use crate::ordered::objects::Kind;
use crate::ordered::{Next, Progress};

/// # Safety
/// As for close_endpoint; this entry owns the role and one endpoint credit.
pub(crate) unsafe fn endpoint(
    ctx: *mut TaskCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> Stopped<u8> {
    if word & TASK_SET != 0 {
        super::sets::close(ctx, word)?;
        return Ok(STATUS_OK);
    }
    // A sender is statically drop.trivial, including a provisional duplicate
    // whose earlier initialization is still in this same owner's log.
    if engine::is_sender(ctx, word) {
        engine::sender_close(ctx, word)?;
        return Ok(STATUS_OK);
    }
    engine::at_head(ctx)?;
    let (_, sender) = scheduler(ctx).channels.endpoint_of(word);
    let name = if sender {
        "drop.trivial"
    } else {
        let descriptor = engine::carrier_descriptor(ctx, Kind::Receiver, word);
        let layout = unsafe { &*(*ctx).descriptor(descriptor) };
        if layout.finalizer.is_none() && layout.step_count == 0 {
            "drop.trivial"
        } else {
            "drop.leftovers"
        }
    };
    let (left, progress) = engine::head_effect(ctx, name, || {
        (scheduler(ctx).channels.close(word), Next::Keep)
    })?;
    assert!(matches!(progress, Progress::Absorbed(_)));
    let Some((left, descriptor)) = left else {
        return Ok(STATUS_OK);
    };
    // Finalizers can obtain these handles without a scheduling point. Refresh
    // before invoking any of them, including nested receivers in later images.
    for index in 0..left.len() {
        unsafe { engine::obtained_image(ctx, left.at(index), descriptor) };
    }
    let mut status = STATUS_OK;
    for index in 0..left.len() {
        let mut continuation = std::mem::MaybeUninit::<crate::finalize::Cleanup>::uninit();
        // SAFETY: the detached FIFO owns every live image through this drop.
        status |= unsafe {
            crate::finalize::hirsel_rt_v2_finalize(
                ctx,
                descriptor,
                left.at(index),
                std::ptr::null(),
                continuation.as_mut_ptr(),
                anchor,
                operation_site,
            )
        };
    }
    Ok(status)
}

/// # Safety
/// As for close_mutex; the last wrapper exclusively owns the cell.
pub(crate) unsafe fn mutex(
    ctx: *mut TaskCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> Stopped<u8> {
    engine::at_head(ctx)?;
    let descriptor = engine::carrier_descriptor(ctx, Kind::Mutex, word);
    let layout = unsafe { &*(*ctx).descriptor(descriptor) };
    let name = if layout.finalizer.is_none() && layout.step_count == 0 {
        "drop.trivial"
    } else {
        "drop.leftovers"
    };
    let (left, progress) =
        engine::head_effect(ctx, name, || (scheduler(ctx).sync.close(word), Next::Keep))?;
    assert!(matches!(progress, Progress::Absorbed(_)));
    let Some((image, descriptor)) = left else {
        return Ok(STATUS_OK);
    };
    unsafe { engine::obtained_image(ctx, image.at(0), descriptor) };
    let mut continuation = std::mem::MaybeUninit::<crate::finalize::Cleanup>::uninit();
    Ok(unsafe {
        crate::finalize::hirsel_rt_v2_finalize(
            ctx,
            descriptor,
            image.at(0),
            std::ptr::null(),
            continuation.as_mut_ptr(),
            anchor,
            operation_site,
        )
    })
}

/// Root decrement's actual atomic action is supplied by roots::consume. Its
/// walk happens after restoring Frontier and is owned by the releasing task.
/// # Safety
/// The caller owns the role and one exact root credit; action cannot suspend.
pub(crate) unsafe fn root_release(ctx: *mut TaskCtx, action: impl FnOnce() -> bool) -> bool {
    let result = engine::head_effect(ctx, "root.release", || (action(), Next::Keep));
    let (died, progress) = unsafe { crate::stop::check(ctx, result) };
    assert!(matches!(progress, Progress::Absorbed(_)));
    died
}

/// # Safety
/// As for SharedThawTurn; the root remains borrowed through the head gate.
pub(crate) unsafe fn thaw(ctx: *mut TaskCtx, root: *mut Header, out: *mut u8) -> Stopped<u8> {
    let (last, progress) = engine::head_effect(ctx, "shared.thaw", || {
        (unsafe { crate::roots::sole(root) }, Next::Keep)
    })?;
    assert!(matches!(progress, Progress::Absorbed(_)));
    unsafe { out.write(u8::from(last)) };
    Ok(STATUS_OK)
}

/// # Safety
/// As for MutexKill; the failure chain belongs to this head task.
pub(crate) unsafe fn kill(ctx: *mut TaskCtx, word: u64) -> Stopped<u8> {
    let (_, progress) = engine::head_effect(ctx, "mutex.kill", || {
        let path = current_path(ctx);
        let chain = unsafe { (*ctx).failure.chain(&path) };
        scheduler(ctx).sync.kill(word, chain);
        ((), Next::Yield)
    })?;
    engine::continue_at_head(ctx, progress)?;
    Ok(STATUS_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ordered::Policy;

    const NONE: [u64; 2] = [0, 0];

    unsafe extern "C" fn log_drop(ctx: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
        let [tag, id, mut sender] = unsafe { value.cast::<[u64; 3]>().read() };
        assert_eq!(tag, 1);
        assert_eq!(unsafe { crate::finalize::hirsel_rt_v2_shielded(ctx) }, 1);
        let mut canceled = u32::MAX;
        assert_eq!(
            unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) },
            STATUS_OK
        );
        assert_eq!(canceled, 0, "destruction suppresses cancellation");
        let mut message = [1, id];
        let mut status = u32::MAX;
        assert_eq!(
            unsafe {
                hirsel_rt_v2_channel_send(
                    ctx,
                    &mut status,
                    &mut sender,
                    message.as_mut_ptr().cast(),
                )
            },
            STATUS_OK
        );
        assert_eq!(status, crate::channels::DONE);
        assert_eq!(message, NONE);
        STATUS_OK
    }

    unsafe extern "C" fn sibling(ctx: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
        for _ in 0..8 {
            let mut canceled = 0;
            assert_eq!(
                unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) },
                STATUS_OK
            );
        }
        let mut result = [1u64, 9];
        unsafe {
            hirsel_rt_v2_task_finish(ctx, result.as_mut_ptr().cast(), NONE.as_ptr().cast(), 16)
        }
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

    fn step(kind: u32, offset: u64) -> crate::Step {
        crate::Step {
            kind,
            field: 0,
            variant: 0,
            span: 0,
            offset,
            stride: 0,
            count: 0,
            count_offset: 0,
            layout: std::ptr::null(),
        }
    }

    fn run_drops(width: u32) {
        use hirsel_core_contract::handle_scan::TYPED_HANDLE;
        let close_sender = step(crate::STEP_CLOSE_HANDLE, 16);
        let scan = [TYPED_HANDLE, 16, Kind::Sender.code(), 3, 0, 0, 0, 0];
        let image = layout(16);
        let resource = crate::Layout {
            finalizer: Some(log_drop),
            steps: &close_sender,
            step_count: 1,
            field_count: 1,
            scan: scan.as_ptr(),
            scan_words: scan.len() as u64,
            ..layout(24)
        };
        let close_mutex = step(crate::STEP_CLOSE_HANDLE, 0);
        let wrapper = crate::Layout {
            flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
            steps: &close_mutex,
            step_count: 1,
            field_count: 1,
            ..layout(8)
        };
        let scalar_root = crate::Layout {
            flags: crate::LAYOUT_FROZEN,
            ..layout(8)
        };
        let environment = layout(8);
        let layouts = [
            &image as *const crate::Layout,
            &resource,
            &wrapper,
            &scalar_root,
            &environment,
        ];
        let mut run = crate::host::BodyRun::new(width, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = layouts.len() as u32;
        run.config.count_max = 3;
        // SAFETY: supported task/E/drop/root/thaw boundaries, unbudgeted.
        // Finalizers use only supported task and channel entries; no host.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
            |ctx| unsafe {
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    STATUS_OK
                );
                let environment =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 6, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(environment).cast::<u64>().write(0);
                let mut child = 0;
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        sibling,
                        environment
                    ),
                    STATUS_OK
                );
                let mut log = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_channel_open(
                        ctx,
                        std::ptr::null_mut(),
                        log.as_mut_ptr(),
                        &0,
                        NONE.as_ptr().cast(),
                        2
                    ),
                    STATUS_OK
                );
                let none = [0u64; 3];
                let mut resources = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_channel_open(
                        ctx,
                        std::ptr::null_mut(),
                        resources.as_mut_ptr(),
                        &0,
                        none.as_ptr().cast(),
                        3
                    ),
                    STATUS_OK
                );
                for id in [101, 102] {
                    let mut duplicate = 0;
                    assert_eq!(
                        hirsel_rt_v2_sender_dup(
                            ctx,
                            std::ptr::null_mut(),
                            &mut duplicate,
                            &0,
                            &log[0]
                        ),
                        STATUS_OK
                    );
                    let mut payload = [1, id, duplicate];
                    let mut status = u32::MAX;
                    assert_eq!(
                        hirsel_rt_v2_channel_send(
                            ctx,
                            &mut status,
                            &mut resources[0],
                            payload.as_mut_ptr().cast()
                        ),
                        STATUS_OK
                    );
                    assert_eq!(status, crate::channels::DONE);
                    assert_eq!(payload, none);
                }
                assert_eq!(
                    crate::finalize::hirsel_rt_v2_close_handle(ctx, resources[1], 0, 0),
                    STATUS_OK
                );
                assert_eq!(
                    crate::finalize::hirsel_rt_v2_close_handle(ctx, resources[0], 0, 0),
                    STATUS_OK
                );
                let mut duplicate = 0;
                assert_eq!(
                    hirsel_rt_v2_sender_dup(ctx, std::ptr::null_mut(), &mut duplicate, &0, &log[0]),
                    STATUS_OK
                );
                let mut payload = [1, 103, duplicate];
                let mut mutex = 0;
                assert_eq!(
                    hirsel_rt_v2_mutex_create(
                        ctx,
                        std::ptr::null_mut(),
                        &mut mutex,
                        payload.as_mut_ptr().cast(),
                        none.as_ptr().cast(),
                        3,
                        4
                    ),
                    STATUS_OK
                );
                let block = mutex as *mut Header;
                crate::roots::hirsel_rt_v2_root_dup(ctx, block);
                assert_eq!(crate::roots::count(&*ctx, block), 2);
                assert_eq!(
                    crate::roots::hirsel_rt_v2_root_drop(ctx, block, 0, 0),
                    STATUS_OK
                );
                assert_eq!(crate::roots::count(&*ctx, block), 1);
                assert_eq!(
                    crate::roots::hirsel_rt_v2_root_drop(ctx, block, 0, 0),
                    STATUS_OK
                );
                for id in [101, 102, 103] {
                    let mut message = NONE;
                    let mut status = u32::MAX;
                    assert_eq!(
                        hirsel_rt_v2_channel_receive(
                            ctx,
                            0,
                            0,
                            &mut status,
                            &mut log[1],
                            message.as_mut_ptr().cast()
                        ),
                        STATUS_OK
                    );
                    assert_eq!(status, crate::channels::DONE);
                    assert_eq!(
                        message,
                        [1, id],
                        "leftover and mutex finalizers run in release order"
                    );
                }
                assert_eq!(
                    crate::finalize::hirsel_rt_v2_close_handle(ctx, log[0], 0, 0),
                    STATUS_OK
                );
                let mut message = NONE;
                let mut status = u32::MAX;
                assert_eq!(
                    hirsel_rt_v2_channel_receive(
                        ctx,
                        0,
                        0,
                        &mut status,
                        &mut log[1],
                        message.as_mut_ptr().cast()
                    ),
                    STATUS_OK
                );
                assert_eq!(
                    status,
                    crate::channels::CLOSED,
                    "last sender close is visible at the receive boundary"
                );
                assert_eq!(
                    crate::finalize::hirsel_rt_v2_close_handle(ctx, log[1], 0, 0),
                    STATUS_OK
                );
                let root =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 5, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(root).cast::<u64>().write(77);
                crate::shared::hirsel_rt_v2_freeze(ctx, root);
                crate::roots::hirsel_rt_v2_root_dup(ctx, root);
                crate::roots::hirsel_rt_v2_root_dup(ctx, root);
                assert_eq!(crate::roots::count(&*ctx, root), 3);
                crate::roots::hirsel_rt_v2_root_release(ctx, root);
                let mut last = 99;
                assert_eq!(
                    crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, root),
                    STATUS_OK
                );
                assert_eq!(last, 0);
                crate::roots::hirsel_rt_v2_root_release(ctx, root);
                assert_eq!(
                    crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, root),
                    STATUS_OK
                );
                assert_eq!(last, 1);
                crate::roots::hirsel_rt_v2_root_release(ctx, root);
                let mut result = NONE;
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut child,
                        result.as_mut_ptr().cast(),
                        16
                    ),
                    STATUS_OK
                );
                assert_eq!(result, [1, 9]);
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
                role::enter(ctx);
                engine::at_head(ctx).unwrap();
                let task = engine::current(ctx);
                engine::driver(ctx).finishing(task);
                role::release(ctx);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn receiver_leftovers_and_last_mutex_roots_run_yielding_finalizers_at_the_head() {
        for width in [2, 4, 8] {
            run_drops(width);
        }
    }
}
