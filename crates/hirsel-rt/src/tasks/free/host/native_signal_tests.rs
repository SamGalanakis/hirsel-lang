//! Signal arrivals are host observations; replay needs neither a source nor a Binding.
use super::*;
use crate::io::{GrantResult, SignalResult};
use hirsel_run_schema::SignalKind;

unsafe fn wait(ctx: *mut TaskCtx, signals: u64, kinds: &[SignalKind]) -> SignalResult {
    let list = unsafe { crate::alloc::allocate_sequence(ctx, 0, kinds.len() as u64, 4, 4) };
    for (index, kind) in kinds.iter().enumerate() {
        unsafe {
            crate::alloc::payload(list)
                .add(crate::alloc::sequence_elements_offset(4) as usize)
                .cast::<u32>()
                .add(index)
                .write(kind.ordinal())
        };
    }
    let mut out = SignalResult {
        tag: 99,
        value: 99,
        cause: 99,
    };
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_signal_wait(ctx, std::ptr::null_mut(), &mut out, &signals, list)
        },
        STATUS_OK
    );
    assert_eq!(
        unsafe {
            crate::release::hirsel_rt_v2_release(ctx, list, (*list).index(), std::ptr::null(), 0, 0)
        },
        STATUS_OK
    );
    out
}

unsafe fn program(ctx: *mut TaskCtx, mode: u8) {
    use SignalKind::*;
    let mut first = GrantResult::default();
    let mut second = GrantResult::default();
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_signals(ctx, std::ptr::null_mut(), &mut first, &1) },
        STATUS_OK
    );
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_signals(ctx, std::ptr::null_mut(), &mut second, &1) },
        STATUS_OK
    );
    assert_eq!(first.tag, u32::from(mode == 1));
    assert_eq!(second.tag, first.tag);
    if mode == 1 {
        assert_eq!((first.value, second.value), (0, 0));
        return;
    }
    assert_ne!(first.value, second.value);
    if mode == 2 {
        let _role = unsafe { role::Guard::new(ctx) };
        scheduler(ctx).cancel(0, 7);
    }
    let result = match mode {
        2 => unsafe { wait(ctx, first.value, &[Terminate]) },
        3 => unsafe { wait(ctx, first.value, &[]) },
        4 => unsafe { wait(ctx, first.value, &[Hangup]) },
        _ => unsafe { wait(ctx, first.value, &[Interrupt, Terminate]) },
    };
    assert_eq!(
        result,
        match mode {
            2 => SignalResult {
                tag: 1,
                value: 0,
                cause: 7
            },
            3 => SignalResult {
                tag: 1,
                value: 2,
                cause: 0
            },
            4 => SignalResult {
                tag: 1,
                value: 1,
                cause: 0
            },
            _ => SignalResult {
                tag: 0,
                value: Terminate.ordinal(),
                cause: 0
            },
        }
    );
    if mode == 0 {
        assert_eq!(
            unsafe { wait(ctx, second.value, &[Interrupt]) },
            SignalResult {
                tag: 0,
                value: Interrupt.ordinal(),
                cause: 0
            }
        );
    }
}

#[test]
fn native_signal_abi_replays_idle_arrivals_errors_and_cancellation_without_a_source() {
    use SignalKind::*;
    for width in [1, 8] {
        for durable in [false, true] {
            for mode in 0..5 {
                let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
                real.discard_history();
                if mode != 1 {
                    real.set_signal_grant(&hirsel_run_schema::SignalsGrant {
                        provider: "signals.script@1".into(),
                        kinds: vec![Interrupt, Terminate],
                        script: Some(if mode == 0 {
                            vec![Terminate, Interrupt]
                        } else {
                            Vec::new()
                        }),
                    })
                    .unwrap();
                }
                let adapter = Arc::new(
                    real.into_concurrent()
                        .unwrap_or_else(|_| panic!("Send signal source")),
                );
                let mut admitted = Some(adapter.clone());
                let recording = super::super::record::Recording::new(b"native signal ABI", durable);
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
                    records.iter().any(|record| matches!(
                        record,
                        hirsel_free::record::Record::Blocked { .. }
                    )),
                    mode == 0
                );
                for width in [1, 8] {
                    let replay = super::super::replay::Replay::new(&bytes).unwrap();
                    let install = replay.clone();
                    let mut run = crate::host::BodyRun::new(width, false);
                    run.free = true;
                    unsafe { run.use_physical_dispatch() };
                    let control = run.control();
                    let outcome = run.run(
                        move |ctx| unsafe {
                            install.root(ctx);
                            native::enable(ctx);
                        },
                        move |ctx| unsafe {
                            program(ctx, mode);
                            let _role = role::Guard::new(ctx);
                            super::super::replay::start(ctx);
                            unreachable!("signal replay beyond cut");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    // A terminal cut can reach home before the root's idle
                    // callback retires. Observe reclamation, not that race.
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while control.status().reclamation_pending != 0 {
                        assert!(
                            std::time::Instant::now() < until,
                            "finite signal replay did not retire"
                        );
                        std::thread::yield_now();
                    }
                    assert_eq!(control.status().reclamation_pending, 0);
                    assert_eq!(control.status().lost_permits, 0);
                    assert_eq!(replay.finish().unwrap(), projection);
                }
            }
        }
    }
}
