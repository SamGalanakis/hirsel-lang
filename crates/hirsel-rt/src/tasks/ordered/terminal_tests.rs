//! Terminal publication uses the same saved owner/head boundary as H ops.

use super::super::*;
use super::engine;
use crate::ordered::Policy;
use crate::stop::Stop;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Default)]
struct Probe {
    attempted: AtomicBool,
    terminal: AtomicU64,
    older_effect: AtomicBool,
    mode: AtomicU64,
}

unsafe extern "C" fn older(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let [probe, atomic] = unsafe { crate::alloc::payload(env).cast::<[u64; 2]>().read() };
    let probe = unsafe { &*(probe as *const Probe) };
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !probe.attempted.load(Ordering::Acquire) {
        assert!(
            std::time::Instant::now() < until,
            "terminal task never ran ahead"
        );
        std::thread::yield_now();
    }
    loop {
        let pending = {
            let _role = unsafe { role::Guard::new(ctx) };
            engine::driver(ctx).head_paused(probe.terminal.load(Ordering::Acquire))
        };
        if pending {
            break;
        }
        assert!(
            std::time::Instant::now() < until,
            "terminal task never saved at its head gate"
        );
        std::thread::yield_now();
    }
    match probe.mode.load(Ordering::Acquire) {
        2 => unsafe {
            // The live environment already fills the budget. This older
            // allocation precedes the younger UB in the serial charge stream.
            crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
            unreachable!("older allocation must exhaust memory");
        },
        3 => {
            // A host request is allowed to interrupt a pending terminal gate.
            unsafe { (*ctx).run_ref().control.request(Stop::Requested) };
        }
        _ => {}
    }
    // The younger task has already reached its fatal condition, but the
    // serial reference still performs this older scheduling-point effect.
    let value = 23u64;
    assert_eq!(
        unsafe { hirsel_rt_v2_atomic_store(ctx, &atomic, (&value as *const u64).cast(), 8) },
        STATUS_OK
    );
    probe.older_effect.store(true, Ordering::Release);
    STATUS_OK
}

fn terminal_run(mode: u64) {
    let layout = crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 16,
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
    for width in [2, 4, 8] {
        let probe = Arc::new(Probe::default());
        probe.mode.store(mode, Ordering::Release);
        let seen = probe.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                static FAILURES: [u8; 40] = {
                    let mut bytes = [0; 40];
                    bytes[0] = 1; // One site with code zero and empty names.
                    bytes[16] = 7;
                    bytes
                };
                (*ctx).run().failure_table = crate::isolate::FailureTable::new(&FAILURES);
                if mode == 2 {
                    let charge = crate::alloc::allocation_charge(ctx, 2, 16, 8);
                    (*ctx).set_budgets(None, Some(charge));
                }
                engine::enable(ctx, Policy::admit(&Default::default(), None));
            },
            move |ctx| unsafe {
                let mut atomic = 0;
                assert_eq!(
                    hirsel_rt_v2_atomic_create(
                        ctx,
                        std::ptr::null_mut(),
                        &mut atomic,
                        (&0u64 as *const u64).cast(),
                        8
                    ),
                    STATUS_OK
                );
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    STATUS_OK
                );
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(env)
                    .cast::<[u64; 2]>()
                    .write([Arc::as_ptr(&seen) as u64, atomic]);
                let mut child = 0;
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        older,
                        env,
                        0
                    ),
                    STATUS_OK
                );
                {
                    let _role = role::Guard::new(ctx);
                    seen.terminal.store(engine::current(ctx), Ordering::Release);
                }
                seen.attempted.store(true, Ordering::Release);
                match mode {
                    0 => crate::stop::raise(ctx, Stop::Refused("terminal fixture".into())),
                    4 => STATUS_OK,
                    5 => crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 0, 0, 0),
                    _ => crate::hirsel_rt_v2_unchecked_add_overflow(ctx, 0),
                }
            },
        );
        match mode {
            0 => assert_eq!(outcome.ended, Err(Stop::Refused("terminal fixture".into()))),
            1 => {
                let Err(Stop::UndefinedBehavior(diagnostic)) = &outcome.ended else {
                    panic!("{outcome:?}");
                };
                assert_eq!(diagnostic.site.line, 7);
                assert!(diagnostic.message.contains("int.unchecked_add at line 7"));
            }
            2 => assert_eq!(
                outcome.ended,
                Err(Stop::BudgetExceeded(hirsel_run_schema::BudgetKind::Memory))
            ),
            3 => assert_eq!(outcome.ended, Err(Stop::Requested)),
            4 => assert_eq!(outcome.ended, Ok(STATUS_OK)),
            5 => assert_eq!(outcome.ended, Ok(STATUS_FAILURE)),
            _ => unreachable!(),
        }
        assert_eq!(
            probe.older_effect.load(Ordering::Acquire),
            !matches!(mode, 2 | 3),
            "wrong retired prefix at width {width}"
        );
    }
}

#[test]
fn younger_terminal_condition_waits_for_the_older_retired_prefix() {
    terminal_run(0);
}

#[test]
fn younger_unchecked_add_ub_waits_for_the_older_retired_prefix() {
    terminal_run(1);
}

#[test]
fn older_budget_stop_precedes_a_younger_pending_ub() {
    terminal_run(2);
}

#[test]
fn external_stop_interrupts_a_younger_pending_terminal_gate() {
    terminal_run(3);
}

#[test]
fn root_success_waits_for_its_final_turn_to_retire_before_publication() {
    terminal_run(4);
}

#[test]
fn root_failure_waits_for_its_final_turn_to_retire_before_publication() {
    terminal_run(5);
}
