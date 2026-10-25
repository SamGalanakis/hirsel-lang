//! Owned bounded host jobs through the production recorded Free phases: a
//! controlled disk adapter holds its blocking calls on job threads while the
//! tests cut, cancel and reorder around them.
use super::super::*;
use crate::io_concurrent::ConcurrentAdapter;
use crate::io_keys::{Key, Kind, NAMESPACE};
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

type Recording = crate::tasks::free::record::Recording;

fn until(mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while !done() {
        if start.elapsed() > Duration::from_secs(20) {
            return false;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Point {
    Classify,
    Before,
    After,
    Finish,
}

type Hook = fn(&Probe, &Request, Point);

#[derive(Default)]
struct Probe {
    recording: Mutex<Option<Arc<Recording>>>,
    jobs: Mutex<Option<jobs::Probe>>,
    flags: Mutex<BTreeSet<String>>,
    log: Mutex<Vec<String>>,
    effects: AtomicU32,
    on_worker: AtomicU32,
    violations: Mutex<Vec<String>>,
    active: Mutex<Vec<(u64, Key, Mode)>>,
    ready: Mutex<Vec<u64>>,
    blocked: Mutex<BTreeSet<u64>>,
    handles: AtomicU64,
    home: Mutex<Option<std::thread::ThreadId>>,
}

impl Probe {
    fn set(&self, flag: impl Into<String>) {
        self.flags.lock().unwrap().insert(flag.into());
    }
    fn has(&self, flag: &str) -> bool {
        self.flags.lock().unwrap().contains(flag)
    }
    fn wait(&self, flag: &str) -> bool {
        until(|| self.has(flag))
    }
    fn note(&self, entry: String) {
        self.log.lock().unwrap().push(entry);
    }
    fn jobs(&self) -> jobs::Probe {
        self.jobs.lock().unwrap().clone().expect("installed jobs")
    }
    fn stop(&self) -> (Option<Progress>, hirsel_free::claim::Sweep) {
        let recording = self.recording.lock().unwrap().clone().unwrap();
        let (cut, sweep) = recording.recorder.stop("stopped", None).unwrap().unwrap();
        (cut.progress.get(&Vec::new()).copied(), sweep)
    }
}

struct Disk {
    probe: Arc<Probe>,
    hook: Hook,
}

fn alias(file: u64) -> u64 {
    // File 9 on the second mount names the same storage as file 7.
    if file == 9 { 7 } else { file }
}

fn blocking(request: &Request) -> bool {
    matches!(
        request,
        Request::FileRead { .. } | Request::Open { .. } | Request::Remove { .. }
    )
}

impl ConcurrentAdapter for Disk {
    fn keys(&self, request: &Request) -> KeySet {
        use Mode::{Exclusive, Shared};
        KeySet::new(match request {
            Request::FileRead { file, .. } => {
                vec![
                    (Key(100 + alias(*file) as u32), Exclusive),
                    (NAMESPACE, Shared),
                ]
            }
            Request::Open { .. } => vec![(Key(90), Exclusive), (NAMESPACE, Shared)],
            Request::Remove { .. } => vec![(NAMESPACE, Exclusive)],
            // A file-backed sink is a namespace member as well as its own resource.
            Request::Print { stdout, .. } => {
                vec![(Key(200 + *stdout as u32), Exclusive), (NAMESPACE, Shared)]
            }
            Request::SleepUntil { .. } => vec![(Key(50), Exclusive)],
            Request::ChildWait { .. } => vec![(Key(60), Exclusive)],
            _ => return KeySet::coupled(),
        })
    }
    fn identity(&self, handle: u64) -> Option<(Key, Kind)> {
        (handle >= 1000).then(|| (Key(300 + handle as u32 - 1000), Kind::Other))
    }
    fn blocking(&self, request: &Request) -> bool {
        (self.hook)(&self.probe, request, Point::Classify);
        blocking(request)
    }
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!("Free calls carry their wait token")
    }
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        let probe = &self.probe;
        let job = std::thread::current().name() == Some("hirsel-host-job");
        if blocking(&request) != job {
            probe.on_worker.fetch_add(1, Ordering::AcqRel);
        }
        let footprint: Vec<_> = self.keys(&request).keys().to_vec();
        {
            let mut active = probe.active.lock().unwrap();
            for &(key, mode) in &footprint {
                if active
                    .iter()
                    .any(|&(_, k, m)| k == key && (m == Mode::Exclusive || mode == Mode::Exclusive))
                {
                    probe
                        .violations
                        .lock()
                        .unwrap()
                        .push(format!("{request:?} overlapped key {}", key.0));
                }
            }
            active.extend(footprint.iter().map(|&(key, mode)| (token, key, mode)));
        }
        (self.hook)(probe, &request, Point::Before);
        probe.effects.fetch_add(1, Ordering::AcqRel);
        let result = match &request {
            Request::FileRead { file, .. } => Ok(Completion::Bytes(Ok(vec![*file as u8]))),
            Request::Open { .. } => Ok(Completion::Handle(Ok(
                1000 + probe.handles.fetch_add(1, Ordering::AcqRel)
            ))),
            Request::Remove { .. } | Request::Print { .. } => Ok(Completion::Unit(Ok(()))),
            Request::SleepUntil { .. } => {
                if probe.blocked.lock().unwrap().insert(token) {
                    Err(AdapterError::Blocked)
                } else {
                    Ok(Completion::Wait(Ok(())))
                }
            }
            Request::ChildWait { child, .. } => {
                if *child == 1 && probe.blocked.lock().unwrap().insert(token) {
                    Err(AdapterError::Blocked)
                } else {
                    Ok(Completion::ChildExit(Ok(
                        hirsel_run_schema::ExitStatus::Exited(0),
                    )))
                }
            }
            other => panic!("unexpected {other:?}"),
        };
        (self.hook)(probe, &request, Point::After);
        probe
            .active
            .lock()
            .unwrap()
            .retain(|&(owner, _, _)| owner != token);
        result
    }
    fn finish(&self, _: u64) {
        (self.hook)(&self.probe, &Request::Stdout { io: 0 }, Point::Finish);
    }
    fn ready(&self) -> Result<crate::io::Ready, AdapterError> {
        let mut tokens = std::mem::take(&mut *self.probe.ready.lock().unwrap());
        tokens.sort_unstable();
        tokens.dedup();
        Ok(crate::io::Ready::Tokens(tokens))
    }
}

fn file(file: u64) -> Request {
    Request::FileRead {
        directory: if file == 9 { 2 } else { 1 },
        file,
        max: 16,
    }
}

/// # Safety
/// Root body, role held, before any host request.
unsafe fn install(ctx: *mut TaskCtx, adapter: Arc<dyn ConcurrentAdapter>, probe: &Probe) {
    {
        let _segment = unsafe { role::Segment::new(ctx) };
        unsafe { concurrent::Service::install(ctx, adapter) }.unwrap();
    }
    let io = unsafe { (*ctx).run_ref().free_io.get().unwrap() };
    *probe.jobs.lock().unwrap() = Some(io.jobs.probe());
}

/// A task waits for a test flag outside the scheduler role: spinning while
/// holding R would stall every owner that must re-enter it.
/// # Safety
/// A running task holding the role.
unsafe fn await_flag(ctx: *mut TaskCtx, probe: &Probe, flag: &str) -> bool {
    let _segment = unsafe { role::Segment::new(ctx) };
    probe.wait(flag)
}

/// A child's request returned to it: set `flag` and wake a joining parent.
/// # Safety
/// A running task holding the role.
unsafe fn finished(ctx: *mut TaskCtx, probe: &Probe, flag: &str) {
    probe.set(flag);
    crate::tasks::wake_retrying(ctx);
}

/// These test children are unstructured and the run ends with its root, so
/// the root parks until each child's request has returned to it.
/// # Safety
/// A running task holding the role.
unsafe fn join(ctx: *mut TaskCtx, probe: &Probe, flags: &[&str]) {
    while !flags.iter().all(|flag| probe.has(flag)) {
        crate::tasks::park_retry(ctx, false);
    }
}

type Body = Box<dyn FnOnce(*mut TaskCtx) + Send>;

unsafe extern "C" fn child_entry(ctx: *mut u8, data: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let body = unsafe { Box::from_raw(data.cast::<Body>()) };
    let role = unsafe { role::Guard::new(ctx) };
    body(ctx);
    unsafe {
        let s = scheduler(ctx);
        let me = s.current;
        s.task(me).finishing = true;
        drop(role);
        role::enter(ctx);
        crate::pool::free::finish_current(ctx);
    }
}

/// A recorded child task, published before its parent's next operation.
/// # Safety
/// The parent holds the role in a recorded Free run.
unsafe fn spawn(ctx: *mut TaskCtx, body: Body) {
    unsafe {
        let stack = Stack::new();
        let child_ctx = stack.context();
        child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
        let path = if crate::tasks::free::record::on(ctx) {
            let child = crate::stop::check(ctx, crate::tasks::free::record::child(ctx));
            let path = child.path().to_vec();
            (*child_ctx).recording = Some(Box::new(child));
            path
        } else {
            vec![scheduler(ctx).tasks.len() as u32]
        };
        let s = scheduler(ctx);
        let id = s.tasks.len() as u32;
        (*child_ctx).task_id = id;
        (*child_ctx).task_sp = stack.prepare(
            child_entry,
            child_ctx.cast(),
            Box::into_raw(Box::new(body)).cast(),
        );
        let mut row = crate::tasks::Task::new(1, child_ctx, None, path, None);
        row.stack = Some(stack);
        s.tasks.push(Some(row));
        crate::pool::free::schedule_saved(
            (*ctx).run_ptr(),
            crate::tasks::scheduler(ctx).key(id),
            Owner::Any,
        );
    }
}

fn run_with(
    width: u32,
    durable: bool,
    probe: &Arc<Probe>,
    hook: Hook,
    body: impl FnOnce(*mut TaskCtx, &Arc<Probe>) + Send + 'static,
) -> crate::host::BodyOutcome {
    let recording = (!probe.has("unrecorded")).then(|| Recording::new(b"runtime host 50", durable));
    *probe.recording.lock().unwrap() = recording.clone();
    *probe.home.lock().unwrap() = Some(std::thread::current().id());
    let adapter: Arc<dyn ConcurrentAdapter> = Arc::new(Disk {
        probe: probe.clone(),
        hook,
    });
    let mut admitted = Some(adapter);
    let mut body = Some(body);
    let observed = probe.clone();
    let mut run = crate::host::BodyRun::new(width, false);
    run.free = true;
    unsafe { run.use_physical_dispatch() };
    run.run(
        move |ctx| unsafe {
            crate::tasks::free::enable(ctx);
            if let Some(recording) = recording {
                recording.root(ctx);
            }
        },
        move |ctx| unsafe {
            let _role = role::Guard::new(ctx);
            install(ctx, admitted.take().unwrap(), &observed);
            body.take().unwrap()(ctx, &observed);
            STATUS_OK
        },
    )
}

/// # Safety
/// A recorded Free task holding the role.
unsafe fn request(ctx: *mut TaskCtx, request: Request) -> Result<Completion, AdapterError> {
    if crate::tasks::free::record::on(ctx) {
        return unsafe { crate::stop::check(ctx, concurrent::request(ctx, request)) };
    }
    // Recording off: the same ownership through the live claim word.
    let io = unsafe { (*ctx).run_ref().free_io.get().unwrap() };
    let keys = io.adapter.keys(&request);
    let blocking = io.adapter.blocking(&request);
    unsafe { crate::stop::check(ctx, live::request(ctx, request, keys, blocking)) }
}

fn compute() -> u64 {
    let mut x = 1u64;
    for i in 0..200_000u64 {
        x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(i));
    }
    x
}

fn deadline_hook(probe: &Probe, request: &Request, point: Point) {
    match (request, point) {
        // The deadline passed already: report readiness at its first attempt.
        (Request::SleepUntil { .. }, Point::After) => {
            let token = probe.blocked.lock().unwrap().iter().copied().max().unwrap();
            probe.ready.lock().unwrap().push(token);
        }
        (Request::FileRead { .. }, Point::Before) => {
            probe.set("job held");
            if probe.wait("deadline") {
                probe.set("call saw deadline");
            }
            if probe.has("abandon") {
                assert!(probe.wait("release"), "test releases the held call");
            }
        }
        _ => {}
    }
}

#[test]
fn runtime50_blocking_file_allows_compute_and_deadline() {
    for width in [1, 8] {
        for abandon in [false, true] {
            for recorded in [true, false] {
                if !recorded && abandon {
                    continue;
                }
                let probe = Arc::new(Probe::default());
                if abandon {
                    probe.set("abandon");
                }
                if !recorded {
                    probe.set("unrecorded");
                }
                let outcome = run_with(width, false, &probe, deadline_hook, move |ctx, probe| {
                    let child = probe.clone();
                    unsafe {
                        spawn(
                            ctx,
                            Box::new(move |ctx| {
                                child.set(format!("computed {}", compute() & 1));
                                let deadline = Request::SleepUntil {
                                    clock: 17,
                                    deadline: 5,
                                };
                                assert_eq!(
                                    unsafe { request(ctx, deadline) },
                                    Ok(Completion::Wait(Ok(())))
                                );
                                child.set("deadline");
                                if child.has("abandon") {
                                    unsafe { await_flag(ctx, &child, "call saw deadline") };
                                    // Stop the run while the root's job is still held.
                                    unsafe { await_flag(ctx, &child, "job held") };
                                    unsafe {
                                        (*ctx).run_ref().control.request_inside(Stop::Requested);
                                        crate::stop::raise(ctx, Stop::Requested);
                                    }
                                }
                            }),
                        );
                        assert_eq!(request(ctx, file(7)), Ok(Completion::Bytes(Ok(vec![7]))));
                    }
                    probe.set("delivered");
                });
                let jobs = probe.jobs();
                let stats = jobs.stats();
                assert_eq!(probe.on_worker.load(Ordering::Acquire), 0);
                assert!(probe.violations.lock().unwrap().is_empty());
                assert!(
                    probe.has("call saw deadline"),
                    "width {width}: compute stalled: {outcome:?} {:?} {}",
                    probe.flags.lock().unwrap(),
                    probe.effects.load(Ordering::Acquire)
                );
                if abandon {
                    assert_eq!(outcome.ended, Err(Stop::Requested), "width {width}");
                    assert!(!probe.has("delivered"));
                    // The run has returned; the abandoned call still owns its capacity.
                    assert_eq!(jobs.held(), 1, "credit released before actual return");
                    assert_eq!(stats.returned.load(Ordering::Acquire), 0);
                    probe.set("release");
                    assert!(until(|| jobs.held() == 0));
                    assert!(until(|| stats.reclaimed.load(Ordering::Acquire) == 1));
                    assert_eq!(stats.quarantined.load(Ordering::Acquire), 1);
                    assert_eq!(stats.delivered.load(Ordering::Acquire), 0);
                } else {
                    assert_eq!(outcome.ended, Ok(STATUS_OK), "width {width}");
                    assert!(probe.has("delivered"));
                    assert_eq!(stats.delivered.load(Ordering::Acquire), 1);
                    assert_eq!(stats.quarantined.load(Ordering::Acquire), 0);
                    assert_eq!(jobs.held(), 0);
                }
                assert_eq!(probe.effects.load(Ordering::Acquire), 3);
            }
        }
    }
}

const CUT_FLAGS: [&str; 5] = [
    "cut before claim",
    "cut after claim",
    "cut after syscall",
    "cut at ended",
    "cut at done",
];

fn cut_hook(probe: &Probe, request: &Request, point: Point) {
    let at = |flag: &str| probe.has(flag) && !probe.has("cut");
    let stop = |probe: &Probe| {
        probe.set("cut");
        let (progress, sweep) = probe.stop();
        probe.note(format!("{progress:?} {sweep:?}"));
    };
    match point {
        Point::Classify if at(CUT_FLAGS[0]) => stop(probe),
        Point::Before if at(CUT_FLAGS[1]) => stop(probe),
        Point::After if at(CUT_FLAGS[2]) => stop(probe),
        Point::Finish if at(CUT_FLAGS[3]) => stop(probe),
        _ => {}
    }
    let _ = request;
}

/// A home-affine bounded call with the same claim word: before_claim and
/// the call both run on home through the synchronous bridge.
/// # Safety
/// Root body holding the role.
unsafe fn home_request(ctx: *mut TaskCtx, probe: &Arc<Probe>) -> Completion {
    let request = Request::Print {
        stdout: 3,
        bytes: b"home".to_vec(),
    };
    unsafe {
        crate::stop::check(ctx, prepare_with_keys(ctx, request, KeySet::coupled()));
        let before = probe.clone();
        let call = probe.clone();
        let claimed = crate::stop::check(
            ctx,
            issue_home_loan(
                ctx,
                move || {
                    assert_eq!(
                        Some(std::thread::current().id()),
                        *before.home.lock().unwrap()
                    );
                    cut_hook(&before, &Request::Stdout { io: 0 }, Point::Classify);
                },
                move |_| {
                    assert_eq!(
                        Some(std::thread::current().id()),
                        *call.home.lock().unwrap()
                    );
                    cut_hook(&call, &Request::Stdout { io: 0 }, Point::Before);
                    call.effects.fetch_add(1, Ordering::AcqRel);
                    cut_hook(&call, &Request::Stdout { io: 0 }, Point::After);
                    Ok(Completion::Unit(Ok(())))
                },
            ),
        );
        if !claimed {
            crate::stop::check(ctx, Err::<(), _>(Stop::Requested));
        }
        crate::stop::check(ctx, ended(ctx));
        cut_hook(probe, &Request::Stdout { io: 0 }, Point::Finish);
        crate::stop::check(ctx, complete(ctx))
    }
}

#[test]
fn runtime50_host_claim_cut_delivery_matrix() {
    for home in [false, true] {
        for width in [1, 8] {
            for durable in [false, true] {
                for (point, flag) in CUT_FLAGS.iter().enumerate() {
                    let probe = Arc::new(Probe::default());
                    probe.set(*flag);
                    let outcome = run_with(width, durable, &probe, cut_hook, move |ctx, probe| {
                        let result = if home {
                            Ok(unsafe { home_request(ctx, probe) })
                        } else {
                            unsafe { request(ctx, file(7)) }
                        };
                        probe.set("delivered");
                        assert!(!probe.has("cut"), "a cut request delivered its result");
                        let expected = if home {
                            Completion::Unit(Ok(()))
                        } else {
                            Completion::Bytes(Ok(vec![7]))
                        };
                        assert_eq!(result, Ok(expected));
                    });
                    let case = format!("home {home} width {width} durable {durable} {flag}");
                    let effects = probe.effects.load(Ordering::Acquire);
                    assert_eq!(probe.on_worker.load(Ordering::Acquire), 0, "{case}");
                    if point == 4 {
                        assert_eq!(outcome.ended, Ok(STATUS_OK), "{case}");
                        assert!(probe.has("delivered"), "{case}");
                        let (progress, sweep) = probe.stop();
                        assert_eq!(progress, Some(Progress::new(1, Phase::Done)), "{case}");
                        assert!(sweep.in_doubt.is_empty() && sweep.withdrawn.is_empty());
                        assert_eq!(effects, 1, "{case}");
                    } else {
                        assert_eq!(outcome.ended, Err(Stop::Requested), "{case}");
                        assert!(!probe.has("delivered"), "{case}: late delivery");
                        let log = probe.log.lock().unwrap().clone();
                        assert_eq!(log.len(), 1, "{case}");
                        let phase = match point {
                            0 => Phase::Entered,
                            1 | 2 => Phase::Claimed,
                            _ => Phase::Ended(0),
                        };
                        let (withdrawn, in_doubt) = if point == 0 {
                            ("[1]", "[]")
                        } else {
                            ("[]", "[1]")
                        };
                        assert_eq!(
                            log[0],
                            format!(
                                "{:?} Sweep {{ withdrawn: {withdrawn}, in_doubt: {in_doubt} }}",
                                Some(Progress::new(1, phase))
                            ),
                            "{case}"
                        );
                        // A withdrawn request never issues; a claimed one is in doubt.
                        assert_eq!(effects, u32::from(point != 0), "{case}");
                    }
                    if !home {
                        let jobs = probe.jobs();
                        assert!(until(|| jobs.held() == 0), "{case}");
                        let stats = jobs.stats();
                        assert_eq!(
                            stats.submitted.load(Ordering::Acquire),
                            u64::from(point != 0),
                            "{case}"
                        );
                        assert_eq!(
                            stats.delivered.load(Ordering::Acquire)
                                + stats.quarantined.load(Ordering::Acquire),
                            u64::from(point != 0),
                            "{case}: exactly one owner"
                        );
                    }
                }
            }
        }
    }
}

fn handle_hook(probe: &Probe, request: &Request, point: Point) {
    if let (Request::Open { path, .. }, Point::After) = (request, point) {
        probe.set(format!("opened {path}"));
        // Delay the initialized handle until the canceler ran.
        assert!(probe.wait(&format!("canceled {path}")));
        if probe.has("stop") && path == "h2" {
            assert!(probe.wait("release"));
        }
    }
}

#[test]
fn runtime50_handle_cancel_shield_and_reuse() {
    const ROUNDS: u64 = 3;
    for width in [1, 8] {
        for stop in [false, true] {
            let probe = Arc::new(Probe::default());
            if stop {
                probe.set("stop");
            }
            let outcome = run_with(width, false, &probe, handle_hook, move |ctx, probe| {
                for round in 0..ROUNDS {
                    let path = format!("h{round}");
                    let canceler = probe.clone();
                    let last = round + 1 == ROUNDS;
                    unsafe {
                        let target = path.clone();
                        spawn(
                            ctx,
                            Box::new(move |ctx| {
                                let opened = format!("opened {target}");
                                assert!(unsafe { await_flag(ctx, &canceler, &opened) });
                                // Cancel the owner while its handle is in flight.
                                scheduler(ctx).cancel(0, 3);
                                canceler.set(format!("canceled {target}"));
                                if last && canceler.has("stop") {
                                    unsafe {
                                        (*ctx).run_ref().control.request_inside(Stop::Requested);
                                        crate::stop::raise(ctx, Stop::Requested);
                                    }
                                }
                            }),
                        );
                        // Shielded cleanup: the cancellation cannot drop the handle.
                        (*ctx).shield = 1;
                        let opened = request(
                            ctx,
                            Request::Open {
                                directory: 1,
                                path: path.clone(),
                            },
                        );
                        (*ctx).shield = 0;
                        assert_eq!(opened, Ok(Completion::Handle(Ok(1000 + round))));
                        probe.set(format!("delivered {round}"));
                        // The run-owned row and token are free for the next round.
                        let io = (*ctx).run_ref().free_io.get().unwrap();
                        assert!(!io.test_waiting());
                        assert!((*ctx).pending_host.is_none());
                    }
                }
            });
            let jobs = probe.jobs();
            let stats = jobs.stats();
            let case = format!("width {width} stop {stop}");
            assert_eq!(probe.on_worker.load(Ordering::Acquire), 0, "{case}");
            if stop {
                assert_eq!(outcome.ended, Err(Stop::Requested), "{case}");
                assert!(!probe.has(&format!("delivered {}", ROUNDS - 1)), "{case}");
                assert_eq!(jobs.held(), 1, "{case}");
                probe.set("release");
                assert!(until(|| jobs.held() == 0), "{case}");
                assert!(until(|| stats.reclaimed.load(Ordering::Acquire) == 1));
                assert_eq!(
                    stats.delivered.load(Ordering::Acquire),
                    ROUNDS - 1,
                    "{case}"
                );
                assert_eq!(stats.quarantined.load(Ordering::Acquire), 1, "{case}");
            } else {
                assert_eq!(outcome.ended, Ok(STATUS_OK), "{case}");
                assert_eq!(stats.delivered.load(Ordering::Acquire), ROUNDS, "{case}");
                assert_eq!(stats.quarantined.load(Ordering::Acquire), 0, "{case}");
                assert_eq!(stats.reclaimed.load(Ordering::Acquire), 0, "{case}");
            }
            assert_eq!(stats.submitted.load(Ordering::Acquire), ROUNDS, "{case}");
            assert_eq!(
                probe.effects.load(Ordering::Acquire) as u64,
                ROUNDS,
                "{case}"
            );
        }
    }
}

fn order_hook(probe: &Probe, request: &Request, point: Point) {
    match (request, point) {
        (Request::FileRead { file, .. }, Point::Before) => {
            probe.note(format!("start {file}"));
            if *file == 7 {
                // Out of order: the later independent file returns first.
                probe.set("7 started");
                assert!(probe.wait("8 returned"), "independent file stalled");
            }
        }
        (Request::FileRead { file, .. }, Point::After) => {
            probe.note(format!("end {file}"));
            probe.set(format!("{file} returned"));
        }
        (Request::Remove { .. }, Point::Before) => {
            probe.note("start remove".into());
            // Long enough that an unordered shared member would overlap it.
            std::thread::sleep(Duration::from_millis(20));
        }
        (Request::Remove { .. }, Point::After) => probe.note("end remove".into()),
        (Request::Print { .. }, Point::Before) => probe.note("print".into()),
        (Request::ChildWait { child, .. }, Point::After) => {
            probe.note(format!("wait {child}"));
            if *child == 1 {
                probe.set("first waiting");
            } else {
                // The second waiter on this key ran while the first was parked.
                let token = *probe.blocked.lock().unwrap().iter().min().unwrap();
                probe.ready.lock().unwrap().push(token);
            }
        }
        _ => {}
    }
}

#[test]
fn runtime50_independent_keys_and_namespace() {
    for width in [1, 8] {
        let probe = Arc::new(Probe::default());
        let outcome = run_with(width, false, &probe, order_hook, move |ctx, probe| unsafe {
            let other = probe.clone();
            spawn(
                ctx,
                Box::new(move |ctx| {
                    assert!(unsafe { await_flag(ctx, &other, "7 started") });
                    assert_eq!(
                        unsafe { request(ctx, file(8)) },
                        Ok(Completion::Bytes(Ok(vec![8])))
                    );
                    unsafe { finished(ctx, &other, "8 delivered") };
                }),
            );
            assert_eq!(request(ctx, file(7)), Ok(Completion::Bytes(Ok(vec![7]))));
            // A namespace mutation races an alias of file 7 through another
            // mount and a file-backed sink; the exclusive epoch orders them.
            let remover = probe.clone();
            spawn(
                ctx,
                Box::new(move |ctx| {
                    let removed = Request::Remove {
                        directory: 1,
                        path: "x".into(),
                    };
                    assert_eq!(
                        unsafe { request(ctx, removed) },
                        Ok(Completion::Unit(Ok(())))
                    );
                    unsafe { finished(ctx, &remover, "removed") };
                }),
            );
            assert_eq!(request(ctx, file(9)), Ok(Completion::Bytes(Ok(vec![9]))));
            let sink = Request::Print {
                stdout: 5,
                bytes: b"sink".to_vec(),
            };
            assert_eq!(request(ctx, sink), Ok(Completion::Unit(Ok(()))));
            // A process wait keeps no key while it waits for readiness.
            let waiter = probe.clone();
            spawn(
                ctx,
                Box::new(move |ctx| {
                    assert!(unsafe { await_flag(ctx, &waiter, "first waiting") });
                    let second = Request::ChildWait {
                        process: 4,
                        child: 2,
                        deadline: None,
                    };
                    assert_eq!(
                        unsafe { request(ctx, second) },
                        Ok(Completion::ChildExit(Ok(
                            hirsel_run_schema::ExitStatus::Exited(0)
                        )))
                    );
                }),
            );
            let first = Request::ChildWait {
                process: 4,
                child: 1,
                deadline: None,
            };
            assert_eq!(
                request(ctx, first),
                Ok(Completion::ChildExit(Ok(
                    hirsel_run_schema::ExitStatus::Exited(0)
                )))
            );
            join(ctx, probe, &["8 delivered", "removed"]);
        });
        assert_eq!(outcome.ended, Ok(STATUS_OK), "width {width}");
        assert!(
            probe.violations.lock().unwrap().is_empty(),
            "{:?}",
            probe.violations
        );
        assert_eq!(probe.on_worker.load(Ordering::Acquire), 0);
        let log = probe.log.lock().unwrap().clone();
        let at = |entry: &str| log.iter().position(|e| e == entry).unwrap_or(usize::MAX);
        assert!(at("start 7") < at("start 8"), "{log:?}");
        assert!(
            at("end 8") < at("end 7"),
            "out-of-order independent files: {log:?}"
        );
        // The mutation is alone in its namespace epoch (no overlap above),
        // and the aliased resource keeps its own exclusive order.
        assert!(at("end 7") < at("start remove"), "{log:?}");
        assert!(
            at("end remove") < at("start 9") || at("end 9") < at("start remove"),
            "alias overlapped the namespace mutation: {log:?}"
        );
        assert!(at("start 9") < at("print"), "{log:?}");
        // wait 1 (Blocked), wait 2 on the same key while 1 waits, then 1 again.
        let waits: Vec<_> = log.iter().filter(|e| e.starts_with("wait ")).collect();
        assert_eq!(waits[..2], ["wait 1", "wait 2"], "{log:?}");
        assert!(
            waits.iter().filter(|e| **e == "wait 1").count() == 2,
            "{log:?}"
        );
        let jobs = probe.jobs();
        assert!(
            jobs.stats().peak.load(Ordering::Acquire) >= 2,
            "two jobs overlapped"
        );
        assert_eq!(jobs.stats().delivered.load(Ordering::Acquire), 4);
    }
}

/// Both children issue their blocking calls only once the root's own body has
/// ended: the run lasts until each takes its job's result, which the root's
/// join awaits.
#[test]
fn runtime50_root_joins_children_whose_jobs_outlast_its_body() {
    for width in [1, 8] {
        let probe = Arc::new(Probe::default());
        let outcome = run_with(width, false, &probe, order_hook, move |ctx, probe| unsafe {
            let reader = probe.clone();
            spawn(
                ctx,
                Box::new(move |ctx| {
                    unsafe { join(ctx, &reader, &["root ended"]) };
                    assert_eq!(
                        unsafe { request(ctx, file(8)) },
                        Ok(Completion::Bytes(Ok(vec![8])))
                    );
                    unsafe { finished(ctx, &reader, "8 delivered") };
                }),
            );
            let remover = probe.clone();
            spawn(
                ctx,
                Box::new(move |ctx| {
                    unsafe { join(ctx, &remover, &["root ended"]) };
                    let removed = Request::Remove {
                        directory: 1,
                        path: "x".into(),
                    };
                    assert_eq!(
                        unsafe { request(ctx, removed) },
                        Ok(Completion::Unit(Ok(())))
                    );
                    unsafe { finished(ctx, &remover, "removed") };
                }),
            );
            assert_eq!(request(ctx, file(9)), Ok(Completion::Bytes(Ok(vec![9]))));
            assert_eq!(request(ctx, file(10)), Ok(Completion::Bytes(Ok(vec![10]))));
            assert!(!probe.has("8 delivered") && !probe.has("removed"));
            finished(ctx, probe, "root ended");
            join(ctx, probe, &["8 delivered", "removed"]);
        });
        assert_eq!(outcome.ended, Ok(STATUS_OK), "width {width}");
        assert!(
            probe.violations.lock().unwrap().is_empty(),
            "{:?}",
            probe.violations
        );
        assert_eq!(probe.on_worker.load(Ordering::Acquire), 0);
        let log = probe.log.lock().unwrap().clone();
        assert!(log.contains(&"end 8".to_string()), "{log:?}");
        assert!(log.contains(&"end remove".to_string()), "{log:?}");
        assert_eq!(probe.jobs().stats().delivered.load(Ordering::Acquire), 4);
    }
}
