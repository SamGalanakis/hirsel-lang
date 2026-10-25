//! Native file entries use only Binding's immutable names; effects are direct.
use super::*;
use crate::io::{BytesResult, HandleResult, UnitResult};
use crate::io_concurrent::ConcurrentAdapter;

struct NamesOnly;
impl crate::io::Adapter for NamesOnly {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        panic!("home preflight")
    }
    fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
        panic!("home effect")
    }
    fn ready(&mut self) -> Result<crate::io::Ready, AdapterError> {
        panic!("home poll")
    }
}

unsafe fn release_bytes(ctx: *mut TaskCtx, slice: crate::views::Slice) {
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

unsafe fn file_program(ctx: *mut TaskCtx) {
    let (root, path) = {
        let _role = unsafe { role::Guard::new(ctx) };
        let binding = unsafe { crate::io::Binding::get(ctx) };
        (binding.intern_path(""), binding.intern_path("data"))
    };
    let mut out = HandleResult {
        tag: 99,
        padding: 99,
        value: 99,
    };
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_dir(ctx, std::ptr::null_mut(), &mut out, &1, &root) },
        STATUS_OK
    );
    assert_eq!(out.tag, 0);
    let directory = out.value;
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_file_create(
                ctx,
                std::ptr::null_mut(),
                &mut out,
                &directory,
                &path,
            )
        },
        STATUS_OK
    );
    assert_eq!(out.tag, 0);
    let mut file = out.value;
    let expected = b"native Free file bytes\0\xff";
    let bytes = unsafe { crate::text::allocate_bytes(ctx, expected) };
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_file_write(
                ctx,
                std::ptr::null_mut(),
                &mut out,
                &directory,
                &mut file,
                bytes.owner,
                bytes.start,
                bytes.length,
            )
        },
        STATUS_OK
    );
    assert_eq!(
        out,
        HandleResult {
            tag: 0,
            padding: 0,
            value: expected.len() as u64
        }
    );
    unsafe { release_bytes(ctx, bytes) };
    let mut unit = UnitResult {
        tag: 99,
        error: 99,
        code: 99,
    };
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_file_seek(
                ctx,
                std::ptr::null_mut(),
                &mut unit,
                &directory,
                &mut file,
                0,
            )
        },
        STATUS_OK
    );
    assert_eq!(
        unit,
        UnitResult {
            tag: 0,
            error: 0,
            code: 0
        }
    );
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_file_size(
                ctx,
                std::ptr::null_mut(),
                &mut out,
                &directory,
                &path,
            )
        },
        STATUS_OK
    );
    assert_eq!(out.value, expected.len() as u64);
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_file_open(
                ctx,
                std::ptr::null_mut(),
                &mut out,
                &directory,
                &path,
            )
        },
        STATUS_OK
    );
    assert_eq!(out.tag, 0);
    let alias = out.value;
    let mut alias = alias;
    let mut buffer = unsafe { crate::text::allocate_bytes(ctx, b"prefix/").owner };
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_file_read(
                ctx,
                std::ptr::null_mut(),
                &mut out,
                &directory,
                &mut alias,
                &mut buffer,
                100,
            )
        },
        STATUS_OK
    );
    assert_eq!(
        out,
        HandleResult {
            tag: 0,
            padding: 0,
            value: expected.len() as u64
        }
    );
    let count = unsafe { crate::alloc::payload(buffer).cast::<u64>().read() };
    assert_eq!(
        unsafe { crate::io::slice_bytes(buffer, crate::alloc::sequence_slice_start(1), count) },
        [b"prefix/", &expected[..]].concat()
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

    assert_ne!(file, alias);
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_close(ctx, file, 0, 0) },
        STATUS_OK
    );
    let mut read = std::mem::MaybeUninit::<BytesResult>::uninit();
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_file_read_bytes(
                ctx,
                std::ptr::null_mut(),
                read.as_mut_ptr(),
                &directory,
                &path,
            )
        },
        STATUS_OK
    );
    let read = unsafe { read.assume_init() };
    assert_eq!(read.tag, 0);
    assert_eq!(
        unsafe { crate::io::slice_bytes(read.value.owner, read.value.start, read.value.length) },
        expected
    );
    unsafe { release_bytes(ctx, read.value) };
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_close(ctx, alias, 0, 0) },
        STATUS_OK
    );
}

#[test]
fn native_file_abi_records_direct_effects_and_replays_without_mount_or_home_adapter() {
    for width in [1, 8] {
        for durable in [false, true] {
            let path = std::env::temp_dir().join(format!(
                "hirsel-f1-native-file-{}-{width}-{durable}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            let mut real = crate::io_real::RealIo::grant(&path).unwrap();
            real.discard_history();
            let adapter = Arc::new(
                real.into_concurrent()
                    .unwrap_or_else(|_| panic!("Send file adapter")),
            );
            let mut admitted = Some(adapter.clone());
            let recording = super::super::record::Recording::new(b"native file ABI", durable);
            let install = recording.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.adapter = Some(Box::new(NamesOnly));
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
                    file_program(ctx);
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            assert_eq!(Arc::strong_count(&adapter), 1);
            drop(adapter);
            assert_eq!(
                std::fs::read(path.join("data")).unwrap(),
                b"native Free file bytes\0\xff"
            );
            std::fs::remove_dir_all(path).unwrap();
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
                run.adapter = Some(Box::new(NamesOnly));
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        install.root(ctx);
                        native::enable(ctx);
                    },
                    move |ctx| unsafe {
                        file_program(ctx);
                        let _role = role::Guard::new(ctx);
                        super::super::replay::start(ctx);
                        unreachable!("native file replay beyond cut");
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

#[test]
fn native_file_keys_validate_all_resource_metadata_and_namespace_modes() {
    use crate::io_keys::{Key, Kind, NAMESPACE};
    let identity = |word| match word {
        17 => Ok((Key(7), Kind::File)),
        18 => Ok((Key(8), Kind::File)),
        _ => Err(Stop::Diverged("unmapped handle".into())),
    };
    assert_eq!(
        native::request_keys(
            &Request::FileRead {
                directory: 17,
                file: 18,
                max: 8
            },
            identity
        )
        .unwrap(),
        KeySet::new(vec![(NAMESPACE, Mode::Shared), (Key(8), Mode::Exclusive)])
    );
    assert_eq!(
        native::request_keys(
            &Request::Commit {
                directory: 17,
                file: 18
            },
            identity
        )
        .unwrap(),
        KeySet::new(vec![
            (NAMESPACE, Mode::Exclusive),
            (Key(8), Mode::Exclusive)
        ])
    );
    assert!(
        native::request_keys(
            &Request::FileRead {
                directory: 999,
                file: 18,
                max: 8
            },
            identity
        )
        .is_err()
    );
    assert!(
        native::request_keys(
            &Request::FileRead {
                directory: 17,
                file: 999,
                max: 8
            },
            identity
        )
        .is_err()
    );
    // Native span and child closes now share the existing Other identity kind.
    assert_eq!(
        native::request_keys(&Request::Close { handle: 18 }, |_| Ok((
            Key(8),
            Kind::Other
        )))
        .unwrap(),
        KeySet::new(vec![(Key(8), Mode::Exclusive)])
    );
    // Item2 admits a half close with its own exclusive and parent shared keys.
    assert_eq!(
        native::request_keys(&Request::Close { handle: 18 }, |_| Ok((
            Key(8),
            Kind::Half { stream: Key(9) }
        )))
        .unwrap(),
        KeySet::new(vec![(Key(8), Mode::Exclusive), (Key(9), Mode::Shared)])
    );
    for (resource, parent) in [(8, 0), (8, 1), (8, 2), (8, 3), (8, 8), (0, 9), (3, 9)] {
        assert!(
            native::request_keys(&Request::Close { handle: 18 }, |_| Ok((
                Key(resource),
                Kind::Half {
                    stream: Key(parent)
                }
            )))
            .is_err()
        );
    }
    for file_backed in [false, true] {
        assert!(
            native::request_keys(&Request::Close { handle: 18 }, |_| Ok((
                Key(8),
                Kind::Sink { file_backed }
            )))
            .is_err()
        );
    }
}

struct ExtraDomain;
impl ConcurrentAdapter for ExtraDomain {
    fn keys(&self, _: &Request) -> KeySet {
        KeySet::new(vec![
            (crate::io_keys::NAMESPACE, Mode::Shared),
            (crate::io_keys::COUPLED, Mode::Exclusive),
        ])
    }
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        panic!("unadmitted preflight")
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        panic!("unadmitted effect")
    }
}
#[test]
fn native_extra_namespace_domain_refuses_before_prepare_or_effect() {
    let recording = super::super::record::Recording::new(b"unadmitted native domain", false);
    let install = recording.clone();
    let mut run = crate::host::BodyRun::new(8, false);
    run.free = true;
    unsafe { run.use_physical_dispatch() };
    let outcome = run.run(
        move |ctx| unsafe {
            super::super::enable(ctx);
            install.root(ctx);
        },
        move |ctx| unsafe {
            let _role = role::Guard::new(ctx);
            {
                let _segment = role::Segment::new(ctx);
                concurrent::Service::install(ctx, Arc::new(ExtraDomain)).unwrap();
                native::enable(ctx);
            }
            let _ = crate::stop::check(
                ctx,
                crate::host_waits::issue(
                    ctx,
                    Request::Directory {
                        io: 1,
                        path: String::new(),
                    },
                ),
            );
            unreachable!("extra domain was admitted");
        },
    );
    assert_eq!(
        outcome.ended,
        Err(Stop::Refused(
            "native host footprint has unadmitted domains".into()
        ))
    );
    assert_eq!(outcome.pending, 0);
    assert!(recording.trace().is_empty());
    let (_, sweep) = recording.recorder.stop("refused", None).unwrap().unwrap();
    assert!(sweep.withdrawn.is_empty());
    assert!(sweep.in_doubt.is_empty());
}

/// Tokio's file write vectors (tokio/tests/fs_file.rs:41-100 at ce685ee:
/// basic_write, basic_write_and_shutdown, write_vectored as two writes, and
/// rewind_seek_position) through the recording-off native entry. Each write
/// is a blocking job handed off and delivered to its parked owner once.
unsafe fn live_vectors(ctx: *mut TaskCtx) {
    const HELLO: &[u8] = b"hello world...";
    let names = {
        let _role = unsafe { role::Guard::new(ctx) };
        let binding = unsafe { crate::io::Binding::get(ctx) };
        ["", "basic", "twice", "rewind"].map(|name| binding.intern_path(name))
    };
    let mut out = HandleResult {
        tag: 99,
        padding: 99,
        value: 99,
    };
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_io_dir(ctx, std::ptr::null_mut(), &mut out, &1, &names[0])
        },
        STATUS_OK
    );
    assert_eq!(out.tag, 0);
    let directory = out.value;
    let create = |path: &u64| {
        let mut out = HandleResult {
            tag: 99,
            padding: 99,
            value: 99,
        };
        assert_eq!(
            unsafe {
                crate::io::hirsel_rt_v2_file_create(
                    ctx,
                    std::ptr::null_mut(),
                    &mut out,
                    &directory,
                    path,
                )
            },
            STATUS_OK
        );
        assert_eq!(out.tag, 0);
        out.value
    };
    let write = |file: &mut u64, expected: &[u8]| {
        let bytes = unsafe { crate::text::allocate_bytes(ctx, expected) };
        let mut out = HandleResult {
            tag: 99,
            padding: 99,
            value: 99,
        };
        assert_eq!(
            unsafe {
                crate::io::hirsel_rt_v2_file_write(
                    ctx,
                    std::ptr::null_mut(),
                    &mut out,
                    &directory,
                    file,
                    bytes.owner,
                    bytes.start,
                    bytes.length,
                )
            },
            STATUS_OK
        );
        assert_eq!(
            out,
            HandleResult {
                tag: 0,
                padding: 0,
                value: expected.len() as u64
            }
        );
        unsafe { release_bytes(ctx, bytes) };
    };
    let close = |file: u64| {
        assert_eq!(
            unsafe { crate::io::hirsel_rt_v2_io_close(ctx, file, 0, 0) },
            STATUS_OK
        );
    };
    let read = |path: &u64| {
        let mut read = std::mem::MaybeUninit::<BytesResult>::uninit();
        assert_eq!(
            unsafe {
                crate::io::hirsel_rt_v2_file_read_bytes(
                    ctx,
                    std::ptr::null_mut(),
                    read.as_mut_ptr(),
                    &directory,
                    path,
                )
            },
            STATUS_OK
        );
        let read = unsafe { read.assume_init() };
        assert_eq!(read.tag, 0);
        let bytes = unsafe {
            crate::io::slice_bytes(read.value.owner, read.value.start, read.value.length)
        }
        .to_vec();
        unsafe { release_bytes(ctx, read.value) };
        bytes
    };
    // basic_write / basic_write_and_shutdown: the closed file holds the write.
    let mut basic = create(&names[1]);
    write(&mut basic, HELLO);
    close(basic);
    assert_eq!(read(&names[1]), HELLO);
    // write_vectored: consecutive writes advance one cursor.
    let mut twice = create(&names[2]);
    write(&mut twice, HELLO);
    write(&mut twice, HELLO);
    close(twice);
    assert_eq!(read(&names[2]), [HELLO, HELLO].concat());
    // rewind_seek_position. Deviation from Tokio: Hirsel writes always append
    // regardless of the seek position (docs/spec/core-library.md, file I/O).
    let mut rewind = create(&names[3]);
    write(&mut rewind, HELLO);
    let mut unit = UnitResult {
        tag: 99,
        error: 99,
        code: 99,
    };
    assert_eq!(
        unsafe {
            crate::io::hirsel_rt_v2_file_seek(
                ctx,
                std::ptr::null_mut(),
                &mut unit,
                &directory,
                &mut rewind,
                0,
            )
        },
        STATUS_OK
    );
    assert_eq!(unit.tag, 0);
    write(&mut rewind, b"J");
    close(rewind);
    assert_eq!(read(&names[3]), b"hello world...J");
}

#[test]
fn native_live_file_writes_follow_tokio_vectors_and_deliver_each_job_once() {
    for width in [1, 8] {
        let path = std::env::temp_dir().join(format!(
            "hirsel-live-file-vectors-{}-{width}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        let mut real = crate::io_real::RealIo::grant(&path).unwrap();
        real.discard_history();
        let adapter = Arc::new(
            real.into_concurrent()
                .unwrap_or_else(|_| panic!("Send file adapter")),
        );
        let mut admitted = Some(adapter.clone());
        let jobs = Arc::new(std::sync::Mutex::new(None));
        let probe = jobs.clone();
        let mut run = crate::host::BodyRun::new(width, false);
        run.free = true;
        run.adapter = Some(Box::new(NamesOnly));
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe { super::super::enable(ctx) },
            move |ctx| unsafe {
                {
                    let _role = role::Guard::new(ctx);
                    let _segment = role::Segment::new(ctx);
                    concurrent::Service::install(ctx, admitted.take().unwrap()).unwrap();
                    native::enable(ctx);
                    let io = (*ctx).run_ref().free_io.get().unwrap();
                    *probe.lock().unwrap() = Some(io.jobs.probe());
                }
                live_vectors(ctx);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        // The run released its one adapter owner; every job returned, was
        // delivered exactly once and gave its credit back.
        assert_eq!(Arc::strong_count(&adapter), 1);
        let jobs = jobs.lock().unwrap().take().unwrap();
        let stats = jobs.stats();
        let submitted = stats.submitted.load(std::sync::atomic::Ordering::Relaxed);
        assert!(submitted >= 4, "{submitted} jobs");
        assert_eq!(
            stats.returned.load(std::sync::atomic::Ordering::Relaxed),
            submitted
        );
        assert_eq!(
            stats.delivered.load(std::sync::atomic::Ordering::Relaxed),
            submitted
        );
        assert_eq!(
            stats.quarantined.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(jobs.held(), 0);
        std::fs::remove_dir_all(path).unwrap();
    }
}
