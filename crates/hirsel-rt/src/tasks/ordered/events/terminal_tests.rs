use super::*;
use crate::tasks::*;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Delay {
    Save,
    Receive,
    Cut,
}

struct Handshake {
    saved: Mutex<mpsc::Receiver<()>>,
    delay: Delay,
}

fn prefix(s: &Scheduler, parked: bool) -> bool {
    let Logical::Ordered(driver) = &s.logical else {
        unreachable!()
    };
    let row = s.tasks[0].as_ref().unwrap();
    driver
        .events
        .as_ref()
        .unwrap()
        .awaiting
        .contains_key(&Vec::new())
        && if parked {
            matches!(row.wait, Some(Wait::Any { .. }))
        } else {
            row.wait.is_none() && row.state == State::Ready
        }
}

fn delay(at: Delay, selected: Delay) {
    if at == selected {
        std::thread::sleep(Duration::from_millis(10));
    }
}

unsafe extern "C" fn fail_budget_after_parent_await(
    ctx: *mut TaskCtx,
    environment: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    unsafe { fail_budget(ctx, environment, true) }
}

unsafe extern "C" fn fail_budget_after_parent_yield(
    ctx: *mut TaskCtx,
    environment: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    unsafe { fail_budget(ctx, environment, false) }
}

unsafe fn fail_budget(ctx: *mut TaskCtx, environment: *mut crate::Header, parked: bool) -> u8 {
    unsafe {
        // The root retains this per-run state until every stopped worker leaves.
        let handshake = &*crate::alloc::payload(environment)
            .cast::<*const Handshake>()
            .read();
        // Exact serial handoffs: spawn -> add -> Awaiting Yield -> optional
        // Any Park. These are protocol turns, not retries of a transient state.
        for _ in 0..if parked { 3 } else { 2 } {
            let mut cause = 0;
            assert_eq!(
                hirsel_rt_v2_task_canceled(ctx, &mut cause, &0),
                crate::STATUS_OK
            );
            assert_eq!(cause, 0);
        }
        delay(Delay::Receive, handshake.delay);
        // Like Tokio loom_notify.rs:12-48, publication may precede the receive;
        // the channel retains the event. No scheduler operation advances the
        // parent's continuation between this handshake and the memory cut.
        handshake
            .saved
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .expect("parent did not save its await_any prefix");
        {
            let _role = role::Guard::new(ctx);
            let s = scheduler(ctx);
            assert!(prefix(s, parked));
            assert!(readiness::saved(s, 0));
            if parked {
                let Logical::Ordered(driver) = &s.logical else {
                    unreachable!()
                };
                let log = driver.events.as_ref().unwrap();
                assert_eq!(terminal_waits::awaits(s, log).unwrap().len(), 1);
                let original_wait = s.task(0).wait;
                s.task(0).wait = Some(Wait::Final);
                let Logical::Ordered(driver) = &s.logical else {
                    unreachable!()
                };
                assert!(terminal_waits::awaits(s, driver.events.as_ref().unwrap()).is_err());
                s.task(0).wait = original_wait;
                // A terminal cause cannot rescue a mismatched identity or an
                // unpublished prefix. Check the actual saved parent record.
                let Logical::Ordered(driver) = &mut s.logical else {
                    unreachable!()
                };
                let log = driver.events.as_mut().unwrap();
                log.awaiting.get_mut(&Vec::new()).unwrap().set.ordinal += 1;
                let Logical::Ordered(driver) = &s.logical else {
                    unreachable!()
                };
                assert!(terminal_waits::awaits(s, driver.events.as_ref().unwrap()).is_err());
                let Logical::Ordered(driver) = &mut s.logical else {
                    unreachable!()
                };
                let log = driver.events.as_mut().unwrap();
                log.awaiting.get_mut(&Vec::new()).unwrap().set.ordinal -= 1;
                let prefix = log
                    .canonical
                    .iter()
                    .position(|event| matches!(event.outcome, CanonicalOutcome::Awaiting { .. }))
                    .unwrap();
                let original =
                    std::mem::replace(&mut log.canonical[prefix].outcome, CanonicalOutcome::Unit);
                let Logical::Ordered(driver) = &s.logical else {
                    unreachable!()
                };
                assert!(terminal_waits::awaits(s, driver.events.as_ref().unwrap()).is_err());
                let Logical::Ordered(driver) = &mut s.logical else {
                    unreachable!()
                };
                driver.events.as_mut().unwrap().canonical[prefix].outcome = original;
            }
        }
        delay(Delay::Cut, handshake.delay);
        // This original allocator entry selects the global Memory cut while
        // the parent's already-published Awaiting prefix still owns its set.
        crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16384, 8, crate::alloc::DOMAIN_HEAP, 0);
        unreachable!("the global memory budget must stop this allocation")
    }
}

#[test]
fn global_budget_cut_closes_a_saved_parent_await_without_fabricating_a_result() {
    check_budget_cut(fail_budget_after_parent_await, true);
}

#[test]
fn global_budget_cut_closes_a_yielded_await_before_its_native_park() {
    check_budget_cut(fail_budget_after_parent_yield, false);
}

fn check_budget_cut(entry: TaskCode, parked: bool) {
    for (width, delay_at) in [1, 4]
        .into_iter()
        .flat_map(|width| [Delay::Save, Delay::Receive, Delay::Cut].map(|delay| (width, delay)))
    {
        let (notify, saved) = mpsc::channel();
        let handshake = Arc::new(Handshake {
            saved: Mutex::new(saved),
            delay: delay_at,
        });
        let child_handshake = handshake.clone();
        let environment = crate::Layout {
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
        let layouts = [&environment as *const _];
        let mut run = crate::host::BodyRun::new(width, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        // Go exec_test.go:470-521: a bounded request/response handshake and
        // exact completion assertions. The watchdog covers scheduler handoffs
        // too, including failure to reach the child's receive.
        let control = run.control();
        let (done, finished) = mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            if finished.recv_timeout(Duration::from_secs(5)).is_err() {
                control.stop();
                return false;
            }
            true
        });
        let (outcome, trace) = run.run_traced_after_bounded_stop(
            |ctx| unsafe {
                (*ctx).set_budgets(None, Some(4096));
                super::super::engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), Some(4096)),
                );
                enable(ctx);
            },
            move |ctx| unsafe {
                let run_address = (*ctx).run_ptr() as usize;
                crate::pool::free::test_before_save(ctx, move |task, completed| {
                    if task != 0 || completed {
                        return;
                    }
                    let run = run_address as *mut crate::context::RunShared;
                    let target = {
                        let _role = role::IdleGuard::new(run);
                        prefix((*(*run).tasks.get()).as_deref().unwrap(), parked)
                    };
                    if target {
                        delay(Delay::Save, delay_at);
                    }
                });
                let sent = std::sync::atomic::AtomicBool::new(false);
                let notify = notify.clone();
                super::super::engine::with_driver(ctx, |driver| {
                    driver.after_save = Some(Box::new(move |s, task, completed| {
                        if task == 0 && !completed && prefix(s, parked) && readiness::saved(s, 0) {
                            assert!(!sent.swap(true, std::sync::atomic::Ordering::SeqCst));
                            notify.send(()).unwrap();
                        }
                    }));
                });
                let mut scope = 0;
                let mut set = 0;
                let mut child = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    crate::STATUS_OK
                );
                assert_eq!(
                    hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope),
                    crate::STATUS_OK
                );
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(env)
                    .cast::<*const Handshake>()
                    .write(Arc::as_ptr(&child_handshake));
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        entry,
                        env,
                        0
                    ),
                    crate::STATUS_OK
                );
                assert_eq!(
                    hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut child),
                    crate::STATUS_OK
                );
                let mut answer = IndexOption {
                    tag: 99,
                    padding: 99,
                    index: 99,
                };
                hirsel_rt_v2_task_await_any(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut answer,
                    &scope,
                    &set,
                )
            },
        );
        let _ = done.send(());
        assert!(
            watchdog.join().unwrap(),
            "terminal cut watchdog: {width}/{delay_at:?}"
        );
        assert_eq!(
            outcome.ended,
            Err(crate::stop::Stop::BudgetExceeded(
                hirsel_run_schema::BudgetKind::Memory
            )),
            "{width}/{delay_at:?}"
        );
        assert_eq!(outcome.pending, 0);
        assert!(
            matches!(
                handshake.saved.lock().unwrap().try_recv(),
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected)
            ),
            "the child consumed exactly one saved-prefix notification"
        );
        let trace =
            trace.expect("saved known wait prefix and selected budget cut must export completely");
        let rows: Vec<serde_json::Value> = std::str::from_utf8(trace.bytes())
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(rows.last().unwrap()["outcome"]["kind"], "stopped");
        assert!(rows.iter().any(|r| r["outcome"]["kind"] == "awaiting"));
        let abandoned: Vec<_> = rows
            .iter()
            .filter(|r| r["outcome"]["kind"] == "await_abandoned")
            .collect();
        assert_eq!(abandoned.len(), 1);
        let prefix = rows
            .iter()
            .find(|r| r["outcome"]["kind"] == "awaiting")
            .unwrap();
        assert_eq!(abandoned[0]["item"], prefix["item"]);
        assert_eq!(abandoned[0]["outcome"]["task"], prefix["task"]);
        assert_eq!(abandoned[0]["outcome"]["ordinal"], prefix["ordinal"]);
        assert_eq!(abandoned[0]["ordinal"], rows.last().unwrap()["ordinal"]);
        assert!(
            !rows.iter().any(|r| r["outcome"]["kind"] == "selected"),
            "no result was observed at the abandoned await"
        );
    }
}
