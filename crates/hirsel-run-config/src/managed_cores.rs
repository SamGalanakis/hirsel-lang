//! Host-only placement resolution and diagnostics. Never used to derive identity or
//! replay inputs. The live executor owns worker changes and admitted destinations.
use crate::{Cores, Placement, RunRefusal, invalid};
use serde::Serialize;
use std::time::Duration;

mod linux;
mod metrics;
pub use linux::LinuxLimits;
pub use metrics::{MetricsSnapshot, MetricsWriter, Percentiles, StallSite, Width, WorkerPhase};

/// Sorted, disjoint inclusive intervals; even `0-4294967295` uses bounded storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuSet(Vec<(u32, u32)>);
impl CpuSet {
    pub fn parse(text: &str) -> Result<Self, RunRefusal> {
        let mut ranges = Vec::new();
        for part in text.trim().split(',') {
            let (lo, hi) = part.split_once('-').unwrap_or((part, part));
            let number = |s: &str| {
                s.parse::<u32>()
                    .ok()
                    .filter(|n| n.to_string() == s)
                    .ok_or_else(|| invalid("placement.cpus", "expected a cpulist"))
            };
            let (lo, hi) = (number(lo)?, number(hi)?);
            if lo > hi {
                return Err(invalid("placement.cpus", "reversed CPU interval"));
            }
            ranges.push((lo, hi));
        }
        ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::new();
        for (lo, hi) in ranges {
            if let Some(last) = merged.last_mut()
                && u64::from(lo) <= u64::from(last.1) + 1
            {
                last.1 = last.1.max(hi);
            } else {
                merged.push((lo, hi));
            }
        }
        Ok(Self(merged))
    }
    pub fn intersection(&self, other: &Self) -> Self {
        let (mut i, mut j) = (0, 0);
        let mut ranges = Vec::new();
        while i < self.0.len() && j < other.0.len() {
            let (a, b) = self.0[i];
            let (c, d) = other.0[j];
            if a.max(c) <= b.min(d) {
                ranges.push((a.max(c), b.min(d)));
            }
            if b < d { i += 1 } else { j += 1 }
        }
        Self(ranges)
    }
    pub fn len(&self) -> u64 {
        self.0
            .iter()
            .map(|&(lo, hi)| u64::from(hi) - u64::from(lo) + 1)
            .sum()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn ranges(&self) -> &[(u32, u32)] {
        &self.0
    }
}

/// Launcher authority, outside the run record. Invisible ancestors must be folded
/// into max_cores by the manager. Pinning needs a separate exclusive cpuset grant.
#[derive(Clone, Debug)]
pub struct HostGrant {
    pub max_cores: u64,
    pub cpus: CpuSet,
}

/// A positive rational quota, compared exactly (no floating-point rounding).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Quota {
    quota: u64,
    period: u64,
}
impl Quota {
    /// Port of Go internal/runtime/cgroup/cgroup.go:72-107 and its test vectors
    /// (ebe2feefc400), with positive periods enforced and exact integer arithmetic.
    pub fn parse(text: &str) -> Result<Option<Self>, RunRefusal> {
        let bad = || {
            invalid(
                "cpu.max",
                "expected quota|max and a positive period, followed by newline",
            )
        };
        let text = text.strip_suffix('\n').ok_or_else(bad)?;
        let (quota, period) = text.split_once(' ').ok_or_else(bad)?;
        let positive = |s: &str| {
            s.parse::<u64>()
                .ok()
                .filter(|n| *n > 0 && n.to_string() == s)
                .ok_or_else(bad)
        };
        let period = positive(period)?;
        if quota == "max" {
            return Ok(None);
        }
        Ok(Some(Self {
            quota: positive(quota)?,
            period,
        }))
    }
    /// Whole compute workers covered without fractional oversubscription.
    pub fn floor(self) -> u64 {
        self.quota / self.period
    }
    pub fn ceil(self) -> u64 {
        self.quota.div_ceil(self.period)
    }
    pub fn minimum(self, other: Self) -> Self {
        if u128::from(self.quota) * u128::from(other.period)
            <= u128::from(other.quota) * u128::from(self.period)
        {
            self
        } else {
            other
        }
    }
}

#[derive(Clone, Debug)]
pub struct HostLimits {
    /// Original admission-thread view, preserved across worker pinning.
    pub admission_affinity: CpuSet,
    pub effective_cpuset: CpuSet,
    pub quota: Option<Quota>,
    /// Cumulative cpu.stat value for the run's cgroup.
    pub throttled_usec: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resolution {
    pub requested: Cores,
    /// Target for the executor; actual effective/pending widths belong to it.
    pub target: u64,
    pub hard_limit: u64,
    pub quota: Option<Quota>,
    pub cpus: CpuSet,
    pub cpus_conflict: bool,
}

/// `external` is only for a running launch. Admission and control validation must
/// use false, so an empty request refuses rather than overriding the request.
pub fn resolve(
    placement: &Placement,
    grant: &HostGrant,
    limits: &HostLimits,
    external: bool,
) -> Result<Resolution, RunRefusal> {
    placement.validate()?;
    let hard = limits
        .admission_affinity
        .intersection(&limits.effective_cpuset)
        .intersection(&grant.cpus);
    let requested = placement.cpus.as_deref().map(CpuSet::parse).transpose()?;
    let selected = requested
        .as_ref()
        .map_or_else(|| hard.clone(), |cpus| hard.intersection(cpus));
    let conflict = external && selected.is_empty() && !hard.is_empty();
    let cpus = if conflict { hard } else { selected };
    let h = cpus.len().min(grant.max_cores);
    if h == 0 {
        return Err(invalid(
            "placement.cpus",
            "empty host CPU intersection or zero host grant",
        ));
    }
    let auto = limits.quota.map_or(h, |q| q.ceil().min(h));
    let target = match placement.cores {
        Cores::Fixed(n) => n.min(h),
        Cores::Auto(_) => auto,
        Cores::Range(ref r) => auto.clamp(r.min, r.max).min(h),
    };
    Ok(Resolution {
        requested: placement.cores.clone(),
        target,
        hard_limit: h,
        quota: limits.quota,
        cpus,
        cpus_conflict: conflict,
    })
}

/// One-second recheck seam. The caller uses a monotonic time since launch and
/// supplies fresh limits/grants. No thread, timer, permit or worker is owned here.
pub struct CoreResolver {
    placement: Placement,
    resolution: Resolution,
    checked_at: Duration,
}
impl CoreResolver {
    pub fn admit(
        placement: Placement,
        grant: &HostGrant,
        limits: &HostLimits,
        now: Duration,
    ) -> Result<Self, RunRefusal> {
        let resolution = resolve(&placement, grant, limits, false)?;
        Ok(Self {
            placement,
            resolution,
            checked_at: now,
        })
    }
    pub fn resolution(&self) -> &Resolution {
        &self.resolution
    }
    pub fn refresh(
        &mut self,
        now: Duration,
        read: impl FnOnce() -> Result<(HostGrant, HostLimits), RunRefusal>,
    ) -> Result<bool, RunRefusal> {
        if now.saturating_sub(self.checked_at) < Duration::from_secs(1) {
            return Ok(false);
        }
        self.checked_at = now;
        let (grant, limits) = read()?;
        let next = resolve(&self.placement, &grant, &limits, true)?;
        let changed = next != self.resolution;
        self.resolution = next;
        Ok(changed)
    }
    /// Whole-request validation before publication; failed control leaves state intact.
    pub fn replace(
        &mut self,
        placement: Placement,
        grant: &HostGrant,
        limits: &HostLimits,
    ) -> Result<(), RunRefusal> {
        let next = resolve(&placement, grant, limits, false)?;
        self.placement = placement;
        self.resolution = next;
        Ok(())
    }
}

/// A bounded live control transaction. Destination and executor-policy changes
/// require a fresh admission; only live core/CPU selection is mutable here.
pub enum ControlRequest {
    Metrics,
    Placement {
        cores: Option<crate::Cores>,
        cpus: Option<Option<String>>,
    },
}
impl ControlRequest {
    pub fn from_json(bytes: &[u8]) -> Result<Self, RunRefusal> {
        crate::integer_tokens(bytes)?;
        use serde::Deserialize;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Request {
            #[serde(default, deserialize_with = "present")]
            placement: Option<Patch>,
            #[serde(default, deserialize_with = "present")]
            metrics: Option<bool>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Patch {
            #[serde(default, deserialize_with = "present")]
            cores: Option<crate::Cores>,
            #[serde(default, deserialize_with = "present")]
            cpus: Option<Option<String>>,
        }
        // Preserve omitted vs explicit null; Cores itself still refuses null.
        fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
            d: D,
        ) -> Result<Option<T>, D::Error> {
            T::deserialize(d).map(Some)
        }
        let request: Request =
            serde_json::from_slice(bytes).map_err(|e| invalid("control", &e.to_string()))?;
        match (request.placement, request.metrics) {
            (None, Some(true)) => Ok(Self::Metrics),
            (Some(Patch { cores, cpus }), None) => Ok(Self::Placement { cores, cpus }),
            _ => Err(invalid("control", "expected placement or metrics: true")),
        }
    }
}
