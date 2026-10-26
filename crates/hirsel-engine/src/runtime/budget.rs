//! A process allowance shared by all simultaneously live immutable snapshots.
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub struct ProcessBudget {
    limit: u64,
    reserved: Mutex<u64>,
    released: Condvar,
}
impl ProcessBudget {
    /// The engine subtracts its fixed overhead from the host limit first.
    pub fn new(limit: u64) -> Arc<Self> {
        Arc::new(Self {
            limit,
            reserved: Mutex::new(0),
            released: Condvar::new(),
        })
    }
    /// Nonblocking: reject a new source snapshot instead of multiplying caps
    /// or waiting while holding another snapshot's allowance.
    pub fn reserve(self: &Arc<Self>, bytes: u64) -> std::io::Result<MemoryReservation> {
        let mut reserved = self.reserved.lock().unwrap_or_else(|e| e.into_inner());
        if bytes > self.limit.saturating_sub(*reserved) {
            return Err(std::io::Error::other(format!(
                "process query allowance exhausted: {bytes} bytes requested with {} of {} reserved",
                *reserved, self.limit
            )));
        }
        *reserved += bytes;
        Ok(MemoryReservation {
            budget: self.clone(),
            bytes,
            return_to: None,
        })
    }
    pub fn reserved(&self) -> u64 {
        *self.reserved.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn limit(&self) -> u64 {
        self.limit
    }
    /// Block until an owner releases part of this allowance, a host reports
    /// idle work, or the timeout passes. Only a caller holding none of this
    /// budget may wait; one holding a charge could wait on itself.
    pub fn wait_for_release(&self, timeout: Duration) {
        let reserved = self.reserved.lock().unwrap_or_else(|e| e.into_inner());
        drop(self.released.wait_timeout(reserved, timeout));
    }
    /// Wake waiters after work became idle without releasing its charge yet.
    pub fn notify_idle(&self) {
        self.released.notify_all();
    }
}

/// Held through worker shutdown or the last externally held input/map owner.
/// This type is not Clone: a retained shared mapping shares an Arc around its
/// one reservation, rather than duplicating or silently dropping its charge.
pub struct MemoryReservation {
    budget: Arc<ProcessBudget>,
    bytes: u64,
    return_to: Option<Arc<Pool>>,
}

struct Pool {
    available: Mutex<Option<MemoryReservation>>,
    return_to: Option<Arc<Pool>>,
    _metadata: MemoryReservation,
}

/// Reuses one already admitted partition. A child returns to this pool only
/// when its last owner drops it; retained source/product owners keep their
/// exact charge unavailable to later rounds. This never reserves process bytes.
pub struct MemoryReservationPool {
    inner: Arc<Pool>,
}
impl MemoryReservationPool {
    pub fn metadata_bytes() -> u64 {
        (size_of::<Pool>() + 2 * size_of::<usize>()) as u64
    }
    pub fn new(mut reservation: MemoryReservation) -> Result<Self, MemoryReservation> {
        let Some(metadata) = reservation.split(Self::metadata_bytes()) else {
            return Err(reservation);
        };
        Ok(Self {
            inner: Arc::new(Pool {
                return_to: reservation.return_to.clone(),
                available: Mutex::new(Some(reservation)),
                _metadata: metadata,
            }),
        })
    }
    pub fn available_bytes(&self) -> u64 {
        self.inner
            .available
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map_or(0, MemoryReservation::bytes)
    }
    pub fn take(&self, bytes: u64) -> Option<MemoryReservation> {
        let mut parent = self
            .inner
            .available
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut child = parent.as_mut()?.split(bytes)?;
        child.return_to = Some(self.inner.clone());
        Some(child)
    }
}
impl Drop for MemoryReservationPool {
    fn drop(&mut self) {
        // Close lending and release unused bytes immediately. Detached children
        // retain the pool's charged allocation metadata until their last Drop.
        let unused = self
            .inner
            .available
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        drop(unused);
    }
}
impl MemoryReservation {
    /// Transfer part of this already charged allowance to a distinct owner.
    /// The process total never changes, including on failure; each resulting
    /// owner releases exactly its part on Drop.
    pub fn split(&mut self, bytes: u64) -> Option<Self> {
        self.bytes = self.bytes.checked_sub(bytes)?;
        Some(Self {
            budget: self.budget.clone(),
            bytes,
            return_to: self.return_to.clone(),
        })
    }
    /// Take over a charge on the same process budget and return partition.
    /// A charge from another budget or pool is handed back unchanged.
    pub fn join(&mut self, mut other: Self) -> Result<(), Self> {
        let same_partition = match (&self.return_to, &other.return_to) {
            (None, None) => true,
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            _ => false,
        };
        if !Arc::ptr_eq(&self.budget, &other.budget) || !same_partition {
            return Err(other);
        }
        self.bytes += std::mem::take(&mut other.bytes);
        Ok(())
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}
impl Drop for MemoryReservation {
    fn drop(&mut self) {
        let bytes = std::mem::take(&mut self.bytes);
        if bytes == 0 {
            return;
        }
        let mut destination = self.return_to.as_deref();
        while let Some(pool) = destination {
            let mut available = pool.available.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(parent) = available.as_mut() {
                parent.bytes += bytes;
                return;
            }
            destination = pool.return_to.as_deref();
        }
        *self
            .budget
            .reserved
            .lock()
            .unwrap_or_else(|e| e.into_inner()) -= bytes;
        self.budget.released.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_transfers_one_charge_and_each_owner_releases_its_part() {
        let budget = ProcessBudget::new(100);
        let mut outer = budget.reserve(100).unwrap();
        assert!(outer.split(101).is_none());
        assert_eq!(outer.bytes(), 100);
        assert_eq!(budget.reserved(), 100);
        let first = outer.split(30).unwrap();
        let mut second = outer.split(50).unwrap();
        let nested = second.split(40).unwrap();
        assert_eq!(
            outer.bytes() + first.bytes() + second.bytes() + nested.bytes(),
            100
        );
        assert_eq!(budget.reserved(), 100);
        assert!(budget.reserve(1).is_err());
        drop(outer);
        assert_eq!(budget.reserved(), 80);
        let replacement = budget.reserve(20).unwrap();
        drop(first);
        drop(second);
        drop(nested);
        assert_eq!(budget.reserved(), 20);
        drop(replacement);
        assert_eq!(budget.reserved(), 0);
    }
}

#[cfg(test)]
mod pool_tests {
    use super::*;
    #[test]
    fn retained_clone_keeps_its_charge_and_later_rounds_reuse_only_released_bytes() {
        let total = MemoryReservationPool::metadata_bytes() + 100;
        let budget = ProcessBudget::new(total);
        let pool = MemoryReservationPool::new(budget.reserve(total).unwrap())
            .ok()
            .unwrap();
        assert_eq!(pool.available_bytes(), 100);
        let held = Arc::new(pool.take(30).unwrap());
        let cloned = held.clone();
        drop(held);
        assert_eq!(pool.available_bytes(), 70);
        assert!(pool.take(71).is_none());
        for _ in 0..3 {
            let mut stage = pool.take(70).unwrap();
            let scratch = stage.split(20).unwrap();
            assert_eq!(pool.available_bytes(), 0);
            std::thread::spawn(move || drop(scratch)).join().unwrap();
            assert_eq!(pool.available_bytes(), 20);
            let reuse = pool.take(20).unwrap();
            assert!(stage.join(reuse).is_ok());
            assert_eq!(stage.bytes(), 70);
            drop(stage);
            assert_eq!(pool.available_bytes(), 70);
            assert_eq!(budget.reserved(), total);
            assert!(budget.reserve(1).is_err());
        }
        drop(cloned);
        assert_eq!(pool.available_bytes(), 100);
        drop(pool);
        assert_eq!(budget.reserved(), 0);
    }
    #[test]
    fn pool_metadata_refuses_n_minus_one_and_detached_children_release_after_parent_drop() {
        let metadata = MemoryReservationPool::metadata_bytes();
        let budget = ProcessBudget::new(metadata + 100);
        let short = budget.reserve(metadata - 1).unwrap();
        let refused = MemoryReservationPool::new(short)
            .err()
            .expect("metadata grant");
        assert_eq!(refused.bytes(), metadata - 1);
        drop(refused);
        assert_eq!(budget.reserved(), 0);
        let empty = MemoryReservationPool::new(budget.reserve(metadata).unwrap())
            .ok()
            .unwrap();
        assert_eq!(empty.available_bytes(), 0);
        assert!(empty.take(1).is_none());
        drop(empty);
        let pool = MemoryReservationPool::new(budget.reserve(metadata + 100).unwrap())
            .ok()
            .unwrap();
        let held = Arc::new(pool.take(40).unwrap());
        let clone = held.clone();
        drop(pool);
        assert_eq!(budget.reserved(), metadata + 40);
        drop(held);
        assert_eq!(budget.reserved(), metadata + 40);
        drop(clone);
        assert_eq!(budget.reserved(), 0);
    }
    #[test]
    fn nested_pools_return_to_their_original_partition_and_refuse_foreign_budget_join() {
        let metadata = MemoryReservationPool::metadata_bytes();
        let budget = ProcessBudget::new(2 * metadata + 100);
        let pool = MemoryReservationPool::new(budget.reserve(2 * metadata + 100).unwrap())
            .ok()
            .unwrap();
        let nested = MemoryReservationPool::new(pool.take(metadata + 60).unwrap())
            .ok()
            .unwrap();
        assert_eq!(pool.available_bytes(), 40);
        assert_eq!(nested.available_bytes(), 60);
        let mut held = nested.take(60).unwrap();
        let foreign_budget = ProcessBudget::new(1);
        let foreign = held
            .join(foreign_budget.reserve(1).unwrap())
            .err()
            .expect("foreign charge");
        assert_eq!(foreign.bytes(), 1);
        assert_eq!(held.bytes(), 60);
        drop(foreign);
        assert_eq!(foreign_budget.reserved(), 0);
        drop(held);
        drop(nested);
        assert_eq!(pool.available_bytes(), metadata + 100);
        drop(pool);
        assert_eq!(budget.reserved(), 0);
    }
}

#[cfg(test)]
mod pool_partition_tests {
    use super::*;
    #[test]
    fn joins_cannot_move_a_live_subcharge_into_another_partition_on_the_same_budget() {
        let metadata = MemoryReservationPool::metadata_bytes();
        let budget = ProcessBudget::new(2 * metadata + 100);
        let first = MemoryReservationPool::new(budget.reserve(metadata + 60).unwrap())
            .ok()
            .unwrap();
        let second = MemoryReservationPool::new(budget.reserve(metadata + 40).unwrap())
            .ok()
            .unwrap();
        let mut held = first.take(30).unwrap();
        let foreign = second.take(20).unwrap();
        let refused = held
            .join(foreign)
            .err()
            .expect("distinct original partitions");
        assert_eq!(held.bytes(), 30);
        assert_eq!(refused.bytes(), 20);
        assert_eq!(first.available_bytes(), 30);
        assert_eq!(second.available_bytes(), 20);
        drop(refused);
        assert_eq!(second.available_bytes(), 40);
        drop(held);
        assert_eq!(first.available_bytes(), 60);
        assert_eq!(budget.reserved(), 2 * metadata + 100);
        drop(first);
        drop(second);
        assert_eq!(budget.reserved(), 0);
    }
}

#[cfg(test)]
mod pool_late_child_tests {
    use super::*;
    #[test]
    fn closed_nested_pool_returns_late_child_and_metadata_to_the_live_outer_partition() {
        let metadata = MemoryReservationPool::metadata_bytes();
        let total = 2 * metadata + 100;
        let budget = ProcessBudget::new(total);
        let outer = MemoryReservationPool::new(budget.reserve(total).unwrap())
            .ok()
            .unwrap();
        let inner = MemoryReservationPool::new(outer.take(metadata + 60).unwrap())
            .ok()
            .unwrap();
        let held = Arc::new(inner.take(40).unwrap());
        let clone = held.clone();
        assert_eq!(outer.available_bytes(), 40);
        drop(inner);
        assert_eq!(outer.available_bytes(), 60);
        assert_eq!(budget.reserved(), total);
        drop(held);
        assert_eq!(outer.available_bytes(), 60);
        drop(clone);
        assert_eq!(outer.available_bytes(), metadata + 100);
        assert_eq!(budget.reserved(), total);
        let next_round = outer.take(metadata + 100).unwrap();
        assert_eq!(outer.available_bytes(), 0);
        drop(next_round);
        assert_eq!(outer.available_bytes(), metadata + 100);
        drop(outer);
        assert_eq!(budget.reserved(), 0);
    }
}
