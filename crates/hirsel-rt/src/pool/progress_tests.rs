//! Real Ordered selection with a contracting process grant, without Free policy.
use super::*;
use crate::tasks::{self, role};
use std::sync::mpsc;

struct Probe {
    arrived: (Mutex<bool>, Condvar),
    proceed: (Mutex<bool>, Condvar),
    called: AtomicBool,
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
    fn call(
        &mut self,
        request: crate::io::Request,
    ) -> Result<crate::io::Completion, crate::io::AdapterError> {
        assert!(matches!(request, crate::io::Request::Stdout { io: 1 }));
        Ok(crate::io::Completion::Handle(Ok(12)))
    }

    fn preflight(
        &self,
        _: &crate::io::Request,
    ) -> Result<Option<crate::io::Completion>, crate::io::AdapterError> {
        Ok(None)
    }

    fn wants_issuer(&self) -> bool {
        self.0.called.store(true, Ordering::Release);
        eprintln!("home completed the role owner's synchronous metadata call");
        false
    }
}

unsafe extern "C" fn bridge_child(
    ctx: *mut TaskCtx,
    environment: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    unsafe {
        let address = crate::alloc::payload(environment).cast::<usize>().read();
        let probe = &*(address as *const Probe);
        let _role = role::Guard::new(ctx);
        eprintln!("bridge child entered R");
        // B is the real selected parent, running ahead after its spawn.
        // Both physical Running tokens and permits came from free::resume.
        wait(&probe.arrived);
        assert_eq!(compute_permits(), (2, 2, 0));
        set_compute_grant(1);
        publish(&probe.proceed);
        eprintln!("A owns C+R; B owns C and needs R; process grant contracted 2 -> 1");
        // The actual Ordered host-request path asks for issuer metadata
        // through HomeAdapter while retaining R, before issuing the effect.
        assert!(matches!(
            crate::host_waits::issue(ctx, crate::io::Request::Stdout { io: 1 }),
            Ok((Ok(crate::io::Completion::Handle(Ok(12))), false))
        ));
        assert!(probe.called.load(Ordering::Acquire));
        eprintln!("A returned from home with its permit");
        // Complete the actual host operation before ordinary child cleanup.
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

#[test]
fn role_owned_home_bridge_progresses_after_compute_grant_contraction() {
    const CHILD: &str = "HIRSEL_HOME_BRIDGE_CONTRACTION";
    if std::env::var_os(CHILD).is_none() {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "pool::progress_tests::role_owned_home_bridge_progresses_after_compute_grant_contraction",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .spawn()
            .unwrap();
        let pid = child.id();
        let (done, ended) = mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            let mut child = child;
            done.send(child.wait().unwrap()).unwrap();
        });
        let status = ended.recv_timeout(Duration::from_secs(5));
        if status.is_err() {
            // Only our child, so a deadlocked runtime cannot strand workers.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        watchdog.join().unwrap();
        assert!(
            status.is_ok_and(|status| status.success()),
            "role owner could not return from home after the compute grant contracted"
        );
        return;
    }
    set_compute_grant(2);
    let probe = Arc::new(Probe {
        arrived: Default::default(),
        proceed: Default::default(),
        called: AtomicBool::new(false),
    });
    let layout = crate::Layout {
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
    };
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
            let mut scope = 0;
            assert_eq!(
                tasks::hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                crate::STATUS_OK
            );
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env)
                .cast::<usize>()
                .write(Arc::as_ptr(&seen) as usize);
            eprintln!("root spawning child");
            let mut child = 0;
            assert_eq!(
                tasks::hirsel_rt_v2_task_spawn(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut child,
                    &scope,
                    bridge_child,
                    env,
                    0
                ),
                crate::STATUS_OK
            );
            assert!((*current(execs(ctx))).held.load(Ordering::Acquire));
            eprintln!("selected parent holds C before its next runtime entry");
            publish(&seen.arrived);
            wait(&seen.proceed);
            tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0)
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
    assert!(probe.called.load(Ordering::Acquire));
    assert_eq!(compute_permits(), (1, 0, 0));
}
