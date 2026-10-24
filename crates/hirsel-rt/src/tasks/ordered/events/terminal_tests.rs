use super::*;
use crate::tasks::*;

unsafe extern "C" fn fail_budget_after_parent_await(
    ctx: *mut TaskCtx,
    _: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    unsafe { fail_budget(ctx, true) }
}

unsafe extern "C" fn fail_budget_after_parent_yield(
    ctx: *mut TaskCtx,
    _: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    unsafe { fail_budget(ctx, false) }
}

unsafe fn fail_budget(ctx: *mut TaskCtx, parked: bool) -> u8 {
    unsafe {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let waiting = {
                let _role = role::Guard::new(ctx);
                let s = scheduler(ctx);
                let Logical::Ordered(driver) = &s.logical else {
                    unreachable!()
                };
                let entered = driver
                    .events
                    .as_ref()
                    .unwrap()
                    .awaiting
                    .contains_key(&Vec::new());
                entered
                    && readiness::saved(s, 0)
                    && if parked {
                        matches!(s.task(0).wait, Some(Wait::Any { .. }))
                    } else {
                        s.task(0).wait.is_none() && s.task(0).state == State::Ready
                    }
            };
            if waiting {
                if !parked {
                    break;
                }
                let _role = role::Guard::new(ctx);
                let s = scheduler(ctx);
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
                break;
            }
            assert!(
                std::time::Instant::now() < until,
                "parent did not save its await_any prefix"
            );
            let mut cause = 0;
            assert_eq!(
                hirsel_rt_v2_task_canceled(ctx, &mut cause, &0),
                crate::STATUS_OK
            );
        }
        // This original allocator entry selects the global Memory cut while
        // the parent's already-published Awaiting prefix still owns its set.
        crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16384, 8, crate::alloc::DOMAIN_HEAP, 0);
        unreachable!("the global memory budget must stop this allocation")
    }
}

#[test]
fn global_budget_cut_closes_a_saved_parent_await_without_fabricating_a_result() {
    check_budget_cut(fail_budget_after_parent_await);
}

#[test]
fn global_budget_cut_closes_a_yielded_await_before_its_native_park() {
    check_budget_cut(fail_budget_after_parent_yield);
}

fn check_budget_cut(entry: TaskCode) {
    for width in [1, 4] {
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
        assert_eq!(
            outcome.ended,
            Err(crate::stop::Stop::BudgetExceeded(
                hirsel_run_schema::BudgetKind::Memory
            ))
        );
        assert_eq!(outcome.pending, 0);
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
