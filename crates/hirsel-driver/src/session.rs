//! Filesystem and event delivery over the shared immutable compiler graph.
//! Sessions retain observations, policy and work metadata. Semantic retention
//! and invalidation belong to engine-selected queries for fresh and resident use.
use hirsel_engine::Verify;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

/// One changed file: its new bytes, or None when it no longer exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileChange {
    pub path: PathBuf,
    pub bytes: Option<Vec<u8>>,
}

/// What a session did, for tests and `--timings`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionStats {
    /// The source revision: one more for each `apply` that changed something.
    pub revision: u64,
    /// Input-footprint discoveries (the first, then after loader changes).
    pub loads: u64,
    /// Source observations replaced by an explicit edit or disk refresh.
    pub replaced: u64,
    /// Distinct query memos read or written since the first apply through the last check (O3).
    pub touched: u64,
    /// Every raw memo operation in that interval, including repeated operations on one memo.
    pub memo_operations: u64,
}

/// Input work at the resident boundary, independent of query counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionInputWork {
    pub polled_files: u64,
    pub polled_bytes: u64,
    pub applied_sources: u64,
    pub applied_bytes: u64,
}

/// The ordinary host facade over the engine-owned compiler request selector.
pub type Session = crate::resident::ObservedSession<crate::requests::CompilerRequests>;

impl Session {
    /// Capture host policy now; observe source inputs lazily on first request.
    pub fn open(entry: &Path, verify: Verify) -> Self {
        let compiler = crate::COMPILER
            .with_verify(verify)
            .with_retention(hirsel_engine::memo::Retention::Resident);
        Self::open_configured(entry, compiler.configuration())
    }
    /// Explicit stores, lowering, jobs and zero-byte policies remain exact.
    pub fn open_configured(entry: &Path, config: hirsel_engine::CompileConfig) -> Self {
        Self::open_with_compiler(entry, crate::COMPILER.with_config(config))
    }
    /// An explicit compiler factory and policy, captured before observation.
    pub fn open_with_compiler(entry: &Path, compiler: hirsel_engine::Compiler) -> Self {
        let config = compiler.configuration();
        let compiler = compiler.with_config(config.clone()).with_owned_engines();
        let engine = crate::requests::CompilerRequests::for_host(compiler.clone(), config.clone());
        let limits = engine.input_limits();
        let attempts = engine.capture_attempts();
        Self::new(entry.to_owned(), compiler, config, engine, limits, attempts)
    }
}

/// The sessions a resident host keeps, by (working directory, entry as
/// given, verification), most recently used last. The entry keeps its
/// spelling: a standalone source's diagnostics print the path as given.
struct Registry {
    sessions: Vec<(Key, Session)>,
    limit: usize,
}
thread_local! {
    static RESIDENT: RefCell<Option<Registry>> = const { RefCell::new(None) };
}

/// Keep up to `limit` sessions on this thread from now on: the path entry
/// points lease theirs from here instead of loading afresh. `hirsel serve`
/// turns it on; one-shot processes never do.
pub fn enable_resident(limit: usize) {
    RESIDENT.with(|r| {
        *r.borrow_mut() = Some(Registry {
            sessions: vec![],
            limit: limit.max(1),
        })
    });
}
/// Current logical input paths, including absent manifests and sidecars.
/// Event hosts register these before comparing bytes; event metadata alone
/// never establishes source identity.
pub fn resident_watched() -> Vec<PathBuf> {
    RESIDENT.with(|r| {
        r.borrow().as_ref().map_or_else(Vec::new, |registry| {
            registry
                .sessions
                .iter()
                .flat_map(|(_, session)| session.watched().map(Path::to_path_buf))
                .collect()
        })
    })
}
/// Whether a file of any kept session changed since it was last used.
pub fn resident_changed() -> bool {
    RESIDENT.with(|r| {
        r.borrow()
            .as_ref()
            .is_some_and(|r| r.sessions.iter().any(|(_, s)| !s.poll().is_empty()))
    })
}
pub fn resident() -> bool {
    RESIDENT.with(|r| r.borrow().is_some())
}

/// Fresh and resident callers use the same immutable request selector. The
/// registry changes only the lifetime of the input cursor and shared host.
pub(crate) fn with_session<R>(path: &Path, verify: Verify, f: impl FnOnce(&mut Session) -> R) -> R {
    let mut session = take(path, verify).unwrap_or_else(|| Session::open(path, verify));
    let result = f(&mut session);
    put(path, verify, session);
    result
}

pub(crate) fn request(
    path: &Path,
    verify: Verify,
    operation: crate::requests::CompileOperation,
) -> Result<std::sync::Arc<crate::requests::CompileOutput>, hirsel_engine::ToolError> {
    with_session(path, verify, |session| session.request(operation))
}

/// `path`'s session out of the registry (a new one if it has none), or None
/// when this thread keeps no sessions. A session is taken while in use so a
/// nested lease of the same program loads its own.
fn take(path: &Path, verify: Verify) -> Option<Session> {
    let key = key(path, verify);
    RESIDENT.with(|r| {
        let mut r = r.borrow_mut();
        let registry = r.as_mut()?;
        Some(
            match registry.sessions.iter().position(|(k, _)| *k == key) {
                Some(i) => registry.sessions.remove(i).1,
                None => Session::open(path, verify),
            },
        )
    })
}
fn put(path: &Path, verify: Verify, session: Session) {
    let key = key(path, verify);
    RESIDENT.with(|r| {
        if let Some(registry) = r.borrow_mut().as_mut() {
            registry.sessions.push((key, session));
            let over = registry.sessions.len().saturating_sub(registry.limit);
            registry.sessions.drain(..over);
        }
    });
}
type Key = (PathBuf, PathBuf, bool);
fn key(path: &Path, verify: Verify) -> Key {
    let directory = std::env::current_dir().unwrap_or_default();
    (directory, path.to_path_buf(), verify == Verify::On)
}
