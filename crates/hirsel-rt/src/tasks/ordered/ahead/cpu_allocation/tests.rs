use super::*;
use crate::pool::cpu_scoped::ledger::{BackingFull, Ledger};
use crate::tasks::ordered::{engine, events};

unsafe extern "C" fn unused(_: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
    crate::STATUS_OK
}

fn native(ctx: *mut TaskCtx, live: *mut u64) -> Box<Native> {
    let certificate = super::super::native::Certificate {
        entry: unused,
        stack_bytes: 256,
        host: std::ptr::null(),
        resume: None,
        cpu: true,
        live,
        mask: 1,
        source: std::ptr::null_mut(),
    };
    let mut native = Native::new(ctx, std::ptr::null_mut(), 0, certificate);
    // Exercise the exact claim arm/enter leaves. No allocation is permitted
    // before enter, even though this native record has a CPU certificate.
    unsafe {
        (*ctx).ahead_physical = &raw mut native.physical;
        super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_arm(ctx);
        super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_enter((*ctx).ahead_physical);
    }
    native
}

// These are the entire private test frames. In particular no Rust caller or
// allocator frame is saved on the private stack at the request handoff.
#[unsafe(naked)]
unsafe extern "C" fn requesting(_ctx: *mut u8, _native: *mut u8) -> ! {
    core::arch::naked_asm!(
        "push rbx", "mov rbx, rsi", "mov rdi, rbx",
        "mov esi, [rbx + {index}]", "mov rdx, [rbx + {extent}]", "mov rcx, [rbx + {align}]", "call {allocate}",
        "mov qword ptr [rax + 8], 112",
        "mov rdi, rbx", "mov esi, 16", "call {handoff}", "ud2",
        index = const std::mem::offset_of!(Native, allocation) + std::mem::offset_of!(Request, index),
        extent = const std::mem::offset_of!(Native, allocation) + std::mem::offset_of!(Request, extent),
        align = const std::mem::offset_of!(Native, allocation) + std::mem::offset_of!(Request, align),
        allocate = sym hirsel_rt_v2_cpu_ahead_alloc,
        handoff = sym super::super::native::hirsel_rt_v2_ahead_handoff,
    );
}

unsafe fn enter(native: &mut Native, sp: *mut u8) {
    // SAFETY: one live test-owned mapping and one exclusive idle save slot.
    unsafe { crate::fiber::switch(&raw mut native.physical.idle_sp, sp) };
}

fn grant() -> hirsel_run_schema::CpuGrant {
    hirsel_run_schema::CpuGrant {
        provider: "cpu.scoped@1".into(),
        max_workers: 4,
        max_domains: 1,
        max_physical_bytes: 2 << 30,
        max_batch_bytes: 2 << 20,
    }
}

fn run_request(refuse: bool, extent: u64, align: u64, diagnostic: bool) {
    let grant = grant();
    let ledger = Ledger::new(&grant);
    let topology = ledger.topology(4).unwrap();
    let batch_limit = grant.max_batch_bytes;
    let mut run = crate::host::BodyRun::new(1, false);
    if diagnostic {
        run.config.flags |= crate::context::CONFIG_DIAGNOSTIC;
    }
    let body_ledger = ledger.clone();
    let outcome = run.run(
        |ctx| unsafe {
            (*ctx).set_budgets(Some(100), Some(batch_limit));
            (*ctx).heap().idle_cap = 0;
            (*ctx)
                .heap_ref()
                .meter
                .cpu
                .set(ledger.clone())
                .ok()
                .unwrap();
            engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
            events::enable(ctx);
        },
        move |ctx| unsafe {
            engine::fuel_in(ctx);
            let ledger = &body_ledger;
            let held = refuse.then(|| ledger.reserve_heap(grant.max_batch_bytes).unwrap());
            let mut live = 1;
            let mut native = native(ctx, &raw mut live);
            native.allocation.extent = extent;
            native.allocation.align = align;
            let stack = crate::fiber::Stack::new();
            let sp = stack.prepare(requesting, ctx.cast(), (&raw mut *native).cast());
            let before = (*ctx).heap_ref().backing_bytes();
            let initial = engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled);
            (*ctx).view.compiled.fuel -= 3;
            enter(&mut native, sp);
            assert_eq!(native.physical.reason, REASON);
            // The fresh frame starts at sp + 64. Startup/requesting use 16
            // bytes, leaving exactly 80 for the allocation call and save.
            assert_eq!(native.physical.saved_sp, sp.wrapping_sub(32));
            assert_eq!(native.allocation.phase, REQUESTED);
            assert!(native.allocation.result.is_null());
            assert_eq!((*ctx).heap_ref().backing_bytes(), before);
            let bytes = crate::alloc::allocation_charge(ctx, 0, extent, align);
            // Simulate native::run's actual idle/role boundary. This controls
            // TLS only, so the saved private stack cannot execute Rust here.
            crate::stop::set_running(std::ptr::null_mut());
            crate::tasks::role::enter(ctx);
            assert_eq!(serve(ctx, &raw mut *native), Ok(!refuse));
            let admitted = engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled);
            assert_eq!(admitted.level, initial.level + bytes);
            assert_eq!(admitted.fuel, initial.fuel - 3);
            if refuse {
                assert_eq!(native.allocation.phase, CHARGED);
                assert!(native.allocation.result.is_null());
                assert_eq!((*ctx).heap_ref().backing_bytes(), before);
                for _ in 0..3 {
                    assert_eq!(serve(ctx, &raw mut *native), Ok(false));
                    assert_eq!(
                        engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                        admitted
                    );
                    assert!((*ctx).run_ref().control.stopped().is_none());
                }
                drop(held);
                assert_eq!(serve(ctx, &raw mut *native), Ok(true));
                assert_eq!(
                    engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                    admitted
                );
            }
            assert!(!native.allocation.pending());
            let block = native.allocation.result;
            assert!(!block.is_null());
            assert_eq!((*block).index(), 0);
            assert_eq!((*block).count, 1);
            assert_eq!((crate::alloc::payload(block) as usize) % align as usize, 0);
            assert_eq!(
                ledger.reserve_heap(grant.max_batch_bytes).err(),
                Some(BackingFull)
            );
            crate::tasks::role::release(ctx);
            crate::stop::set_running(ctx);
            // The same saved allocation call returns its one unpublished
            // block. The requesting frame then hands off at a distinct PC.
            let saved = native.physical.saved_sp;
            enter(&mut native, saved);
            assert_eq!(native.physical.reason, 16);
            assert_eq!(native.physical.saved_sp, saved.wrapping_add(16));
            assert!(native.allocation.result.is_null());
            assert_eq!(crate::alloc::payload(block).cast::<u64>().read(), 112);
            (*ctx).heap().free(block);
            (*ctx).heap().trim();
            assert_eq!((*ctx).heap_ref().backing_bytes(), before);
            assert!(ledger.reserve_heap(grant.max_batch_bytes).is_ok());
            (*ctx).ahead_physical = std::ptr::null_mut();
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
    drop(topology);
}

#[test]
fn naked_cpu_allocation_hands_off_before_heap_borrow_and_returns_one_block() {
    for diagnostic in [false, true] {
        for (extent, align) in [(16, 8), (4097, 8), (31, 64)] {
            run_request(false, extent, align, diagnostic);
        }
    }
}

#[test]
fn refused_cpu_allocation_keeps_saved_request_and_settles_once_on_retry() {
    for diagnostic in [false, true] {
        for (extent, align) in [(16, 8), (4097, 8), (31, 64)] {
            run_request(true, extent, align, diagnostic);
        }
    }
}

#[test]
fn logical_allocation_stop_precedes_backing_admission() {
    let grant = grant();
    let ledger = Ledger::new(&grant);
    let topology = ledger.topology(4).unwrap();
    let body_ledger = ledger.clone();
    let outcome = crate::host::BodyRun::new(1, false).run(
        |ctx| unsafe {
            (*ctx).set_budgets(None, Some(1));
            (*ctx)
                .heap_ref()
                .meter
                .cpu
                .set(ledger.clone())
                .ok()
                .unwrap();
            engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
        },
        move |ctx| unsafe {
            let ledger = &body_ledger;
            let mut live = 1;
            let mut native = native(ctx, &raw mut live);
            native.allocation.phase = REQUESTED;
            native.allocation.extent = 4097;
            native.allocation.align = 8;
            crate::stop::set_running(std::ptr::null_mut());
            crate::tasks::role::enter(ctx);
            assert_eq!(
                serve(ctx, &raw mut *native),
                Err(crate::stop::Stop::BudgetExceeded(
                    hirsel_run_schema::BudgetKind::Memory
                ))
            );
            assert_eq!(native.allocation.phase, REQUESTED);
            assert!(native.allocation.result.is_null());
            assert_eq!((*ctx).heap_ref().backing_bytes(), 0);
            assert!(ledger.reserve_heap(grant.max_batch_bytes).is_ok());
            crate::tasks::role::release(ctx);
            crate::stop::set_running(ctx);
            (*ctx).ahead_physical = std::ptr::null_mut();
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    drop(topology);
}
