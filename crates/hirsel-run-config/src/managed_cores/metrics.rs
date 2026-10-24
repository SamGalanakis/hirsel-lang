use crate::Cores;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    time::Duration,
};

#[derive(Clone, Debug, Serialize)]
pub struct Width {
    pub requested: Cores,
    /// Live executor destinations capped by process grant minus lost permits;
    /// not a count of simultaneously running threads or acquired permits.
    pub effective: u64,
    /// The full resolved target while effective differs, otherwise zero.
    /// This is not a count of queued tasks or of missing workers.
    pub pending: u64,
}

/// Lifecycle of the topology sampled by the live runtime. Socket visibility
/// precedes worker startup; a starting sample is not a physical-width proof.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum WorkerPhase {
    #[default]
    Starting,
    Running,
    Draining,
    Finished,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Percentiles {
    pub p50: u64,
    pub p99: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct StallSite {
    pub source: String,
    pub elapsed_ns: u64,
}

/// Diagnostic §7 sidecar; unavailable executor measurements are null, never
/// fabricated zeroes. The executor supplies actual effective and pending widths.
/// This structure is never an event, canonical input, or replay requirement.
#[derive(Clone, Debug, Serialize)]
pub struct MetricsSnapshot {
    pub width: Width,
    /// None when the embedding executor does not supply lifecycle diagnostics.
    pub worker_phase: Option<WorkerPhase>,
    pub runnable: Option<u64>,
    pub segment_ready_delay_ns: Option<Percentiles>,
    pub role_busy: Option<f64>,
    pub hol_stall: Option<f64>,
    pub early_commits: Option<u64>,
    pub run_ahead_depth: Option<u64>,
    pub speculation_pauses: Option<BTreeMap<String, u64>>,
    pub cut_pause_ns: Option<u64>,
    pub gate_waits: Option<u64>,
    pub remote_frees_per_s: Option<f64>,
    pub orphaned_heaps: Option<u64>,
    pub steals_per_s: Option<f64>,
    pub migrations_per_s: Option<f64>,
    pub switches_per_op: Option<f64>,
    pub throttled_usec: u64,
    pub reclamation_pending: Option<u64>,
    pub lost_permits: Option<u64>,
    pub home_busy: Option<bool>,
    pub stop_requested: Option<bool>,
    pub cpus_conflict: bool,
    pub stall_sites: Option<Vec<StallSite>>,
}
impl MetricsSnapshot {
    pub fn new(width: Width, cpus_conflict: bool) -> Self {
        Self {
            width,
            worker_phase: None,
            cpus_conflict,
            runnable: None,
            segment_ready_delay_ns: None,
            role_busy: None,
            hol_stall: None,
            early_commits: None,
            run_ahead_depth: None,
            speculation_pauses: None,
            cut_pause_ns: None,
            gate_waits: None,
            remote_frees_per_s: None,
            orphaned_heaps: None,
            steals_per_s: None,
            migrations_per_s: None,
            switches_per_op: None,
            throttled_usec: 0,
            reclamation_pending: None,
            lost_permits: None,
            home_busy: None,
            stop_requested: None,
            stall_sites: None,
        }
    }
}

/// The caller must first admit/pin the destination against mounts and sinks.
/// No path is accepted here; replay never constructs this writer. A failed write
/// is returned to the launcher and disables the stream (a partial line cannot be
/// repaired by appending a new snapshot).
pub struct MetricsWriter<W> {
    writer: W,
    last: Duration,
    throttled_total: u64,
    failed: bool,
}
impl<W: Write> MetricsWriter<W> {
    pub fn new(writer: W, now: Duration, throttled_total: u64) -> Self {
        Self {
            writer,
            last: now,
            throttled_total,
            failed: false,
        }
    }
    /// One-second cadence. Callers can serialize the same snapshot on control
    /// requests independently; those requests must not consume the periodic delta.
    pub fn emit(
        &mut self,
        now: Duration,
        throttled_total: u64,
        snapshot: &mut MetricsSnapshot,
    ) -> io::Result<bool> {
        if self.failed {
            return Err(io::Error::other("metrics stream previously failed"));
        }
        if now.saturating_sub(self.last) < Duration::from_secs(1) {
            return Ok(false);
        }
        snapshot.throttled_usec = throttled_total.saturating_sub(self.throttled_total);
        let mut line = serde_json::to_vec(snapshot)?;
        line.push(b'\n');
        if let Err(error) = self
            .writer
            .write_all(&line)
            .and_then(|()| self.writer.flush())
        {
            self.failed = true;
            return Err(error);
        }
        self.last = now;
        self.throttled_total = throttled_total;
        Ok(true)
    }
    pub fn into_inner(self) -> W {
        self.writer
    }
}
