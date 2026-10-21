//! A resident compile session (design §1g): one loaded program kept across
//! edits, so a re-check recomputes only what the edit changed. `Session` is
//! the library API; `hirsel serve` and `hirsel check --watch` hold sessions,
//! and the path entry points (`check_path`, the native builds and runs) lease
//! one from the process's registry once a resident host enables it.
//!
//! A session's output is the output of a fresh process at the same source
//! state, byte for byte: `apply` gives the engine a new snapshot of each
//! changed source under its old logical name, so the engine's memos decide
//! what to recompute, and any change loading itself read (a manifest, a
//! sidecar appearing, a prechecked package's source) loads the program again.
use hirsel_engine::{CheckOutput, LoadedProgram, LoweringPolicy, ToolError, Verify};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    /// Fresh loads of the program (the first, then one per reload).
    pub loads: u64,
    /// Sources replaced in place.
    pub replaced: u64,
    /// Query memo reads/writes touched by the last check (O3).
    pub touched: u64,
}

pub struct Session {
    entry: PathBuf,
    verify: Verify,
    program: Option<LoadedProgram>,
    /// Every file the loaded program depends on, with the bytes it was loaded
    /// or last applied with (None: absent). Sources are replaced in place;
    /// any other file's change loads again.
    seen: BTreeMap<PathBuf, (bool, Option<Vec<u8>>)>,
    /// The last check's output and the revision it checked: a check at the
    /// same revision is the same check.
    checked: Option<(u64, CheckOutput)>,
    stats: SessionStats,
    loaded_revision: u64,
}

impl Session {
    /// A session over the program `entry` names (a source or a launch
    /// manifest), loaded on first use.
    pub fn open(entry: &Path, verify: Verify) -> Self {
        Self {
            entry: entry.to_path_buf(),
            verify,
            program: None,
            seen: BTreeMap::new(),
            checked: None,
            stats: SessionStats::default(),
            loaded_revision: 0,
        }
    }
    pub fn entry(&self) -> &Path {
        &self.entry
    }
    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }
    /// The files the session watches: the loaded program's sources and the
    /// manifests and sidecars loading read.
    pub fn watched(&self) -> impl Iterator<Item = &Path> {
        self.seen.keys().map(PathBuf::as_path)
    }
    /// The changes on disk since the session last loaded or applied: every
    /// watched file whose bytes differ.
    pub fn poll(&self) -> Vec<FileChange> {
        self.seen
            .iter()
            .filter_map(|(path, (_, seen))| {
                let now = std::fs::read(path).ok();
                (now != *seen).then(|| FileChange {
                    path: path.clone(),
                    bytes: now,
                })
            })
            .collect()
    }
    /// Apply `changes`. A source becomes a new snapshot of itself; anything
    /// else (a manifest, a vanished or undecodable source, a source of a
    /// prechecked package) makes the next use load the program again. Returns
    /// the new revision.
    pub fn apply(&mut self, changes: Vec<FileChange>) -> u64 {
        if changes.is_empty() {
            return self.stats.revision;
        }
        self.stats.revision += 1;
        let Some(program) = &mut self.program else {
            return self.stats.revision;
        };
        for change in changes {
            let source = self
                .seen
                .get(&change.path)
                .is_some_and(|(source, _)| *source);
            let text = change
                .bytes
                .as_ref()
                .and_then(|b| String::from_utf8(b.clone()).ok());
            let replaced = match text {
                Some(text) if source => program.replace_source(&change.path, text).unwrap_or(false),
                _ => false,
            };
            if !replaced {
                self.program = None;
                self.seen.clear();
                return self.stats.revision;
            }
            self.stats.replaced += 1;
            self.seen.insert(change.path, (true, change.bytes));
        }
        self.stats.revision
    }
    /// Apply what changed on disk, then the program at this revision, loading
    /// it if it is not loaded. A source that no longer parses fails here as it
    /// fails a fresh load.
    pub fn program(&mut self) -> Result<&mut LoadedProgram, ToolError> {
        let changes = self.poll();
        self.apply(changes);
        if self.stats.revision.saturating_sub(self.loaded_revision) >= 64 {
            self.program = None;
        }
        if self.program.is_none() {
            self.load()?;
        }
        // Manifest loading parses source packages before returning a program.
        // Keep that same error boundary after an in-place edit; standalone
        // sources instead report syntax diagnostics in CheckOutput.
        let mut sidecar = self.entry.as_os_str().to_os_string();
        sidecar.push(".json");
        if self.entry.extension().is_some_and(|ext| ext == "json") || Path::new(&sidecar).exists() {
            self.program.as_mut().unwrap().parse_sources()?;
        }
        Ok(self.program.as_mut().unwrap())
    }
    fn load(&mut self) -> Result<(), ToolError> {
        self.stats.loads += 1;
        self.loaded_revision = self.stats.revision;
        self.seen.clear();
        self.seen
            .insert(self.entry.clone(), (false, std::fs::read(&self.entry).ok()));
        let mut sidecar = self.entry.as_os_str().to_os_string();
        sidecar.push(".json");
        let sidecar = PathBuf::from(sidecar);
        self.seen
            .insert(sidecar.clone(), (false, std::fs::read(sidecar).ok()));
        let program = crate::COMPILER
            .with_verify(self.verify)
            .load_program(&self.entry)?;
        // A source is watched with the text the program holds, so an edit
        // racing the load is applied on the next use.
        for (file, _) in &program.files {
            let source = program.engine.sources.file(*file).unwrap();
            self.seen.insert(
                source.path.clone(),
                (true, Some(source.text().as_bytes().to_vec())),
            );
        }
        for input in &program.inputs {
            self.seen
                .insert(input.clone(), (false, std::fs::read(input).ok()));
        }
        self.program = Some(program);
        Ok(())
    }
    /// `hirsel check`: every package checked, lowering what compile-time
    /// evaluation executes.
    pub fn check(&mut self) -> Result<CheckOutput, ToolError> {
        let before = self
            .program
            .as_ref()
            .map_or(0, |p| p.engine.memo_stats().touched);
        let loads = self.stats.loads;
        self.stats.touched = 0;
        self.program()?;
        let before = if self.stats.loads == loads { before } else { 0 };
        let revision = self.stats.revision;
        if let Some((checked, output)) = &self.checked
            && *checked == revision
        {
            return Ok(output.clone());
        }
        let program = self.program.as_mut().unwrap();
        let output = program.check(LoweringPolicy::Comptime);
        self.stats.touched = program.engine.memo_stats().touched.saturating_sub(before);
        let output = output.map_err(|error| program.error(error))?;
        self.checked = Some((revision, output.clone()));
        Ok(output)
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
/// Whether a file of any kept session changed since it was last used.
pub fn resident_changed() -> bool {
    RESIDENT.with(|r| {
        r.borrow().as_ref().is_some_and(|r| {
            r.sessions
                .iter()
                .any(|(_, s)| s.program.is_none() || !s.poll().is_empty())
        })
    })
}
pub fn resident() -> bool {
    RESIDENT.with(|r| r.borrow().is_some())
}

/// Run `f` on the program `path` names: the resident session's, brought up
/// to date, when this thread keeps sessions, else a fresh load. The program's
/// entry is restored afterwards, since a launch may bind a callee entry.
pub(crate) fn with_program<R>(
    path: &Path,
    verify: Verify,
    f: impl FnOnce(&mut LoadedProgram) -> Result<R, ToolError>,
) -> Result<R, ToolError> {
    let Some(mut session) = take(path, verify) else {
        let mut program = crate::COMPILER.with_verify(verify).load_program(path)?;
        return f(&mut program);
    };
    let result = session.program().and_then(|program| {
        let entry = program.entry.clone();
        let result = f(program);
        program.entry = entry;
        result
    });
    put(path, verify, session);
    result
}
/// Run `f` on `path`'s resident session (None when this thread keeps none).
pub(crate) fn with_session<R>(
    path: &Path,
    verify: Verify,
    f: impl FnOnce(&mut Session) -> R,
) -> Option<R> {
    let mut session = take(path, verify)?;
    let result = f(&mut session);
    put(path, verify, session);
    Some(result)
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
