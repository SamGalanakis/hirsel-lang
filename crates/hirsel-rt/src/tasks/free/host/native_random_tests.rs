//! Native Random grants share their draw sequence, including across replay widths.
use super::*;
use crate::io::{BytesResult, GrantResult};

unsafe fn program(ctx: *mut TaskCtx, granted: bool) {
    let mut first = GrantResult::default();
    let mut second = GrantResult::default();
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_random(ctx, std::ptr::null_mut(), &mut first, &1) },
        STATUS_OK
    );
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_random(ctx, std::ptr::null_mut(), &mut second, &1) },
        STATUS_OK
    );
    assert_eq!(first.tag, u32::from(!granted));
    assert_eq!(second.tag, first.tag);
    if !granted {
        return;
    }
    assert_ne!(first.value, second.value);
    let mut distinct = None;
    for (word, count) in [(first.value, 0), (first.value, 19), (second.value, 19)] {
        let mut out = std::mem::MaybeUninit::<BytesResult>::uninit();
        assert_eq!(
            unsafe {
                crate::io::hirsel_rt_v2_random_bytes(
                    ctx,
                    std::ptr::null_mut(),
                    out.as_mut_ptr(),
                    &word,
                    count,
                )
            },
            STATUS_OK
        );
        let out = unsafe { out.assume_init() };
        assert_eq!((out.tag, out.padding, out.value.length), (0, 0, count));
        let bytes =
            unsafe { crate::io::slice_bytes(out.value.owner, out.value.start, out.value.length) };
        if count > 0 {
            if let Some(prior) = distinct.as_ref() {
                assert_ne!(bytes, prior);
            }
            distinct = Some(bytes.to_vec());
        }
        assert_eq!(
            unsafe {
                crate::release::hirsel_rt_v2_release(
                    ctx,
                    out.value.owner,
                    (*out.value.owner).index(),
                    std::ptr::null(),
                    0,
                    0,
                )
            },
            STATUS_OK
        );
        assert!(unsafe { (*ctx).native_host_result.is_none() });
    }
}

#[test]
fn native_random_abi_replays_grant_aliases_and_draws_without_a_provider() {
    for width in [1, 8] {
        for durable in [false, true] {
            for granted in [false, true] {
                let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
                real.discard_history();
                real.set_random(hirsel_run_schema::ProviderContract::RandomSplitMix64, 13);
                if !granted {
                    real.set_projections(Default::default());
                }
                let mut adapter = Some(Arc::new(
                    real.into_concurrent()
                        .unwrap_or_else(|_| panic!("Send random")),
                ));
                let recording = super::super::record::Recording::new(b"native random ABI", durable);
                let install = recording.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        {
                            let _role = role::Guard::new(ctx);
                            let _segment = role::Segment::new(ctx);
                            concurrent::Service::install(ctx, adapter.take().unwrap()).unwrap();
                            native::enable(ctx);
                        }
                        program(ctx, granted);
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                let cut = recording
                    .recorder
                    .stop("returned", None)
                    .unwrap()
                    .unwrap()
                    .0;
                let projection = hirsel_free::projection::Projection::new(
                    "returned",
                    &cut.progress,
                    &recording.trace(),
                );
                let bytes = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink()
                    .bytes;
                let tape = hirsel_free::tape::parse(&bytes).unwrap();
                let records = hirsel_free::record::decode(&tape.logs[&Vec::new()]).unwrap();
                assert_eq!(
                    records
                        .iter()
                        .filter(|record| matches!(
                            record,
                            hirsel_free::record::Record::Prepared { .. }
                        ))
                        .count(),
                    if granted { 4 } else { 2 },
                    "zero draw creates no request"
                );
                for width in [1, 8] {
                    let replay = super::super::replay::Replay::new(&bytes).unwrap();
                    let install = replay.clone();
                    let mut run = crate::host::BodyRun::new(width, false);
                    run.free = true;
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        move |ctx| unsafe {
                            install.root(ctx);
                            native::enable(ctx);
                        },
                        move |ctx| unsafe {
                            program(ctx, granted);
                            let _role = role::Guard::new(ctx);
                            super::super::replay::start(ctx);
                            unreachable!("random replay beyond cut");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    assert_eq!(outcome.pending, 0);
                    assert_eq!(replay.finish().unwrap(), projection);
                }
            }
        }
    }
}

#[test]
fn native_conversion_keeps_buffers_task_owned_through_allocation_and_stop() {
    for width in [1, 8] {
        for stop in [false, true] {
            let recording = super::super::record::Recording::new(b"conversion ownership", false);
            let install = recording.clone();
            let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let observer = observed.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                    native::enable(ctx);
                },
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    native::convert_result(
                        ctx,
                        Completion::Bytes(Ok(vec![42; 10000])),
                        |completion| {
                            let Completion::Bytes(Ok(bytes)) = completion else {
                                unreachable!()
                            };
                            assert!(std::ptr::eq(
                                completion,
                                (*ctx)
                                    .native_host_result
                                    .as_deref()
                                    .unwrap()
                                    .as_ref()
                                    .unwrap()
                            ));
                            observer.store(true, std::sync::atomic::Ordering::Release);
                            if stop {
                                (*ctx).set_budgets(None, Some(1));
                            }
                            let out = crate::text::allocate_bytes(ctx, bytes);
                            assert_eq!(
                                crate::io::slice_bytes(out.owner, out.start, out.length),
                                bytes
                            );
                            assert_eq!(
                                crate::release::hirsel_rt_v2_release(
                                    ctx,
                                    out.owner,
                                    (*out.owner).index(),
                                    std::ptr::null(),
                                    0,
                                    0
                                ),
                                STATUS_OK
                            );
                        },
                    );
                    assert!((*ctx).native_host_result.is_none());
                    STATUS_OK
                },
            );
            assert!(observed.load(std::sync::atomic::Ordering::Acquire));
            if stop {
                assert!(
                    matches!(outcome.ended, Err(Stop::BudgetExceeded(_))),
                    "{outcome:?}"
                );
            } else {
                assert_eq!(outcome.ended, Ok(STATUS_OK));
            }
            assert_eq!(outcome.pending, 0);
            assert_eq!(
                Arc::strong_count(&recording),
                1,
                "task owners reclaimed after cut"
            );
        }
    }
}

#[test]
fn native_random_abi_allocation_stop_reclaims_the_completed_request() {
    for width in [1, 8] {
        let real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
        let adapter = Arc::new(
            real.into_concurrent()
                .unwrap_or_else(|_| panic!("Send random")),
        );
        let mut admitted = Some(adapter.clone());
        let recording = super::super::record::Recording::new(b"random ABI allocation cut", false);
        let install = recording.clone();
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reached = called.clone();
        let mut run = crate::host::BodyRun::new(width, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                {
                    let _role = role::Guard::new(ctx);
                    let _segment = role::Segment::new(ctx);
                    concurrent::Service::install(ctx, admitted.take().unwrap()).unwrap();
                    native::enable(ctx);
                }
                let mut grant = GrantResult::default();
                assert_eq!(
                    crate::io::hirsel_rt_v2_io_random(ctx, std::ptr::null_mut(), &mut grant, &1),
                    STATUS_OK
                );
                assert_eq!(grant.tag, 0);
                {
                    let _role = role::Guard::new(ctx);
                    (*ctx).set_budgets(None, Some(1));
                }
                reached.store(true, std::sync::atomic::Ordering::Release);
                let mut result = std::mem::MaybeUninit::<BytesResult>::uninit();
                crate::io::hirsel_rt_v2_random_bytes(
                    ctx,
                    std::ptr::null_mut(),
                    result.as_mut_ptr(),
                    &grant.value,
                    10000,
                );
                unreachable!("first ABI result allocation must exceed one byte");
            },
        );
        assert!(called.load(std::sync::atomic::Ordering::Acquire));
        assert!(
            matches!(outcome.ended, Err(Stop::BudgetExceeded(_))),
            "{outcome:?}"
        );
        assert_eq!(outcome.pending, 0);
        assert_eq!(Arc::strong_count(&adapter), 1);
        assert_eq!(Arc::strong_count(&recording), 1);
        let (_, sweep) = recording
            .recorder
            .stop("memory budget exceeded", None)
            .unwrap()
            .unwrap();
        assert!(
            sweep.in_doubt.is_empty(),
            "host completion precedes the ABI allocation cut"
        );
        let tape = hirsel_free::tape::parse(
            &Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink()
                .bytes,
        )
        .unwrap();
        assert_eq!(
            hirsel_free::record::decode(&tape.logs[&Vec::new()])
                .unwrap()
                .iter()
                .filter(|record| matches!(record, hirsel_free::record::Record::Completion { .. }))
                .count(),
            2
        );
    }
}
