//! Role-free object custody on the real Free dispatcher: immediate ops take no
//! scheduler role, and every forced custody ordering keeps one payload owner.
use super::*;
use crate::tasks::free::record;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

/// The process-global compute grant needs a child process per test.
fn isolated(name: &str, variable: &str) -> bool {
    if std::env::var_os(variable).is_some() {
        crate::pool::set_compute_grant(4);
        return true;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("tasks::free::objects::tests::{name}"),
            "--nocapture",
        ])
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

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !done() {
        assert!(std::time::Instant::now() < until, "{what}");
        std::thread::yield_now();
    }
}

fn plain(size: u64) -> crate::Layout {
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

/// Descriptors 2 (a [tag, value] image), 3 (the mutex wrapper) and 4 (a
/// one-pointer child environment). Leaked: run tables keep the pointers, and
/// a finalized image's scope end names its own descriptor.
struct Layouts([&'static crate::Layout; 3]);

impl Layouts {
    fn new(image: &'static crate::Layout, wrapper: crate::Layout) -> Self {
        Self([
            image,
            Box::leak(Box::new(wrapper)),
            Box::leak(Box::new(plain(8))),
        ])
    }

    fn install(&self, run: &mut crate::host::BodyRun, pointers: &mut Vec<*const crate::Layout>) {
        pointers.extend(self.0.iter().map(|&layout| layout as *const _));
        run.config.layouts = pointers.as_ptr();
        run.config.layout_count = 3;
    }
}

fn wrapper() -> crate::Layout {
    crate::Layout {
        size: 8,
        flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
        ..plain(16)
    }
}

unsafe fn mutex(ctx: *mut TaskCtx, value: u64) -> u64 {
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

unsafe fn spawn(
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

unsafe fn data<'a, T>(env: *mut Header) -> &'a T {
    unsafe { &*crate::alloc::payload(env).cast::<*const T>().read() }
}

/// Immediate rounds per child inside the measured window: 6 ops each, within
/// one fresh 128-op quantum.
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
}

unsafe fn round(ctx: *mut TaskCtx, objects: &mut Disjoint, value: u64) {
    let mut payload = [0u64; 2];
    let mut status = 99;
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
        let mut message = [1u64, value];
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
    for value in 1..=ROUNDS {
        unsafe { round(ctx, objects, value) };
    }
    window.done.fetch_add(1, Ordering::AcqRel);
    wait_until("the window closes", || {
        window.release.load(Ordering::Acquire)
    });
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
    for recorded in [false, true] {
        let recording = recorded.then(|| record::Recording::new(b"runtime50 immediate", false));
        let install = recording.clone();
        let acquisitions = Arc::new(AtomicU64::new(u64::MAX));
        let observed = acquisitions.clone();
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
                observed.store(role.acquisitions() - before, Ordering::Release);
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
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK), "recorded {recorded}");
        assert_eq!(outcome.pending, 0);
        assert_eq!(
            acquisitions.load(Ordering::Acquire),
            0,
            "recorded {recorded}: {} immediate ops entered R",
            2 * 6 * ROUNDS
        );
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
        }
    }
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
unsafe fn contend(ctx: *mut TaskCtx, shared: &Contended, count: u32) -> Vec<(u32, u64)> {
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
    order
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
                let first = contend(ctx, shared, 3);
                // Reuse: the vacated rows serve a second generation of waiters.
                shared.saves.lock().unwrap().clear();
                let second = contend(ctx, shared, 3);
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
