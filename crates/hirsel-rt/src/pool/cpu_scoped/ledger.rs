//! CPU grant storage follows backing allocations, including orphan heaps.
use std::sync::{Arc, Mutex};

pub(crate) mod blocks;

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
    pub(crate) blocks: Mutex<blocks::Blocks>,
}

pub(crate) struct Topology {
    ledger: Arc<Ledger>,
    bytes: u64,
}

/// Physical capacity is a refusal to allocate speculative backing, never a
/// language budget stop. The caller pauses/discards before borrowing a heap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BackingFull;

/// Capacity reserved before an allocator borrow. An abandoned request returns
/// its reservation; backing actually obtained keeps its charge until freed.
pub(crate) struct HeapReservation {
    ledger: Arc<Ledger>,
    bytes: u64,
}

impl HeapReservation {
    pub(crate) fn consume(&mut self, bytes: u64) {
        self.bytes = self.bytes.checked_sub(bytes).expect("reserved CPU backing");
    }
}

impl Drop for HeapReservation {
    fn drop(&mut self) {
        if self.bytes != 0 {
            self.ledger.credit_heap(self.bytes);
        }
    }
}

impl Ledger {
    pub(crate) fn new(grant: &hirsel_run_schema::CpuGrant) -> Arc<Self> {
        Arc::new(Self {
            physical_limit: grant.max_physical_bytes,
            batch_limit: grant.max_batch_bytes,
            held: Mutex::default(),
            blocks: Mutex::default(),
        })
    }
    #[cfg(test)]
    fn charge_heap(&self, bytes: u64) {
        if self.try_charge_job_heap(bytes).is_err() {
            crate::budget::exceeded(hirsel_run_schema::BudgetKind::Memory);
        }
    }
    pub(crate) fn reserve_heap(
        self: &Arc<Self>,
        bytes: u64,
    ) -> Result<HeapReservation, BackingFull> {
        self.try_charge_job_heap(bytes)?;
        Ok(HeapReservation {
            ledger: self.clone(),
            bytes,
        })
    }
    /// Job charges are summarized separately in Stretch. Backing
    /// admission checks the grant; its order cannot select a
    /// semantic stop. On failure no backing has been charged or allocated.
    pub(crate) fn try_charge_job_heap(&self, bytes: u64) -> Result<(), BackingFull> {
        let mut held = self.held.lock().unwrap();
        let next = held.heap.checked_add(bytes).ok_or(BackingFull)?;
        if held.topology != 0
            && (next > self.batch_limit
                || !next
                    .checked_add(held.topology)
                    .and_then(|n| n.checked_add(held.stacks))
                    .is_some_and(|n| n <= self.physical_limit))
        {
            return Err(BackingFull);
        }
        held.heap = next;
        Ok(())
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
    use crate::budget::stretch::{Budgets, Event, JobStop, Jobs, Settled, summarise};
    use hirsel_run_schema::BudgetKind;
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
    fn outstanding_heap_reservations_share_the_cap_and_return_only_unused_bytes() {
        let ledger = Ledger::new(&grant(2 << 30, 200));
        let topology = ledger.topology(4).unwrap();
        let mut first = ledger.reserve_heap(100).unwrap();
        let second = ledger.reserve_heap(100).unwrap();
        assert!(matches!(ledger.reserve_heap(1), Err(BackingFull)));
        first.consume(60);
        drop(first);
        assert_eq!(ledger.held.lock().unwrap().heap, 160);
        drop(second);
        assert_eq!(ledger.held.lock().unwrap().heap, 60);
        ledger.credit_heap(60);
        assert_eq!(ledger.held.lock().unwrap().heap, 0);
        drop(topology);
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

    #[test]
    fn cpu_job_cumulative_stop_is_job_three_despite_job_five_backing_refusal() {
        for width in [4, 8, 16] {
            let grant = grant(2 << 30, 250);
            let ledger = Ledger::new(&grant);
            let topology = ledger.topology(width).unwrap();
            // Negative control: the legacy allocator callback selects job
            // five from physical order. It cannot wait while a heap is lent.
            let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let physical = observed.clone();
            let legacy = ledger.clone();
            let outcome = crate::host::BodyRun::new(1, false).run(
                |_| {},
                move |_| {
                    for job in [0, 1, 4, 3, 2] {
                        physical.store(job + 1, std::sync::atomic::Ordering::Relaxed);
                        legacy.charge_heap(100);
                    }
                    crate::STATUS_OK
                },
            );
            assert_eq!(
                outcome.ended,
                Err(crate::stop::Stop::BudgetExceeded(BudgetKind::Memory))
            );
            assert_eq!(observed.load(std::sync::atomic::Ordering::Relaxed), 5);
            ledger.credit_heap(200);
            let state = Settled {
                level: 0,
                limit: u64::MAX,
                fuel: 100,
            };
            let mut live = Jobs::cpu(&grant, width, width as usize * 2, state);
            let mut recorded = Vec::new();
            // Jobs one and two retain 200 bytes. The next physical request
            // belongs to job five, while job three is the first serial stop.
            for job in [0, 1, 4, 3, 2] {
                let charges = summarise(
                    Budgets {
                        fuel: true,
                        memory: true,
                    },
                    &[Event::Fuel, Event::Alloc(100)],
                )
                .unwrap();
                recorded.push((job, charges.clone(), job < 2));
                let backing = ledger.try_charge_job_heap(100);
                assert_eq!(backing.is_ok(), job < 2);
                if backing.is_ok() {
                    live.publish(job, charges);
                } else {
                    live.pause(job, charges);
                }
                match job {
                    0 => assert_eq!(live.retire(), Ok(1)),
                    1 | 4 | 3 => assert_eq!(live.retire(), Ok(2)),
                    2 => assert_eq!(
                        live.retire(),
                        Err(JobStop {
                            job: 2,
                            kind: BudgetKind::Memory
                        })
                    ),
                    _ => unreachable!(),
                }
            }
            let stop = live.retire();
            assert_eq!(ledger.held.lock().unwrap().heap, 200);
            ledger.credit_heap(200);
            drop(topology);
            // Replay starts no topology and does not reserve backing. It
            // consumes the very same pieces in serial order.
            recorded.sort_by_key(|(job, _, _)| *job);
            let mut replay = Jobs::cpu(&grant, width, width as usize * 2, state);
            for (job, charges, complete) in recorded {
                if complete {
                    replay.publish(job, charges);
                } else {
                    replay.pause(job, charges);
                }
            }
            assert_eq!(replay.retire(), stop, "width {width}");
        }
    }

    #[test]
    fn job_backing_refuses_physical_exhaustion_and_overflow_without_a_semantic_stop() {
        let template = Ledger::new(&grant(u64::MAX, 1000));
        let overhead = template.topology(4).unwrap().bytes;
        let ledger = Ledger::new(&grant(overhead + 100, 1000));
        let topology = ledger.topology(4).unwrap();
        assert_eq!(ledger.try_charge_job_heap(100), Ok(()));
        assert_eq!(ledger.try_charge_job_heap(1), Err(BackingFull));
        assert_eq!(ledger.try_charge_job_heap(u64::MAX), Err(BackingFull));
        assert_eq!(ledger.held.lock().unwrap().heap, 100);
        ledger.credit_heap(100);
        drop(topology);
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
