use super::super::*;
use std::sync::{Barrier, mpsc};

fn context() -> *mut TaskCtx {
    let mut ctx = std::ptr::null_mut();
    unsafe {
        assert_eq!(
            crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
            0
        );
        init_execs(ctx);
    }
    ctx
}

#[test]
fn cpu_receipt_stop_before_admission_never_calls_the_core() {
    let ctx = context();
    let address = ctx.addr();
    unsafe { (*ctx).run_ref().control.request(Stop::Requested) };
    std::thread::spawn(move || unsafe {
        let ctx = address as *mut TaskCtx;
        let exec = Exec::new(Kind::Compute(0), std::ptr::null_mut(), false);
        EXEC.with(|e| e.set(Arc::as_ptr(&exec)));
        assert_eq!(
            call_local_in((*ctx).run_ptr(), &mut || panic!("stopped receipt ran")),
            Err(Stop::Requested)
        );
        EXEC.with(|e| e.set(std::ptr::null()));
    })
    .join()
    .unwrap();
    unsafe { crate::context::hirsel_rt_v2_ctx_destroy(ctx) };
}

#[test]
fn cpu_receipt_delayed_before_claim_is_withdrawn_without_invoking_the_core() {
    let ctx = context();
    let address = ctx.addr();
    let (prepared, pending) = mpsc::channel();
    let release = Arc::new(Barrier::new(2));
    let worker_release = release.clone();
    let worker = std::thread::spawn(move || unsafe {
        let ctx = address as *mut TaskCtx;
        let exec = Exec::new(Kind::Compute(0), std::ptr::null_mut(), false);
        exec.held.store(true, Ordering::Relaxed);
        EXEC.with(|e| e.set(Arc::as_ptr(&exec)));
        let result = call_in(
            (*ctx).run_ptr(),
            &mut || panic!("withdrawn receipt ran"),
            |_, _, call| {
                prepared.send(()).unwrap();
                worker_release.wait();
                run_call(call);
                true
            },
        );
        assert_eq!(result, Err(Stop::Requested));
        assert!(
            exec.held.load(Ordering::Relaxed),
            "the synchronous loan retains its permit"
        );
        EXEC.with(|e| e.set(std::ptr::null()));
    });
    pending
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    unsafe {
        let run = (*ctx).run_ref();
        run.control.request(Stop::Requested);
        let sweep = run.execs.as_deref().unwrap().calls.sweep();
        assert_eq!(sweep.withdrawn.len(), 1);
        assert!(sweep.in_doubt.is_empty());
    }
    release.wait();
    worker.join().unwrap();
    unsafe { crate::context::hirsel_rt_v2_ctx_destroy(ctx) };
}

#[test]
fn cpu_receipt_stop_during_claim_keeps_the_borrow_and_wins_over_a_later_error() {
    for held in [false, true] {
        let ctx = context();
        let address = ctx.addr();
        let (entered, pending) = mpsc::channel();
        let release = Arc::new(Barrier::new(2));
        let worker_release = release.clone();
        let worker = std::thread::spawn(move || unsafe {
            let ctx = address as *mut TaskCtx;
            let exec = Exec::new(Kind::Compute(0), std::ptr::null_mut(), false);
            exec.held.store(held, Ordering::Relaxed);
            EXEC.with(|e| e.set(Arc::as_ptr(&exec)));
            let mut receipt = Ok(());
            let result = call_local_in((*ctx).run_ptr(), &mut || {
                entered.send(()).unwrap();
                worker_release.wait();
                receipt = Err(crate::io::AdapterError::InvalidCompletion);
            })
            .and_then(|()| receipt.map_err(Stop::refusal));
            if held {
                assert_eq!(result, Err(Stop::Requested));
            } else {
                assert_eq!(
                    result,
                    Err(Stop::refusal(crate::io::AdapterError::InvalidCompletion))
                );
            }
            assert_eq!(exec.held.load(Ordering::Relaxed), held);
            EXEC.with(|e| e.set(std::ptr::null()));
        });
        pending
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        unsafe {
            let run = (*ctx).run_ref();
            run.control.request(Stop::Requested);
            let sweep = run.execs.as_deref().unwrap().calls.sweep();
            assert!(sweep.withdrawn.is_empty());
            assert_eq!(sweep.in_doubt.len(), 1);
        }
        release.wait();
        worker.join().unwrap();
        unsafe { crate::context::hirsel_rt_v2_ctx_destroy(ctx) };
    }
}
