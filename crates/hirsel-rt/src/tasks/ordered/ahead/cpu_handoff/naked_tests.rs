use super::*;
use crate::tasks::ordered::{cpu, engine, events};

unsafe extern "C" fn unused(_: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
    crate::STATUS_OK
}

#[unsafe(naked)]
unsafe extern "C" fn publishing(_summary: *mut u8, _native: *mut u8) -> ! {
    core::arch::naked_asm!(
        "push rbx", "mov rbx, rsi", "mov rsi, rdi", "mov rdi, rbx",
        "call {publish}", "test eax, eax", "jne 2f",
        "mov rdi, rbx", "mov esi, 32", "call {handoff}", "2:", "ud2",
        publish = sym hirsel_rt_v2_cpu_ahead_publish,
        handoff = sym super::super::native::hirsel_rt_v2_ahead_handoff,
    );
}

#[test]
fn naked_cpu_summary_returns_to_the_same_call_after_one_head_settlement() {
    let outcome = crate::host::BodyRun::new(1, false).run(
        |ctx| unsafe {
            (*ctx).run().cpu_grant = Some(hirsel_run_schema::CpuGrant {
                provider: "cpu.scoped@1".into(),
                max_workers: 4,
                max_domains: 1,
                max_physical_bytes: 2 << 30,
                max_batch_bytes: 1 << 20,
            });
            (*ctx).set_budgets(Some(100), None);
            engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
            events::enable(ctx);
        },
        |ctx| unsafe {
            engine::fuel_in(ctx);
            assert_eq!(cpu::control(ctx, 4, 2, 1, || 0), Ok(0));
            assert_eq!(cpu::control(ctx, 4, 3, 0, || 0), Ok(0));
            let mut live = 1;
            let certificate = super::super::native::Certificate {
                entry: unused,
                stack_bytes: 256,
                host: std::ptr::null(),
                resume: None,
                cpu: true,
                preclaimed: false,
                live: &raw mut live,
                mask: 1,
                source: std::ptr::null_mut(),
            };
            let mut native = Native::new(ctx, std::ptr::null_mut(), 0, certificate);
            (*ctx).ahead_physical = &raw mut native.physical;
            super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_arm(ctx);
            super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_enter((*ctx).ahead_physical);
            let value = 42u64;
            let mut summary = Summary {
                version: VERSION,
                phase: SUMMARY_PENDING,
                ordinal: 0,
                value: (&raw const value).cast(),
                value_bytes: 8,
                descriptor: 0,
                retained_count: 0,
                retained: std::ptr::null(),
            };
            let stack = crate::fiber::Stack::new();
            let sp = stack.prepare(
                publishing,
                (&raw mut summary).cast(),
                (&raw mut *native).cast(),
            );
            let initial = engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled);
            (*ctx).view.compiled.fuel -= 3;
            crate::fiber::switch(&raw mut native.physical.idle_sp, sp);
            assert_eq!(native.physical.reason, 16);
            assert_eq!(native.physical.saved_sp, sp.wrapping_sub(32));
            assert_eq!(summary.phase, SUMMARY_PENDING);
            assert_eq!(
                engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                initial
            );
            crate::stop::set_running(std::ptr::null_mut());
            crate::tasks::role::enter(ctx);
            assert_eq!(serve(ctx, &raw mut *native), Ok(true));
            assert_eq!(summary.phase, SUMMARY_RETIRED);
            assert!(native.summary.is_null());
            assert_eq!(
                engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled.fuel),
                initial.fuel - 3
            );
            crate::tasks::role::release(ctx);
            crate::stop::set_running(ctx);
            let saved = native.physical.saved_sp;
            crate::fiber::switch(&raw mut native.physical.idle_sp, saved);
            assert_eq!(native.physical.reason, 32);
            assert_eq!(summary.value.cast::<u64>().read(), 42);
            (*ctx).ahead_physical = std::ptr::null_mut();
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
}
