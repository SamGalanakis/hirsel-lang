use super::*;
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct Probe {
    recording: Option<Arc<record::Recording>>,
    first: AtomicBool,
    second: AtomicBool,
}

unsafe extern "C" fn destroy(ctx: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    unsafe {
        let [tag, probe] = value.cast::<[u64; 2]>().read();
        assert_eq!(tag, 1);
        let probe = &*(probe as *const Probe);
        assert_eq!((*ctx).detached_images.len(), 1);
        assert_eq!(crate::finalize::hirsel_rt_v2_shielded(ctx), 1);
        probe.first.store(true, Ordering::Release);
        if let Some(recording) = &probe.recording {
            recording.recorder.flush().unwrap().unwrap();
        }
        for _ in 0..130 {
            let _role = role::Guard::new(ctx);
            try_yield(&mut *ctx).unwrap();
        }
        let mut canceled = 99;
        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
        assert_eq!(canceled, 0);
        probe.second.store(true, Ordering::Release);
        if let Some(recording) = &probe.recording {
            recording.recorder.flush().unwrap().unwrap();
        }
        let mut atomic = 0;
        hirsel_rt_v2_atomic_create(
            ctx,
            std::ptr::null_mut(),
            &mut atomic,
            (&0u64 as *const u64).cast(),
            8,
            0,
            8,
        );
    }
    STATUS_OK
}

unsafe fn close(ctx: *mut TaskCtx, probe: &Probe, mutex: bool) {
    unsafe {
        let mut message = [1u64, probe as *const Probe as u64];
        let none = [0u64; 2];
        if mutex {
            let mut handle = 0;
            hirsel_rt_v2_mutex_create(
                ctx,
                std::ptr::null_mut(),
                &mut handle,
                message.as_mut_ptr().cast(),
                none.as_ptr().cast(),
                2,
                3,
            );
            let _role = role::Guard::new(ctx);
            assert_eq!(close_mutex(ctx, mutex_word(&handle), 0, 0), STATUS_OK);
        } else {
            let mut pair = [0; 2];
            hirsel_rt_v2_channel_open(
                ctx,
                std::ptr::null_mut(),
                pair.as_mut_ptr(),
                &0,
                none.as_ptr().cast(),
                2,
            );
            let mut status = 99;
            hirsel_rt_v2_channel_send(ctx, &mut status, &mut pair[0], message.as_mut_ptr().cast());
            assert_eq!(status, crate::channels::DONE);
            let _role = role::Guard::new(ctx);
            assert_eq!(close_endpoint(ctx, pair[1], 0, 0), STATUS_OK);
        }
        assert_eq!((*ctx).detached_images.len(), 0);
    }
}

unsafe fn nested_channel(ctx: *mut TaskCtx, depth: u64) {
    unsafe {
        let mut pair = [0; 2];
        let none = [0u64; 2];
        hirsel_rt_v2_channel_open(
            ctx,
            std::ptr::null_mut(),
            pair.as_mut_ptr(),
            &0,
            none.as_ptr().cast(),
            2,
        );
        let mut value = [1u64, depth];
        let mut status = 99;
        hirsel_rt_v2_channel_send(ctx, &mut status, &mut pair[0], value.as_mut_ptr().cast());
        assert_eq!(status, crate::channels::DONE);
        let _role = role::Guard::new(ctx);
        assert_eq!(close_endpoint(ctx, pair[0], 0, 0), STATUS_OK);
        assert_eq!(close_endpoint(ctx, pair[1], 0, 0), STATUS_OK);
    }
}

unsafe extern "C" fn nested_destroy(ctx: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    unsafe {
        let [tag, depth] = value.cast::<[u64; 2]>().read();
        assert_eq!(tag, 1);
        let images = (*ctx).detached_images.len();
        if depth > 0 {
            nested_channel(ctx, depth - 1);
        }
        for _ in 0..130 {
            let _role = role::Guard::new(ctx);
            try_yield(&mut *ctx).unwrap();
        }
        assert_eq!((*ctx).detached_images.len(), images);
        assert_eq!(
            value.cast::<[u64; 2]>().read(),
            [1, depth],
            "nested growth and stack saves preserve the outer image"
        );
    }
    STATUS_OK
}

#[test]
fn nested_finalizer_replay_keeps_outer_images_stable_across_owner_growth_and_saves() {
    let layout = crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 16,
        align: 8,
        finalizer: Some(nested_destroy),
        steps: std::ptr::null(),
        step_count: 0,
        field_count: 0,
        // Hand-written runtime test thunks carry no emitted scope marker.
        flags: crate::release::LAYOUT_IMPLEMENTATION_FINALIZER,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: 0,
    };
    let layouts = [&raw const layout];
    for durable in [false, true] {
        let recording = record::Recording::new(b"nested finalizer replay", durable);
        let install = recording.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                enable(ctx);
                install.root(ctx);
            },
            |ctx| unsafe {
                nested_channel(ctx, 8);
                assert_eq!((*ctx).detached_images.len(), 0);
                let mut word = 0;
                hirsel_rt_v2_atomic_create(
                    ctx,
                    std::ptr::null_mut(),
                    &mut word,
                    (&0u64 as *const u64).cast(),
                    8,
                    0,
                    8,
                );
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        let cut = recording
            .recorder
            .stop("returned", None)
            .unwrap()
            .unwrap()
            .0;
        let trace = recording.trace();
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        for width in [1, 2, 8] {
            let replay = replay::Replay::new(&bytes).unwrap();
            let install = replay.clone();
            let checked = Arc::new(AtomicBool::new(false));
            let observed = checked.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe { install.root(ctx) },
                move |ctx| unsafe {
                    nested_channel(ctx, 8);
                    assert_eq!((*ctx).detached_images.len(), 0);
                    observed.store(true, Ordering::Release);
                    let mut word = 0;
                    hirsel_rt_v2_atomic_create(
                        ctx,
                        std::ptr::null_mut(),
                        &mut word,
                        (&0u64 as *const u64).cast(),
                        8,
                        0,
                        8,
                    );
                    panic!("nested finalizer replay passed its cut");
                },
            );
            assert!(
                matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                "{outcome:?}"
            );
            assert!(checked.load(Ordering::Acquire));
            assert_eq!(
                replay.finish().unwrap(),
                Projection::new("returned", &cut.progress, &trace)
            );
        }
    }
}

#[test]
fn yielding_object_finalizers_replay_only_the_admitted_cleanup_prefix() {
    let plain = crate::Layout {
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
    let image = crate::Layout {
        finalizer: Some(destroy),
        flags: crate::release::LAYOUT_IMPLEMENTATION_FINALIZER,
        ..plain
    };
    let wrapper = crate::Layout {
        size: 8,
        flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
        ..plain
    };
    let layouts = [&raw const image, &raw const wrapper];
    for (durable, mutex) in [(false, false), (true, false), (false, true), (true, true)] {
        let recording = record::Recording::new(b"native finalizer replay", durable);
        let install = recording.clone();
        let probe = Arc::new(Probe {
            recording: Some(recording.clone()),
            first: false.into(),
            second: false.into(),
        });
        let observed = probe.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 2;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                close(ctx, &observed, mutex);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert!(probe.first.load(Ordering::Acquire));
        assert!(probe.second.load(Ordering::Acquire));
        drop(probe);
        recording.recorder.stop("returned", None).unwrap().unwrap();
        let trace = recording.trace();
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        let close_op = if mutex { 2 } else { 3 };
        let mut ends = vec![bytes.len()];
        for ordinal in [close_op, close_op + 1] {
            let end = (1..bytes.len())
                .find(|&end| {
                    hirsel_free::tape::parse(&bytes[..end]).is_ok_and(|tape| {
                        tape.cut.get(&Vec::new()) == Some(&Progress::new(ordinal, Phase::Done))
                    })
                })
                .unwrap();
            ends.extend([end, end + 1]);
        }
        for end in ends {
            let parsed = hirsel_free::tape::parse(&bytes[..end]).unwrap();
            let op = parsed.cut[&Vec::new()].op;
            let expected = Projection::new(
                if end == bytes.len() {
                    "returned"
                } else {
                    "truncated"
                },
                &parsed.cut,
                &trace,
            );
            for width in [1, 2, 8] {
                let replay = replay::Replay::new(&bytes[..end]).unwrap();
                let install = replay.clone();
                let probe = Arc::new(Probe {
                    recording: None,
                    first: false.into(),
                    second: false.into(),
                });
                let observed = probe.clone();
                let reclaimed = Arc::new(AtomicBool::new(false));
                let teardown = reclaimed.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 2;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        install.root(ctx);
                        (*ctx).detached_images.reclaimed = Some(teardown);
                    },
                    move |ctx| unsafe {
                        close(ctx, &observed, mutex);
                        panic!("finalizer replay passed its cut");
                    },
                );
                assert!(
                    matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                    "{outcome:?}"
                );
                assert_eq!(probe.first.load(Ordering::Acquire), op > close_op);
                assert_eq!(probe.second.load(Ordering::Acquire), op > close_op + 1);
                assert!(
                    reclaimed.load(Ordering::Acquire),
                    "cut teardown frees task-owned detached images"
                );
                assert_eq!(replay.finish().unwrap(), expected);
            }
        }
    }
}
