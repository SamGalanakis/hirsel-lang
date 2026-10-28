use super::*;
use crate::tasks::*;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

const WIDTH: u32 = 4;
const NONE: [u64; 2] = [0; 2];

#[cfg(all(feature = "tsan", target_arch = "x86_64"))]
#[test]
fn tsan_direct_switch_covers_naked_cpu_handoff() {
    isolated(
        std::any::type_name_of_val(&tsan_direct_switch_covers_naked_cpu_handoff),
        || {
            #[derive(Debug)]
            struct Ping {
                idle: *mut u8,
                saved: *mut u8,
                entered: bool,
                callback: [u64; 6],
                result: u64,
                private_sp: u64,
            }
            unsafe extern "C" fn entry(pointer: *mut u8, _: *mut u8) -> ! {
                let ping = pointer.cast::<Ping>();
                unsafe {
                    (*ping).entered = true;
                    for _ in 0..8 {
                        let marker = 0u64;
                        (*ping).private_sp = (&raw const marker) as u64;
                        (*ping).result =
                            crate::tsan_callbacks::tests::probe((*ping).callback.as_mut_ptr());
                        crate::fiber::fiber_switch(&raw mut (*ping).saved, (*ping).idle);
                    }
                }
                unreachable!("the completed control is never resumed")
            }
            let stack = crate::fiber::Stack::new();
            let mut ping = Ping {
                idle: std::ptr::null_mut(),
                saved: std::ptr::null_mut(),
                entered: false,
                callback: [0; 6],
                result: 0,
                private_sp: 0,
            };
            let before = crate::fiber::TSAN_SWITCHES.load(SeqCst);
            unsafe {
                let start = stack.prepare(entry, (&raw mut ping).cast(), std::ptr::null_mut());
                crate::tsan_callbacks::bind(&raw mut ping.idle);
                let mut target = start;
                for _ in 0..8 {
                    crate::fiber::fiber_switch(&raw mut ping.idle, target);
                    target = ping.saved;
                }
                crate::tsan_callbacks::clear();
            }
            assert!(ping.entered);
            assert_eq!(ping.result, 0x123456789abcdef0);
            assert_eq!(&ping.callback[1..], &[11, 22, 33, 44, 55]);
            let native_sp = (&raw const ping) as u64;
            assert!(
                native_sp.abs_diff(ping.callback[0]) < 65536,
                "callback must run on native idle: {native_sp:x} {ping:?}"
            );
            assert!(ping.private_sp.abs_diff(ping.callback[0]) > 65536);
            assert_eq!(crate::fiber::TSAN_SWITCHES.load(SeqCst) - before, 16);
        },
    );
}

#[derive(Clone, Copy)]
enum Schedule {
    CancelQueued,
    Contract,
    ReverseFailures,
    CancelBeforeTransfer,
    CancelAfterTransfer,
}

struct Census {
    schedule: Schedule,
    claimed: Barrier,
    changed: AtomicBool,
    transferred: AtomicBool,
    failed: AtomicBool,
    starts: [AtomicU32; 8],
    environments: [AtomicU32; 4],
    allocated: [[AtomicU32; 8]; 2],
    destroyed: [[AtomicU32; 8]; 2],
    threads: Mutex<Vec<std::thread::ThreadId>>,
}

fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "forced CPU schedule stalled");
        std::thread::yield_now();
    }
}

fn isolated(name: &str, body: impl FnOnce()) {
    let name = name.strip_prefix("hirsel_rt::").unwrap_or(name);
    const CHILD: &str = "HIRSEL_CPU_ROWS_CHILD";
    if std::env::var(CHILD).as_deref() != Ok(name) {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", name, "--nocapture"])
            .env(CHILD, name)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                let limit = libc::rlimit {
                    rlim_cur: 10,
                    rlim_max: 10,
                };
                if libc::setrlimit(libc::RLIMIT_CPU, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut expired = false;
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                expired = true;
                child.kill().unwrap();
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            !expired,
            "{name} exceeded its bounded subprocess deadline: {output:?}"
        );
        assert!(output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{output:?}"
        );
        return;
    }
    crate::pool::set_compute_grant(WIDTH);
    body();
    assert_eq!(crate::pool::compute_permits(), (WIDTH, 0, 0));
    assert_eq!(crate::pool::lost_compute_permits(), 0);
}

fn descriptor() -> crate::Layout {
    crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 32,
        align: 8,
        finalizer: Some(drop_environment),
        steps: std::ptr::null(),
        step_count: 0,
        field_count: 0,
        flags: crate::LAYOUT_IMPLEMENTATION_FINALIZER,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: 0,
    }
}

unsafe extern "C" fn drop_environment(_: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    unsafe {
        let census = Arc::from_raw(value.cast::<*const Census>().read());
        let index = value.add(8).cast::<u64>().read() as usize;
        assert_eq!(census.environments[index].fetch_add(1, SeqCst), 0);
    }
    crate::STATUS_OK
}

unsafe extern "C" fn drop_item(_: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    unsafe {
        let census = Arc::from_raw(value.cast::<*const Census>().read());
        let tag = value.add(8).cast::<u64>().read() as usize;
        assert_eq!(census.destroyed[tag / 8][tag % 8].fetch_add(1, SeqCst), 0);
    }
    crate::STATUS_OK
}

unsafe fn item(
    ctx: *mut TaskCtx,
    census: &Arc<Census>,
    kind: usize,
    index: usize,
) -> *mut crate::Header {
    assert_eq!(census.allocated[kind][index].fetch_add(1, SeqCst), 0);
    let value = unsafe { crate::alloc::hirsel_rt_v2_alloc(ctx, 3, 16, 8, crate::DOMAIN_HEAP, 0) };
    unsafe {
        value
            .add(1)
            .cast::<*const Census>()
            .write(Arc::into_raw(census.clone()));
        value
            .add(1)
            .cast::<u64>()
            .add(1)
            .write((kind * 8 + index) as u64);
    }
    value
}

fn release_item(ctx: *mut TaskCtx, value: *mut crate::Header) {
    assert_eq!(
        unsafe { crate::release::hirsel_rt_v2_release(ctx, value, 3, std::ptr::null(), 0, 0) },
        0
    );
}

fn step(kind: u32, offset: u64) -> crate::Step {
    crate::Step {
        kind,
        field: crate::WHOLE_VALUE,
        variant: 0,
        span: 0,
        offset,
        stride: 0,
        count: 0,
        count_offset: crate::NO_OFFSET,
        layout: std::ptr::null(),
    }
}

fn control(ctx: *mut TaskCtx, action: u32) -> u32 {
    let mut value = u32::MAX;
    assert_eq!(
        unsafe { hirsel_rt_v2_cpu_map_scoped(ctx, &mut value, &0, WIDTH, action, 0) },
        0
    );
    value
}

unsafe extern "C" fn mapper(ctx: *mut TaskCtx, env: *mut crate::Header, _: u64, _: u64) -> u8 {
    let (census, index) = unsafe {
        let data = env.add(1).cast::<u8>();
        (
            &*data.cast::<*const Census>().read(),
            data.add(8).cast::<u64>().read() as usize,
        )
    };
    let census = unsafe {
        let pointer = std::ptr::from_ref(census);
        Arc::increment_strong_count(pointer);
        Arc::from_raw(pointer)
    };
    assert_eq!(control(ctx, 2), 0);
    assert_eq!(census.starts[index].fetch_add(1, SeqCst), 0);
    census
        .threads
        .lock()
        .unwrap()
        .push(std::thread::current().id());
    // Each of the four addressed owners holds its first job's environment.
    // No runtime role is retained across this native barrier.
    census.claimed.wait();
    if matches!(
        census.schedule,
        Schedule::CancelBeforeTransfer | Schedule::CancelAfterTransfer
    ) {
        if index == 1 {
            if matches!(census.schedule, Schedule::CancelAfterTransfer) {
                until(|| census.transferred.load(SeqCst));
            }
            {
                let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
                crate::tasks::cancel_root(ctx, CANCEL_REQUESTED);
            }
            census.changed.store(true, SeqCst);
        }
        if index != 0 || matches!(census.schedule, Schedule::CancelBeforeTransfer) {
            until(|| census.changed.load(SeqCst));
        }
    } else if matches!(census.schedule, Schedule::ReverseFailures) {
        if index == 0 {
            until(|| census.failed.load(SeqCst));
        }
        if index <= 1 {
            let status = unsafe {
                crate::failure::hirsel_rt_v2_failure_begin(
                    ctx,
                    1,
                    1,
                    index as u64 + 100,
                    index as u64 + 200,
                )
            };
            if index == 1 {
                census.failed.store(true, SeqCst);
            }
            drop(census);
            return status;
        }
    } else {
        if index == 0 {
            match census.schedule {
                Schedule::CancelQueued => {
                    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
                    crate::tasks::cancel_root(ctx, CANCEL_REQUESTED);
                }
                Schedule::Contract => crate::pool::set_compute_grant(1),
                Schedule::ReverseFailures
                | Schedule::CancelBeforeTransfer
                | Schedule::CancelAfterTransfer => unreachable!(),
            }
            census.changed.store(true, SeqCst);
        }
        until(|| census.changed.load(SeqCst));
        // The first claimed job still owns its storage. The second job must
        // be withdrawn at the same boundary the generated CPU worker uses.
        let status = control(ctx, 2);
        let expected = if matches!(census.schedule, Schedule::Contract) {
            8
        } else {
            16 + CANCEL_PARENT
        };
        assert_eq!(status, expected);
        if status == 0 {
            census.starts[index + 4].fetch_add(1, SeqCst);
        }
    }
    let input = unsafe {
        env.add(1)
            .cast::<*mut crate::Header>()
            .add(2)
            .replace(std::ptr::null_mut())
    };
    assert_eq!(
        unsafe { input.add(1).cast::<u64>().add(1).read() },
        index as u64
    );
    release_item(ctx, input);
    let result = unsafe { item(ctx, &census, 1, index) };
    drop(census);
    let mut slot = [1u64, result.expose_provenance() as u64];
    let status = unsafe {
        hirsel_rt_v2_task_finish(ctx, slot.as_mut_ptr().cast(), NONE.as_ptr().cast(), 16, 4)
    };
    assert_eq!(
        slot, NONE,
        "the parent must consume exactly one held result"
    );
    status
}

fn exercise(schedule: Schedule) {
    let census = Arc::new(Census {
        schedule,
        claimed: Barrier::new(WIDTH as usize),
        changed: AtomicBool::new(false),
        transferred: AtomicBool::new(false),
        failed: AtomicBool::new(false),
        starts: Default::default(),
        environments: Default::default(),
        allocated: Default::default(),
        destroyed: Default::default(),
        threads: Mutex::new(Vec::new()),
    });
    let observed = census.clone();
    let env_steps = [
        crate::Step {
            span: 1,
            ..step(crate::STEP_NONNULL, 16)
        },
        step(crate::STEP_RELEASE, 16),
        crate::Step {
            span: 1,
            ..step(crate::STEP_NONNULL, 24)
        },
        step(crate::STEP_RELEASE, 24),
    ];
    let mut layout = descriptor();
    layout.steps = env_steps.as_ptr();
    layout.step_count = env_steps.len() as u64;
    let mut payload = descriptor();
    payload.size = 16;
    payload.finalizer = Some(drop_item);
    let result_steps = [
        crate::Step {
            variant: 1,
            span: 1,
            ..step(crate::STEP_TAG, 0)
        },
        step(crate::STEP_RELEASE, 8),
    ];
    let mut result = payload;
    result.finalizer = None;
    result.steps = result_steps.as_ptr();
    result.step_count = result_steps.len() as u64;
    let layouts = [&raw const layout, &raw const payload, &raw const result];
    let mut run = crate::host::BodyRun::new(1, false);
    run.free = true;
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = layouts.len() as u32;
    unsafe { run.use_physical_dispatch() };
    let outcome = run.run(
        |ctx| unsafe {
            crate::tasks::free::enable(ctx);
            (*ctx).run().cpu_grant = Some(hirsel_run_schema::CpuGrant {
                provider: "cpu.scoped@1".into(),
                max_workers: WIDTH,
                max_domains: 1,
                max_physical_bytes: 1 << 30,
                max_batch_bytes: 1 << 20,
            });
        },
        move |ctx| unsafe {
            {
                let _role = crate::tasks::role::Guard::new(ctx);
                enter(ctx, WIDTH).unwrap();
                // Ordinary unit actions may have only one CPU. This fixture
                // tests simultaneous owners and process-grant contraction;
                // exact host-width qualification has its own native targets.
                assert!((*ctx).run_ref().capacity.is_none());
            }
            let mut scope = 0;
            assert_eq!(
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                0
            );
            let mut tasks = [0; 4];
            let mut returned = Vec::new();
            for (index, task) in tasks.iter_mut().enumerate() {
                let environment =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 32, 8, crate::DOMAIN_HEAP, 0);
                let data = environment.add(1).cast::<u8>();
                data.cast::<*const Census>()
                    .write(Arc::into_raw(census.clone()));
                data.add(8).cast::<u64>().write(index as u64);
                data.add(16)
                    .cast::<*mut crate::Header>()
                    .write(item(ctx, &census, 0, index));
                data.add(24)
                    .cast::<*mut crate::Header>()
                    .write(item(ctx, &census, 0, index + 4));
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        task,
                        &scope,
                        mapper,
                        environment,
                        0
                    ),
                    0
                );
            }
            for (index, task) in tasks.iter_mut().enumerate() {
                let mut slot = NONE;
                let status =
                    hirsel_rt_v2_task_join(ctx, 0, 0, &scope, task, slot.as_mut_ptr().cast(), 16);
                if matches!(schedule, Schedule::ReverseFailures) && index <= 1 {
                    assert_eq!(status, crate::STATUS_FAILURE);
                    assert_eq!(slot, NONE);
                } else {
                    assert_eq!(status, 0);
                    assert_eq!(slot[0], 1);
                    assert_ne!(slot[1], 0);
                    let result =
                        std::ptr::with_exposed_provenance_mut::<crate::Header>(slot[1] as usize);
                    assert_eq!(result.add(1).cast::<u64>().add(1).read(), 8 + index as u64);
                    returned.push(result);
                    if index == 0
                        && matches!(
                            schedule,
                            Schedule::CancelBeforeTransfer | Schedule::CancelAfterTransfer
                        )
                    {
                        census.transferred.store(true, SeqCst);
                        until(|| census.changed.load(SeqCst));
                    }
                }
            }
            if matches!(schedule, Schedule::ReverseFailures) {
                let _role = crate::tasks::role::Guard::new(ctx);
                let chain = (*ctx).failure.chain(&[]);
                assert_eq!(
                    chain
                        .iter()
                        .map(|(record, path)| (record.site, record.operation_site, path.clone()))
                        .collect::<Vec<_>>(),
                    [(100, 200, vec![0]), (101, 201, vec![1])]
                );
                (*ctx).failure.clear();
            }
            assert_eq!(hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0), 0);
            let expected = match schedule {
                Schedule::CancelQueued
                | Schedule::CancelBeforeTransfer
                | Schedule::CancelAfterTransfer => 16 + CANCEL_REQUESTED,
                Schedule::Contract => 8,
                Schedule::ReverseFailures => 0,
            };
            assert_eq!(control(ctx, 1), expected);
            assert!(workers(super::super::execs(ctx)).is_empty());
            for result in returned {
                release_item(ctx, result);
            }
            if matches!(schedule, Schedule::Contract) {
                crate::pool::set_compute_grant(WIDTH);
            }
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(0));
    assert_eq!(outcome.pending, 0);
    assert_eq!(
        observed.starts.each_ref().map(|n| n.load(SeqCst)),
        [1, 1, 1, 1, 0, 0, 0, 0]
    );
    assert_eq!(
        observed.environments.each_ref().map(|n| n.load(SeqCst)),
        [1; 4]
    );
    let threads = observed.threads.lock().unwrap();
    assert_eq!(
        threads
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        4
    );
    assert_eq!(
        observed.allocated[0].each_ref().map(|n| n.load(SeqCst)),
        [1; 8]
    );
    assert_eq!(
        observed.destroyed[0].each_ref().map(|n| n.load(SeqCst)),
        [1; 8]
    );
    let expected = if matches!(schedule, Schedule::ReverseFailures) {
        [0, 0, 1, 1, 0, 0, 0, 0]
    } else {
        [1, 1, 1, 1, 0, 0, 0, 0]
    };
    assert_eq!(
        observed.allocated[1].each_ref().map(|n| n.load(SeqCst)),
        expected
    );
    assert_eq!(
        observed.destroyed[1].each_ref().map(|n| n.load(SeqCst)),
        expected
    );
    assert_eq!(Arc::strong_count(&observed), 1);
}

#[test]
fn cancellation_with_four_claimed_owners_withdraws_every_second_job() {
    isolated(
        concat!(
            module_path!(),
            "::cancellation_with_four_claimed_owners_withdraws_every_second_job"
        ),
        || exercise(Schedule::CancelQueued),
    );
}

#[test]
fn contraction_with_four_claimed_owners_revokes_the_batch_and_drains() {
    isolated(
        concat!(
            module_path!(),
            "::contraction_with_four_claimed_owners_revokes_the_batch_and_drains"
        ),
        || exercise(Schedule::Contract),
    );
}

#[test]
fn simultaneous_failures_keep_spawn_order_when_job_one_fails_first() {
    isolated(
        concat!(
            module_path!(),
            "::simultaneous_failures_keep_spawn_order_when_job_one_fails_first"
        ),
        || exercise(Schedule::ReverseFailures),
    );
}

#[test]
fn stopped_scoped_pure_loop_retains_its_frame_heap_and_last_permit_until_exit() {
    isolated(
        concat!(
            module_path!(),
            "::stopped_scoped_pure_loop_retains_its_frame_heap_and_last_permit_until_exit"
        ),
        || {
            let running = Arc::new(AtomicBool::new(false));
            let release = Arc::new(AtomicBool::new(false));
            let finished = Arc::new(AtomicBool::new(false));
            let mut run = crate::host::BodyRun::new(1, false);
            run.free = true;
            unsafe { run.use_physical_dispatch() };
            let control = run.control();
            let stopper = std::thread::spawn({
                let running = running.clone();
                let control = control.clone();
                move || {
                    until(|| running.load(SeqCst));
                    control.stop();
                }
            });
            let outcome = run.run(|_| {}, {
                let release = release.clone();
                let finished = finished.clone();
                move |ctx| unsafe {
                    {
                        let _role = crate::tasks::role::Guard::new(ctx);
                        enter(ctx, WIDTH).unwrap();
                    }
                    // Keep a live allocation on the scoped worker throughout the
                    // uninstrumented loop. The external stop cannot reclaim it.
                    let block =
                        crate::alloc::hirsel_rt_v2_alloc(ctx, 0, 32, 8, crate::DOMAIN_HEAP, 0);
                    block.add(1).cast::<u64>().write(0xfeedbeef);
                    running.store(true, SeqCst);
                    while !release.load(SeqCst) {
                        std::hint::spin_loop();
                    }
                    assert_eq!(block.add(1).cast::<u64>().read(), 0xfeedbeef);
                    finished.store(true, SeqCst);
                    crate::STATUS_OK
                }
            });
            stopper.join().unwrap();
            assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
            assert!(outcome.pending >= 1);
            until(|| control.status().lost_permits == 1);
            assert!(!finished.load(SeqCst));
            crate::pool::set_compute_grant(1);
            assert!(matches!(
                super::super::process_permits().try_reserve(1),
                Err(Refusal::WidthUnavailable { available: 0, .. })
            ));
            release.store(true, SeqCst);
            until(|| control.status().reclamation_pending == 0);
            assert!(finished.load(SeqCst));
            assert_eq!(control.status().lost_permits, 0);
            assert_eq!(crate::pool::compute_permits(), (1, 0, 0));
            crate::pool::set_compute_grant(WIDTH);
        },
    );
}

#[test]
fn cancellation_on_either_side_of_result_transfer_keeps_one_owner() {
    isolated(
        concat!(
            module_path!(),
            "::cancellation_on_either_side_of_result_transfer_keeps_one_owner"
        ),
        || {
            exercise(Schedule::CancelBeforeTransfer);
            exercise(Schedule::CancelAfterTransfer);
        },
    );
}

#[test]
fn physical_cpu_publication_cannot_cross_terminal_and_torn_cuts() {
    isolated(
        concat!(
            module_path!(),
            "::physical_cpu_publication_cannot_cross_terminal_and_torn_cuts"
        ),
        || {
            use crate::tasks::free::record::{self, Recording};
            use hirsel_free::phase::{Phase, Progress};
            use hirsel_free::session::Session;
            for after_publication in [false, true] {
                let recording = Recording::new(b"physical CPU cut", false);
                let install = recording.clone();
                let snapshot = recording.clone();
                let cutter = recording.clone();
                let (ready, reached) = std::sync::mpsc::sync_channel(0);
                let (release, stopped) = std::sync::mpsc::sync_channel(0);
                let stopper = std::thread::spawn(move || {
                    reached.recv_timeout(Duration::from_secs(5)).unwrap();
                    let cut = cutter.recorder.stop("stopped", None).unwrap().unwrap().0;
                    release.send(()).unwrap();
                    cut
                });
                let mut run = crate::host::BodyRun::new(1, false);
                run.free = true;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        crate::tasks::free::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        let _role = crate::tasks::role::Guard::new(ctx);
                        assert_eq!(
                            record::cpu_control(ctx, WIDTH, 0, 0, || {
                                enter(ctx, WIDTH).unwrap();
                                0
                            }),
                            Ok(0)
                        );
                        assert_eq!(workers(super::super::execs(ctx)).len(), WIDTH as usize);
                        snapshot.recorder.flush().unwrap().unwrap();
                        let result = record::cpu_control(ctx, WIDTH, 2, 0, || {
                            if !after_publication {
                                ready.send(()).unwrap();
                                stopped.recv_timeout(Duration::from_secs(5)).unwrap();
                            }
                            0
                        });
                        if after_publication {
                            assert_eq!(result, Ok(0));
                            ready.send(()).unwrap();
                            stopped.recv_timeout(Duration::from_secs(5)).unwrap();
                        } else {
                            assert_eq!(result, Err(crate::stop::Stop::Requested));
                        }
                        // Only test teardown bypasses the stopped semantic entry.
                        // The terminal must retain no half-published CPU receipt.
                        leave(ctx);
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(0));
                assert_eq!(outcome.pending, 0);
                let cut = stopper.join().unwrap();
                assert_eq!(
                    cut.progress[&Vec::new()],
                    Progress::new(if after_publication { 2 } else { 1 }, Phase::Done)
                );
                let marked = Arc::try_unwrap(recording)
                    .ok()
                    .expect("no retained recorder owners")
                    .recorder
                    .into_sink();
                let parsed = hirsel_free::tape::parse(&marked.bytes).unwrap();
                assert_eq!(parsed.cut, cut.progress);
                let mut session = Session::replay(&marked.bytes).unwrap();
                assert!(session.start(&[]));
                for action in [0u32, 2]
                    .into_iter()
                    .take(if after_publication { 2 } else { 1 })
                {
                    session.begin(&[]);
                    let mut request = [0u8; 16];
                    request[..4].copy_from_slice(&WIDTH.to_le_bytes());
                    request[4..8].copy_from_slice(&action.to_le_bytes());
                    session.replay_host_request(&[], &request).unwrap();
                    assert_eq!(session.load(&[], u128::MAX).unwrap(), 0);
                    session.publish(&[], Phase::Done).unwrap();
                }
                assert_eq!(session.finish().unwrap().cut, cut.progress);
                let previous = marked.marks[marked.marks.len() - 2];
                let prefix = hirsel_free::tape::parse(&marked.bytes[..previous]).unwrap();
                for end in previous + 1..marked.bytes.len() {
                    let torn = hirsel_free::tape::parse(&marked.bytes[..end]).unwrap();
                    assert!(torn.torn && torn.end.is_none());
                    assert_eq!(torn.cut, prefix.cut);
                    assert_eq!(torn.logs, prefix.logs);
                }
            }
        },
    );
}

#[cfg(feature = "tsan")]
#[test]
fn tsan_positive_control_detects_conflicting_accesses() {
    const CHILD: &str = "HIRSEL_CPU_TSAN_CONTROL";
    if std::env::var_os(CHILD).is_some() {
        unsafe extern "C" {
            fn __tsan_write8(address: *mut std::ffi::c_void);
        }
        let storage = Box::new(0u64);
        let address = (&*storage as *const u64) as usize;
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    // Instrument the conflicting accesses without executing a Rust
                    // data race. This is the compiler-rt hook an instrumented
                    // eight-byte store calls.
                    unsafe { __tsan_write8(address as *mut std::ffi::c_void) };
                });
            }
        });
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "pool::cpu_scoped::race_tests::tsan_positive_control_detects_conflicting_accesses",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(66), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("ThreadSanitizer: data race"),
        "{output:?}"
    );
}
