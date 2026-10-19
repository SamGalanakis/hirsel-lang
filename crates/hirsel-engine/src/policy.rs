//! The compile policies a session holds (whether V runs, which bodies lower) and
//! whether the process frees a finished session.
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether V, the second checker, validates lowered TIR. `On` is the default for
/// every command; `--no-verify` is the only way to select `Off`, and every
/// bug-catching harness refuses it. V never decides acceptance: `Off` changes
/// only how long a compile takes and which compiler bugs it can report.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Verify {
    #[default]
    On,
    Off,
}

/// Which executable bodies a compile lowers (and V validates). Lowering
/// reports source diagnostics only through compile-time evaluation (CT-1, CT-2,
/// CT-4), so both policies report the same diagnostics.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LoweringPolicy {
    /// `check`: exactly the bodies compile-time evaluation executes or
    /// materializes into: constants, bodies containing or reading a compile-time
    /// value, the functions evaluation calls, and the generated instances CT-5
    /// selects.
    Comptime,
    /// `run`, `test` and `build`: every body of the program's own package and
    /// the dependency bodies it reaches.
    Reachable,
}

static FREE_SESSIONS: AtomicBool = AtomicBool::new(true);

/// Whether a dropped session (an engine a `Compiler` opened) frees its memory.
/// The CLI, which exits right after its one session, turns this off: the
/// operating system reclaims the memory at exit faster than the session frees
/// it. Engines a session opens internally always free, and so does every
/// session under a heap meter (`--mem-usage`), which records what each frees.
pub fn free_sessions(free: bool) {
    FREE_SESSIONS.store(free, Ordering::Relaxed);
}

pub(crate) fn frees_sessions() -> bool {
    FREE_SESSIONS.load(Ordering::Relaxed) || hirsel_intern::memory::metered()
}
