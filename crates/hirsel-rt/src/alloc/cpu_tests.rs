use super::*;
use crate::pool::cpu_scoped::ledger::{BackingFull, Ledger};
use std::sync::Arc;

fn grant(batch: u64) -> hirsel_run_schema::CpuGrant {
    hirsel_run_schema::CpuGrant {
        provider: "cpu.scoped@1".into(),
        max_workers: 16,
        max_domains: 1,
        max_physical_bytes: 2 << 30,
        max_batch_bytes: batch,
    }
}

fn assert_room(ledger: &Arc<Ledger>, bytes: u64) {
    let all = ledger.reserve_heap(bytes).expect("remaining backing fits");
    assert!(matches!(ledger.reserve_heap(1), Err(BackingFull)));
    drop(all);
}

fn context(ledger: &Arc<Ledger>, diagnostic: bool) -> *mut TaskCtx {
    let mut ctx = ptr::null_mut();
    let config = crate::Config {
        flags: if diagnostic {
            crate::context::CONFIG_DIAGNOSTIC
        } else {
            0
        },
        ..crate::Config::new()
    };
    // SAFETY: this test owns the new context until its explicit destruction.
    unsafe {
        assert_eq!(
            crate::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
            0
        );
        (*ctx)
            .heap_ref()
            .meter
            .cpu
            .set(ledger.clone())
            .ok()
            .unwrap();
        (*ctx).heap().idle_cap = 0;
    }
    ctx
}

#[test]
fn abandoned_cpu_preflight_returns_capacity_without_allocating_or_stopping() {
    for diagnostic in [false, true] {
        let bytes = if cfg!(feature = "system-allocator") {
            cpu_large_charge(request_capacity(16, 8), 8)
        } else {
            cpu_slab_charge(diagnostic)
        };
        let ledger = Ledger::new(&grant(bytes));
        let topology = ledger.topology(4).unwrap();
        let ctx = context(&ledger, diagnostic);
        // SAFETY: one exclusive context, and no allocation follows this reservation.
        unsafe {
            assert_eq!(cpu_preflight(ctx, 0, 16, 8), Ok(()));
            assert_room(&ledger, 0);
            assert_eq!((*ctx).heap_ref().backing_bytes(), 0);
            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
        }
        assert_room(&ledger, bytes);
        drop(topology);
    }
}

#[test]
#[cfg(not(feature = "system-allocator"))]
fn cpu_slab_refusal_leaves_heap_usable_and_local_frees_need_no_more_backing() {
    for diagnostic in [false, true] {
        for align in [8, 16] {
            let bytes = cpu_slab_charge(diagnostic);
            let ledger = Ledger::new(&grant(bytes));
            let topology = ledger.topology(4).unwrap();
            let ctx = context(&ledger, diagnostic);
            let mut blocks = Vec::new();
            // SAFETY: all blocks are private to this context and are shallow-freed once.
            unsafe {
                while cpu_preflight(ctx, 0, 4096, align).is_ok() {
                    let block = alloc_prepared(ctx, 0, 4096, align, DOMAIN_HEAP, 0);
                    (*ctx).heap().cpu_allocation = None;
                    blocks.push(block);
                    assert!(blocks.len() < 32, "a single 64 KiB slab is bounded");
                }
                assert!(blocks.len() > 1);
                assert_eq!(cpu_preflight(ctx, 0, 4096, align), Err(BackingFull));
                assert!((*ctx).heap_ref().cpu_allocation.is_none());
                assert_room(&ledger, 0);
                let returned = blocks.pop().unwrap();
                (*ctx).heap().free(returned);
                // Physical refusal did not select a Stop or poison the heap.
                let reused = hirsel_rt_v2_alloc(ctx, 0, 4096, align, DOMAIN_HEAP, 0);
                assert_eq!(reused, returned);
                blocks.push(reused);
                assert_room(&ledger, 0);
                for block in blocks {
                    (*ctx).heap().free(block);
                }
                (*ctx).heap().trim();
                assert_room(&ledger, bytes);
                crate::context::hirsel_rt_v2_ctx_destroy(ctx);
            }
            assert_room(&ledger, bytes);
            drop(topology);
        }
    }
}

#[test]
fn cpu_large_preflight_charges_once_and_refusal_can_retry_after_free() {
    for diagnostic in [false, true] {
        for (extent, align) in [(4097, 8), (31, 64), (4097, 65536)] {
            let bytes = cpu_large_charge(request_capacity(extent, align), align);
            let ledger = Ledger::new(&grant(bytes));
            let topology = ledger.topology(4).unwrap();
            let ctx = context(&ledger, diagnostic);
            // SAFETY: one live context and two successive private allocations.
            unsafe {
                let block = hirsel_rt_v2_alloc(ctx, 0, extent, align, DOMAIN_HEAP, 0);
                assert_room(&ledger, 0);
                assert_eq!(cpu_preflight(ctx, 0, extent, align), Err(BackingFull));
                (*ctx).heap().free(block);
                assert_room(&ledger, bytes);
                let block = hirsel_rt_v2_alloc(ctx, 0, extent, align, DOMAIN_HEAP, 0);
                assert_room(&ledger, 0);
                (*ctx).heap().free(block);
                crate::context::hirsel_rt_v2_ctx_destroy(ctx);
            }
            assert_room(&ledger, bytes);
            drop(topology);
        }
    }
}

#[test]
fn logical_stop_returns_unconsumed_cpu_backing_during_run_reclamation() {
    let bytes = cpu_large_charge(request_capacity(5000, 8), 8);
    let ledger = Ledger::new(&grant(bytes));
    let topology = ledger.topology(4).unwrap();
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
        },
        |ctx| unsafe {
            hirsel_rt_v2_alloc(ctx, 0, 5000, 8, DOMAIN_HEAP, 0);
            panic!("logical allocation must stop");
        },
    );
    assert_eq!(
        outcome.ended,
        Err(crate::stop::Stop::BudgetExceeded(
            hirsel_run_schema::BudgetKind::Memory
        ))
    );
    assert_eq!(outcome.pending, 0);
    assert_room(&ledger, bytes);
    drop(topology);
}
