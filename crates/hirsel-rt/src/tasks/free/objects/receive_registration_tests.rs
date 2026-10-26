//! A live empty receive registers its own token and rechecks under the cell
//! latch, then parks without the role; its save callback parks the row.
//! Tokio's `recv` registers its waker and tries again
//! (`tokio/src/sync/mpsc/chan.rs:302-354`, ce685ee6); its close with an
//! outstanding reservation (`tokio/tests/sync_mpsc.rs:483-521`) maps to
//! Hirsel's unbounded channel as a receiver parked while a second sender
//! lives: the first sender's close leaves it parked, the second's message
//! wakes it, and its close ends the next receive with Closed. Hirsel adds
//! what Tokio's single receiver lacks: several receivers in FIFO registration
//! order, and cancellation, which takes no message and passes a spent wake on.
use super::tests::{Layouts, data, isolated_at, parked_tokens, plain, spawn, wait_until, wrapper};
use super::*;
use crate::tasks::free::record;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

fn isolated(name: &str, variable: &str) -> bool {
    isolated_at(
        &format!("tasks::free::objects::receive_registration_tests::{name}"),
        variable,
    )
}

/// As `isolated`, passing on the child's count lines.
fn isolated_counting(name: &str, variable: &str) -> bool {
    if std::env::var_os(variable).is_some() {
        crate::pool::set_compute_grant(4);
        return true;
    }
    let test = format!("tasks::free::objects::receive_registration_tests::{name}");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test, "--nocapture"])
        .env(variable, "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        if line.starts_with("free53 cycles") {
            eprintln!("{line}");
        }
    }
    false
}

fn runs(
    name: &'static [u8],
    body: impl Fn(*mut TaskCtx, Option<&Arc<crate::pool::WorkerMetrics>>) + Clone + Send + 'static,
) {
    let layouts = Layouts::new(Box::leak(Box::new(plain(16))), wrapper());
    for recorded in [false, true] {
        let recording = recorded.then(|| record::Recording::new(name, false));
        let install = recording.clone();
        let mut run = crate::host::BodyRun::new(4, false);
        run.free = true;
        let metrics = run.worker_metrics();
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
                body(ctx, Some(&metrics));
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

unsafe fn send(ctx: *mut TaskCtx, sender: *mut u64, value: u64) -> u32 {
    let mut message = [1u64, value];
    let mut status = 99;
    unsafe { hirsel_rt_v2_channel_send(ctx, &mut status, sender, message.as_mut_ptr().cast()) };
    status
}

/// A receive's status and value, and this thread's role acquisitions across
/// it when it returned on the thread it started on.
unsafe fn receive(ctx: *mut TaskCtx, receiver: *mut u64) -> (u32, u64, Option<u64>) {
    let mut message = [0u64; 2];
    let mut status = 99;
    let thread = std::thread::current().id();
    let before = role::thread_acquisitions();
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
    let roles =
        (std::thread::current().id() == thread).then(|| role::thread_acquisitions() - before);
    (status, message[1], roles)
}

unsafe fn close(ctx: *mut TaskCtx, word: u64) {
    let _role = unsafe { role::Guard::new(ctx) };
    assert_eq!(unsafe { close_endpoint(ctx, word, 0, 0) }, STATUS_OK);
}

/// Whether `task` is parked and its frame saved.
fn saved_parked(ctx: *mut TaskCtx, task: u32) -> bool {
    let _role = unsafe { role::Guard::new(ctx) };
    let s = scheduler(ctx);
    s.task(task).state == State::Parked
        && s.task(task).dispatch.as_ref().is_some_and(|f| f.is_saved())
}

/// The receiver parks, registered, and is saved.
fn until_saved(ctx: *mut TaskCtx, sender: u64, task: u32) {
    wait_until(
        "the receiver registers",
        || unsafe { parked_tokens(ctx, sender) } == 1,
    );
    wait_until("the receiver's save parks it", || saved_parked(ctx, task));
}

const LEASE: u32 = 1;
const SENT: u32 = 2;
const PARKING: u32 = 3;

struct Vectors {
    sender: u64,
    dup: u64,
    receiver: u64,
    receiving: AtomicU32,
    stage: AtomicU32,
    /// The task whose next save waits until `sent`: a wake before save.
    hold: AtomicU32,
    held: AtomicBool,
    sent: AtomicBool,
    received: Mutex<Vec<(u32, u64)>>,
    immediate_roles: AtomicU64,
    send_roles: Mutex<Vec<u64>>,
}

unsafe extern "C" fn vector_receiver(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let v = unsafe { data::<Vectors>(env) };
    let receiver = &v.receiver as *const u64 as *mut u64;
    v.receiving
        .store(unsafe { (*ctx).task_id }, Ordering::Release);
    let keep = |(status, value, _): (u32, u64, Option<u64>)| {
        v.received.lock().unwrap().push((status, value))
    };
    // The first receive resolves the receiver's lease under the role.
    keep(unsafe { receive(ctx, receiver) });
    v.stage.store(LEASE, Ordering::Release);
    wait_until("the sender sends before registration", || {
        v.stage.load(Ordering::Acquire) == SENT
    });
    // Send before register: the receive finds its message, takes no role
    // and registers nothing.
    let (status, value, roles) = unsafe { receive(ctx, receiver) };
    v.immediate_roles.store(
        roles.expect("an immediate receive stays"),
        Ordering::Release,
    );
    keep((status, value, roles));
    v.stage.store(PARKING, Ordering::Release);
    // Registered, saved, then woken by the send.
    keep(unsafe { receive(ctx, receiver) });
    // Woken while still saving: the frame remembers the wake.
    v.hold.store(unsafe { (*ctx).task_id }, Ordering::Release);
    keep(unsafe { receive(ctx, receiver) });
    // One of two senders closes; the other's message still arrives.
    keep(unsafe { receive(ctx, receiver) });
    // The last sender closes.
    keep(unsafe { receive(ctx, receiver) });
    STATUS_OK
}

unsafe extern "C" fn vector_sender(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let v = unsafe { data::<Vectors>(env) };
    let sender = &v.sender as *const u64 as *mut u64;
    let dup = &v.dup as *const u64 as *mut u64;
    // Each send resolves its sender's lease: the dup's first message is the
    // receiver's first.
    assert_eq!(unsafe { send(ctx, dup, 1) }, crate::channels::DONE);
    wait_until("the vector receiver resolves its lease", || {
        v.stage.load(Ordering::Acquire) == LEASE
    });
    wait_until("parent leaves scope before immediate receive", || {
        let _role = unsafe { role::Guard::new(ctx) };
        scheduler(ctx).task(0).scopes.is_empty()
    });
    assert_eq!(unsafe { send(ctx, sender, 2) }, crate::channels::DONE);
    v.stage.store(SENT, Ordering::Release);
    wait_until("the receiver's immediate receive", || {
        v.stage.load(Ordering::Acquire) == PARKING
    });
    let receiver = v.receiving.load(Ordering::Acquire);
    until_saved(ctx, v.sender, receiver);
    let mut measured = |value| {
        unsafe { crate::pool::free::renew(ctx) };
        let before = role::thread_acquisitions();
        assert_eq!(unsafe { send(ctx, sender, value) }, crate::channels::DONE);
        v.send_roles
            .lock()
            .unwrap()
            .push(role::thread_acquisitions() - before);
    };
    measured(3);
    wait_until("the receiver holds its save", || {
        v.held.load(Ordering::Acquire)
    });
    measured(4);
    v.sent.store(true, Ordering::Release);
    until_saved(ctx, v.sender, receiver);
    unsafe { close(ctx, v.sender) };
    // A sender is left: the receiver stays registered and parked.
    assert_eq!(unsafe { parked_tokens(ctx, v.dup) }, 1);
    assert!(saved_parked(ctx, receiver));
    assert_eq!(unsafe { send(ctx, dup, 123) }, crate::channels::DONE);
    until_saved(ctx, v.dup, receiver);
    unsafe { close(ctx, v.dup) };
    STATUS_OK
}

struct Cancel {
    receiver: u64,
    /// The receiver whose measured receive may start, by start order.
    turn: AtomicU32,
    tasks: Mutex<Vec<u32>>,
    received: Mutex<Vec<(u32, u32, u64)>>,
    stale: Mutex<Option<crate::pool::free::Token>>,
}

unsafe extern "C" fn cancel_receiver(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let c = unsafe { data::<Cancel>(env) };
    let me = unsafe { (*ctx).task_id };
    // Resolve the receiver's lease with a message of its own.
    let receiver = &c.receiver as *const u64 as *mut u64;
    assert_eq!(unsafe { receive(ctx, receiver) }.0, crate::channels::DONE);
    let index = {
        let mut tasks = c.tasks.lock().unwrap();
        tasks.push(me);
        tasks.len() as u32 - 1
    };
    wait_until("this receiver's turn", || {
        c.turn.load(Ordering::Acquire) == index
    });
    let (status, value, _) = unsafe { receive(ctx, receiver) };
    c.received.lock().unwrap().push((me, status, value));
    let mut stale = c.stale.lock().unwrap();
    if stale.is_none() {
        *stale = super::wake(ctx);
    }
    STATUS_OK
}

#[test]
fn free53_empty_receive_registers_and_parks_without_role() {
    if !isolated(
        "free53_empty_receive_registers_and_parks_without_role",
        "HIRSEL_FREE53_RECEIVE",
    ) {
        return;
    }
    runs(b"free53 receive registration", |ctx, _| unsafe {
        let [sender, receiver] = open(ctx);
        let mut dup = 0;
        hirsel_rt_v2_sender_dup(ctx, std::ptr::null_mut(), &mut dup, &0, &sender);
        let shared = Arc::new(Vectors {
            sender,
            dup,
            receiver,
            receiving: AtomicU32::new(u32::MAX),
            stage: AtomicU32::new(0),
            hold: AtomicU32::new(u32::MAX),
            held: AtomicBool::new(false),
            sent: AtomicBool::new(false),
            received: Mutex::default(),
            immediate_roles: AtomicU64::new(u64::MAX),
            send_roles: Mutex::default(),
        });
        let hook = shared.clone();
        crate::pool::free::test_before_save(ctx, move |task, completed| {
            if !completed && hook.hold.load(Ordering::Acquire) == task {
                hook.hold.store(u32::MAX, Ordering::Release);
                hook.held.store(true, Ordering::Release);
                wait_until("the send before this save", || {
                    hook.sent.load(Ordering::Acquire)
                });
            }
        });
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        let env = Arc::as_ptr(&shared).cast();
        spawn(ctx, &scope, vector_receiver, env);
        spawn(ctx, &scope, vector_sender, env);
        wait_until("receiver resolved before parent leaves scope", || {
            shared.stage.load(Ordering::Acquire) == LEASE
        });
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
        let done = crate::channels::DONE;
        assert_eq!(
            *shared.received.lock().unwrap(),
            [
                (done, 1),
                (done, 2),
                (done, 3),
                (done, 4),
                (done, 123),
                (crate::channels::CLOSED, 0)
            ]
        );
        assert_eq!(shared.immediate_roles.load(Ordering::Acquire), 0);
        assert!(
            shared.held.load(Ordering::Acquire),
            "a wake beat the receiver's save"
        );
        assert_eq!(
            *shared.send_roles.lock().unwrap(),
            [0, 0],
            "each send woke its registered receiver without the role"
        );
        close(ctx, receiver);

        // Canceled oldest: the send spends its wake on the oldest registered
        // receiver, already canceled; it takes no message and passes the wake
        // to the next, which takes the message.
        let [sender, receiver] = open(ctx);
        let cancel = Cancel {
            receiver,
            turn: AtomicU32::new(u32::MAX),
            tasks: Mutex::default(),
            received: Mutex::default(),
            stale: Mutex::default(),
        };
        let mut root_sender = sender;
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        let env = (&cancel as *const Cancel).cast();
        // Both take a warm-up message before either registers: a send to a
        // registered receiver would wake it instead.
        for _ in 0..2 {
            assert_eq!(send(ctx, &mut root_sender, 0), crate::channels::DONE);
            spawn(ctx, &scope, cancel_receiver, env);
        }
        wait_until("both cancel receivers resolve their leases", || {
            cancel.tasks.lock().unwrap().len() == 2
        });
        for child in 0..2 {
            let task = cancel.tasks.lock().unwrap()[child];
            cancel.turn.store(child as u32, Ordering::Release);
            wait_until("the cancel receiver parks", || saved_parked(ctx, task));
        }
        let (oldest, next) = {
            let tasks = cancel.tasks.lock().unwrap();
            (tasks[0], tasks[1])
        };
        {
            let _role = role::Guard::new(ctx);
            scheduler(ctx).cancel(oldest, CANCEL_REQUESTED);
            assert_eq!(send(ctx, &mut root_sender, 7), crate::channels::DONE);
        }
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
        let mut received = cancel.received.lock().unwrap().clone();
        received.sort_unstable();
        let mut expected = vec![
            (oldest, crate::channels::CANCELED + CANCEL_REQUESTED, 0),
            (next, done, 7),
        ];
        expected.sort_unstable();
        assert_eq!(received, expected);

        // Stale key: the finished receiver's own token, notified while a new
        // receiver (perhaps in its reused slot) is parked, wakes no one.
        let stale = cancel
            .stale
            .lock()
            .unwrap()
            .take()
            .expect("a finished receiver's token");
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        assert_eq!(send(ctx, &mut root_sender, 0), crate::channels::DONE);
        spawn(ctx, &scope, cancel_receiver, env);
        wait_until("the fresh receiver resolves its lease", || {
            cancel.tasks.lock().unwrap().len() == 3
        });
        let fresh = cancel.tasks.lock().unwrap()[2];
        cancel.turn.store(2, Ordering::Release);
        wait_until("the fresh receiver parks", || saved_parked(ctx, fresh));
        crate::pool::free::notify((*ctx).run_ptr(), &stale);
        drop(stale);
        for _ in 0..1000 {
            std::thread::yield_now();
        }
        assert!(
            saved_parked(ctx, fresh),
            "a stale token woke the fresh receiver"
        );
        assert_eq!(cancel.received.lock().unwrap().len(), 2);
        assert_eq!(send(ctx, &mut root_sender, 9), crate::channels::DONE);
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
        assert_eq!(cancel.received.lock().unwrap()[2], (fresh, done, 9));
        close(ctx, sender);
        close(ctx, receiver);
    });
}

/// Receives that park before each message in one receiver/sender ping-pong.
const CYCLES: u64 = 32;

struct PingPong {
    sender: u64,
    receiver: u64,
    ready: AtomicBool,
    received: Mutex<Vec<u64>>,
}

unsafe extern "C" fn pong(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let p = unsafe { data::<PingPong>(env) };
    let receiver = &p.receiver as *const u64 as *mut u64;
    // The first receive resolves the lease.
    assert_eq!(unsafe { receive(ctx, receiver) }.0, crate::channels::DONE);
    p.ready.store(true, Ordering::Release);
    for _ in 0..CYCLES {
        let (status, value, _) = unsafe { receive(ctx, receiver) };
        assert_eq!(status, crate::channels::DONE);
        p.received.lock().unwrap().push(value);
    }
    STATUS_OK
}

unsafe extern "C" fn ping(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let p = unsafe { data::<PingPong>(env) };
    let sender = &p.sender as *const u64 as *mut u64;
    // The first send resolves the sender's lease and the receiver's.
    assert_eq!(
        unsafe { send(ctx, sender, u64::MAX) },
        crate::channels::DONE
    );
    wait_until("pong resolves its lease", || {
        p.ready.load(Ordering::Acquire)
    });
    for value in 0..CYCLES {
        wait_until(
            "the receiver registers",
            || unsafe { parked_tokens(ctx, p.sender) } == 1,
        );
        assert_eq!(unsafe { send(ctx, sender, value) }, crate::channels::DONE);
    }
    STATUS_OK
}

/// Each parked receive's scheduler-role acquisitions and task switches,
/// whole-run, for the REPORT's before/after comparison.
#[test]
fn free53_parked_receive_cycles_count_roles_and_switches() {
    if !isolated_counting(
        "free53_parked_receive_cycles_count_roles_and_switches",
        "HIRSEL_FREE53_CYCLES",
    ) {
        return;
    }
    runs(b"free53 receive cycles", |ctx, metrics| unsafe {
        let [sender, receiver] = open(ctx);
        let shared = PingPong {
            sender,
            receiver,
            ready: AtomicBool::new(false),
            received: Mutex::default(),
        };
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        let env = (&shared as *const PingPong).cast();
        let run = &*(*ctx).run_ptr();
        let (roles, switches) = (run.role.acquisitions(), metrics.unwrap().switches());
        spawn(ctx, &scope, pong, env);
        spawn(ctx, &scope, ping, env);
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
        let roles = run.role.acquisitions() - roles;
        let switches = metrics.unwrap().switches() - switches;
        assert_eq!(
            *shared.received.lock().unwrap(),
            (0..CYCLES).collect::<Vec<_>>()
        );
        eprintln!(
            "free53 cycles: {CYCLES} parked receives, {roles} role acquisitions, {switches} switches"
        );
        close(ctx, sender);
        close(ctx, receiver);
    });
}
