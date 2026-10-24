use super::super::*;
use crate::tasks::role::{self, lent, with_role};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// A role table borrow is lexical: the boundary checks fault inside one and
/// pass outside, and an unwinding closure leaves no borrow behind.
fn assert_boundary_faults(boundary: impl FnOnce()) {
    assert!(!lent());
    assert!(catch_unwind(AssertUnwindSafe(boundary)).is_err());
    assert!(!lent(), "an unwound closure left its borrow");
}

#[test]
fn runtime50_role_borrow_home_reentry() {
    let mut ctx = std::ptr::null_mut();
    unsafe {
        assert_eq!(
            crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
            0
        );
        role::enable(ctx);
        init_execs(ctx);
    }
    let address = ctx.addr();
    let run = unsafe { (*ctx).run_ptr() };
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let reentered = AtomicU32::new(0);
    std::thread::scope(|scope| {
        let reentered = &reentered;
        let worker = scope.spawn(move || unsafe {
            let ctx = address as *mut TaskCtx;
            let run = (*ctx).run_ptr();
            let exec = Exec::new(Kind::Compute(0), std::ptr::null_mut(), false);
            EXEC.with(|e| e.set(Arc::as_ptr(&exec)));
            // R: a bounded table transition. The role stays entered, but the
            // table borrow ends with the closure.
            let root = with_role(ctx, |t| {
                assert!(lent());
                t.task_key(0).unwrap()
            });
            assert!(!lent());
            // Nested borrows and every boundary inside a borrow fault.
            assert_boundary_faults(|| with_role(ctx, |_| with_role(ctx, |_| ())));
            assert_boundary_faults(|| with_role(ctx, |_| drop(role::Segment::new(ctx))));
            assert_boundary_faults(|| {
                with_role(ctx, |_| {
                    let _ = call_home_in(run, &mut || ());
                })
            });
            assert_boundary_faults(|| with_role(ctx, |_| (*ctx).switch_out()));
            assert_boundary_faults(|| with_role(ctx, |_| role::release(ctx)));
            assert_boundary_faults(|| {
                with_role(ctx, |_| {
                    crate::tasks::scheduler(ctx);
                })
            });
            held_tx.send(()).unwrap();
            // Home is waiting to enter R: it must serve this loan meanwhile.
            while !(*run).role.home_waiting() {
                std::thread::yield_now();
            }
            // R → home: a synchronous loan while R is held and no borrow is.
            call_home_in(run, &mut || {
                // On home, inside the loan: no borrow crossed the thread hop.
                assert!(!lent());
                assert!(!std::ptr::eq(own_exec(), Arc::as_ptr(&exec)));
                // home → R: the loan reenters its caller's R without waiting
                // and borrows the tables lexically, then returns unlent.
                let key = with_role(ctx, |t| t.task_key(0).unwrap());
                assert_eq!(key, root);
                assert!(!lent());
                reentered.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
            assert!(!lent());
            assert_eq!(with_role(ctx, |t| t.live_task(root)), Some(0));
            role::release(ctx);
            EXEC.with(|e| e.set(std::ptr::null()));
        });
        held_rx.recv().unwrap();
        // home → R: an idle home merge waits for R, serving the loan first.
        let guard = unsafe { role::IdleGuard::home(run) };
        assert_eq!(reentered.load(Ordering::SeqCst), 1);
        assert!(!lent());
        drop(guard);
        worker.join().unwrap();
    });
    assert_eq!(unsafe { (*run).role.home_waiting() }, false);
    unsafe { crate::context::hirsel_rt_v2_ctx_destroy(ctx) };
}
