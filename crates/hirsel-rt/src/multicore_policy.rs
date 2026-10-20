//! M0b's measured defaults for the executors and core manager.
//!
//! These are unobservable placement choices, never an early-commit eligibility
//! check. E1 must still prove all four rules before each early commit. Its
//! initial probe and periodic probes supply the samples below; K1 retains the
//! width-1 default until the differential gate passes. See the checked-in
//! `reports/mc-profile-2-2026-10-20/REPORT.md` for measurement and ownership.

/// The view representation selected using refresh cost and workload frequency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewOption {
    /// Stable task view, copying the worker's 64 page pointers on stamp mismatch.
    InlineEpochCopy,
    /// Stable task view pointing to its current worker's page-pointer array.
    PagesIndirection,
}

/// B1 placement-model migration rates stay below 10%; retain the inline ABI.
pub const SELECTED_VIEW: ViewOption = ViewOption::InlineEpochCopy;
/// The current ABI still exposes inline pages until the approved ABI lane lands.
pub const ACTIVE_VIEW: ViewOption = ViewOption::InlineEpochCopy;
/// Above this percentage of added ping-pong time, select an indirect pages view.
pub const VIEW_REFRESH_LIMIT_PERCENT: u64 = 10;
/// Conservative round-up of eight times the measured 8-core baton handoff.
pub const AUTO_MIN_SEGMENT_NS: u64 = 2_000;
/// Require at least one in four eligibility probes to pass the conflict model.
pub const AUTO_MIN_PASS_PERCENT: u64 = 25;
/// Require measured saved work to exceed speculation/role overhead by 10%.
pub const AUTO_GAIN_MARGIN_PERCENT: u64 = 10;
/// Probe again after each window, so a temporarily conflicting shape can recover.
pub const AUTO_WINDOW_TURNS: u64 = 64;
/// Bound speculative deferrable ops per task; this does not relax rule 1 for E.
pub const AUTO_RUN_AHEAD_OPS: u64 = 32;
/// Manager growth stops at the design's scheduler-role saturation boundary.
pub const AUTO_ROLE_BUSY_PERCENT: u64 = 80;

/// One bounded window's measured shape. Times are from this run only.
#[derive(Clone, Copy, Debug, Default)]
pub struct AutoSample {
    pub segment_ns: u64,
    pub probes: u64,
    pub eligible: u64,
    pub saved_ns: u64,
    pub overhead_ns: u64,
}

/// Whether `reorder: auto` should speculate in the next window.
///
/// Empty, short, conflicting or losing windows use strict commit. Executors
/// keep doing periodic bounded probes even while this returns false. The
/// result changes performance only; it conveys no permission to commit an op.
pub fn reorder(sample: AutoSample) -> bool {
    sample.segment_ns >= AUTO_MIN_SEGMENT_NS
        && sample.probes != 0
        && sample.eligible <= sample.probes
        && u128::from(sample.eligible) * 100
            >= u128::from(sample.probes) * u128::from(AUTO_MIN_PASS_PERCENT)
        && sample.saved_ns != 0
        && u128::from(sample.saved_ns) * 100
            >= u128::from(sample.overhead_ns) * (100 + u128::from(AUTO_GAIN_MARGIN_PERCENT))
}

/// Whether the manager may grow after ready delay rises; hard core and quota
/// limits, permit admission and the differential gate remain independent.
pub fn may_grow(ready_delay_rising: bool, role_busy_percent: u64) -> bool {
    ready_delay_rising && role_busy_percent < AUTO_ROLE_BUSY_PERCENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_conflicting_and_losing_windows_fall_back() {
        let good = AutoSample {
            segment_ns: 2_000,
            probes: 64,
            eligible: 16,
            saved_ns: 1_100,
            overhead_ns: 1_000,
        };
        assert!(reorder(good));
        assert!(!reorder(AutoSample {
            segment_ns: 1_999,
            ..good
        }));
        assert!(!reorder(AutoSample {
            eligible: 15,
            ..good
        }));
        assert!(!reorder(AutoSample {
            saved_ns: 1_099,
            ..good
        }));
        assert!(!reorder(AutoSample { probes: 0, ..good }));
        assert!(!reorder(AutoSample {
            eligible: 65,
            ..good
        }));
        assert!(reorder(AutoSample {
            eligible: u64::MAX,
            probes: u64::MAX,
            saved_ns: u64::MAX,
            overhead_ns: 0,
            ..good
        }));
    }

    #[test]
    fn role_saturation_blocks_growth_and_requires_ready_pressure() {
        assert!(may_grow(true, 79));
        assert!(!may_grow(true, 80));
        assert!(!may_grow(false, 0));
    }
}
