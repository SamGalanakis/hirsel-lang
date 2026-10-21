//! The compiler switches the oracles need and the compiler does not have yet.
//!
//! A run that needs a missing switch reports `Verdict::Todo(switch)`, never a
//! pass. The lane that lands a switch makes `available` return true for it and
//! replaces the oracle's `Err(Switch::..)` at the use site named in `needs` with
//! the real call, so that oracle cell turns green (or red) on its own.
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Switch {
    /// `CompileConfig.jobs` over `hirsel_intern::jobs::map`.
    Jobs,
    /// `hirsel_intern::salt_ids` under the `id-salt` feature.
    IdSalt,
    /// The resident `Session` (`open`, `apply`, `check`).
    Session,
    /// The `dir@1` `ResultStore`, shared by two processes.
    DirStore,
    /// `CompileConfig.verify_store = VerifyStore::Recompute`.
    VerifyStore,
    /// The CLI's `--verify-cache`.
    VerifyCache,
    /// `Verify::Off` (`--no-verify`).
    VerifyOff,
    /// The slow-path counters: why each memo was invalidated, by `ReadKey`
    /// kind, with V's own recomputes.
    SlowPathCounters,
    /// `SessionStats.touched`.
    Touched,
}

impl Switch {
    pub const ALL: [Switch; 9] = [
        Switch::Jobs,
        Switch::IdSalt,
        Switch::Session,
        Switch::DirStore,
        Switch::VerifyStore,
        Switch::VerifyCache,
        Switch::VerifyOff,
        Switch::SlowPathCounters,
        Switch::Touched,
    ];

    /// Whether this compiler has the switch. Flipped by the lane that lands it.
    pub fn available(self) -> bool {
        match self {
            Switch::Jobs
            | Switch::IdSalt
            | Switch::Session
            | Switch::DirStore
            | Switch::VerifyStore
            | Switch::VerifyOff => true,
            Switch::VerifyCache | Switch::SlowPathCounters | Switch::Touched => false,
        }
    }

    /// The lane that owns the switch (design.md §4).
    pub fn lane(self) -> &'static str {
        match self {
            Switch::Jobs => "ids, arch-engine",
            Switch::IdSalt => "ids",
            Switch::Session
            | Switch::DirStore
            | Switch::VerifyStore
            | Switch::SlowPathCounters
            | Switch::Touched => "arch-engine",
            Switch::VerifyCache => "arch-engine, arch-native",
            Switch::VerifyOff => "policy",
        }
    }

    /// What the oracle calls once the switch exists, and where.
    pub fn needs(self) -> &'static str {
        match self {
            Switch::Jobs => "produce: run with CompileConfig.jobs = Jobs::new(n)",
            Switch::IdSalt => {
                "produce: build with the id-salt feature and call salt_ids(seed) before any interning"
            }
            Switch::Session => {
                "o2: Session::open on the before tree, apply the edit, check; o3: count through the Session"
            }
            Switch::DirStore => {
                "produce --store dir:<path>: a dir@1 ResultStore at <path>, shared by a second process"
            }
            Switch::VerifyStore => {
                "produce --verify-store: CompileConfig.verify_store = VerifyStore::Recompute"
            }
            Switch::VerifyCache => "o4: hirsel check and build with --verify-cache",
            Switch::VerifyOff => "produce --verify off: CompileConfig.verify = Verify::Off",
            Switch::SlowPathCounters => {
                "o3: per-definition invalidations by ReadKey kind, including V recomputes"
            }
            Switch::Touched => "o3: SessionStats.touched after the edit",
        }
    }
}

impl fmt::Display for Switch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TODO({}): {:?}: {}", self.lane(), self, self.needs())
    }
}

/// The first switch `wanted` names that this compiler does not have.
pub fn missing(wanted: impl IntoIterator<Item = Switch>) -> Result<(), Switch> {
    wanted
        .into_iter()
        .find(|switch| !switch.available())
        .map_or(Ok(()), Err)
}
