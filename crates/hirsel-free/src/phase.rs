//! Op phases and published progress (design §5 Op phases).
//!
//! Every phase of an op is one op section and publishes the task's progress as (k, phase, i):
//! k is the op's ordinal (semantic ops of the task, counted from 1), and i numbers repeated
//! phases. A retry or a wake never changes k. Progress (0, done) is a task that has completed
//! no op: its admission runs nothing, which loses nothing because a segment has no shared
//! effect.

use std::cmp::Ordering;
use std::fmt;

/// A phase of an op.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Phase {
    /// Every op: applies the task's pending releases; a host request also takes its issue
    /// positions and writes `Prepared`.
    Entered,
    /// A host request: the CAS `Prepared -> ClaimedForIssue`, then the host call outside it.
    Claimed,
    /// A host request waiting for readiness: attempt i (i >= 1) takes positions for one more
    /// bounded host call.
    Attempt(u32),
    /// A host request: the bounded call of the claim (i = 0) or of attempt i returned or
    /// registered its wait, and its positions end.
    Ended(u32),
    /// A task end: applies the releases the last freeing walk deferred (i >= 1).
    Cleanup(u32),
    /// Every op: the action that fixes the outcome.
    Done,
}

impl Phase {
    /// The phase's place among its op's phases, distinct for distinct phases. An op has either
    /// request phases (claimed, attempts and ends) or cleanup phases (a task end), never both,
    /// so placing every cleanup after every request phase orders both kinds correctly.
    fn rank(self) -> u64 {
        match self {
            Phase::Entered => 0,
            Phase::Claimed => 1,
            Phase::Attempt(i) => 2 * u64::from(i) + 1,
            Phase::Ended(i) => 2 * u64::from(i) + 2,
            Phase::Cleanup(i) => (1 << 40) + u64::from(i),
            Phase::Done => u64::MAX,
        }
    }
    fn code(self) -> (u64, u32) {
        match self {
            Phase::Entered => (0, 0),
            Phase::Claimed => (1, 0),
            Phase::Attempt(i) => (2, i),
            Phase::Ended(i) => (3, i),
            Phase::Cleanup(i) => (4, i),
            Phase::Done => (5, 0),
        }
    }
    fn from_code(code: u64, index: u32) -> Option<Phase> {
        Some(match (code, index) {
            (0, 0) => Phase::Entered,
            (1, 0) => Phase::Claimed,
            (2, i) if i >= 1 => Phase::Attempt(i),
            (3, i) => Phase::Ended(i),
            (4, i) if i >= 1 => Phase::Cleanup(i),
            (5, 0) => Phase::Done,
            _ => return None,
        })
    }
}

/// A task's published progress: the latest phase whose section closed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Progress {
    pub op: u64,
    pub phase: Phase,
}

/// Bits of the packed word below the ordinal: 3 for the phase, 21 for its index.
const INDEX_BITS: u32 = 21;
const LOW_BITS: u32 = 3 + INDEX_BITS;
/// The largest ordinal and phase index a packed word holds.
pub const MAX_OP: u64 = (1 << (64 - LOW_BITS)) - 1;
pub const MAX_INDEX: u32 = (1 << INDEX_BITS) - 1;

impl Progress {
    /// A task that has completed no op.
    pub const START: Progress = Progress {
        op: 0,
        phase: Phase::Done,
    };

    pub fn new(op: u64, phase: Phase) -> Progress {
        Progress { op, phase }
    }

    /// One word, for publication with a single store.
    pub fn pack(self) -> u64 {
        let (code, index) = self.phase.code();
        assert!(
            self.op <= MAX_OP && index <= MAX_INDEX,
            "progress {self} out of range"
        );
        self.op << LOW_BITS | code << INDEX_BITS | u64::from(index)
    }

    pub fn unpack(word: u64) -> Option<Progress> {
        let phase = Phase::from_code(word >> INDEX_BITS & 7, (word & u64::from(MAX_INDEX)) as u32)?;
        Some(Progress {
            op: word >> LOW_BITS,
            phase,
        })
    }

    /// Whether a cut at this progress admits `other`: it is at or before this progress.
    pub fn admits(self, other: Progress) -> bool {
        other <= self
    }
}

impl Ord for Progress {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.op, self.phase.rank()).cmp(&(other.op, other.phase.rank()))
    }
}
impl PartialOrd for Progress {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.phase {
            Phase::Entered => write!(f, "({}, entered)", self.op),
            Phase::Claimed => write!(f, "({}, claimed)", self.op),
            Phase::Attempt(i) => write!(f, "({}, attempt {i})", self.op),
            Phase::Ended(i) => write!(f, "({}, ended {i})", self.op),
            Phase::Cleanup(i) => write!(f, "({}, cleanup {i})", self.op),
            Phase::Done => write!(f, "({}, done)", self.op),
        }
    }
}
