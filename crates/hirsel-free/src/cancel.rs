//! Cancellation flags and an epoch-validated ancestor cache.
//!
//! Go context/context.go:433-441,549-586 at ebe2feefc400f813aefdc8a036e66d2619da6649
//! keeps an atomic cancellation cause and visits linked children on cancellation.
//! Hirsel's runtime owns the child links under its scheduler role. Unlike Go,
//! a task checks its own cause before its innermost active scope; leaving that
//! scope can uncover another cause, so cached observations carry a topology
//! epoch and cannot permanently cache the first nonempty ancestor observation.

use crate::sync::{AtomicU32, AtomicU64, Ordering};
use std::cell::Cell;

#[derive(Default)]
pub struct Flag(AtomicU32);

impl From<Option<u32>> for Flag {
    fn from(cause: Option<u32>) -> Self {
        Self(AtomicU32::new(cause.map_or(0, encode)))
    }
}

fn encode(cause: u32) -> u32 {
    cause
        .checked_add(1)
        .expect("a cancellation cause fits its flag")
}

impl Flag {
    pub fn cause(&self) -> Option<u32> {
        self.0.load(Ordering::Acquire).checked_sub(1)
    }

    /// First cause wins. The writer publishes an epoch change before it
    /// releases the runtime role, so subsequent cached readers invalidate.
    pub fn cancel(&self, cause: u32) -> bool {
        self.0
            .compare_exchange(0, encode(cause), Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

#[derive(Default)]
pub struct Epoch(AtomicU64);

impl Epoch {
    pub fn load(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    /// A cause or active ancestor set changed under the runtime role.
    pub fn changed(&self) {
        let old = self.0.fetch_add(1, Ordering::Release);
        assert_ne!(old, u64::MAX, "cancellation epoch overflow");
    }
}

/// Owner-local observations, accessed only while holding the scheduler role.
/// Flags remain independently atomic; a cache is deliberately not Sync.
pub struct Cache(Cell<(u64, Option<u32>)>);

impl Default for Cache {
    fn default() -> Self {
        Self(Cell::new((u64::MAX, None)))
    }
}

impl Cache {
    pub fn get(&self, epoch: u64, observe: impl FnOnce() -> Option<u32>) -> Option<u32> {
        let (seen, cause) = self.0.get();
        if seen == epoch {
            return cause;
        }
        let cause = observe();
        self.0.set((epoch, cause));
        cause
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_empty_and_nonempty_ancestor_observations_invalidate_at_topology_changes() {
        crate::sync::model(|| {
            let epoch = Epoch::default();
            let cache = Cache::default();
            assert_eq!(cache.get(epoch.load(), || None), None);
            assert_eq!(
                cache.get(epoch.load(), || panic!("uncanceled cache miss")),
                None
            );
            epoch.changed();
            assert_eq!(cache.get(epoch.load(), || Some(3)), Some(3));
            assert_eq!(
                cache.get(epoch.load(), || panic!("canceled cache miss")),
                Some(3)
            );
            epoch.changed();
            assert_eq!(cache.get(epoch.load(), || Some(1)), Some(1));
            epoch.changed();
            assert_eq!(cache.get(epoch.load(), || None), None);
        });
    }
}
