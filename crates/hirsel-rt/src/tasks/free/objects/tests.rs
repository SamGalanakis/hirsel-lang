//! Role-free object custody on the real Free dispatcher: immediate ops take no
//! scheduler role, and every forced custody ordering keeps one payload owner.
use super::*;
use crate::tasks::free::record;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

/// The process-global compute grant needs a child process per test.
fn isolated(name: &str, variable: &str) -> bool {
    isolated_at(&format!("tasks::free::objects::tests::{name}"), variable)
}

pub(in crate::tasks::free) fn isolated_at(test: &str, variable: &str) -> bool {
    if std::env::var_os(variable).is_some() {
        crate::pool::set_compute_grant(4);
        return true;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env(variable, "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("1 passed"),
        "the isolated body ran: {stdout}"
    );
    false
}

pub(in crate::tasks::free) fn wait_until(what: &str, done: impl Fn() -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !done() {
        assert!(std::time::Instant::now() < until, "{what}");
        std::thread::yield_now();
    }
}

pub(in crate::tasks::free) fn plain(size: u64) -> crate::Layout {
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

/// Descriptors 2 (a [tag, value] image), 3 (the mutex wrapper), 4 (a
/// one-pointer child environment) and 5 (an inline value owning one heap
/// block). Leaked: run tables keep the pointers, and a finalized image's
/// scope end names its own descriptor.
pub(in crate::tasks::free) struct Layouts([&'static crate::Layout; 4]);

impl Layouts {
    pub(in crate::tasks::free) fn new(
        image: &'static crate::Layout,
        wrapper: crate::Layout,
    ) -> Self {
        let release = Box::leak(Box::new(crate::Step {
            kind: crate::STEP_RELEASE,
            field: 0,
            variant: 0,
            span: 0,
            offset: 0,
            stride: 0,
            count: 0,
            count_offset: 0,
            layout: std::ptr::null(),
        }));
        Self([
            image,
            Box::leak(Box::new(wrapper)),
            Box::leak(Box::new(plain(8))),
            Box::leak(Box::new(crate::Layout {
                steps: release,
                step_count: 1,
                field_count: 1,
                ..plain(8)
            })),
        ])
    }

    pub(in crate::tasks::free) fn install(
        &self,
        run: &mut crate::host::BodyRun,
        pointers: &mut Vec<*const crate::Layout>,
    ) {
        pointers.extend(self.0.iter().map(|&layout| layout as *const _));
        run.config.layouts = pointers.as_ptr();
        run.config.layout_count = 4;
    }
}

pub(in crate::tasks::free) fn wrapper() -> crate::Layout {
    crate::Layout {
        size: 8,
        flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
        ..plain(16)
    }
}

pub(in crate::tasks::free) unsafe fn mutex(ctx: *mut TaskCtx, value: u64) -> u64 {
    let mut handle = 0;
    let mut payload = [1u64, value];
    let none = [0u64; 2];
    unsafe {
        hirsel_rt_v2_mutex_create(
            ctx,
            std::ptr::null_mut(),
            &mut handle,
            payload.as_mut_ptr().cast(),
            none.as_ptr().cast(),
            2,
            3,
        )
    };
    assert_eq!(payload, none);
    handle
}

pub(in crate::tasks::free) unsafe fn spawn(
    ctx: *mut TaskCtx,
    scope: &u64,
    entry: unsafe extern "C" fn(*mut TaskCtx, *mut Header, u64, u64) -> u8,
    data: *const u8,
) {
    unsafe {
        let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 4, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(env).cast::<*const u8>().write(data);
        let mut child = 0;
        hirsel_rt_v2_task_spawn(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            &mut child,
            scope,
            entry,
            env,
            0,
        );
    }
}

pub(in crate::tasks::free) unsafe fn data<'a, T>(env: *mut Header) -> &'a T {
    unsafe { &*crate::alloc::payload(env).cast::<*const T>().read() }
}

/// Immediate rounds per child inside the measured window: 6 ops and a
/// cleanup each, within one fresh 128-op quantum.
const ROUNDS: u64 = 20;

struct Window {
    ready: AtomicU32,
    go: AtomicBool,
    done: AtomicU32,
    release: AtomicBool,
}

struct Disjoint {
    mutex: u64,
    endpoints: [u64; 2],
    atomic: u64,
    window: *const Window,
    /// This child's own role acquisitions across its measured rounds.
    roles: AtomicU64,
}

/// One round; returns the calling thread's role acquisitions.
unsafe fn round(ctx: *mut TaskCtx, objects: &mut Disjoint, value: u64) -> u64 {
    let mut payload = [0u64; 2];
    let mut message = [1u64, value];
    let mut status = 99;
    let before = role::thread_acquisitions();
    unsafe {
        hirsel_rt_v2_mutex_take(
            ctx,
            &mut status,
            &objects.mutex,
            payload.as_mut_ptr().cast(),
        );
        assert_eq!((status, payload), (0, [1, value]));
        payload[1] += 1;
        hirsel_rt_v2_mutex_put(ctx, &objects.mutex, payload.as_mut_ptr().cast());
        assert_eq!(payload, [0, 0]);
        // The body's cleanup: an inline value whose drop frees a heap block
        // needs no lifecycle administration.
        let mut owned = [crate::alloc::hirsel_rt_v2_alloc(
            ctx,
            4,
            8,
            8,
            crate::alloc::DOMAIN_HEAP,
            0,
        )];
        let mut cleanup = std::mem::MaybeUninit::<crate::finalize::Cleanup>::uninit();
        assert_eq!(
            crate::finalize::hirsel_rt_v2_finalize(
                ctx,
                5,
                owned.as_mut_ptr().cast(),
                std::ptr::null(),
                cleanup.as_mut_ptr(),
                0,
                0,
            ),
            STATUS_OK
        );
        hirsel_rt_v2_channel_send(
            ctx,
            &mut status,
            &mut objects.endpoints[0],
            message.as_mut_ptr().cast(),
        );
        assert_eq!((status, message), (crate::channels::DONE, [0, 0]));
        hirsel_rt_v2_channel_receive(
            ctx,
            0,
            0,
            &mut status,
            &mut objects.endpoints[1],
            message.as_mut_ptr().cast(),
        );
        assert_eq!((status, message), (crate::channels::DONE, [1, value]));
        hirsel_rt_v2_atomic_store(ctx, &objects.atomic, (&raw const value).cast(), 8);
        let mut loaded = 0u64;
        hirsel_rt_v2_atomic_load(ctx, &objects.atomic, (&raw mut loaded).cast(), 8);
        assert_eq!(loaded, value);
    }
    role::thread_acquisitions() - before
}

unsafe extern "C" fn disjoint_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    // Each child exclusively owns its endpoint words; the root waits for it.
    let objects = unsafe { &mut *crate::alloc::payload(env).cast::<*mut Disjoint>().read() };
    let window = unsafe { &*objects.window };
    // The first round resolves every lease and the receive's cancellation.
    unsafe { round(ctx, objects, 0) };
    unsafe { crate::pool::free::renew(ctx) };
    window.ready.fetch_add(1, Ordering::AcqRel);
    wait_until("the window opens", || window.go.load(Ordering::Acquire));
    let mut roles = 0;
    for value in 1..=ROUNDS {
        roles += unsafe { round(ctx, objects, value) };
    }
    objects.roles.store(roles, Ordering::Release);
    window.done.fetch_add(1, Ordering::AcqRel);
    wait_until("the window closes", || {
        window.release.load(Ordering::Acquire)
    });
    STATUS_OK
}

/// The measured window's run-wide role acquisitions, and the children's own.
#[derive(Default)]
struct Measured {
    window: AtomicU64,
    ops: AtomicU64,
}

/// Two children run immediate rounds on disjoint objects inside a window.
unsafe fn immediate(ctx: *mut TaskCtx, measured: &Measured) -> u8 {
    unsafe {
        let start = (*(*ctx).run_ptr()).role.acquisitions();
        let window = Window {
            ready: AtomicU32::new(0),
            go: AtomicBool::new(false),
            done: AtomicU32::new(0),
            release: AtomicBool::new(false),
        };
        let mut objects = Vec::new();
        for _ in 0..2 {
            let mutex = mutex(ctx, 0);
            let mut endpoints = [0; 2];
            let none = [0u64; 2];
            hirsel_rt_v2_channel_open(
                ctx,
                std::ptr::null_mut(),
                endpoints.as_mut_ptr(),
                &0,
                none.as_ptr().cast(),
                2,
            );
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
            objects.push(Disjoint {
                mutex,
                endpoints,
                atomic,
                window: &window,
                roles: AtomicU64::new(0),
            });
        }
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        for objects in &mut objects {
            spawn(
                ctx,
                &scope,
                disjoint_child,
                (objects as *mut Disjoint).cast(),
            );
        }
        wait_until("both children resolve their leases", || {
            window.ready.load(Ordering::Acquire) == 2
        });
        let role = &(*(*ctx).run_ptr()).role;
        let before = role.acquisitions();
        assert!(
            before > start,
            "this run counts R: creation, spawn and resolution entered it"
        );
        window.go.store(true, Ordering::Release);
        wait_until("both immediate loops finish", || {
            window.done.load(Ordering::Acquire) == 2
        });
        measured
            .window
            .store(role.acquisitions() - before, Ordering::Release);
        let ops = objects
            .iter()
            .map(|objects| objects.roles.load(Ordering::Acquire))
            .sum();
        measured.ops.store(ops, Ordering::Release);
        window.release.store(true, Ordering::Release);
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
        // Each mutex kept one payload through every take and put.
        for objects in &objects {
            let mut payload = [0u64; 2];
            let mut status = 99;
            hirsel_rt_v2_mutex_take(
                ctx,
                &mut status,
                &objects.mutex,
                payload.as_mut_ptr().cast(),
            );
            assert_eq!((status, payload), (0, [1, ROUNDS + 1]));
            hirsel_rt_v2_mutex_put(ctx, &objects.mutex, payload.as_mut_ptr().cast());
        }
    }
    STATUS_OK
}

#[test]
fn runtime50_immediate_objects_do_not_enter_role() {
    if !isolated(
        "runtime50_immediate_objects_do_not_enter_role",
        "HIRSEL_RUNTIME50_IMMEDIATE",
    ) {
        return;
    }
    let layouts = Layouts::new(Box::leak(Box::new(plain(16))), wrapper());
    let run = || {
        let mut run = crate::host::BodyRun::new(4, false);
        run.free = true;
        let mut pointers = Vec::new();
        layouts.install(&mut run, &mut pointers);
        unsafe { run.use_physical_dispatch() };
        (run, pointers)
    };
    let mut tape = None;
    for recorded in [false, true] {
        let recording = recorded.then(|| record::Recording::new(b"runtime50 immediate", false));
        let install = recording.clone();
        let measured = Arc::new(Measured {
            window: AtomicU64::new(u64::MAX),
            ..Default::default()
        });
        let observed = measured.clone();
        let (run, _pointers) = run();
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                if let Some(recording) = &install {
                    recording.root(ctx);
                }
            },
            move |ctx| unsafe { immediate(ctx, &observed) },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK), "recorded {recorded}");
        assert_eq!(outcome.pending, 0);
        assert_eq!(
            measured.window.load(Ordering::Acquire),
            0,
            "recorded {recorded}: {} immediate ops entered R",
            2 * 6 * ROUNDS
        );
        assert_eq!(measured.ops.load(Ordering::Acquire), 0);
        if let Some(recording) = recording {
            let cut = recording
                .recorder
                .stop("returned", None)
                .unwrap()
                .unwrap()
                .0;
            for child in [vec![0], vec![1]] {
                // Every immediate op is its own recorded ordinal.
                assert!(
                    cut.progress[&child].op >= 6 * (ROUNDS + 1),
                    "child {child:?}: {:?}",
                    cut.progress[&child]
                );
            }
            let trace = recording.trace();
            let bytes = Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink()
                .bytes;
            tape = Some((bytes, trace));
        }
    }
    // Replay admits each op from the task's own token: its mutex, channel and
    // atomic ops take no R.
    let (bytes, trace) = tape.unwrap();
    let parsed = hirsel_free::tape::parse(&bytes).unwrap();
    let replay = super::super::replay::Replay::new(&bytes).unwrap();
    let install = replay.clone();
    let measured = Arc::new(Measured::default());
    let observed = measured.clone();
    let (run, _pointers) = run();
    let outcome = run.run(
        move |ctx| unsafe {
            super::super::enable(ctx);
            install.root(ctx);
        },
        move |ctx| unsafe { immediate(ctx, &observed) },
    );
    assert!(
        matches!(
            outcome.ended,
            Ok(STATUS_OK) | Err(crate::stop::Stop::Blocked(_))
        ),
        "{outcome:?}"
    );
    assert_eq!(
        measured.ops.load(Ordering::Acquire),
        0,
        "replayed mutex, channel and atomic ops entered R"
    );
    assert_eq!(
        replay.finish().unwrap(),
        hirsel_free::projection::Projection::new("returned", &parsed.cut, &trace)
    );
}

/// A contended mutex: waiters register in a forced order around a holder's
/// put, its own barging retake, a save hook and row reuse.
struct Contended {
    mutex: AtomicU64,
    /// Task ids in acquisition order, with the payload value each observed.
    order: Mutex<Vec<(u32, u64)>>,
    registered: AtomicU32,
    finished: AtomicU32,
    /// The first waiter's id, then whether its save hook is held for a put.
    first: AtomicU32,
    saving: AtomicBool,
    put: AtomicBool,
    resumed_before_save: AtomicBool,
    saves: Mutex<Vec<u32>>,
    /// Each waiter's role acquisitions on its own worker across its put.
    put_roles: Mutex<Vec<u64>>,
}

unsafe extern "C" fn contended_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let shared = unsafe { data::<Contended>(env) };
    let handle = shared.mutex.load(Ordering::Acquire);
    let me = unsafe { (*ctx).task_id };
    let _ = shared
        .first
        .compare_exchange(u32::MAX, me, Ordering::AcqRel, Ordering::Acquire);
    let mut payload = [0u64; 2];
    let mut status = 99;
    shared.registered.fetch_add(1, Ordering::AcqRel);
    unsafe { hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast()) };
    if !shared.saves.lock().unwrap().contains(&me) {
        shared.resumed_before_save.store(true, Ordering::Release);
    }
    assert_eq!((status, payload[0]), (0, 1));
    shared.order.lock().unwrap().push((me, payload[1]));
    payload[1] += 1;
    // Count before the put: the holder's retake may resume at once.
    shared.finished.fetch_add(1, Ordering::AcqRel);
    // A put that reserves the permit for its successor wakes it with a token:
    // the successor's dispatch takes the role on another worker, this put none.
    unsafe { crate::pool::free::renew(ctx) };
    let before = role::thread_acquisitions();
    unsafe { hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast()) };
    let roles = role::thread_acquisitions() - before;
    shared.put_roles.lock().unwrap().push(roles);
    STATUS_OK
}

unsafe fn waiting(ctx: *mut TaskCtx, word: u64) -> usize {
    let cell = unsafe { (*ctx).free_ops.mutexes.get(word) }.expect("a resolved holder lease");
    cell.lock().permit.waiting()
}

/// Hold the mutex, register `count` waiters in spawn order, put while the
/// first waiter's save hook is held, then retake through the role path.
unsafe fn contend(
    ctx: *mut TaskCtx,
    shared: &Contended,
    count: u32,
) -> (Vec<(u32, u64)>, Vec<crate::tasks::registry::TaskLease>) {
    let handle = shared.mutex.load(Ordering::Acquire);
    let word = unsafe { mutex_word(&handle) };
    let mut payload = [0u64; 2];
    let mut status = 99;
    unsafe { hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast()) };
    assert_eq!(status, 0);
    shared.order.lock().unwrap().clear();
    shared.put_roles.lock().unwrap().clear();
    shared.registered.store(0, Ordering::Release);
    shared.finished.store(0, Ordering::Release);
    shared.first.store(u32::MAX, Ordering::Release);
    shared.put.store(false, Ordering::Release);
    let mut scope = 0;
    unsafe { hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0) };
    for waiter in 0..count {
        unsafe {
            spawn(
                ctx,
                &scope,
                contended_child,
                (shared as *const Contended).cast(),
            )
        };
        // Registration, not save: the put may beat the waiter's logical park.
        wait_until("a waiter registers in FIFO order", || unsafe {
            waiting(ctx, word) == waiter as usize + 1
        });
    }
    wait_until("the first waiter reaches its save", || {
        shared.saving.load(Ordering::Acquire)
    });
    let leases = {
        let _role = unsafe { role::Guard::new(ctx) };
        role::with_role(ctx, |s| {
            let children = s.scope(scope as u32).children.clone();
            children
                .into_iter()
                .map(|child| s.lease(child))
                .collect::<Vec<_>>()
        })
    };
    let value = payload[1];
    unsafe { hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast()) };
    shared.put.store(true, Ordering::Release);
    // A barging retake must queue behind every reserved waiter.
    unsafe { hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast()) };
    assert_eq!((status, payload), (0, [1, value + u64::from(count)]));
    assert_eq!(shared.finished.load(Ordering::Acquire), count);
    unsafe { hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast()) };
    unsafe { hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0) };
    let order = shared.order.lock().unwrap().clone();
    assert_eq!(
        order.iter().map(|&(_, value)| value).collect::<Vec<_>>(),
        (value..value + u64::from(count)).collect::<Vec<_>>(),
        "exactly one owner observed each payload version"
    );
    assert_eq!(
        *shared.put_roles.lock().unwrap(),
        vec![0; count as usize],
        "every waiter's put woke its reserved successor without the role"
    );
    (order, leases)
}

/// Tokio's `tests/sync_mutex.rs` vectors on the Free mutex ABI:
/// straight_execution, readiness and mutex_fifo_with_coop_budget (Tokio
/// ce685ee6, lines 21-63 and 184-205). Tokio's mutex_coop_drop_after_take_permit
/// has no counterpart here: a Hirsel take is not a cancellation point, and a
/// registered waiter leaves its queue only by death, which the custody matrix
/// covers.
struct Vectors {
    mutex: AtomicU64,
    /// Waiter labels in acquisition order.
    order: Mutex<Vec<u64>>,
    /// The highest label whose hold the root has observed.
    observed: AtomicU64,
}

/// Take, log `label`, hold until the root has seen this hold, then put.
unsafe fn hold(ctx: *mut TaskCtx, shared: &Vectors, label: u64) {
    let handle = shared.mutex.load(Ordering::Acquire);
    let mut payload = [0u64; 2];
    let mut status = 99;
    unsafe { hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast()) };
    assert_eq!((status, payload[0]), (0, 1));
    shared.order.lock().unwrap().push(label);
    wait_until("the root observes this hold", || {
        shared.observed.load(Ordering::Acquire) >= label
    });
    unsafe { hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast()) };
}

/// Tokio's t_a: spends its whole cooperative budget, then locks.
unsafe extern "C" fn spent_waiter(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let shared = unsafe { data::<Vectors>(env) };
    while unsafe { crate::pool::free::proceed(ctx) } {}
    unsafe { hold(ctx, shared, 1) };
    STATUS_OK
}

/// Tokio's t_b: locks with a fresh budget, after t_a registered.
unsafe extern "C" fn fresh_waiter(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    unsafe { hold(ctx, data::<Vectors>(env), 2) };
    STATUS_OK
}

#[test]
fn tokio_sync_mutex_vectors() {
    if !isolated("tokio_sync_mutex_vectors", "HIRSEL_TOKIO_SYNC_MUTEX") {
        return;
    }
    let layouts = Layouts::new(Box::leak(Box::new(plain(16))), wrapper());
    for recorded in [false, true] {
        let recording = recorded.then(|| record::Recording::new(b"tokio sync_mutex", false));
        let install = recording.clone();
        let shared = Arc::new(Vectors {
            mutex: AtomicU64::new(0),
            order: Mutex::default(),
            observed: AtomicU64::new(0),
        });
        let body = shared.clone();
        let mut run = crate::host::BodyRun::new(4, false);
        run.free = true;
        let mut pointers = Vec::new();
        layouts.install(&mut run, &mut pointers);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                if let Some(recording) = &install {
                    recording.root(ctx);
                }
            },
            move |ctx| unsafe {
                let shared = &*body;
                let handle = mutex(ctx, 100);
                shared.mutex.store(handle, Ordering::Release);
                let word = mutex_word(&handle);
                let mut payload = [0u64; 2];
                let mut status = 99;
                // straight_execution: every lock is ready at once and sees
                // the previous guard's write; once the lease is resolved, a
                // ready lock and its unlock take no role.
                crate::pool::free::renew(ctx);
                for (round, (seen, write)) in
                    [(100, 99), (99, 98), (98, 98)].into_iter().enumerate()
                {
                    let before = role::thread_acquisitions();
                    hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
                    assert_eq!((status, payload), (0, [1, seen]));
                    payload[1] = write;
                    hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
                    assert_eq!(payload, [0, 0]);
                    if round > 0 {
                        assert_eq!(role::thread_acquisitions(), before, "round {round}");
                    }
                }
                // readiness and mutex_fifo_with_coop_budget: with the guard
                // held, a waiter that spent its budget registers first and a
                // fresh one second. The unlock serves the first; the second
                // is not served while the first holds.
                hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
                assert_eq!((status, payload), (0, [1, 98]));
                let mut scope = 0;
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                let data = (shared as *const Vectors).cast();
                spawn(ctx, &scope, spent_waiter, data);
                wait_until("the spent waiter registers", || waiting(ctx, word) == 1);
                spawn(ctx, &scope, fresh_waiter, data);
                wait_until("the fresh waiter registers", || waiting(ctx, word) == 2);
                hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
                wait_until("the unlock serves the oldest waiter", || {
                    !shared.order.lock().unwrap().is_empty()
                });
                assert_eq!(*shared.order.lock().unwrap(), [1]);
                assert_eq!(waiting(ctx, word), 1, "the fresh waiter still waits");
                shared.observed.store(1, Ordering::Release);
                wait_until("the first put serves the fresh waiter", || {
                    shared.order.lock().unwrap().len() == 2
                });
                assert_eq!(*shared.order.lock().unwrap(), [1, 2]);
                shared.observed.store(2, Ordering::Release);
                hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                // Both guards wrote nothing: the payload is unchanged.
                hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
                assert_eq!((status, payload), (0, [1, 98]));
                hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK), "recorded {recorded}");
        assert_eq!(outcome.pending, 0);
        if let Some(recording) = recording {
            recording.recorder.stop("returned", None).unwrap().unwrap();
        }
    }
}

/// Messages one sender sends to one receiver that parks before each.
const WAKES: u64 = 3;

struct Wake {
    sender: u64,
    receiver: u64,
    received: Mutex<Vec<u64>>,
    /// The sender's role acquisitions on its own worker across each send.
    send_roles: Mutex<Vec<u64>>,
}

pub(in crate::tasks::free) unsafe fn parked_tokens(ctx: *mut TaskCtx, word: u64) -> usize {
    let endpoint = unsafe { (*ctx).free_ops.endpoints.get(word) }.expect("a resolved sender lease");
    endpoint.cell.lock().tokens()
}

unsafe extern "C" fn waking_receiver(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    // Each child borrows only its own endpoint word of the shared fixture.
    let wake = unsafe { crate::alloc::payload(env).cast::<*mut Wake>().read() };
    for _ in 0..WAKES {
        let mut message = [0u64; 2];
        let mut status = 99;
        unsafe {
            hirsel_rt_v2_channel_receive(
                ctx,
                0,
                0,
                &mut status,
                &raw mut (*wake).receiver,
                message.as_mut_ptr().cast(),
            )
        };
        assert_eq!((status, message[0]), (crate::channels::DONE, 1));
        unsafe { &(*wake).received }
            .lock()
            .unwrap()
            .push(message[1]);
    }
    STATUS_OK
}

unsafe extern "C" fn waking_sender(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let wake = unsafe { crate::alloc::payload(env).cast::<*mut Wake>().read() };
    for value in 0..WAKES {
        if value > 0 {
            // The receiver parked with its token: this send wakes it alone.
            wait_until("the receiver parks with a token", || unsafe {
                parked_tokens(ctx, (*wake).sender) == 1
            });
        }
        unsafe { crate::pool::free::renew(ctx) };
        let before = role::thread_acquisitions();
        let mut message = [1u64, value];
        let mut status = 99;
        unsafe {
            hirsel_rt_v2_channel_send(
                ctx,
                &mut status,
                &raw mut (*wake).sender,
                message.as_mut_ptr().cast(),
            )
        };
        assert_eq!(status, crate::channels::DONE);
        let roles = role::thread_acquisitions() - before;
        unsafe { &(*wake).send_roles }.lock().unwrap().push(roles);
    }
    STATUS_OK
}

#[test]
fn runtime50_channel_send_wakes_a_parked_receiver_without_role() {
    if !isolated(
        "runtime50_channel_send_wakes_a_parked_receiver_without_role",
        "HIRSEL_RUNTIME50_CHANNEL_WAKE",
    ) {
        return;
    }
    let layouts = Layouts::new(Box::leak(Box::new(plain(16))), wrapper());
    for recorded in [false, true] {
        let recording = recorded.then(|| record::Recording::new(b"runtime50 channel wake", false));
        let install = recording.clone();
        let wake = Arc::new(Mutex::new(None::<(Vec<u64>, Vec<u64>)>));
        let result = wake.clone();
        let mut run = crate::host::BodyRun::new(4, false);
        run.free = true;
        let mut pointers = Vec::new();
        layouts.install(&mut run, &mut pointers);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                if let Some(recording) = &install {
                    recording.root(ctx);
                }
            },
            move |ctx| unsafe {
                let mut endpoints = [0; 2];
                let none = [0u64; 2];
                hirsel_rt_v2_channel_open(
                    ctx,
                    std::ptr::null_mut(),
                    endpoints.as_mut_ptr(),
                    &0,
                    none.as_ptr().cast(),
                    2,
                );
                let mut shared = Wake {
                    sender: endpoints[0],
                    receiver: endpoints[1],
                    received: Mutex::default(),
                    send_roles: Mutex::default(),
                };
                let mut scope = 0;
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                let env = (&mut shared as *mut Wake).cast();
                spawn(ctx, &scope, waking_receiver, env);
                spawn(ctx, &scope, waking_sender, env);
                hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                *result.lock().unwrap() = Some((
                    shared.received.lock().unwrap().clone(),
                    shared.send_roles.lock().unwrap().clone(),
                ));
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK), "recorded {recorded}");
        assert_eq!(outcome.pending, 0);
        let (received, roles) = wake.lock().unwrap().take().expect("the root finished");
        assert_eq!(
            received,
            (0..WAKES).collect::<Vec<_>>(),
            "recorded {recorded}"
        );
        // The first send resolves the sender's lease under the role.
        assert_eq!(
            roles[1..],
            vec![0; WAKES as usize - 1],
            "recorded {recorded}: each send woke its parked receiver without the role"
        );
        drop(recording);
    }
}

static DESTROYED: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn destroy(_: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    assert_eq!(unsafe { value.cast::<[u64; 2]>().read() }[0], 1);
    DESTROYED.fetch_add(1, Ordering::AcqRel);
    STATUS_OK
}

#[test]
fn runtime50_object_custody_race_matrix() {
    if !isolated(
        "runtime50_object_custody_race_matrix",
        "HIRSEL_RUNTIME50_CUSTODY",
    ) {
        return;
    }
    let close = crate::Step {
        kind: crate::STEP_CLOSE_HANDLE,
        field: 0,
        variant: 0,
        span: 0,
        offset: 0,
        stride: 0,
        count: 0,
        count_offset: 0,
        layout: std::ptr::null(),
    };
    let layouts = Layouts::new(
        super::super::finalized(crate::Layout {
            finalizer: Some(destroy),
            ..plain(16)
        }),
        crate::Layout {
            steps: &close,
            step_count: 1,
            field_count: 1,
            ..wrapper()
        },
    );
    for recorded in [false, true] {
        DESTROYED.store(0, Ordering::Release);
        let recording = recorded.then(|| record::Recording::new(b"runtime50 custody", false));
        let install = recording.clone();
        let shared = Arc::new(Contended {
            mutex: AtomicU64::new(0),
            order: Mutex::default(),
            registered: AtomicU32::new(0),
            finished: AtomicU32::new(0),
            first: AtomicU32::new(u32::MAX),
            saving: AtomicBool::new(false),
            put: AtomicBool::new(false),
            resumed_before_save: AtomicBool::new(false),
            saves: Mutex::default(),
            put_roles: Mutex::default(),
        });
        let body = shared.clone();
        let mut run = crate::host::BodyRun::new(4, false);
        run.free = true;
        let mut pointers = Vec::new();
        layouts.install(&mut run, &mut pointers);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                if let Some(recording) = &install {
                    recording.root(ctx);
                }
            },
            move |ctx| unsafe {
                let shared = &*body;
                shared.mutex.store(mutex(ctx, 0), Ordering::Release);
                let handle = shared.mutex.load(Ordering::Acquire);
                let hook = body.clone();
                crate::pool::free::test_before_save(ctx, move |task, completed| {
                    if completed {
                        return;
                    }
                    hook.saves.lock().unwrap().push(task);
                    // Hold the first waiter between registration and save
                    // until its holder has put: a wake before save.
                    if hook.first.load(Ordering::Acquire) == task
                        && !hook.put.load(Ordering::Acquire)
                    {
                        hook.saving.store(true, Ordering::Release);
                        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
                        while !hook.put.load(Ordering::Acquire) {
                            assert!(std::time::Instant::now() < until, "the holder puts");
                            std::thread::yield_now();
                        }
                        hook.saving.store(false, Ordering::Release);
                    }
                });
                let (first, leases) = contend(ctx, shared, 3);
                {
                    let _role = role::Guard::new(ctx);
                    role::with_held(ctx, |s| {
                        s.task_slots.reclaim_all();
                        let retained = s.task_slots.retained();
                        assert!(
                            leases
                                .iter()
                                .all(|lease| retained.contains(&lease.key().slot)),
                            "retired waiter slots remain unavailable while leases are held"
                        );
                    });
                }
                drop(leases);
                // ebc6caa7 / cb21891a: logical scope completion can precede
                // physical save acknowledgement and the final dispatch lease.
                // Exercise reuse only after all first-generation rows can be
                // reused; keep the exact row-set assertion below.
                wait_until(
                    "the first waiter generation is physically reclaimable",
                    || {
                        let _role = role::Guard::new(ctx);
                        role::with_held(ctx, |s| {
                            s.task_slots.reclaim_all();
                            s.task_slots.retained().is_empty()
                        })
                    },
                );
                shared.saves.lock().unwrap().clear();
                let (second, leases) = contend(ctx, shared, 3);
                drop(leases);
                let ids = |order: &[(u32, u64)]| {
                    let mut ids = order.iter().map(|&(id, _)| id).collect::<Vec<_>>();
                    ids.sort_unstable();
                    ids
                };
                assert_eq!(ids(&first), ids(&second), "the second waiters reuse rows");
                assert!(
                    !shared.resumed_before_save.load(Ordering::Acquire),
                    "every notified waiter resumed only after its save"
                );
                // Kill wakes registered waiters without a payload; a later
                // immediate take on the resolved lease observes death.
                let word = mutex_word(&handle);
                let mut payload = [0u64; 2];
                let mut status = 99;
                hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
                assert_eq!(status, 0);
                {
                    let _role = role::Guard::new(ctx);
                    super::super::mutex::kill(ctx, word).unwrap();
                }
                let before = (*(*ctx).run_ptr()).role.acquisitions();
                let mut dead = [0u64; 2];
                hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, dead.as_mut_ptr().cast());
                assert_eq!((status, dead), (1, [0, 0]));
                assert_eq!((*(*ctx).run_ptr()).role.acquisitions(), before);
                // Close detaches once; a resolved lease keeps only the
                // storage of the closed cell, never its payload.
                let closing = mutex(ctx, 9);
                let closing_word = mutex_word(&closing);
                hirsel_rt_v2_mutex_take(ctx, &mut status, &closing, payload.as_mut_ptr().cast());
                assert_eq!((status, payload), (0, [1, 9]));
                hirsel_rt_v2_mutex_put(ctx, &closing, payload.as_mut_ptr().cast());
                assert_eq!(DESTROYED.load(Ordering::Acquire), 0);
                assert_eq!(
                    crate::roots::hirsel_rt_v2_root_drop(ctx, closing as *mut crate::Header, 0, 0),
                    STATUS_OK
                );
                assert_eq!(DESTROYED.load(Ordering::Acquire), 1);
                assert_eq!(
                    Arc::strong_count((*ctx).free_ops.mutexes.get(closing_word).unwrap()),
                    1,
                    "only the task's lease retains the closed cell's storage"
                );
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK), "recorded {recorded}");
        assert_eq!(outcome.pending, 0);
        assert_eq!(
            DESTROYED.load(Ordering::Acquire),
            1,
            "exactly one destruction"
        );
        if let Some(recording) = recording {
            recording.recorder.stop("returned", None).unwrap().unwrap();
        }
    }
}

/// A role-free token taken while its task runs, then notified before that
/// task parks on a wait no object wake ends: the save remembers the wake and
/// defers its quiet verdict, and the wake's dispatch returns the frame
/// unstarted. That return still reaches the verdict, so a run whose only task
/// is parked for good reports Blocked instead of waiting forever.
#[test]
fn a_stale_wake_returned_unstarted_still_reaches_the_quiet_verdict() {
    let mut run = crate::host::BodyRun::new(2, false);
    run.free = true;
    unsafe { run.use_physical_dispatch() };
    let outcome = run.run(
        |ctx| unsafe { super::super::enable(ctx) },
        |ctx| unsafe {
            let token = role::with_role(ctx, |s| {
                let me = s.current;
                let row = s.task(me);
                let owner = if row.pinned { Owner::Any } else { row.owner };
                crate::pool::free::token((*ctx).run_ptr(), s.key(me), owner)
            })
            .expect("physical dispatch");
            crate::pool::free::notify((*ctx).run_ptr(), &token);
            drop(token);
            crate::tasks::park_final(ctx)
        },
    );
    let Err(crate::stop::Stop::Blocked(waits)) = outcome.ended else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        waits
            .iter()
            .map(|wait| wait.kind.as_str())
            .collect::<Vec<_>>(),
        ["free.final"]
    );
}

/// Waiters that already hold a resolved lease contend for it: each registers
/// in the cell's FIFO with its own token and parks with no role on its stack.
struct Registering {
    mutex: AtomicU64,
    /// Each waiter's task id, by label.
    ids: [AtomicU32; REGISTERING],
    labels: AtomicU32,
    resolved: AtomicU32,
    /// Waiters below this label may take.
    go: AtomicU32,
    /// Labels in acquisition order, with each take's status.
    order: Mutex<Vec<(u32, u32)>>,
    finished: AtomicU32,
    release: AtomicBool,
    /// The first waiter's save is held until the holder's put or kill.
    saving: AtomicBool,
    ended: AtomicBool,
}

const REGISTERING: usize = 3;

unsafe extern "C" fn registering_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let shared = unsafe { data::<Registering>(env) };
    let handle = shared.mutex.load(Ordering::Acquire);
    let label = shared.labels.fetch_add(1, Ordering::AcqRel);
    shared.ids[label as usize].store(unsafe { (*ctx).task_id }, Ordering::Release);
    let mut payload = [0u64; 2];
    let mut status = 99;
    // Resolve the lease while the mutex is free.
    unsafe { hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast()) };
    assert_eq!(status, 0);
    unsafe { hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast()) };
    shared.resolved.fetch_add(1, Ordering::AcqRel);
    wait_until("this waiter's turn to register", || {
        shared.go.load(Ordering::Acquire) > label
    });
    unsafe { hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast()) };
    shared.order.lock().unwrap().push((label, status));
    if status == 0 {
        assert_eq!(payload[0], 1);
        payload[1] += 1;
        unsafe { hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast()) };
    } else {
        assert_eq!(payload, [0, 0], "a dead take moves no payload");
    }
    shared.finished.fetch_add(1, Ordering::AcqRel);
    wait_until("the root releases its waiters", || {
        shared.release.load(Ordering::Acquire)
    });
    STATUS_OK
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum End {
    Put,
    /// Role-path kill: tokenless retried wakes under the role.
    RoleKill,
    /// Lease kill: token wakes after the unlock.
    LeaseKill,
}

/// Tokio's readiness and FIFO vectors (`tests/sync_mutex.rs:41-63,184-205`,
/// ce685ee6) on the lease path: with the guard held, waiters register in a
/// forced order; the first waiter's save is held while the holder puts or
/// kills, so its wake beats its park. A put serves them oldest first, each
/// seeing its predecessor's write; a kill wakes them all dead. Live, each
/// contended take costs the run its save callback's and its dispatch's role
/// acquisitions, and an idle save's quiet recheck: registration, park,
/// retry and its put take none.
#[test]
fn contended_lease_takes_register_and_park_without_role() {
    if !isolated(
        "contended_lease_takes_register_and_park_without_role",
        "HIRSEL_LEASE_REGISTRATION",
    ) {
        return;
    }
    let layouts = Layouts::new(Box::leak(Box::new(plain(16))), wrapper());
    for recorded in [false, true] {
        for end in [End::Put, End::RoleKill, End::LeaseKill] {
            let recording = recorded.then(|| record::Recording::new(b"lease registration", false));
            let install = recording.clone();
            let shared = Arc::new(Registering {
                mutex: AtomicU64::new(0),
                ids: Default::default(),
                labels: AtomicU32::new(0),
                resolved: AtomicU32::new(0),
                go: AtomicU32::new(0),
                order: Mutex::default(),
                finished: AtomicU32::new(0),
                release: AtomicBool::new(false),
                saving: AtomicBool::new(false),
                ended: AtomicBool::new(false),
            });
            let body = shared.clone();
            let roles = Arc::new(AtomicU64::new(u64::MAX));
            let window = roles.clone();
            let mut run = crate::host::BodyRun::new(4, false);
            run.free = true;
            let mut pointers = Vec::new();
            layouts.install(&mut run, &mut pointers);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    if let Some(recording) = &install {
                        recording.root(ctx);
                    }
                },
                move |ctx| unsafe {
                    let shared = &*body;
                    let handle = mutex(ctx, 0);
                    shared.mutex.store(handle, Ordering::Release);
                    let word = mutex_word(&handle);
                    let hook = body.clone();
                    crate::pool::free::test_before_save(ctx, move |task, completed| {
                        if completed
                            || hook.go.load(Ordering::Acquire) == 0
                            || hook.ids[0].load(Ordering::Acquire) != task
                            || hook.ended.load(Ordering::Acquire)
                        {
                            return;
                        }
                        hook.saving.store(true, Ordering::Release);
                        wait_until("the holder puts or kills", || {
                            hook.ended.load(Ordering::Acquire)
                        });
                    });
                    let mut scope = 0;
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                    let data = (shared as *const Registering).cast();
                    for _ in 0..REGISTERING {
                        spawn(ctx, &scope, registering_child, data);
                    }
                    wait_until("every waiter resolves its lease", || {
                        shared.resolved.load(Ordering::Acquire) == REGISTERING as u32
                    });
                    let mut payload = [0u64; 2];
                    let mut status = 99;
                    hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
                    assert_eq!(status, 0);
                    let value = payload[1];
                    let role = &(*(*ctx).run_ptr()).role;
                    let before = role.acquisitions();
                    for label in 0..REGISTERING {
                        shared.go.store(label as u32 + 1, Ordering::Release);
                        wait_until("the waiter registers in FIFO order", || {
                            waiting(ctx, word) == label + 1
                        });
                    }
                    wait_until("the first waiter reaches its save", || {
                        shared.saving.load(Ordering::Acquire)
                    });
                    // No waiter is served while the guard is held.
                    assert!(shared.order.lock().unwrap().is_empty());
                    match end {
                        End::Put => {
                            hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
                        }
                        End::RoleKill => {
                            let _role = role::Guard::new(ctx);
                            super::super::mutex::kill(ctx, word).unwrap();
                        }
                        End::LeaseKill => super::mutex_kill(ctx, word)
                            .expect("an admitted lease kill")
                            .unwrap(),
                    }
                    shared.ended.store(true, Ordering::Release);
                    wait_until("every waiter is served", || {
                        shared.finished.load(Ordering::Acquire) == REGISTERING as u32
                    });
                    window.store(role.acquisitions() - before, Ordering::Release);
                    shared.release.store(true, Ordering::Release);
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                    let order = shared.order.lock().unwrap().clone();
                    if end == End::Put {
                        assert_eq!(order, [(0, 0), (1, 0), (2, 0)], "oldest first");
                        hirsel_rt_v2_mutex_take(
                            ctx,
                            &mut status,
                            &handle,
                            payload.as_mut_ptr().cast(),
                        );
                        assert_eq!((status, payload), (0, [1, value + REGISTERING as u64]));
                        hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
                    } else {
                        let mut statuses = order.clone();
                        statuses.sort_unstable();
                        assert_eq!(statuses, [(0, 1), (1, 1), (2, 1)], "every waiter died");
                    }
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK), "recorded {recorded} {end:?}");
            assert_eq!(outcome.pending, 0);
            let roles = roles.load(Ordering::Acquire);
            eprintln!("lease registration: recorded {recorded} {end:?}: {roles} role acquisitions");
            if !recorded && end != End::RoleKill {
                // A save that leaves its frame idle also reruns the quiet
                // verdict (`tasks::saved_idle`); one woken first does not.
                let waiters = REGISTERING as u64;
                assert!(
                    (2 * waiters..=3 * waiters).contains(&roles),
                    "{end:?}: {roles}"
                );
            }
            if let Some(recording) = recording {
                recording.recorder.stop("returned", None).unwrap().unwrap();
            }
        }
    }
}

#[test]
fn empty_cancellation_lease_survives_topology_and_rechecks_new_causes() {
    let mut run = crate::host::BodyRun::new(1, false);
    let outcome = run.run(
        |_| {},
        |ctx| unsafe {
            let _role = role::Guard::new(ctx);
            let s = scheduler(ctx);
            keep_uncanceled(ctx, s, 0);
            assert!(uncanceled(&(*ctx).free_ops, 0));
            let (scope, _) = s.open_scope(None);
            assert!(uncanceled(&(*ctx).free_ops, 0));
            s.cancel_scope(scope, CANCEL_DEADLINE);
            assert!(!uncanceled(&(*ctx).free_ops, 0));
            assert_eq!(s.canceled_task(0, 0), Some(CANCEL_DEADLINE));
            assert_eq!(s.task(0).scopes.pop(), Some(scope));
            s.cancel_epoch.changed();
            assert_eq!(s.canceled_task(0, 0), None);
            keep_uncanceled(ctx, s, 0);
            assert!(uncanceled(&(*ctx).free_ops, 0));
            assert!(!uncanceled(&(*ctx).free_ops, 1));
            s.cancel(0, CANCEL_REQUESTED);
            assert!(!uncanceled(&(*ctx).free_ops, 0));
            assert_eq!(s.canceled_task(0, 0), Some(CANCEL_REQUESTED));
            STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(STATUS_OK));
}
