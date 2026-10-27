use super::growth::serve;
use super::*;
use crate::alloc;
use crate::pool::cpu_scoped::ledger::Ledger;
use crate::tasks::ordered::{cpu, engine, events};

unsafe extern "C" fn unused(_: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
    crate::STATUS_OK
}

#[unsafe(naked)]
unsafe extern "C" fn growing(_request: *mut u8, _native: *mut u8) -> ! {
    core::arch::naked_asm!(
        "push rbx", "mov rbx, rsi", "mov rsi, rdi", "mov rdi, rbx",
        "call {grow}", "cmp eax, [rbx + {status}]", "jne 2f",
        "mov rdi, rbx", "mov esi, 64", "call {handoff}", "2:", "ud2",
        grow = sym hirsel_rt_v2_cpu_ahead_grow,
        status = const std::mem::offset_of!(Native, growth_status),
        handoff = sym super::super::native::hirsel_rt_v2_ahead_handoff,
    );
}

fn run_growth(sequence: bool, refuse: bool, repeat: bool) {
    let grant = hirsel_run_schema::CpuGrant {
        provider: "cpu.scoped@1".into(),
        max_workers: 4,
        max_domains: 1,
        max_physical_bytes: 2 << 30,
        max_batch_bytes: 2 << 20,
    };
    let ledger = Ledger::new(&grant);
    let _topology = ledger.topology(4).unwrap();
    let configured = ledger.clone();
    let setup_grant = grant.clone();
    let outcome = crate::host::BodyRun::new(1, false).run(
        move |ctx| unsafe {
            (*ctx).run().cpu_grant = Some(setup_grant.clone());
            (*ctx).set_budgets(Some(100), Some(setup_grant.max_batch_bytes));
            (*ctx).heap().idle_cap = 0;
            (*ctx).heap_ref().meter.cpu.set(configured).ok().unwrap();
            engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
            events::enable(ctx);
        },
        move |ctx| unsafe {
            engine::fuel_in(ctx);
            assert_eq!(cpu::control(ctx, 4, 2, 1, || 0), Ok(0));
            assert_eq!(cpu::control(ctx, 4, 3, 0, || 0), Ok(0));
            let old = alloc::hirsel_rt_v2_alloc(ctx, 0, 32, 8, alloc::DOMAIN_HEAP, 0);
            alloc::payload(old).cast::<[u64; 4]>().write([1, 2, 112, 0]);
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
            let mut request = Growth {
                version: VERSION,
                kind: if sequence { GROW_SEQUENCE } else { GROW_BLOCK },
                ordinal: 0,
                block: old,
                region: std::ptr::null_mut(),
                index: 0,
                flags: 0,
                extent: 65536,
                align: 8,
                keep: 16,
                element_size: 8,
                element_align: 8,
                capacity: 8192,
            };
            let stack = crate::fiber::Stack::new();
            let sp = stack.prepare(
                growing,
                (&raw mut request).cast(),
                (&raw mut *native).cast(),
            );
            let held = refuse.then(|| {
                ledger
                    .reserve_heap(grant.max_batch_bytes - (*ctx).heap_ref().backing_bytes())
                    .unwrap()
            });
            crate::fiber::switch(&raw mut native.physical.idle_sp, sp);
            assert_eq!(native.physical.reason, 32);
            assert_eq!(native.physical.saved_sp, sp.wrapping_sub(32));
            crate::stop::set_running(std::ptr::null_mut());
            crate::tasks::role::enter(ctx);
            assert_eq!(serve(ctx, &raw mut *native), Ok(!refuse));
            if refuse {
                let state = engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled);
                for _ in 0..3 {
                    assert_eq!(request.block, old, "refusal retains the input owner");
                    assert_eq!(
                        alloc::payload(old).cast::<[u64; 4]>().read(),
                        [1, 2, 112, 0]
                    );
                    assert_eq!((*ctx).heap_ref().live().0, 1);
                    assert_eq!(serve(ctx, &raw mut *native), Ok(false));
                    assert_eq!(
                        engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                        state
                    );
                }
                drop(held);
                assert_eq!(serve(ctx, &raw mut *native), Ok(true));
            }
            let fresh = request.block;
            assert_ne!(fresh, old);
            assert_eq!(
                (*ctx).heap_ref().live().0,
                1,
                "growth must reclaim the old block"
            );
            assert_eq!(alloc::payload(fresh).cast::<u64>().read(), 1);
            if sequence {
                assert_eq!(alloc::payload(fresh).add(16).cast::<u64>().read(), 112);
                let exact = alloc::sequence_slots((*ctx).heap_ref(), fresh, 16, 8);
                assert!(exact >= request.capacity);
                assert_eq!(alloc::payload(fresh).add(8).cast::<u64>().read(), exact);
            } else {
                assert_eq!(alloc::payload(fresh).add(8).cast::<u64>().read(), 2);
            }
            crate::tasks::role::release(ctx);
            crate::stop::set_running(ctx);
            let saved = native.physical.saved_sp;
            crate::fiber::switch(&raw mut native.physical.idle_sp, saved);
            assert_eq!(native.physical.reason, 64);
            assert!(native.growth.is_null());
            if repeat {
                let sp = stack.prepare(
                    growing,
                    (&raw mut request).cast(),
                    (&raw mut *native).cast(),
                );
                crate::fiber::switch(&raw mut native.physical.idle_sp, sp);
                crate::stop::set_running(std::ptr::null_mut());
                crate::tasks::role::enter(ctx);
                assert_eq!(serve(ctx, &raw mut *native), Ok(true));
                assert_eq!(request.block, fresh);
                assert_eq!(
                    (*ctx).heap_ref().live().0,
                    1,
                    "a resumed no-op must not free the result twice"
                );
                crate::tasks::role::release(ctx);
                crate::stop::set_running(ctx);
                let saved = native.physical.saved_sp;
                crate::fiber::switch(&raw mut native.physical.idle_sp, saved);
                assert_eq!(native.physical.reason, 64);
            }
            (*ctx).heap().free(fresh);
            (*ctx).heap().trim();
            assert_eq!((*ctx).heap_ref().live().0, 0);
            assert_eq!((*ctx).heap_ref().backing_bytes(), 0);
            (*ctx).ahead_physical = std::ptr::null_mut();
            engine::at_head(ctx).unwrap();
            assert_eq!(
                engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled.level),
                0
            );
            (*ctx).ahead_physical = std::ptr::null_mut();
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
}

#[test]
fn saved_growth_reclaims_real_old_blocks_without_leaking() {
    for sequence in [false, true] {
        run_growth(sequence, false, false);
    }
}

#[test]
fn repeated_growth_calls_do_not_free_the_returned_block_twice() {
    for sequence in [false, true] {
        run_growth(sequence, false, true);
    }
}

#[test]
fn refused_growth_keeps_the_same_input_and_charge_until_backing_is_available() {
    for sequence in [false, true] {
        run_growth(sequence, true, true);
    }
}
