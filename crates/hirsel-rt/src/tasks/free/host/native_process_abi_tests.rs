//! Actual process ABI calls use immutable Names, not a home process provider.
use super::*;
use crate::io::{Adapter, GrantResult};
use crate::process::{PipelineResult, ProcessResult};
use crate::views::Slice;

struct NamesOnly;
impl Adapter for NamesOnly {
    fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
        panic!("home process effect")
    }
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        panic!("home preflight")
    }
    fn process(&mut self, _: fn() -> Box<dyn crate::process::ProcessHost>) {
        panic!("home provider setup")
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CommandAbi {
    program: u64,
    args: *mut crate::Header,
    directory: [u64; 2],
    env: *mut crate::Header,
    own_group: u64,
}
const _: () = assert!(std::mem::size_of::<CommandAbi>() == 48);
unsafe fn release(ctx: *mut TaskCtx, block: *mut crate::Header) {
    assert_eq!(
        unsafe {
            crate::release::hirsel_rt_v2_release(
                ctx,
                block,
                (*block).index(),
                std::ptr::null(),
                0,
                0,
            )
        },
        STATUS_OK
    );
}
unsafe fn expect(ctx: *mut TaskCtx, slice: Slice, bytes: &[u8]) {
    assert_eq!(
        unsafe { crate::io::slice_bytes(slice.owner, slice.start, slice.length) },
        bytes
    );
    unsafe { release(ctx, slice.owner) };
}
unsafe fn command(ctx: *mut TaskCtx, program: &str, script: &[u8]) -> CommandAbi {
    let _role = unsafe { role::Guard::new(ctx) };
    CommandAbi {
        program: crate::paths::names(ctx).paths.intern(program),
        args: unsafe { crate::text::allocate_bytes_list(ctx, &[b"-c".to_vec(), script.to_vec()]) },
        directory: [0; 2],
        env: std::ptr::null_mut(),
        own_group: 0,
    }
}
unsafe fn program(ctx: *mut TaskCtx, mode: u8) {
    let mut grant = GrantResult::default();
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_process(ctx, std::ptr::null_mut(), &mut grant, &1) },
        STATUS_OK
    );
    assert_eq!(grant.tag, u32::from(mode == 1));
    if mode == 1 {
        return;
    }
    let cmd = unsafe {
        command(
            ctx,
            if mode == 3 { "/bin/false" } else { "/bin/sh" },
            b"printf 'process output'; printf 'process error' >&2",
        )
    };
    let input = unsafe { crate::text::allocate_bytes(ctx, b"") };
    if mode == 2 {
        let _role = unsafe { role::Guard::new(ctx) };
        scheduler(ctx).cancel(0, 7);
    }
    let deadline = [0u64; 3];
    if mode == 4 {
        let _role = unsafe { role::Guard::new(ctx) };
        unsafe { (*ctx).set_budgets(None, Some(50)) };
    }
    let mut out = std::mem::MaybeUninit::<ProcessResult>::uninit();
    assert_eq!(
        unsafe {
            crate::process::hirsel_rt_v2_proc_run(
                ctx,
                std::ptr::null_mut(),
                out.as_mut_ptr(),
                &grant.value,
                std::ptr::from_ref(&cmd).cast(),
                input.owner,
                input.start,
                input.length,
                100,
                deadline.as_ptr().cast(),
            )
        },
        STATUS_OK
    );
    let out = unsafe { out.assume_init() };
    assert_eq!((out.tag, out.padding), (u32::from(mode != 0), 0));
    if mode == 0 {
        assert_eq!((out.kind, out.code), (0, 0));
        unsafe {
            expect(ctx, out.stdout, b"process output");
            expect(ctx, out.stderr, b"process error");
        }
        let first = unsafe { command(ctx, "/bin/sh", b"printf 'pipeline output'") };
        let second = unsafe { command(ctx, "/bin/sh", b"cat; printf 'pipeline error' >&2") };
        let stages = unsafe { crate::alloc::allocate_sequence(ctx, 0, 2, 48, 8) };
        unsafe {
            let at = crate::alloc::payload(stages)
                .add(crate::alloc::sequence_elements_offset(8) as usize)
                .cast::<CommandAbi>();
            at.write(first);
            at.add(1).write(second);
        }
        let mut pipeline = std::mem::MaybeUninit::<PipelineResult>::uninit();
        assert_eq!(
            unsafe {
                crate::process::hirsel_rt_v2_proc_pipeline(
                    ctx,
                    std::ptr::null_mut(),
                    pipeline.as_mut_ptr(),
                    &grant.value,
                    stages,
                    input.owner,
                    input.start,
                    input.length,
                    100,
                    deadline.as_ptr().cast(),
                )
            },
            STATUS_OK
        );
        let pipeline = unsafe { pipeline.assume_init() };
        assert_eq!((pipeline.tag, pipeline.padding), (0, 0));
        let statuses = pipeline.statuses as *mut crate::Header;
        assert_eq!(
            unsafe { crate::alloc::payload(statuses).cast::<u64>().read() },
            2
        );
        let at = crate::alloc::payload(statuses)
            .wrapping_add(crate::alloc::sequence_elements_offset(4) as usize)
            .cast::<u64>();
        assert_eq!(unsafe { [at.read(), at.add(1).read()] }, [0, 0]);
        let mut spawned = crate::process::ChildWordResult::default();
        assert_eq!(
            unsafe {
                crate::process::hirsel_rt_v2_proc_spawn(
                    ctx,
                    std::ptr::null_mut(),
                    &mut spawned,
                    &grant.value,
                    std::ptr::from_ref(&cmd).cast(),
                )
            },
            STATUS_OK
        );
        assert_eq!(spawned.tag, 0);
        let mut child = spawned.value;
        for stderr in [false, true] {
            let mut buffer = unsafe { crate::text::allocate_bytes(ctx, b"").owner };
            for _ in 0..4 {
                let status = if stderr {
                    unsafe {
                        crate::process::hirsel_rt_v2_child_read_stderr(
                            ctx,
                            std::ptr::null_mut(),
                            &mut spawned,
                            &grant.value,
                            &mut child,
                            &mut buffer,
                            100,
                        )
                    }
                } else {
                    unsafe {
                        crate::process::hirsel_rt_v2_child_read_stdout(
                            ctx,
                            std::ptr::null_mut(),
                            &mut spawned,
                            &grant.value,
                            &mut child,
                            &mut buffer,
                            100,
                        )
                    }
                };
                assert_eq!(status, STATUS_OK);
                assert_eq!(spawned.tag, 0);
                if spawned.value == 0 {
                    break;
                }
            }
            let length = unsafe { crate::alloc::payload(buffer).cast::<u64>().read() };
            assert_eq!(
                unsafe {
                    crate::io::slice_bytes(buffer, crate::alloc::sequence_slice_start(1), length)
                },
                if stderr {
                    &b"process error"[..]
                } else {
                    &b"process output"[..]
                }
            );
            unsafe { release(ctx, buffer) };
        }
        let mut waited = crate::process::ChildPairResult::default();
        assert_eq!(
            unsafe {
                crate::process::hirsel_rt_v2_child_wait(
                    ctx,
                    std::ptr::null_mut(),
                    &mut waited,
                    &grant.value,
                    &mut child,
                    deadline.as_ptr().cast(),
                )
            },
            STATUS_OK
        );
        assert_eq!(waited, crate::process::ChildPairResult::default());
        assert_eq!(child, 0);
        assert_eq!(
            unsafe {
                crate::process::hirsel_rt_v2_proc_spawn_pipeline(
                    ctx,
                    std::ptr::null_mut(),
                    &mut spawned,
                    &grant.value,
                    stages,
                    100,
                )
            },
            STATUS_OK
        );
        assert_eq!(spawned.tag, 0);
        let mut group = spawned.value;
        let mut grouped = std::mem::MaybeUninit::<crate::process::GroupResult>::uninit();
        assert_eq!(
            unsafe {
                crate::process::hirsel_rt_v2_proc_wait_group(
                    ctx,
                    std::ptr::null_mut(),
                    grouped.as_mut_ptr(),
                    &grant.value,
                    &mut group,
                    deadline.as_ptr().cast(),
                )
            },
            STATUS_OK
        );
        let grouped = unsafe { grouped.assume_init() };
        assert_eq!((grouped.tag, grouped.padding, group), (0, 0, 0));
        let group_statuses = grouped.statuses as *mut crate::Header;
        assert_eq!(
            unsafe { crate::alloc::payload(group_statuses).cast::<u64>().read() },
            2
        );
        let errors = grouped.stderr as *mut crate::Header;
        assert_eq!(
            unsafe { crate::alloc::payload(errors).cast::<u64>().read() },
            2
        );
        let at = crate::alloc::payload(errors)
            .wrapping_add(crate::alloc::sequence_elements_offset(8) as usize)
            .cast::<Slice>();
        let error = unsafe { at.add(1).read() };
        assert_eq!(
            unsafe { crate::io::slice_bytes(error.owner, error.start, error.length) },
            b"pipeline error"
        );
        unsafe {
            release(ctx, group_statuses);
            release(ctx, errors);
        }
        unsafe {
            expect(ctx, pipeline.stdout, b"pipeline output");
            expect(ctx, pipeline.stderr, b"pipeline error");
            release(ctx, statuses);
            release(ctx, stages);
            release(ctx, first.args);
            release(ctx, second.args);
        }
    } else {
        assert_eq!(
            (out.kind, out.code),
            if mode == 2 { (7, 7) } else { (0, 0) }
        );
    }
    unsafe {
        release(ctx, cmd.args);
        release(ctx, input.owner);
    }
    assert!(unsafe { (*ctx).native_host_result.is_none() });
}

#[test]
fn native_process_run_and_pipeline_abi_replay_without_a_home_process_provider() {
    for width in [1, 8] {
        for durable in [false, true] {
            for mode in 0..4 {
                let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
                real.discard_history();
                real.set_process_grant(&hirsel_run_schema::ProcessGrant {
                    timed: None,
                    provider: "process.host@1".into(),
                    rules: vec![hirsel_run_schema::ProcessRule {
                        program: "/bin/sh".into(),
                        env: vec![],
                    }],
                    runs: None,
                    children: None,
                });
                if mode == 1 {
                    real.set_projections(Default::default());
                }
                let adapter = Arc::new(
                    real.into_concurrent()
                        .unwrap_or_else(|_| panic!("Send process")),
                );
                let mut admitted = Some(adapter.clone());
                let recording =
                    super::super::record::Recording::new(b"native process ABI", durable);
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
                            program(ctx, mode);
                            let _role = role::Guard::new(ctx);
                            super::super::replay::start(ctx);
                            unreachable!("process ABI replay beyond cut");
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
fn native_process_abi_output_allocation_stop_follows_host_completion() {
    for width in [1, 8] {
        let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
        real.discard_history();
        real.set_process_grant(&hirsel_run_schema::ProcessGrant {
            timed: None,
            provider: "process.host@1".into(),
            rules: vec![hirsel_run_schema::ProcessRule {
                program: "/bin/sh".into(),
                env: vec![],
            }],
            runs: None,
            children: None,
        });
        let adapter = Arc::new(
            real.into_concurrent()
                .unwrap_or_else(|_| panic!("Send process")),
        );
        let mut admitted = Some(adapter.clone());
        let recording =
            super::super::record::Recording::new(b"process ABI output allocation cut", false);
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
                program(ctx, 4);
                unreachable!("both process output allocations exceed the budget");
            },
        );
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
            2
        );
    }
}
