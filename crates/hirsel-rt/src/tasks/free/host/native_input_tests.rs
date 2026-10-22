//! Actual stdin ABI, using task-owned delivery while an exclusive List grows.
use super::*;
use crate::io::{GrantResult, HandleResult};
use std::io::Read;
use std::sync::atomic::{AtomicU32, Ordering};

struct Input {
    cursor: std::io::Cursor<Vec<u8>>,
    calls: Arc<AtomicU32>,
    mode: u8,
}
impl Read for Input {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.calls.fetch_add(1, Ordering::Release);
        if self.mode == 4 {
            return Err(std::io::Error::other("input fault"));
        }
        self.cursor.read(bytes)
    }
}
unsafe fn program(ctx: *mut TaskCtx, mode: u8) {
    let mut first = GrantResult::default();
    let mut second = GrantResult::default();
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_stdin(ctx, std::ptr::null_mut(), &mut first, &1) },
        STATUS_OK
    );
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_stdin(ctx, std::ptr::null_mut(), &mut second, &1) },
        STATUS_OK
    );
    assert_eq!(first.tag, u32::from(mode == 1));
    assert_eq!(second.tag, first.tag);
    if mode == 1 {
        return;
    }
    assert_ne!(first.value, second.value);
    if mode == 2 {
        let _role = unsafe { role::Guard::new(ctx) };
        scheduler(ctx).cancel(0, 7);
    }
    if mode == 5 {
        let _role = unsafe { role::Guard::new(ctx) };
        unsafe { (*ctx).set_budgets(None, Some(64)) };
    }
    let mut buffer = unsafe { crate::text::allocate_bytes(ctx, b"prefix/").owner };
    for (word, max) in [(first.value, 3), (second.value, 100), (first.value, 100)] {
        let mut out = HandleResult::default();
        assert_eq!(
            unsafe {
                crate::io::hirsel_rt_v2_stdin_read(
                    ctx,
                    std::ptr::null_mut(),
                    &mut out,
                    &word,
                    &mut buffer,
                    if mode == 3 {
                        0
                    } else if mode == 5 {
                        100
                    } else {
                        max
                    },
                )
            },
            STATUS_OK
        );
        let (tag, error) = match mode {
            2 => (1, Some(1 | (7u64 << 32))),
            3 => (1, Some(2)),
            4 => (1, Some(1u64 << 32)),
            _ => (0, None),
        };
        assert_eq!((out.tag, out.padding), (tag, 0));
        if let Some(error) = error {
            assert_eq!(out.value, error);
        } else {
            assert!(out.value <= max);
        }
        assert!(unsafe { (*ctx).native_host_result.is_none() });
    }
    let count = unsafe { crate::alloc::payload(buffer).cast::<u64>().read() };
    let expected: &[u8] = if mode == 0 {
        b"prefix/input\0\xffbytes"
    } else {
        b"prefix/"
    };
    assert_eq!(
        unsafe { crate::io::slice_bytes(buffer, crate::alloc::sequence_slice_start(1), count) },
        expected
    );
    assert_eq!(
        unsafe {
            crate::release::hirsel_rt_v2_release(
                ctx,
                buffer,
                (*buffer).index(),
                std::ptr::null(),
                0,
                0,
            )
        },
        STATUS_OK
    );
}

#[test]
fn native_input_abi_replays_reads_aliases_errors_and_cancellation_without_live_input() {
    for width in [1, 8] {
        for durable in [false, true] {
            for mode in 0..5 {
                let calls = Arc::new(AtomicU32::new(0));
                let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
                real.discard_history();
                real.set_send_stdin(Input {
                    cursor: std::io::Cursor::new(b"input\0\xffbytes".to_vec()),
                    calls: calls.clone(),
                    mode,
                });
                if mode == 1 {
                    real.set_projections(Default::default());
                }
                let mut adapter = Some(Arc::new(
                    real.into_concurrent()
                        .unwrap_or_else(|_| panic!("Send input")),
                ));
                let recording = super::super::record::Recording::new(b"native input ABI", durable);
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
                let reads = if mode == 0 || mode == 4 { 3 } else { 0 };
                assert_eq!(calls.load(Ordering::Acquire), reads);
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
                            unreachable!("input replay beyond cut");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    assert_eq!(outcome.pending, 0);
                    assert_eq!(replay.finish().unwrap(), projection);
                    assert_eq!(calls.load(Ordering::Acquire), reads);
                }
            }
        }
    }
}

#[test]
fn native_input_abi_append_stop_follows_the_completed_input_effect() {
    for width in [1, 8] {
        let calls = Arc::new(AtomicU32::new(0));
        let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
        real.discard_history();
        real.set_send_stdin(Input {
            cursor: std::io::Cursor::new(b"input\0\xffbytes".to_vec()),
            calls: calls.clone(),
            mode: 5,
        });
        let adapter = Arc::new(
            real.into_concurrent()
                .unwrap_or_else(|_| panic!("Send input")),
        );
        let mut admitted = Some(adapter.clone());
        let recording = super::super::record::Recording::new(b"input append allocation cut", false);
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
                program(ctx, 5);
                unreachable!("append must exceed the memory budget");
            },
        );
        assert!(
            matches!(outcome.ended, Err(Stop::BudgetExceeded(_))),
            "{outcome:?}"
        );
        assert_eq!(calls.load(Ordering::Acquire), 1);
        assert_eq!(outcome.pending, 0);
        assert_eq!(Arc::strong_count(&adapter), 1);
        assert_eq!(Arc::strong_count(&recording), 1);
        let (_, sweep) = recording
            .recorder
            .stop("memory budget exceeded", None)
            .unwrap()
            .unwrap();
        assert!(sweep.in_doubt.is_empty());
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
            3
        );
    }
}
