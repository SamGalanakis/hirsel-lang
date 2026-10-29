//! Audit witness: home callback parent versus role-owning child's home bridge.
use crate::pool::*;
use crate::tasks::{self, role};
use std::sync::mpsc;

struct Probe {
    home: std::thread::ThreadId,
    called: AtomicBool,
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
        assert_eq!(std::thread::current().id(), self.0.home);
        self.0.called.store(true, Ordering::Release);
        eprintln!("AUDIT45 home served child's metadata Call");
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
        assert_ne!(std::thread::current().id(), probe.home);
        let _role = role::Guard::new(ctx);
        assert!((*current(execs(ctx))).held.load(Ordering::Acquire));
        assert!(!(*ctx).run_ref().free_production);
        eprintln!("AUDIT45 selected child owns actual C and R on compute");
        assert!(matches!(
            crate::host_waits::issue(ctx, crate::io::Request::Stdout { io: 1 }),
            Ok((Ok(crate::io::Completion::Handle(Ok(12))), false))
        ));
        eprintln!("AUDIT45 child returned from home");
        assert_eq!(
            tasks::ordered::host::concurrent::finish_home(
                ctx,
                Ok(crate::io::Completion::Handle(Ok(12))),
                true,
                false
            ),
            Ok(crate::io::Completion::Handle(Ok(12)))
        );
        crate::STATUS_OK
    }
}

#[test]
fn home_callback_parent_and_child_bridge_make_progress() {
    const CHILD: &str = "HIRSEL_POOL_HOST_PROGRESS_AUDIT_45";
    if std::env::var_os(CHILD).is_none() {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "pool::audit_modules::progress_audit_45::home_callback_parent_and_child_bridge_make_progress", "--nocapture"])
            .env(CHILD, "1").spawn().unwrap();
        let pid = child.id();
        let (done, ended) = mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            let mut child = child;
            done.send(child.wait().unwrap()).unwrap();
        });
        let status = ended.recv_timeout(Duration::from_secs(5));
        if status.is_err() {
            eprintln!("AUDIT45 five-second watchdog killing only owned child");
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        watchdog.join().unwrap();
        assert!(
            status.is_ok_and(|status| status.success()),
            "home callback and child bridge failed normal progress"
        );
        return;
    }
    set_compute_grant(2);
    let probe = Arc::new(Probe {
        home: std::thread::current().id(),
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
    let outcome = run.run(
        |ctx| unsafe { tasks::ordered::engine::enable(ctx, crate::ordered::Policy::admit(&Default::default(), None)); },
        move |ctx| unsafe {
            let mut interval = [0u64; crate::foreign::FOREIGN_SCOPE_BYTES / 8];
            crate::foreign::hirsel_rt_v2_foreign_enter(ctx, 0, interval.as_mut_ptr().cast(), crate::foreign::FOREIGN_THREADS_HOME);
            assert_eq!(std::thread::current().id(), probe.home);
            assert_eq!(crate::foreign::hirsel_rt_v2_callback_context(), ctx);
            assert_eq!((*ctx).callback_depth, 1);
            assert!((*current(execs(ctx))).held.load(Ordering::Acquire));
            eprintln!("AUDIT45 real home foreign callback entered with C");
            let mut scope = 0;
            assert_eq!(tasks::hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0), crate::STATUS_OK);
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env).cast::<usize>().write(Arc::as_ptr(&probe) as usize);
            let mut child = 0;
            assert_eq!(tasks::hirsel_rt_v2_task_spawn(ctx, 0, 0, std::ptr::null_mut(), &mut child, &scope, bridge_child, env, 0), crate::STATUS_OK);
            assert_eq!(std::thread::current().id(), probe.home);
            assert!(crate::foreign::active());
            assert_eq!((*ctx).callback_depth, 1);
            let home = &execs(ctx).home;
            let mut inbox = home.inbox.lock().unwrap();
            while !inbox.posts.iter().any(|(post, _)| matches!(post, Post::Call(_))) {
                drop(inbox);
                home.bell.wait();
                inbox = home.inbox.lock().unwrap();
            }
            assert!(inbox.posts.iter().any(|(post, _)| matches!(post, Post::Call(call) if call.claim.state() == hirsel_free::claim::Claim::Prepared)));
            drop(inbox);
            assert_eq!(compute_permits(), (2, 2, 0));
            assert!(!probe.called.load(Ordering::Acquire));
            eprintln!("AUDIT45 home callback parent sees child's Prepared Call queued; two real permits; no contraction");
            eprintln!("AUDIT45 home callback parent entering ScopeExit requiring child's R");
            assert_eq!(tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0), crate::STATUS_OK);
            eprintln!("AUDIT45 parent ScopeExit returned");
            crate::foreign::hirsel_rt_v2_callback_leave(ctx);
            crate::foreign::hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
            crate::STATUS_OK
        });
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
    assert_eq!(compute_permits(), (2, 0, 0));
}
