//! CPU grant storage follows backing allocations, including orphan heaps.
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
struct Held {
    heap: u64,
    topology: u64,
    stacks: u64,
}

#[derive(Debug)]
pub(crate) struct Ledger {
    physical_limit: u64,
    batch_limit: u64,
    held: Mutex<Held>,
}

pub(crate) struct Topology {
    ledger: Arc<Ledger>,
    bytes: u64,
}

impl Ledger {
    pub(crate) fn new(grant: &hirsel_run_schema::CpuGrant) -> Arc<Self> {
        Arc::new(Self {
            physical_limit: grant.max_physical_bytes,
            batch_limit: grant.max_batch_bytes,
            held: Mutex::default(),
        })
    }
    pub(crate) fn charge_heap(&self, bytes: u64) {
        let fits = {
            let mut held = self.held.lock().unwrap();
            let next = held.heap.checked_add(bytes);
            if let Some(next) = next.filter(|&next| {
                held.topology == 0
                    || (next <= self.batch_limit
                        && next
                            .checked_add(held.topology)
                            .and_then(|n| n.checked_add(held.stacks))
                            .is_some_and(|n| n <= self.physical_limit))
            }) {
                held.heap = next;
                true
            } else {
                false
            }
        };
        if !fits {
            crate::budget::exceeded(hirsel_run_schema::BudgetKind::Memory);
        }
    }
    pub(crate) fn credit_heap(&self, bytes: u64) {
        let mut held = self.held.lock().unwrap();
        held.heap = held.heap.checked_sub(bytes).expect("CPU backing credit");
    }
    pub(crate) fn stack(self: &Arc<Self>, bytes: u64) -> Backing {
        let mut held = self.held.lock().unwrap();
        let next = held.stacks.checked_add(bytes);
        let fits = next.is_some_and(|n| {
            held.topology == 0
                || n.checked_add(held.heap)
                    .and_then(|n| n.checked_add(held.topology))
                    .is_some_and(|n| n <= self.physical_limit)
        });
        if !fits {
            drop(held);
            crate::budget::exceeded(hirsel_run_schema::BudgetKind::Memory);
        }
        held.stacks = next.unwrap();
        Backing {
            ledger: self.clone(),
            bytes,
        }
    }
    pub(crate) fn topology(self: &Arc<Self>, width: u32) -> Option<Arc<Topology>> {
        // Reserve the actual stack sizes and the existing bounded idle cache.
        // Live caller/child stacks and one retained parent are separate from
        // every worker's four cached stacks. Bookkeeping has a finite margin.
        let fiber = (crate::fiber::STACK_BYTES + crate::fiber::GUARD_BYTES) as u64;
        let bytes = u64::from(width)
            .checked_mul((2 << 20) + (64 << 10) + 5 * fiber + (1 << 20))?
            .checked_add(2 * fiber)?
            .checked_add(self.batch_limit.checked_mul(2)?)?;
        let mut held = self.held.lock().unwrap();
        let next = held.topology.checked_add(bytes)?;
        if held.heap > self.batch_limit
            || held.heap.checked_add(held.stacks)?.checked_add(next)? > self.physical_limit
        {
            return None;
        }
        held.topology = next;
        Some(Arc::new(Topology {
            ledger: self.clone(),
            bytes,
        }))
    }
}
impl Drop for Topology {
    fn drop(&mut self) {
        let mut held = self.ledger.held.lock().unwrap();
        held.topology = held
            .topology
            .checked_sub(self.bytes)
            .expect("CPU topology credit");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn grant(physical: u64, batch: u64) -> hirsel_run_schema::CpuGrant {
        hirsel_run_schema::CpuGrant {
            provider: "cpu.scoped@1".into(),
            max_workers: 16,
            max_domains: 1,
            max_physical_bytes: physical,
            max_batch_bytes: batch,
        }
    }
    #[test]
    fn retained_backing_is_not_reset_by_topology_exit_and_blocks_the_next_reservation() {
        let ledger = Ledger::new(&grant(2 << 30, 1 << 29));
        let topology = ledger.topology(16).unwrap();
        let bytes = topology.bytes;
        ledger.charge_heap(1 << 20);
        let native_guard = topology.clone();
        drop(topology);
        assert_eq!(ledger.held.lock().unwrap().topology, bytes);
        drop(native_guard);
        assert_eq!(ledger.held.lock().unwrap().topology, 0);
        assert_eq!(ledger.held.lock().unwrap().heap, 1 << 20);
        let small_limit = Ledger::new(&grant(2 << 30, 1 << 20))
            .topology(16)
            .unwrap()
            .bytes;
        let small = Ledger::new(&grant(small_limit, 1 << 20));
        small.charge_heap(1);
        assert!(small.topology(16).is_none());
        assert_eq!(small.held.lock().unwrap().topology, 0);
        small.credit_heap(1);
        let parked = small.stack(1);
        assert!(small.topology(16).is_none());
        drop(parked);
        assert!(small.topology(16).is_some());
        ledger.credit_heap(1 << 20);
        assert_eq!(ledger.held.lock().unwrap().heap, 0);
    }
}

pub(crate) struct Backing {
    ledger: Arc<Ledger>,
    bytes: u64,
}
impl Drop for Backing {
    fn drop(&mut self) {
        let mut held = self.ledger.held.lock().unwrap();
        held.stacks = held
            .stacks
            .checked_sub(self.bytes)
            .expect("CPU stack credit");
    }
}
