//! Native telemetry derives context from span IDs already carried by the tape.
use super::*;
use crate::io::{GrantResult, SpanResult, TelemetryResult};
use crate::views::Slice;

unsafe fn bytes(ctx: *mut TaskCtx, content: &[u8]) -> Slice {
    unsafe { crate::text::allocate_bytes(ctx, content) }
}
unsafe fn release(ctx: *mut TaskCtx, slice: Slice) {
    assert_eq!(
        unsafe {
            crate::release::hirsel_rt_v2_release(
                ctx,
                slice.owner,
                (*slice.owner).index(),
                std::ptr::null(),
                0,
                0,
            )
        },
        STATUS_OK
    );
}
unsafe fn begin(ctx: *mut TaskCtx, telemetry: u64, parent: Slice) -> u64 {
    let spec = unsafe { bytes(ctx, b"spec") };
    let mut out = SpanResult {
        tag: 99,
        padding: 99,
        value: 99,
    };
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_capture_begin(
                ctx,
                std::ptr::null_mut(),
                &mut out,
                &telemetry,
                parent.owner,
                parent.start,
                parent.length,
                spec.owner,
                spec.start,
                spec.length,
            )
        },
        STATUS_OK
    );
    assert_eq!((out.tag, out.padding), (0, 0));
    unsafe { release(ctx, spec) };
    out.value
}
unsafe fn context(ctx: *mut TaskCtx, telemetry: u64, span: u64) -> Slice {
    let mut out = std::mem::MaybeUninit::<Slice>::uninit();
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_capture_context(
                ctx,
                std::ptr::null_mut(),
                out.as_mut_ptr(),
                &telemetry,
                &span,
            )
        },
        STATUS_OK
    );
    unsafe { out.assume_init() }
}
unsafe fn program(ctx: *mut TaskCtx, mode: u8) {
    let mut grant = GrantResult::default();
    let mut alias = GrantResult::default();
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_telemetry(ctx, std::ptr::null_mut(), &mut grant, &1) },
        STATUS_OK
    );
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_telemetry(ctx, std::ptr::null_mut(), &mut alias, &1) },
        STATUS_OK
    );
    assert_eq!(
        (grant.tag, alias.tag),
        (u32::from(mode == 1), u32::from(mode == 1))
    );
    if mode == 1 {
        return;
    }
    if mode == 2 {
        let _role = unsafe { role::Guard::new(ctx) };
        scheduler(ctx).cancel(0, 7);
    }
    let telemetry = grant.value;
    let parent = unsafe {
        bytes(
            ctx,
            &[0; 24].into_iter().chain([1, 7, 7]).collect::<Vec<_>>(),
        )
    };
    let root = unsafe { begin(ctx, telemetry, parent) };
    unsafe { release(ctx, parent) };
    let parent = unsafe { context(ctx, telemetry, root) };
    let contents = unsafe { crate::io::slice_bytes(parent.owner, parent.start, parent.length) };
    assert_eq!(&contents[24..], &[1, 7, 7]);
    assert_ne!(&contents[..16], &[0; 16]);
    let mut invalid = std::mem::MaybeUninit::<Slice>::uninit();
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_capture_context(
                ctx,
                std::ptr::null_mut(),
                invalid.as_mut_ptr(),
                &alias.value,
                &root,
            )
        },
        STATUS_FAILURE
    );
    let mut child = unsafe { begin(ctx, telemetry, parent) };
    let child_context = unsafe { context(ctx, telemetry, child) };
    let child_bytes = unsafe {
        crate::io::slice_bytes(
            child_context.owner,
            child_context.start,
            child_context.length,
        )
    };
    assert_eq!(&child_bytes[..16], &contents[..16]);
    assert_ne!(&child_bytes[16..24], &contents[16..24]);
    assert_eq!(&child_bytes[24..], &contents[24..]);
    unsafe {
        release(ctx, child_context);
        release(ctx, parent);
    }
    let payload = unsafe { bytes(ctx, b"event") };
    let mut out = TelemetryResult { tag: 99, error: 99 };
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_capture_event(
                ctx,
                std::ptr::null_mut(),
                &mut out,
                &telemetry,
                &child,
                payload.owner,
                payload.start,
                payload.length,
            )
        },
        STATUS_OK
    );
    assert_eq!((out.tag, out.error), (0, 0));
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_capture_log(
                ctx,
                std::ptr::null_mut(),
                &mut out,
                &telemetry,
                payload.owner,
                payload.start,
                payload.length,
            )
        },
        STATUS_OK
    );
    assert_eq!((out.tag, out.error), (0, 0));
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_capture_measure(
                ctx,
                std::ptr::null_mut(),
                &mut out,
                &telemetry,
                payload.owner,
                payload.start,
                payload.length,
            )
        },
        STATUS_OK
    );
    assert_eq!((out.tag, out.error), (0, 0));
    unsafe { release(ctx, payload) };
    let status = unsafe {
        bytes(
            ctx,
            &vec![
                0;
                if mode == 3 {
                    crate::telemetry::kernel::PAYLOAD_LIMIT + 1
                } else {
                    2
                }
            ],
        )
    };
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_capture_finish(
                ctx,
                std::ptr::null_mut(),
                &mut out,
                &telemetry,
                &mut child,
                status.owner,
                status.start,
                status.length,
            )
        },
        STATUS_OK
    );
    assert_eq!(out.tag, u32::from(mode == 3));
    unsafe { release(ctx, status) };
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_capture_context(
                ctx,
                std::ptr::null_mut(),
                invalid.as_mut_ptr(),
                &telemetry,
                &child,
            )
        },
        STATUS_FAILURE
    );
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_close(ctx, root, 0, 0) },
        STATUS_OK
    );
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_capture_context(
                ctx,
                std::ptr::null_mut(),
                invalid.as_mut_ptr(),
                &telemetry,
                &root,
            )
        },
        STATUS_FAILURE
    );
}

#[test]
fn native_telemetry_abi_replays_contexts_aliases_close_and_finish_without_home_queries() {
    for width in [1, 8] {
        for durable in [false, true] {
            for mode in 0..4 {
                let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
                real.discard_history();
                real.set_telemetry(crate::telemetry::Capture::of(
                    hirsel_run_schema::ProviderContract::TelemetryNull,
                    13,
                    Some(1),
                ));
                if mode == 1 {
                    real.set_projections(Default::default());
                }
                let adapter = Arc::new(
                    real.into_concurrent()
                        .unwrap_or_else(|_| panic!("Send telemetry")),
                );
                let mut admitted = Some(adapter.clone());
                let recording =
                    super::super::record::Recording::new(b"native telemetry ABI", durable);
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
                            concurrent::Service::install(ctx, admitted.take().unwrap()).unwrap();
                            native::enable(ctx);
                        }
                        program(ctx, mode);
                        STATUS_OK
                    },
                );
                assert_eq!(
                    outcome.ended,
                    Ok(STATUS_OK),
                    "width{width} durable{durable} mode{mode}"
                );
                assert_eq!(outcome.pending, 0);
                assert_eq!(Arc::strong_count(&adapter), 1);
                drop(adapter);
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
                    if mode == 1 { 2 } else { 9 },
                    "context adds no host phase"
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
                            program(ctx, mode);
                            let _role = role::Guard::new(ctx);
                            super::super::replay::start(ctx);
                            unreachable!("telemetry replay beyond cut");
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
