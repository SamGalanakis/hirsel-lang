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
            return Err(std::io::Error::other("process query allowance exhausted"));
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
