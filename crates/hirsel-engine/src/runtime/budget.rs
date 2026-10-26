//! A process allowance shared by all simultaneously live immutable snapshots.
use std::sync::{Arc, Mutex};

pub struct ProcessBudget {
    limit: u64,
    reserved: Mutex<u64>,
}
impl ProcessBudget {
    /// The engine subtracts its fixed overhead from the host limit first.
    pub fn new(limit: u64) -> Arc<Self> {
        Arc::new(Self {
            limit,
            reserved: Mutex::new(0),
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
        })
    }
    pub fn reserved(&self) -> u64 {
        *self.reserved.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Held through worker shutdown or the last externally held input/map owner.
/// This type is not Clone: a retained shared mapping shares an Arc around its
/// one reservation, rather than duplicating or silently dropping its charge.
pub struct MemoryReservation {
    budget: Arc<ProcessBudget>,
    bytes: u64,
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
        })
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}
impl Drop for MemoryReservation {
    fn drop(&mut self) {
        *self
            .budget
            .reserved
            .lock()
            .unwrap_or_else(|e| e.into_inner()) -= self.bytes;
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
