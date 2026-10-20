//! The Io ordering row's headroom counters (design §5 Headroom, B1's `io_ordering` row in
//! `bench/multicore/manifest.json`). A run holds them only when metrics are enabled: every
//! collection point is behind an `Option<Arc<IoMetrics>>`, so a run without metrics pays one
//! branch and reads no clock.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Duration;

#[derive(Debug, Default)]
pub struct IoMetrics {
    /// Host calls made through a [`ConcurrentAdapter`](crate::io_concurrent::ConcurrentAdapter)
    /// on the caller's thread.
    concurrent_calls: AtomicU64,
    /// Host calls made by the home thread for another thread, through the bridge.
    bridged_calls: AtomicU64,
    namespace_wait_ns: AtomicU64,
    key_epoch_wait_ns: AtomicU64,
    in_flight: AtomicU64,
    max_in_flight: AtomicU64,
}

impl IoMetrics {
    pub(crate) fn key_wait(&self, namespace: bool, waited: Duration) {
        let ns = u64::try_from(waited.as_nanos()).unwrap_or(u64::MAX);
        self.key_epoch_wait_ns.fetch_add(ns, Relaxed);
        if namespace {
            self.namespace_wait_ns.fetch_add(ns, Relaxed);
        }
    }
    pub(crate) fn bridged(&self) {
        self.bridged_calls.fetch_add(1, Relaxed);
    }
    /// A concurrent host call begins; the guard ends it.
    pub(crate) fn call(&self) -> InFlight<'_> {
        self.concurrent_calls.fetch_add(1, Relaxed);
        let now = self.in_flight.fetch_add(1, Relaxed) + 1;
        self.max_in_flight.fetch_max(now, Relaxed);
        InFlight(self)
    }

    /// The row's metrics by their manifest names: `syscall_serialisation` is the share of
    /// host calls the home thread made for others; `independent_file_overlap` is the most
    /// concurrent host calls observed in flight at once.
    pub fn values(&self) -> [(&'static str, f64); 4] {
        let bridged = self.bridged_calls.load(Relaxed) as f64;
        let all = bridged + self.concurrent_calls.load(Relaxed) as f64;
        [
            (
                "syscall_serialisation",
                if all == 0.0 { 0.0 } else { bridged / all },
            ),
            (
                "namespace_wait_ns",
                self.namespace_wait_ns.load(Relaxed) as f64,
            ),
            (
                "key_epoch_wait_ns",
                self.key_epoch_wait_ns.load(Relaxed) as f64,
            ),
            (
                "independent_file_overlap",
                self.max_in_flight.load(Relaxed) as f64,
            ),
        ]
    }
}

pub(crate) struct InFlight<'a>(&'a IoMetrics);
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Relaxed);
    }
}
