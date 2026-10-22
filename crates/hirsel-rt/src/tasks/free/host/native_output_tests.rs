//! Native output returns and projections, including writes that fail after an effect.
use super::*;
use crate::io::{GrantResult, IoError, UnitResult};
use std::io::Write;
use std::sync::Mutex;

struct Output {
    bytes: Vec<u8>,
    mode: u8,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.mode == 3 {
            let left = 5usize.saturating_sub(self.bytes.len());
            if left == 0 {
                return Err(std::io::Error::other("partial output"));
            }
            let count = bytes.len().min(left);
            self.bytes.extend_from_slice(&bytes[..count]);
            return Ok(count);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.mode == 4 {
            Err(std::io::Error::other("flush failed after write"))
        } else {
            Ok(())
        }
    }
}

unsafe fn program(ctx: *mut TaskCtx, mode: u8) {
    let mut stdout = GrantResult::default();
    let mut stderr = GrantResult::default();
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_stdout(ctx, std::ptr::null_mut(), &mut stdout, &1) },
        STATUS_OK
    );
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_stderr(ctx, std::ptr::null_mut(), &mut stderr, &1) },
        STATUS_OK
    );
    assert_eq!(stdout.tag, u32::from(mode == 1));
    assert_eq!(stderr.tag, stdout.tag);
    assert_eq!((stdout.padding, stderr.padding), (0, 0));
    if mode == 1 {
        assert_eq!((stdout.value, stderr.value), (0, 0));
        return;
    }
    assert_ne!(stdout.value, stderr.value, "distinct grant identities");
    {
        let _role = unsafe { role::Guard::new(ctx) };
        if super::super::replay::on(ctx) {
            assert_eq!(
                replay_created_identity(ctx, stdout.value).unwrap(),
                replay_created_identity(ctx, stderr.value).unwrap()
            );
        }
        if mode == 2 {
            scheduler(ctx).cancel(0, 7);
        }
    }
    let expected = UnitResult::of(match mode {
        2 => Err(IoError::Canceled(7)),
        3 | 4 => Err(IoError::Fault(1)),
        _ => Ok(()),
    });
    for (is_stderr, handle, content) in [
        (false, stdout.value, &b"stdout\0\xff"[..]),
        (true, stderr.value, &b"stderr\n"[..]),
    ] {
        let bytes = unsafe { crate::text::allocate_bytes(ctx, content) };
        let mut result = UnitResult {
            tag: 99,
            error: 99,
            code: 99,
        };
        let status = if is_stderr {
            unsafe {
                crate::io::hirsel_rt_v2_err_write(
                    ctx,
                    std::ptr::null_mut(),
                    &mut result,
                    &handle,
                    bytes.owner,
                    bytes.start,
                    bytes.length,
                )
            }
        } else {
            unsafe {
                crate::io::hirsel_rt_v2_out_print(
                    ctx,
                    std::ptr::null_mut(),
                    &mut result,
                    &handle,
                    bytes.owner,
                    bytes.start,
                    bytes.length,
                )
            }
        };
        assert_eq!(status, STATUS_OK);
        assert_eq!(result, expected);
        assert_eq!(
            unsafe {
                crate::release::hirsel_rt_v2_release(
                    ctx,
                    bytes.owner,
                    (*bytes.owner).index(),
                    std::ptr::null(),
                    0,
                    0,
                )
            },
            STATUS_OK
        );
    }
}

#[test]
fn native_output_abi_replays_shared_sink_prefixes_without_live_output() {
    for width in [1, 8] {
        for durable in [false, true] {
            // Success, denied grants, canceled writes, partial write and flush failure.
            for mode in 0..5 {
                let storage = Arc::new(Mutex::new(Output {
                    bytes: Vec::new(),
                    mode,
                }));
                let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
                real.discard_history();
                real.set_send_output(crate::host::SendSink::shared(storage.clone()));
                // Distinct wrappers of the same storage must use one logical sink.
                real.set_send_stderr(crate::host::SendSink::shared(storage.clone()));
                if mode == 1 {
                    real.deny_output();
                }
                let adapter = Arc::new(
                    real.into_concurrent()
                        .unwrap_or_else(|_| panic!("Send output")),
                );
                let mut admitted = Some(adapter.clone());
                let recording = super::super::record::Recording::new(b"native output ABI", durable);
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
                    "width={width} durable={durable} mode={mode}"
                );
                assert_eq!(outcome.pending, 0);
                assert_eq!(Arc::strong_count(&adapter), 1);
                drop(adapter);
                let actual = storage.lock().unwrap().bytes.clone();
                let expected = match mode {
                    1 | 2 => &b""[..],
                    3 => &b"stdou"[..],
                    _ => &b"stdout\0\xffstderr\n"[..],
                };
                assert_eq!(actual, expected);
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
                if mode == 1 {
                    assert!(projection.outputs.is_empty());
                } else {
                    assert_eq!(projection.outputs.len(), 1, "aliased sinks");
                    assert_eq!(projection.outputs.values().next().unwrap(), &actual);
                }
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
                            unreachable!("output replay beyond cut");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    assert_eq!(outcome.pending, 0);
                    assert_eq!(replay.finish().unwrap(), projection);
                    assert_eq!(
                        storage.lock().unwrap().bytes,
                        actual,
                        "replay writes no host output"
                    );
                }
            }
        }
    }
}

#[test]
fn native_output_footprints_and_counts_refuse_missing_or_mismatched_evidence() {
    use crate::io_keys::{Key, Kind, NAMESPACE};
    let print = Request::Print {
        stdout: 77,
        bytes: vec![1, 2, 3],
    };
    for file_backed in [false, true] {
        let keys =
            native::request_keys(&print, |_| Ok((Key(17), Kind::Sink { file_backed }))).unwrap();
        let mut expected = vec![(Key(17), Mode::Exclusive)];
        if file_backed {
            expected.push((NAMESPACE, Mode::Shared));
        }
        assert_eq!(keys.keys(), KeySet::new(expected).keys());
    }
    assert!(
        native::request_keys(&print, |_| Err(Stop::Diverged("missing identity".into()))).is_err()
    );
    assert!(native::request_keys(&print, |_| Ok((Key(17), Kind::File))).is_err());
    assert!(checked_output(&print, &Ok(Completion::Unit(Ok(()))), None).is_err());
    assert!(checked_output(&print, &Ok(Completion::Unit(Ok(()))), Some(2)).is_err());
    assert!(
        checked_output(
            &print,
            &Ok(Completion::Unit(Err(IoError::Fault(1)))),
            Some(4)
        )
        .is_err()
    );
    assert!(
        checked_output(
            &print,
            &Ok(Completion::Unit(Err(IoError::Canceled(7)))),
            Some(1)
        )
        .is_err()
    );
    assert!(
        checked_output(
            &Request::Stdout { io: 1 },
            &Ok(Completion::Handle(Err(IoError::NotGranted))),
            Some(0)
        )
        .is_err()
    );
    assert_eq!(
        checked_output(
            &print,
            &Ok(Completion::Unit(Err(IoError::Fault(1)))),
            Some(2)
        )
        .unwrap(),
        Some(2)
    );
}
