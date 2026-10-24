//! Runtime r6 S9 witnesses: the two repaired home/role/compute cycles at one
//! process permit, and eager capacity with lost-permit accounting.
use crate::pool::*;
use crate::tasks::{self, role};
use std::sync::mpsc;

/// Run `name` in an owned child process under a five-second watchdog: a
/// deadlocked runtime is killed rather than stranding this test binary.
fn watchdog(name: &str) -> bool {
    const CHILD: &str = "HIRSEL_RUNTIME50_CHILD";
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("pool::audit_modules::runtime50_tests::{name}"),
            "--nocapture",
        ])
        .env(CHILD, "1")
        .spawn()
        .unwrap();
    let pid = child.id();
    let (done, ended) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let mut child = child;
        done.send(child.wait().unwrap()).unwrap();
    });
    let status = ended.recv_timeout(Duration::from_secs(5));
    if status.is_err() {
        // Only our own child, by pid.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
    waiter.join().unwrap();
    assert!(
        status.is_ok_and(|status| status.success()),
        "{name} made no progress within the watchdog"
    );
    false
}

fn eventually(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::yield_now();
    }
}

fn layout() -> crate::Layout {
    crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 8,
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

/// What both cycles share: the home thread's identity, the run, and the
/// contender that wants C and then R while a synchronous home loan is open.
struct Probe {
    home: std::thread::ThreadId,
    run: AtomicUsize,
    contended: AtomicBool,
    contender: Mutex<Option<std::thread::JoinHandle<()>>>,
    called: AtomicBool,
}

impl Probe {
    fn new() -> Arc<Self> {
        Arc::new(Probe {
            home: std::thread::current().id(),
            run: AtomicUsize::new(0),
            contended: AtomicBool::new(false),
            contender: Mutex::new(None),
            called: AtomicBool::new(false),
        })
    }
}

struct Home(Arc<Probe>);
impl crate::io::Adapter for Home {
    fn preflight(
        &self,
        _: &crate::io::Request,
    ) -> Result<Option<crate::io::Completion>, crate::io::AdapterError> {
        Ok(None)
    }
    fn call(
        &mut self,
        request: crate::io::Request,
    ) -> Result<crate::io::Completion, crate::io::AdapterError> {
        assert!(matches!(request, crate::io::Request::Stdout { io: 1 }));
        Ok(crate::io::Completion::Handle(Ok(12)))
    }
    /// Served on home inside the caller's synchronous loan, while the caller
    /// owns R and the only compute permit.
    fn wants_issuer(&self) -> bool {
        assert_eq!(std::thread::current().id(), self.0.home);
        self.0.called.store(true, Ordering::Release);
        let probe = self.0.clone();
        if probe.contended.swap(true, Ordering::AcqRel) {
            return false;
        }
        // Another compute owner that will want R. Had the loan returned C,
        // it would take C and wait for R, while the caller's return waited
        // for C: the fixed R -> C -> R cycle.
        let run = probe.run.load(Ordering::Acquire);
        let contender = std::thread::spawn(move || {
            let exec = Exec::new(Kind::Pool(0), std::ptr::null_mut(), false);
            let permits = process_permits();
            assert!(permits.acquire(&exec, None, None).unwrap());
            {
                let _role = unsafe { role::IdleGuard::new(run as *mut RunShared) };
            }
            permits.release(&exec);
        });
        *probe.contender.lock().unwrap() = Some(contender);
        eventually("the contender waiting for the only permit", || {
            compute_permits() == (1, 1, 1)
        });
        eprintln!("RUNTIME50 loan holds R and C; a C-then-R contender waits");
        false
    }
}

unsafe extern "C" fn loan_child(ctx: *mut TaskCtx, _: *mut crate::Header, _: u64, _: u64) -> u8 {
    unsafe {
        let _role = role::Guard::new(ctx);
        assert!((*current(execs(ctx))).held.load(Ordering::Acquire));
        assert!(matches!(
            crate::host_waits::issue(ctx, crate::io::Request::Stdout { io: 1 }),
            Ok((Ok(crate::io::Completion::Handle(Ok(12))), false))
        ));
        assert!((*current(execs(ctx))).held.load(Ordering::Acquire));
        eprintln!("RUNTIME50 loan returned with its retained permit");
        assert_eq!(
            tasks::ordered::host::concurrent::finish_home(
                ctx,
                Ok(crate::io::Completion::Handle(Ok(12))),
                true,
                false,
            ),
            Ok(crate::io::Completion::Handle(Ok(12)))
        );
        crate::STATUS_OK
    }
}

unsafe fn spawn_loan_child(ctx: *mut TaskCtx, probe: &Arc<Probe>) -> u64 {
    unsafe {
        let mut scope = 0;
        assert_eq!(
            tasks::hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
            crate::STATUS_OK
        );
        let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(env)
            .cast::<usize>()
            .write(Arc::as_ptr(probe) as usize);
        let mut child = 0;
        assert_eq!(
            tasks::hirsel_rt_v2_task_spawn(
                ctx,
                0,
                0,
                std::ptr::null_mut(),
                &mut child,
                &scope,
                loan_child,
                env,
                0
            ),
            crate::STATUS_OK
        );
        scope
    }
}

/// The root lends its permit to the contender, outside R, before returning:
/// the run outlives every role acquisition the witness started.
unsafe fn join_contender(ctx: *mut TaskCtx, probe: &Probe) {
    let contender = probe.contender.lock().unwrap().take();
    release_permit(ctx);
    if let Some(contender) = contender {
        contender.join().unwrap();
    }
    assert!(acquire_permit(ctx));
}

fn ordered_run(probe: &Arc<Probe>, layouts: &[*const crate::Layout; 1]) -> crate::host::BodyRun {
    let mut run = crate::host::BodyRun::new(2, false);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    run.adapter = Some(Box::new(Home(probe.clone())));
    unsafe { run.use_ordered_home_dispatch() };
    run
}

fn admit_ordered(ctx: *mut TaskCtx) {
    unsafe {
        tasks::ordered::engine::enable(
            ctx,
            crate::ordered::Policy::admit(&Default::default(), None),
        )
    };
}

/// R -> C -> R: a role owner's synchronous home loan retains its compute
/// permit while another worker needs C before R. Then home -> R -> home: a
/// home foreign callback waits for a child that owns R and needs home.
#[test]
fn runtime50_home_permit_cycles() {
    if !watchdog("runtime50_home_permit_cycles") {
        return;
    }
    set_compute_grant(1);
    let layout = layout();
    let layouts = [&layout as *const crate::Layout];

    // R -> C -> R at one permit.
    let probe = Probe::new();
    let seen = probe.clone();
    let outcome = ordered_run(&probe, &layouts).run(admit_ordered, move |ctx| unsafe {
        seen.run.store((*ctx).run_ptr() as usize, Ordering::Release);
        let scope = spawn_loan_child(ctx, &seen);
        // Parking at the join returns this permit; the child takes it.
        let status = tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
        join_contender(ctx, &seen);
        status
    });
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
    assert!(probe.called.load(Ordering::Acquire));
    assert!(probe.contended.load(Ordering::Acquire));
    assert_eq!(compute_permits(), (1, 0, 0));
    eprintln!("RUNTIME50 R->C->R completed at one permit");

    // home -> R -> home at one permit: the parent is a home foreign callback.
    let probe = Probe::new();
    let seen = probe.clone();
    let outcome = ordered_run(&probe, &layouts).run(admit_ordered, move |ctx| unsafe {
        seen.run.store((*ctx).run_ptr() as usize, Ordering::Release);
        let mut interval = [0u64; crate::foreign::FOREIGN_SCOPE_BYTES / 8];
        crate::foreign::hirsel_rt_v2_foreign_enter(
            ctx,
            0,
            interval.as_mut_ptr().cast(),
            crate::foreign::FOREIGN_THREADS_HOME,
        );
        assert_eq!(std::thread::current().id(), seen.home);
        assert_eq!(crate::foreign::hirsel_rt_v2_callback_context(), ctx);
        assert_eq!((*ctx).callback_depth, 1);
        assert!((*current(execs(ctx))).held.load(Ordering::Acquire));
        let scope = spawn_loan_child(ctx, &seen);
        assert_eq!(std::thread::current().id(), seen.home);
        eprintln!("RUNTIME50 home callback parent joins a child that needs R and home");
        let status = tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
        assert_eq!(std::thread::current().id(), seen.home);
        join_contender(ctx, &seen);
        crate::foreign::hirsel_rt_v2_callback_leave(ctx);
        crate::foreign::hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
        status
    });
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
    assert!(probe.called.load(Ordering::Acquire));
    assert_eq!(compute_permits(), (1, 0, 0));
    eprintln!("RUNTIME50 home->R->home completed at one permit");
}

fn manager(
    control: crate::host::RunControl,
    width: u32,
) -> Arc<crate::managed_cores::ManagedCores> {
    use hirsel_run_config::managed_cores::{CpuSet, HostGrant, HostLimits};
    let cpus = CpuSet::parse("0-7").unwrap();
    Arc::new(
        crate::managed_cores::ManagedCores::admit(
            hirsel_run_config::Placement {
                cores: hirsel_run_config::Cores::Fixed(width.into()),
                ..Default::default()
            },
            HostGrant {
                max_cores: 8,
                cpus: cpus.clone(),
            },
            HostLimits {
                admission_affinity: cpus.clone(),
                effective_cpuset: cpus,
                quota: None,
                throttled_usec: 0,
            },
            Duration::ZERO,
            control,
            true,
        )
        .unwrap(),
    )
}

#[test]
fn runtime50_capacity_target_and_lost_permit() {
    if !watchdog("runtime50_capacity_target_and_lost_permit") {
        return;
    }
    use hirsel_run_config::managed_cores::WorkerPhase;
    set_compute_grant(4);

    // Eager capacity: the full target is provisioned before the root runs,
    // and pending stays the full target until effective matches it.
    let mut run = crate::host::BodyRun::new(4, false);
    let control = run.control();
    let cores = manager(control.clone(), 4);
    let before = cores.snapshot();
    assert_eq!(before.worker_phase, Some(WorkerPhase::Starting));
    assert_eq!((before.width.effective, before.width.pending), (0, 4));
    let observed = cores.clone();
    unsafe { run.use_physical_dispatch() };
    let outcome = run.run(
        |_| {},
        move |ctx| {
            assert_eq!(execs(ctx).compute.lock().unwrap().active.len(), 4);
            assert_eq!(control.worker_width(), 4);
            let sample = observed.snapshot();
            assert_eq!(sample.worker_phase, Some(WorkerPhase::Running));
            assert_eq!((sample.width.effective, sample.width.pending), (4, 0));
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    let partial = Arc::new(crate::stop::Control::default());
    let cores = manager(crate::host::RunControl::from_inner(partial.clone()), 4);
    partial.live_width.store(3, Ordering::Release);
    let sample = cores.snapshot();
    assert_eq!((sample.width.effective, sample.width.pending), (3, 4));
    partial.finish(0);

    // A stopped nonfuel pure loop keeps its worker and permit, reported lost.
    set_compute_grant(2);
    let release = Arc::new(AtomicBool::new(false));
    let run = crate::host::BodyRun::new(2, false);
    let stopped = run.control();
    let entered = Arc::new(AtomicBool::new(false));
    let stopper = std::thread::spawn({
        let (control, entered) = (stopped.clone(), entered.clone());
        move || {
            eventually("divergent root entry", || entered.load(Ordering::SeqCst));
            control.stop();
        }
    });
    let outcome = run.run(|_| {}, {
        let (release, entered) = (release.clone(), entered.clone());
        move |_| {
            entered.store(true, Ordering::SeqCst);
            while !release.load(Ordering::SeqCst) {
                std::hint::spin_loop();
            }
            crate::STATUS_OK
        }
    });
    stopper.join().unwrap();
    assert_eq!(outcome.ended, Err(Stop::Requested));
    assert_eq!(outcome.pending, 1);
    assert_eq!(lost_compute_permits(), 1);
    assert_eq!(compute_permits(), (2, 1, 0));

    // effective <= grant - lost, and the lost permit is never replaced: with
    // the next root running, the grant is exhausted and a new holder waits.
    let mut run = crate::host::BodyRun::new(2, false);
    let control = run.control();
    let cores = manager(control.clone(), 2);
    unsafe { run.use_physical_dispatch() };
    let observed = cores.clone();
    let outcome = run.run(|_| {}, {
        let release = release.clone();
        move |_| {
            assert_eq!(control.worker_width(), 2);
            let sample = observed.snapshot();
            assert_eq!(sample.lost_permits, Some(1));
            assert_eq!(sample.width.effective, 1, "effective <= grant - lost");
            assert_eq!(sample.width.pending, 2, "pending is the full target");
            assert_eq!(compute_permits(), (2, 2, 0));
            let waiter = std::thread::spawn(|| {
                let exec = Exec::new(Kind::Pool(0), std::ptr::null_mut(), false);
                let permits = process_permits();
                assert!(permits.acquire(&exec, None, None).unwrap());
                permits.release(&exec);
            });
            eventually("a new holder waiting at the grant", || {
                compute_permits() == (2, 2, 1)
            });
            assert_eq!(lost_compute_permits(), 1);
            // Only the stopped worker leaving code returns its permit.
            release.store(true, Ordering::SeqCst);
            waiter.join().unwrap();
            assert_eq!(lost_compute_permits(), 0);
            crate::STATUS_OK
        }
    });
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    eventually("the stopped run reclaimed", || {
        stopped.status().reclamation_pending == 0
    });
    assert_eq!(compute_permits(), (2, 0, 0));
}
