use super::*;
use crate::pool::cpu_scoped::ledger::{BackingFull, Ledger};
use std::sync::Arc;

fn grant(bytes: u64) -> hirsel_run_schema::CpuGrant {
    hirsel_run_schema::CpuGrant {
        provider: "cpu.scoped@1".into(),
        max_workers: 16,
        max_domains: 1,
        max_physical_bytes: 2 << 30,
        max_batch_bytes: bytes,
    }
}

fn pool(ledger: &Arc<Ledger>) -> RegionPool {
    let meter = Arc::<crate::budget::Meter>::default();
    meter.cpu.set(ledger.clone()).ok().unwrap();
    RegionPool::new(meter, 1)
}

fn assert_room(ledger: &Arc<Ledger>, bytes: u64) {
    let reservation = ledger.reserve_heap(bytes).unwrap();
    assert!(matches!(ledger.reserve_heap(1), Err(BackingFull)));
    drop(reservation);
}

#[test]
fn cpu_region_preflight_reuses_returned_segments_after_nonstopping_refusal() {
    let bytes = SEGMENT_SIZE as u64;
    let ledger = Ledger::new(&grant(bytes));
    let topology = ledger.topology(4).unwrap();
    let mut pool = pool(&ledger);
    assert_eq!(pool.reserve_cpu_backing(64), Ok(()));
    assert_room(&ledger, 0);
    let segment = pool.take(64);
    assert_room(&ledger, 0);
    assert_eq!(pool.reserve_cpu_backing(64), Err(BackingFull));
    assert!(pool.cpu_allocation.is_none());
    // SAFETY: this test owns the pool and its one lent segment.
    unsafe { pool.give_back(segment) };
    assert_eq!(pool.reserve_cpu_backing(64), Ok(()));
    assert_eq!(pool.take(64), segment);
    assert_room(&ledger, 0);
    pool.release_all();
    assert_room(&ledger, bytes);
    drop(topology);
}

#[test]
fn cpu_region_preflight_keeps_standard_and_memory_exact_reuse_rules() {
    for metered in [false, true] {
        let bytes = 2 * SEGMENT_SIZE as u64;
        let ledger = Ledger::new(&grant(bytes));
        let topology = ledger.topology(4).unwrap();
        let mut pool = pool(&ledger);
        if metered {
            pool.meter.set_limit(bytes * 4);
        }
        assert_eq!(pool.reserve_cpu_backing(bytes), Ok(()));
        let segment = pool.take(bytes);
        // SAFETY: the segment is the pool's only lent segment.
        unsafe { pool.give_back(segment) };
        assert_eq!(
            pool.reserve_cpu_backing(SEGMENT_SIZE as u64),
            Err(BackingFull),
            "a standard request never pins oversized backing"
        );
        let need = SEGMENT_SIZE as u64 + 1;
        if metered {
            assert_eq!(pool.reserve_cpu_backing(need), Err(BackingFull));
        } else {
            assert_eq!(pool.reserve_cpu_backing(need), Ok(()));
            assert_eq!(pool.take(need), segment);
        }
        pool.release_all();
        assert_room(&ledger, bytes);
        drop(topology);
    }
}

#[test]
fn cpu_region_reclamation_returns_abandoned_reservations() {
    let bytes = SEGMENT_SIZE as u64;
    let ledger = Ledger::new(&grant(bytes));
    let topology = ledger.topology(4).unwrap();
    let mut pool = pool(&ledger);
    assert_eq!(pool.reserve_cpu_backing(64), Ok(()));
    assert_room(&ledger, 0);
    assert_eq!(pool.active_bytes(), 0);
    pool.release_all();
    assert_room(&ledger, bytes);
    drop(topology);
}
