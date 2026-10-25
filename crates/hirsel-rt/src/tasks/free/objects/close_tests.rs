//! Role-free close, kill and cause on resolved cells: each takes no scheduler
//! role, keeps its version event, wakes its registered successors with their
//! tokens after the unlock, and leaves one owner for every detached payload,
//! destroyed on the closing task at its cleanup anchor. Tokio's close with an
//! outstanding reservation (`tokio/tests/sync_mpsc.rs:483-521`, ce685ee6)
//! maps to Hirsel's unbounded channel as a receiver parked with a token when
//! the last sender closes; its semaphore's reserve-oldest-then-wake
//! (`tokio/src/sync/batch_semaphore.rs:334-360`) to a put racing a kill.
use super::tests::{
    Layouts, data, isolated_at, mutex, parked_tokens, plain, spawn, wait_until, wrapper,
};
use super::*;
use crate::tasks::free::record;
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64};

fn isolated(name: &str, variable: &str) -> bool {
    isolated_at(
        &format!("tasks::free::objects::close_tests::{name}"),
        variable,
    )
}

/// This thread's role acquisitions across `op`.
fn roles<T>(op: impl FnOnce() -> T) -> (T, u64) {
    let before = role::thread_acquisitions();
    let value = op();
    (value, role::thread_acquisitions() - before)
}

fn recorded_runs(
    name: &'static [u8],
    layouts: &Layouts,
    body: impl Fn(*mut TaskCtx) + Clone + Send + 'static,
) {
    for recorded in [false, true] {
        let recording = recorded.then(|| record::Recording::new(name, false));
        let install = recording.clone();
        let mut run = crate::host::BodyRun::new(4, false);
        run.free = true;
        let mut pointers = Vec::new();
        layouts.install(&mut run, &mut pointers);
        unsafe { run.use_physical_dispatch() };
        let body = body.clone();
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                if let Some(recording) = &install {
                    recording.root(ctx);
                }
            },
            move |ctx| {
                body(ctx);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK), "recorded {recorded}");
        assert_eq!(outcome.pending, 0, "recorded {recorded}");
        if let Some(recording) = recording {
            recording.recorder.stop("returned", None).unwrap().unwrap();
        }
    }
}

unsafe fn send(ctx: *mut TaskCtx, sender: &mut u64, message: [u64; 2]) -> u32 {
    let mut message = message;
    let mut status = 99;
    unsafe { hirsel_rt_v2_channel_send(ctx, &mut status, sender, message.as_mut_ptr().cast()) };
    status
}

unsafe fn receive(ctx: *mut TaskCtx, receiver: *mut u64) -> (u32, [u64; 2]) {
    let mut message = [0u64; 2];
    let mut status = 99;
    unsafe {
        hirsel_rt_v2_channel_receive(
            ctx,
            0,
            0,
            &mut status,
            receiver,
            message.as_mut_ptr().cast(),
        )
    };
    (status, message)
}

unsafe fn open(ctx: *mut TaskCtx) -> [u64; 2] {
    let mut endpoints = [0; 2];
    let none = [0u64; 2];
    unsafe {
        hirsel_rt_v2_channel_open(
            ctx,
            std::ptr::null_mut(),
            endpoints.as_mut_ptr(),
            &0,
            none.as_ptr().cast(),
            2,
        )
    };
    endpoints
}

#[test]
fn free51_resolved_close_kill_cause_take_no_role() {
    if !isolated(
        "free51_resolved_close_kill_cause_take_no_role",
        "HIRSEL_FREE51_NO_ROLE",
    ) {
        return;
    }
    let layouts = Layouts::new(Box::leak(Box::new(plain(16))), wrapper());
    recorded_runs(b"free51 no role", &layouts, |ctx| unsafe {
        let mut measured = Vec::new();
        // A held resolved mutex dies with this task's chain; a later take
        // observes death and its cause continues the chain, all on the lease.
        let handle = mutex(ctx, 3);
        let word = mutex_word(&handle);
        let mut payload = [0u64; 2];
        let mut status = 99;
        hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
        assert_eq!((status, payload), (0, [1, 3]));
        crate::failure::record_runtime(ctx, "LANG-10", 0, 0);
        crate::pool::free::renew(ctx);
        measured.push(roles(|| hirsel_rt_v2_mutex_kill(ctx, &handle)).1);
        let mut dead = [0u64; 2];
        measured.push(
            roles(|| hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, dead.as_mut_ptr().cast()))
                .1,
        );
        assert_eq!((status, dead), (1, [0, 0]));
        measured.push(roles(|| hirsel_rt_v2_mutex_cause(ctx, &handle)).1);
        assert_eq!(
            (*ctx).failure.count(),
            2,
            "the cause continues the kill's chain"
        );
        (*ctx).failure.truncate(0);
        let (closed, closing) = roles(|| close_mutex(ctx, word, 0, 0));
        assert_eq!(closed, STATUS_OK);
        measured.push(closing);
        let lease = |word| Arc::strong_count((*ctx).free_ops.mutexes.get(word).unwrap());
        assert_eq!(
            (lease(word), (*ctx).free_ops.retired.len()),
            (2, 1),
            "the closed row waits for its closer's next role entry"
        );
        // A live resolved payload detaches at close: inert, so it needs no
        // destruction. Creation is the next role entry.
        let handle = mutex(ctx, 5);
        assert_eq!(
            (lease(word), (*ctx).free_ops.retired.len()),
            (1, 0),
            "the role entry retired the closed row; only the lease keeps its storage"
        );
        let live = mutex_word(&handle);
        hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
        hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
        crate::pool::free::renew(ctx);
        let (closed, closing) = roles(|| close_mutex(ctx, live, 0, 0));
        assert_eq!(closed, STATUS_OK);
        measured.push(closing);
        assert_eq!((lease(live), (*ctx).free_ops.retired.len()), (2, 1));
        // A last receiver's close detaches its unreceived messages; a send
        // then sees Closed, and the last sender's close retires the channel.
        let mut endpoints = open(ctx);
        for value in 0..3 {
            assert_eq!(
                send(ctx, &mut endpoints[0], [1, value]),
                crate::channels::DONE
            );
        }
        assert_eq!(
            receive(ctx, &mut endpoints[1]),
            (crate::channels::DONE, [1, 0])
        );
        crate::pool::free::renew(ctx);
        let (closed, closing) = roles(|| close_endpoint(ctx, endpoints[1], 0, 0));
        assert_eq!(closed, STATUS_OK);
        measured.push(closing);
        assert!((*ctx).free_ops.endpoints.get(endpoints[1]).is_none());
        let (status, sending) = roles(|| send(ctx, &mut endpoints[0], [1, 9]));
        assert_eq!(status, crate::channels::CLOSED);
        measured.push(sending);
        let (closed, closing) = roles(|| close_endpoint(ctx, endpoints[0], 0, 0));
        assert_eq!(closed, STATUS_OK);
        measured.push(closing);
        assert_eq!(
            measured, [0; 8],
            "kill, dead take, cause, two mutex closes, receiver close, closed send, sender close"
        );
        assert_eq!(
            (lease(live), (*ctx).free_ops.retired.len()),
            (1, 2),
            "the channel's open retired the mutex row; both endpoints wait"
        );
        {
            let _role = role::Guard::new(ctx);
            let s = scheduler(ctx);
            assert!(!s.channels.initialized(endpoints[0], true));
            assert!(!s.channels.initialized(endpoints[1], false));
        }
        assert_eq!((*ctx).free_ops.retired.len(), 0);
    });
}

/// One receiver, one sender and a forced order of the receiver's parking
/// against the last sender's close.
struct Shutdown {
    sender: u64,
    receiver: u64,
    order: u32,
    start: AtomicBool,
    received: Mutex<Vec<(u32, u64)>>,
    close_roles: AtomicU64,
}

/// The receiver parks with a token and the close wakes it.
const PARKED: u32 = 0;
/// A send takes the parked receiver's token just before the close: the close
/// finds no token, and the receiver's retry sees Closed after the message.
const STALE: u32 = 1;
/// The close precedes the receiver's first attempt.
const UNREGISTERED: u32 = 2;

unsafe extern "C" fn shutdown_receiver(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let shutdown = unsafe { crate::alloc::payload(env).cast::<*mut Shutdown>().read() };
    if unsafe { (*shutdown).order } == UNREGISTERED {
        wait_until("the sender closed", || unsafe {
            (*shutdown).start.load(Ordering::Acquire)
        });
    }
    loop {
        let (status, [_, value]) = unsafe { receive(ctx, &raw mut (*shutdown).receiver) };
        unsafe { &(*shutdown).received }
            .lock()
            .unwrap()
            .push((status, value));
        if status != crate::channels::DONE {
            return STATUS_OK;
        }
    }
}

unsafe extern "C" fn shutdown_sender(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let shutdown = unsafe { crate::alloc::payload(env).cast::<*mut Shutdown>().read() };
    let sender = unsafe { &raw mut (*shutdown).sender };
    let order = unsafe { (*shutdown).order };
    assert_eq!(
        unsafe { send(ctx, &mut *sender, [1, 0]) },
        crate::channels::DONE
    );
    if order != UNREGISTERED {
        wait_until("the receiver parks with a token", || unsafe {
            parked_tokens(ctx, *sender) == 1
        });
    }
    if order != PARKED {
        assert_eq!(
            unsafe { send(ctx, &mut *sender, [1, 1]) },
            crate::channels::DONE
        );
    }
    unsafe { crate::pool::free::renew(ctx) };
    let (status, roles) = roles(|| unsafe { close_endpoint(ctx, *sender, 0, 0) });
    assert_eq!(status, STATUS_OK);
    unsafe { &(*shutdown).close_roles }.store(roles, Ordering::Release);
    unsafe { &(*shutdown).start }.store(true, Ordering::Release);
    STATUS_OK
}

#[test]
fn free51_last_sender_close_wakes_parked_receivers_without_role() {
    if !isolated(
        "free51_last_sender_close_wakes_parked_receivers_without_role",
        "HIRSEL_FREE51_SHUTDOWN",
    ) {
        return;
    }
    let layouts = Layouts::new(Box::leak(Box::new(plain(16))), wrapper());
    recorded_runs(b"free51 shutdown", &layouts, |ctx| unsafe {
        for order in [PARKED, STALE, UNREGISTERED] {
            let endpoints = open(ctx);
            let mut shared = Shutdown {
                sender: endpoints[0],
                receiver: endpoints[1],
                order,
                start: AtomicBool::new(false),
                received: Mutex::default(),
                close_roles: AtomicU64::new(u64::MAX),
            };
            let mut scope = 0;
            hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
            let env = (&mut shared as *mut Shutdown).cast();
            spawn(ctx, &scope, shutdown_receiver, env);
            spawn(ctx, &scope, shutdown_sender, env);
            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
            let mut expected = vec![(crate::channels::DONE, 0)];
            if order != PARKED {
                expected.push((crate::channels::DONE, 1));
            }
            expected.push((crate::channels::CLOSED, 0));
            assert_eq!(*shared.received.lock().unwrap(), expected, "order {order}");
            assert_eq!(
                shared.close_roles.load(Ordering::Acquire),
                0,
                "order {order}: the last sender's close took no role"
            );
            let _role = role::Guard::new(ctx);
            assert_eq!(close_endpoint(ctx, endpoints[1], 0, 0), STATUS_OK);
        }
    });
}

/// Waiters registered behind a holder, then killed: by the holder itself, or
/// by the first waiter after a put reserved the permit for it.
struct Killing {
    mutex: u64,
    reserved: bool,
    /// Each waiter's task id and take status, in completion order.
    statuses: Mutex<Vec<(u32, u32)>>,
    /// Role acquisitions on each waiter's own thread across its cause or kill.
    roles: Mutex<Vec<u64>>,
}

unsafe extern "C" fn killed_waiter(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let killing = unsafe { data::<Killing>(env) };
    let mut payload = [0u64; 2];
    let mut status = 99;
    unsafe {
        hirsel_rt_v2_mutex_take(
            ctx,
            &mut status,
            &killing.mutex,
            payload.as_mut_ptr().cast(),
        )
    };
    let me = unsafe { (*ctx).task_id };
    killing.statuses.lock().unwrap().push((me, status));
    unsafe { crate::pool::free::renew(ctx) };
    let roles = if status == 0 {
        // The reserved first waiter fails in its body and kills the mutex
        // its successors still wait on.
        assert!(killing.reserved);
        crate::failure::record_runtime(ctx, "LANG-10", 0, 0);
        roles(|| unsafe { hirsel_rt_v2_mutex_kill(ctx, &killing.mutex) }).1
    } else {
        assert_eq!(payload, [0, 0]);
        let roles = roles(|| unsafe { hirsel_rt_v2_mutex_cause(ctx, &killing.mutex) }).1;
        assert_eq!(unsafe { (*ctx).failure.count() }, 1, "the killer's chain");
        roles
    };
    unsafe { (*ctx).failure.truncate(0) };
    killing.roles.lock().unwrap().push(roles);
    STATUS_OK
}

#[test]
fn free51_kill_wakes_reserved_and_registered_waiters_without_role() {
    if !isolated(
        "free51_kill_wakes_reserved_and_registered_waiters_without_role",
        "HIRSEL_FREE51_KILL",
    ) {
        return;
    }
    let layouts = Layouts::new(Box::leak(Box::new(plain(16))), wrapper());
    recorded_runs(b"free51 kill", &layouts, |ctx| unsafe {
        for reserved in [false, true] {
            let handle = mutex(ctx, 1);
            let word = mutex_word(&handle);
            let mut payload = [0u64; 2];
            let mut status = 99;
            hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
            assert_eq!(status, 0);
            let shared = Killing {
                mutex: handle,
                reserved,
                statuses: Mutex::default(),
                roles: Mutex::default(),
            };
            let mut scope = 0;
            hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
            for waiter in 0..3 {
                spawn(
                    ctx,
                    &scope,
                    killed_waiter,
                    (&shared as *const Killing).cast(),
                );
                wait_until("a waiter registers in FIFO order", || {
                    (*ctx)
                        .free_ops
                        .mutexes
                        .get(word)
                        .unwrap()
                        .lock()
                        .permit
                        .waiting()
                        == waiter + 1
                });
            }
            crate::pool::free::renew(ctx);
            let holder = if reserved {
                // The put reserves the permit for the oldest waiter and wakes
                // it; that waiter's kill reaches the two still registered.
                roles(|| hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast())).1
            } else {
                crate::failure::record_runtime(ctx, "LANG-10", 0, 0);
                let roles = roles(|| hirsel_rt_v2_mutex_kill(ctx, &handle)).1;
                (*ctx).failure.truncate(0);
                roles
            };
            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
            let statuses = shared.statuses.lock().unwrap().clone();
            let expected = if reserved { [0, 1, 1] } else { [1, 1, 1] };
            assert_eq!(
                statuses
                    .iter()
                    .map(|&(_, status)| status)
                    .collect::<Vec<_>>(),
                expected,
                "reserved {reserved}: {statuses:?}"
            );
            assert_eq!(holder, 0, "reserved {reserved}: the holder's put or kill");
            assert_eq!(
                *shared.roles.lock().unwrap(),
                [0; 3],
                "reserved {reserved}: each waiter's kill or cause took no role"
            );
            let _role = role::Guard::new(ctx);
            assert_eq!(close_mutex(ctx, word, 0, 0), STATUS_OK);
        }
    });
}

static DESTROYED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// A payload finalizer that saves its task 130 times; the first message's
/// also closes a nested channel whose leftover runs this finalizer inside it.
unsafe extern "C" fn ordered_destroy(ctx: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    let [tag, payload] = unsafe { value.cast::<[u64; 2]>().read() };
    assert_eq!(tag, 1);
    DESTROYED.lock().unwrap().push(payload);
    let images = unsafe { (*ctx).detached_images.len() };
    if payload == 0 {
        unsafe {
            let mut endpoints = open(ctx);
            assert_eq!(send(ctx, &mut endpoints[0], [2, 0]), crate::channels::DONE);
            assert_eq!(
                send(ctx, &mut endpoints[0], [1, 100]),
                crate::channels::DONE
            );
            assert_eq!(
                receive(ctx, &mut endpoints[1]),
                (crate::channels::DONE, [2, 0])
            );
            // Both endpoints are resolved: each nested close is role-free.
            assert_eq!(close_endpoint(ctx, endpoints[1], 0, 0), STATUS_OK);
            assert_eq!(close_endpoint(ctx, endpoints[0], 0, 0), STATUS_OK);
            assert!((*ctx).free_ops.endpoints.get(endpoints[1]).is_none());
        }
    }
    for _ in 0..130 {
        let _role = unsafe { role::Guard::new(ctx) };
        unsafe { try_yield(&mut *ctx) }.unwrap();
    }
    assert_eq!(unsafe { (*ctx).detached_images.len() }, images);
    assert_eq!(
        unsafe { value.cast::<[u64; 2]>().read() },
        [1, payload],
        "nested closes and saves preserve the outer image"
    );
    STATUS_OK
}

#[test]
fn free51_role_free_close_destroys_payloads_in_order_on_its_task() {
    if !isolated(
        "free51_role_free_close_destroys_payloads_in_order_on_its_task",
        "HIRSEL_FREE51_DESTROY",
    ) {
        return;
    }
    let layouts = Layouts::new(
        super::super::finalized(crate::Layout {
            finalizer: Some(ordered_destroy),
            ..plain(16)
        }),
        wrapper(),
    );
    recorded_runs(b"free51 destroy", &layouts, |ctx| unsafe {
        DESTROYED.lock().unwrap().clear();
        let mut endpoints = open(ctx);
        assert_eq!(send(ctx, &mut endpoints[0], [2, 9]), crate::channels::DONE);
        for value in 0..4 {
            assert_eq!(
                send(ctx, &mut endpoints[0], [1, value]),
                crate::channels::DONE
            );
        }
        assert_eq!(
            receive(ctx, &mut endpoints[1]),
            (crate::channels::DONE, [2, 9])
        );
        assert_eq!(close_endpoint(ctx, endpoints[1], 0, 0), STATUS_OK);
        assert!((*ctx).free_ops.endpoints.get(endpoints[1]).is_none());
        assert_eq!(*DESTROYED.lock().unwrap(), [0, 100, 1, 2, 3]);
        assert_eq!(close_endpoint(ctx, endpoints[0], 0, 0), STATUS_OK);
        let handle = mutex(ctx, 7);
        let word = mutex_word(&handle);
        let mut payload = [0u64; 2];
        let mut status = 99;
        hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
        hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
        assert_eq!(close_mutex(ctx, word, 0, 0), STATUS_OK);
        assert_eq!(
            *DESTROYED.lock().unwrap(),
            [0, 100, 1, 2, 3, 7],
            "each detached payload was destroyed once, oldest first"
        );
        assert_eq!((*ctx).detached_images.len(), 0);
        drop(role::Guard::new(ctx));
        assert_eq!(
            Arc::strong_count((*ctx).free_ops.mutexes.get(word).unwrap()),
            1,
            "only the lease keeps the closed cell's storage"
        );
    });
}

struct Probe {
    recording: Option<Arc<record::Recording>>,
    first: AtomicBool,
    second: AtomicBool,
}

/// As finalizer_replay_tests' destroy: a yielding finalizer with ops of its
/// own, whose admitted prefix a cut bounds.
unsafe extern "C" fn probed(ctx: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
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

/// Resolve the object, then close it role-free with one probed payload left.
unsafe fn close_resolved(ctx: *mut TaskCtx, probe: &Probe, mutex: bool) {
    unsafe {
        let probed = [1u64, probe as *const Probe as u64];
        if mutex {
            let mut payload = probed;
            let none = [0u64; 2];
            let mut handle = 0;
            hirsel_rt_v2_mutex_create(
                ctx,
                std::ptr::null_mut(),
                &mut handle,
                payload.as_mut_ptr().cast(),
                none.as_ptr().cast(),
                2,
                3,
            );
            let mut status = 99;
            hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
            hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
            assert_eq!(close_mutex(ctx, mutex_word(&handle), 0, 0), STATUS_OK);
        } else {
            let mut pair = open(ctx);
            assert_eq!(send(ctx, &mut pair[0], [2, 0]), crate::channels::DONE);
            assert_eq!(send(ctx, &mut pair[0], probed), crate::channels::DONE);
            assert_eq!(receive(ctx, &mut pair[1]), (crate::channels::DONE, [2, 0]));
            assert_eq!(close_endpoint(ctx, pair[1], 0, 0), STATUS_OK);
            assert!(
                (*ctx).free_ops.endpoints.get(pair[1]).is_none(),
                "the resolved close was role-free"
            );
        }
        assert_eq!((*ctx).detached_images.len(), 0);
        // Leave no role a declined close might have entered at depth 0.
        drop(role::Guard::new(ctx));
    }
}

#[test]
fn free51_role_free_close_replays_only_the_admitted_cleanup_prefix() {
    let plain = plain(16);
    let image = super::super::finalized(crate::Layout {
        finalizer: Some(probed),
        ..plain
    });
    let wrapper = wrapper();
    let layouts = [&raw const *image, &raw const wrapper];
    // create, take, put, close; open, send, send, receive, close.
    for (durable, mutex, close_op) in [
        (false, false, 5),
        (true, false, 5),
        (false, true, 4),
        (true, true, 4),
    ] {
        let recording = record::Recording::new(b"free51 close replay", durable);
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
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                close_resolved(ctx, &observed, mutex);
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
                let replay = super::super::replay::Replay::new(&bytes[..end]).unwrap();
                let install = replay.clone();
                let probe = Arc::new(Probe {
                    recording: None,
                    first: false.into(),
                    second: false.into(),
                });
                let observed = probe.clone();
                let (teardown, reclaimed) = std::sync::mpsc::channel();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 2;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        install.root(ctx);
                        (*ctx).detached_images.reclaimed = Some(teardown);
                    },
                    move |ctx| unsafe {
                        close_resolved(ctx, &observed, mutex);
                        panic!("finalizer replay passed its cut");
                    },
                );
                assert!(
                    matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                    "{outcome:?}"
                );
                assert_eq!(probe.first.load(Ordering::Acquire), op > close_op);
                assert_eq!(probe.second.load(Ordering::Acquire), op > close_op + 1);
                // A stopped BodyRun may return with workers still retaining
                // the run. Observe the actual image teardown before checking
                // the completed replay; return timing is not reclamation.
                reclaimed
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("cut teardown frees task-owned detached images");
                assert_eq!(replay.finish().unwrap(), expected);
            }
        }
    }
}
