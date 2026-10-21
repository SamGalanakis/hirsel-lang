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
use hirsel_engine::{CheckOutput, LoadedProgram, ToolError, Verify};
use std::cell::{Cell, RefCell};
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

/// Input work at the resident boundary, independent of query counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionInputWork {
    pub polled_files: u64,
    pub polled_bytes: u64,
    pub applied_sources: u64,
    pub applied_bytes: u64,
}

pub struct Session {
    entry: PathBuf,
    verify: Verify,
    config: Option<hirsel_engine::CompileConfig>,
    program: Option<LoadedProgram>,
    /// Every file the loaded program depends on, with the bytes it was loaded
    /// or last applied with (None: absent). Sources are replaced in place;
    /// any other file's change loads again.
    seen: BTreeMap<PathBuf, (bool, Option<Vec<u8>>)>,
    /// The last check's output and the revision it checked: a check at the
    /// same revision is the same check.
    checked: Option<(u64, CheckOutput)>,
    stats: SessionStats,
    input_work: Cell<SessionInputWork>,
    loaded_revision: u64,
    touched_before_apply: Option<u64>,
}

impl Session {
    /// A session over the program `entry` names (a source or a launch
    /// manifest), loaded on first use.
    pub fn open(entry: &Path, verify: Verify) -> Self {
        Self {
            entry: entry.to_path_buf(),
            verify,
            config: None,
            program: None,
            seen: BTreeMap::new(),
            checked: None,
            stats: SessionStats::default(),
            input_work: Cell::new(SessionInputWork::default()),
            loaded_revision: 0,
            touched_before_apply: None,
        }
    }
    /// A resident host's explicit compiler policy and shared result provider.
    /// Retention and lowering follow this config, including an explicit Batch
    /// or zero-byte allowance; `open` keeps its ordinary Resident defaults.
    pub fn open_configured(entry: &Path, config: hirsel_engine::CompileConfig) -> Self {
        let mut session = Self::open(entry, config.verify);
        session.config = Some(config);
        session
    }
    pub fn entry(&self) -> &Path {
        &self.entry
    }
    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }
    pub fn input_work(&self) -> SessionInputWork {
        self.input_work.get()
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
                let mut work = self.input_work.get();
                work.polled_files += 1;
                work.polled_bytes += now.as_ref().map_or(0, |bytes| bytes.len() as u64);
                self.input_work.set(work);
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
        if changes.iter().all(|change| {
            self.seen
                .get(&change.path)
                .is_some_and(|(_, bytes)| *bytes == change.bytes)
        }) {
            return self.stats.revision;
        }
        if self.touched_before_apply.is_none() {
            self.touched_before_apply = self
                .program
                .as_ref()
                .map(|program| program.engine.memo_stats().touched);
        }
        self.stats.revision += 1;
        let Some(program) = &mut self.program else {
            return self.stats.revision;
        };
        for change in changes {
            if self
                .seen
                .get(&change.path)
                .is_some_and(|(_, bytes)| *bytes == change.bytes)
            {
                continue;
            }
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
            let mut work = self.input_work.get();
            work.applied_sources += 1;
            work.applied_bytes += change.bytes.as_ref().map_or(0, |bytes| bytes.len() as u64);
            self.input_work.set(work);
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
        self.program_applied()
    }
    /// The explicitly applied snapshot, without reading unrelated disk inputs.
    /// The caller owns change delivery for every watched source and loader input.
    pub fn program_applied(&mut self) -> Result<&mut LoadedProgram, ToolError> {
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
        let compiler = crate::COMPILER
            .with_verify(self.verify)
            .with_retention(hirsel_engine::memo::Retention::Resident);
        let compiler = match &self.config {
            Some(config) => compiler.with_config(config.clone()),
            None => compiler,
        };
        let program = match compiler.load_program(&self.entry) {
            Ok(program) => program,
            Err(error) => {
                // A first manifest load can fail after reading an imported
                // source. Keep its actual source snapshot so repairing that
                // file wakes an event host as well as changing the manifest.
                if let ToolError::Query { sources, .. } = &error {
                    for (_, file, _) in sources.anchors() {
                        if let Some(source) = sources.file(file) {
                            self.seen.insert(
                                source.path.clone(),
                                (true, Some(source.text().as_bytes().to_vec())),
                            );
                        }
                    }
                }
                return Err(error);
            }
        };
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
        let changes = self.poll();
        self.apply(changes);
        self.check_applied()
    }
    /// Check the explicitly applied snapshot without polling input bytes;
    /// an event/request host must deliver all input changes through `apply`.
    pub fn check_applied(&mut self) -> Result<CheckOutput, ToolError> {
        let before = self.touched_before_apply.take().unwrap_or_else(|| {
            self.program
                .as_ref()
                .map_or(0, |p| p.engine.memo_stats().touched)
        });
        let loads = self.stats.loads;
        self.stats.touched = 0;
        self.program_applied()?;
        let before = if self.stats.loads == loads { before } else { 0 };
        let revision = self.stats.revision;
        if let Some((checked, output)) = &self.checked
            && *checked == revision
        {
            return Ok(output.clone());
        }
        let program = self.program.as_mut().unwrap();
        let output = program.check_configured();
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
