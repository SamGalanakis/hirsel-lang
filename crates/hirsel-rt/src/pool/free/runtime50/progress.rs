//! The two repaired home/role/compute cycles on real threads and fibers, each
//! under controlled orderings of its permit, role and home transitions, in an
//! owned child process with the five-second watchdog of progress_tests.rs and
//! progress_audit_45.rs. Each run checks its transition transcript.
//!
//! - Contraction: a compute child owns C and R and issues a synchronous home
//!   Call while its selected parent owns C and needs R for ScopeExit, and the
//!   process grant contracts 2 -> 1 (before the Call, inside the home's
//!   adapter service, or not at all).
//! - Callback: a home foreign callback parent owns C on home and needs the
//!   child's R for ScopeExit while the child's Prepared Call waits for home
//!   (with or without a contraction once the Call is queued).
use crate::pool::*;
use crate::tasks::{self, role};
use std::sync::mpsc;

const CHILD: &str = "HIRSEL_RUNTIME50_PROGRESS";
const TEST: &str =
    "pool::free::audit_modules::runtime50::progress::runtime50_composed_home_compute_progress";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Contract {
    Never,
    BeforeCall,
    InHome,
}

struct Probe {
    contract: Contract,
    home: std::thread::ThreadId,
    arrived: (Mutex<bool>, Condvar),
    proceed: (Mutex<bool>, Condvar),
    transitions: Mutex<Vec<(&'static str, u32)>>,
}

impl Probe {
    fn new(contract: Contract) -> Arc<Self> {
        Arc::new(Self {
            contract,
            home: std::thread::current().id(),
            arrived: Default::default(),
            proceed: Default::default(),
            transitions: Mutex::new(Vec::new()),
        })
    }
    fn mark(&self, label: &'static str) {
        let (limit, held, _) = compute_permits();
        self.transitions.lock().unwrap().push((label, limit));
        eprintln!("RT50 {label} permits limit={limit} held={held}");
    }
}

fn publish(pair: &(Mutex<bool>, Condvar)) {
    *pair.0.lock().unwrap() = true;
    pair.1.notify_one();
}

fn wait(pair: &(Mutex<bool>, Condvar)) {
    let mut ready = pair.0.lock().unwrap();
    while !*ready {
        ready = pair.1.wait(ready).unwrap();
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
    fn wants_issuer(&self) -> bool {
        // The child's synchronous metadata Call, served on the home thread.
        assert_eq!(std::thread::current().id(), self.0.home);
        if self.0.contract == Contract::InHome {
            set_compute_grant(1);
            self.0
                .mark("home contracted the grant while serving the Call");
        }
        self.0.mark("home served the child's Call");
        false
    }
}

unsafe fn home_call(ctx: *mut TaskCtx) {
    unsafe {
        assert!(matches!(
            crate::host_waits::issue(ctx, crate::io::Request::Stdout { io: 1 }),
            Ok((Ok(crate::io::Completion::Handle(Ok(12))), false))
        ));
        assert_eq!(
            tasks::ordered::host::concurrent::finish_home(
                ctx,
                Ok(crate::io::Completion::Handle(Ok(12))),
                true,
                false,
            ),
            Ok(crate::io::Completion::Handle(Ok(12)))
        );
    }
}

unsafe fn probe(environment: *mut crate::Header) -> &'static Probe {
    unsafe {
        let address = crate::alloc::payload(environment).cast::<usize>().read();
        &*(address as *const Probe)
    }
}

unsafe extern "C" fn contraction_child(
    ctx: *mut TaskCtx,
    environment: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    unsafe {
        let probe = probe(environment);
        let _role = role::Guard::new(ctx);
        wait(&probe.arrived);
        assert!((*current(execs(ctx))).held.load(Ordering::Acquire));
        assert_eq!(compute_permits().1, 2);
        probe.mark("child owns C and R; parent owns C and needs R");
        if probe.contract == Contract::BeforeCall {
            set_compute_grant(1);
            probe.mark("child contracted the grant before its Call");
        }
        publish(&probe.proceed);
        home_call(ctx);
        probe.mark("child returned from home with its permit");
        crate::STATUS_OK
    }
}

unsafe extern "C" fn callback_child(
    ctx: *mut TaskCtx,
    environment: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    unsafe {
        let probe = probe(environment);
        assert_ne!(std::thread::current().id(), probe.home);
        let _role = role::Guard::new(ctx);
        assert!((*current(execs(ctx))).held.load(Ordering::Acquire));
        home_call(ctx);
        probe.mark("child returned from home with its permit");
        crate::STATUS_OK
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

unsafe fn spawn(
    ctx: *mut TaskCtx,
    scope: &u64,
    body: unsafe extern "C" fn(*mut TaskCtx, *mut crate::Header, u64, u64) -> u8,
    probe: &Arc<Probe>,
) {
    unsafe {
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
                scope,
                body,
                env,
                0
            ),
            crate::STATUS_OK
        );
    }
}

fn cycle(callback: bool, contract: Contract) -> Vec<(&'static str, u32)> {
    set_compute_grant(2);
    let probe = Probe::new(contract);
    let layout = layout();
    let layouts = [&layout as *const crate::Layout];
    let mut run = crate::host::BodyRun::new(2, false);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    run.adapter = Some(Box::new(Home(probe.clone())));
    unsafe { run.use_ordered_home_dispatch() };
    let seen = probe.clone();
    let outcome = run.run(
        |ctx| unsafe {
            tasks::ordered::engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
        },
        move |ctx| unsafe {
            let mut interval = [0u64; crate::foreign::FOREIGN_SCOPE_BYTES / 8];
            if callback {
                crate::foreign::hirsel_rt_v2_foreign_enter(
                    ctx,
                    0,
                    interval.as_mut_ptr().cast(),
                    crate::foreign::FOREIGN_THREADS_HOME,
                );
                assert_eq!(std::thread::current().id(), seen.home);
                assert_eq!(crate::foreign::hirsel_rt_v2_callback_context(), ctx);
                assert_eq!((*ctx).callback_depth, 1);
            }
            let mut scope = 0;
            assert_eq!(
                tasks::hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                crate::STATUS_OK
            );
            if callback {
                spawn(ctx, &scope, callback_child, &seen);
                assert!((*current(execs(ctx))).held.load(Ordering::Acquire));
                let home = &execs(ctx).home;
                let mut inbox = home.inbox.lock().unwrap();
                while !inbox.posts.iter().any(|(post, _)| {
                    matches!(post, Post::Call(call) if call.claim.state() == hirsel_free::claim::Claim::Prepared)
                }) {
                    drop(inbox);
                home.bell.wait();
                inbox = home.inbox.lock().unwrap();
                }
                drop(inbox);
                assert_eq!(compute_permits(), (2, 2, 0));
                seen.mark("callback parent owns home C; child's Prepared Call is queued");
                if contract == Contract::BeforeCall {
                    set_compute_grant(1);
                    seen.mark("callback parent contracted the grant");
                }
            } else {
                spawn(ctx, &scope, contraction_child, &seen);
                assert!((*current(execs(ctx))).held.load(Ordering::Acquire));
                seen.mark("selected parent owns C before its next runtime entry");
                publish(&seen.arrived);
                wait(&seen.proceed);
            }
            assert_eq!(
                tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                crate::STATUS_OK
            );
            seen.mark("parent ScopeExit returned");
            if callback {
                crate::foreign::hirsel_rt_v2_callback_leave(ctx);
                crate::foreign::hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
            }
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
    let limit = if contract == Contract::Never { 2 } else { 1 };
    assert_eq!(compute_permits(), (limit, 0, 0));
    probe.transitions.lock().unwrap().clone()
}

/// The transition transcript each controlled ordering must reproduce.
fn expected(callback: bool, contract: Contract) -> Vec<(&'static str, u32)> {
    let mut out = Vec::new();
    let mut limit = 2;
    if callback {
        out.push((
            "callback parent owns home C; child's Prepared Call is queued",
            2,
        ));
        if contract == Contract::BeforeCall {
            limit = 1;
            out.push(("callback parent contracted the grant", 1));
        }
    } else {
        out.push(("selected parent owns C before its next runtime entry", 2));
        out.push(("child owns C and R; parent owns C and needs R", 2));
        if contract == Contract::BeforeCall {
            limit = 1;
            out.push(("child contracted the grant before its Call", 1));
        }
    }
    if contract == Contract::InHome {
        limit = 1;
        out.push(("home contracted the grant while serving the Call", 1));
    }
    out.push(("home served the child's Call", limit));
    out.push(("child returned from home with its permit", limit));
    out.push(("parent ScopeExit returned", limit));
    out
}

const VARIANTS: [(bool, Contract); 6] = [
    (false, Contract::BeforeCall),
    (false, Contract::InHome),
    (false, Contract::Never),
    (true, Contract::Never),
    (true, Contract::BeforeCall),
    (true, Contract::InHome),
];

#[test]
fn runtime50_composed_home_compute_progress() {
    if let Some(variant) = std::env::var_os(CHILD) {
        let (callback, contract) = VARIANTS[variant.to_str().unwrap().parse::<usize>().unwrap()];
        let transcript = cycle(callback, contract);
        assert_eq!(transcript, expected(callback, contract));
        eprintln!(
            "RT50 transcript reproduced: {} transitions",
            transcript.len()
        );
        return;
    }
    for (index, variant) in VARIANTS.iter().enumerate() {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD, index.to_string())
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let (done, ended) = mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            done.send(child.wait_with_output().unwrap()).unwrap();
        });
        let output = ended.recv_timeout(Duration::from_secs(5));
        if output.is_err() {
            // Only our child, so a deadlocked runtime cannot strand workers.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        let output = output.unwrap_or_else(|_| ended.recv().unwrap());
        watchdog.join().unwrap();
        let log = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && log.contains("RT50 transcript reproduced"),
            "variant {variant:?} failed normal progress within the watchdog:\n{log}"
        );
        eprintln!("RUNTIME50 progress {variant:?}: ok");
    }
}
