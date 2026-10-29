use super::*;
use crate::pool::cpu_scoped::ledger::BackingFull;

unsafe extern "C" fn unused(_: *mut TaskCtx, _: *mut crate::Header, _: u64, _: u64) -> u8 {
    crate::STATUS_OK
}

#[test]
fn revoking_a_younger_two_mib_attempt_restores_input_and_the_heads_reserve() {
    for width in [4, 8, 16] {
        let grant = hirsel_run_schema::CpuGrant {
            provider: "cpu.scoped@1".into(),
            max_workers: width,
            max_domains: 1,
            max_physical_bytes: 2 << 30,
            max_batch_bytes: 3 << 20,
        };
        let ledger = Ledger::new(&grant);
        let topology = ledger.topology(width).unwrap();
        let cpu_grant = grant.clone();
        let result = Arc::new(std::sync::Mutex::new(None));
        let observed = result.clone();
        let body_ledger = ledger.clone();
        let outcome = crate::host::BodyRun::new(1, false).run(
            |ctx| unsafe {
                (*ctx).run().cpu_grant = Some(cpu_grant);
                (*ctx).set_budgets(Some(100), Some(3 << 20));
                (*ctx).heap().idle_cap = 0;
                (*ctx).heap_ref().meter.cpu.set(body_ledger).ok().unwrap();
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
            },
            move |ctx| unsafe {
                engine::fuel_in(ctx);
                crate::tasks::ordered::cpu::control(ctx, width, 2, 1, || 0).unwrap();
                crate::tasks::ordered::cpu::control(ctx, width, 3, 0, || 0).unwrap();
                let mut live = 1;
                let mut native = Native::new(
                    ctx,
                    std::ptr::null_mut(),
                    0,
                    super::super::native::Certificate {
                        entry: unused,
                        stack_bytes: 256,
                        host: std::ptr::null(),
                        resume: None,
                        cpu: true,
                        preclaimed: true,
                        live: &raw mut live,
                        mask: 1,
                        source: std::ptr::null_mut(),
                    },
                );
                let task = engine::current(ctx);
                native.cpu_claim =
                    engine::with_driver(ctx, |d| d.cpu_batch.as_ref().unwrap().identity(task));
                let identity = native.cpu_claim.clone().unwrap();
                (*ctx).ahead_physical = &raw mut native.physical;
                crate::stop::set_running(std::ptr::null_mut());
                crate::tasks::role::enter(ctx);
                let original = engine::with_driver(ctx, |d| {
                    let original = d.frontier.take().unwrap();
                    let mut frontier = crate::ordered::Frontier::new(
                        task.wrapping_add(1 << 32),
                        crate::ordered::Policy::admit(&Default::default(), None),
                        original.settled,
                    );
                    frontier.enqueue(task, crate::ordered::footprint::Held::empty());
                    d.frontier = Some(frontier);
                    original
                });
                let mut entry = [42u64; 8];
                let sp = entry.as_mut_ptr().cast::<u8>();
                let old_sp = (*ctx).task_sp;
                native.allocation.begin(0, 2 << 20, 8);
                prepare_at(
                    ctx,
                    &raw mut *native,
                    sp,
                    sp.add(std::mem::size_of_val(&entry)),
                );
                let prepared = active(ctx);
                let served = super::super::cpu_allocation::serve(ctx, &raw mut *native);
                let block = native.allocation.take_result();
                crate::alloc::payload(block).cast::<u64>().write(entry[0]);
                entry[0] = 99;
                let refused = crate::alloc::cpu_preflight(ctx, 0, 2 << 20, 8);
                crate::alloc::cpu_allocation_done(ctx);
                rollback(ctx, &raw mut *native);
                let restored = entry[0] == 42
                    && !active(ctx)
                    && Arc::ptr_eq(&identity, native.cpu_claim.as_ref().unwrap());
                let head = crate::alloc::cpu_preflight(ctx, 0, 2 << 20, 8);
                crate::alloc::cpu_allocation_done(ctx);
                engine::with_driver(ctx, |d| d.frontier = Some(original));
                let rerun = super::super::cpu_allocation::serve(ctx, &raw mut *native);
                let block = native.allocation.take_result();
                crate::alloc::payload(block).cast::<u64>().write(entry[0]);
                let value = crate::alloc::payload(block).cast::<u64>().read();
                (*ctx).heap().free(block);
                (*ctx).heap().trim();
                (*ctx).task_sp = old_sp;
                (*ctx).ahead_physical = std::ptr::null_mut();
                crate::tasks::role::release(ctx);
                crate::stop::set_running(ctx);
                *observed.lock().unwrap() =
                    Some((prepared, served, refused, restored, head, rerun, value));
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert_eq!(
            result.lock().unwrap().take(),
            Some((true, Ok(true), Err(BackingFull), true, Ok(()), Ok(true), 42)),
            "W{width}"
        );
        assert!(ledger.reserve_heap(grant.max_batch_bytes).is_ok());
        drop(topology);
    }
}
