//! `programs-v1`: whole-program build products by the program's identity, so an
//! unchanged `hirsel run --native` or `hirsel build` runs no compiler phase.
//!
//! Every entry is one file named `<kind>-<key>.<extension>`, where the key is a hex
//! SHA-256 of everything its bytes depend on: the compiler's identity, V, every file
//! loading the program reads (`Compiler::program_identity`), the backend, optimization
//! level and target, the link toolchain's files (`toolchain_identity`) and, for an
//! image, the run it binds. Kinds:
//!
//! | entry | holds |
//! |---|---|
//! | `f-<program>.json` | the launch facts a run binds against (`LaunchFacts`) |
//! | `s-<program>.json` | the site table a run's observation reads (`SiteTable`) |
//! | `i-<run>.so` | a program image linked for one run and runner |
//! | `b-<build>.json`, `b-<build>.debug`, `b-<build>.exe` | a `hirsel build` executable and its debug file |
//!
//! Writers write a temporary file and rename it into place, so a reader never sees a
//! partial entry and no lock is taken; two writers of one key write the same bytes. A
//! hit refreshes its entry's modification time. Past `CAP` bytes the least recently
//! used entries are removed, except one a run holds (`runner::hold`) or one used in
//! the last `GRACE`. A missing, unreadable or malformed entry is a miss, never an
//! error. The cache lives under `$HIRSEL_NATIVE_CACHE` when that is set (test
//! harnesses isolate it so), else `$XDG_CACHE_HOME/hirsel` or `~/.cache/hirsel`;
//! `HIRSEL_PROGRAM_CACHE=off` disables it.
use super::link::Toolchain;
use super::stages::{NativeError, StageName};
use hirsel_syntax::ContentHash;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// The format version in the directory's name.
pub(crate) const FORMAT: u32 = 1;
/// The bytes the cache keeps before it removes its least recently used entries.
const CAP: u64 = 2 << 30;
/// An entry used this recently is never removed: a reader between its lookup and
/// its use keeps it.
const GRACE: Duration = Duration::from_secs(60);
/// A temporary file older than this was left by a writer that died.
const ABANDONED: Duration = Duration::from_secs(3600);

/// The cache directory, if it can be used.
#[derive(Clone, Debug)]
pub(crate) struct ProgramCache {
    root: PathBuf,
}

impl ProgramCache {
    /// The process's cache, or `None` when it is disabled or has no directory.
    pub(crate) fn open() -> Option<Self> {
        if std::env::var_os("HIRSEL_PROGRAM_CACHE").is_some_and(|v| v == "off") {
            return None;
        }
        let base = match std::env::var_os("HIRSEL_NATIVE_CACHE") {
            Some(native) => PathBuf::from(native),
            None => std::env::var_os("XDG_CACHE_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME")
                        .filter(|v| !v.is_empty())
                        .map(|home| PathBuf::from(home).join(".cache"))
                })?
                .join("hirsel"),
        };
        Some(Self::at(base.join(format!("programs-v{FORMAT}"))))
    }

    pub(crate) fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// The path of entry `name` if it is present, now marked recently used.
    pub(crate) fn get(&self, name: &str) -> Option<PathBuf> {
        let path = self.root.join(name);
        let file = std::fs::File::options().append(true).open(&path).ok()?;
        let _ = file.set_modified(SystemTime::now());
        Some(path)
    }

    /// The bytes of entry `name`, if present.
    pub(crate) fn read(&self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(self.get(name)?).ok()
    }

    /// Store `bytes` as entry `name`. A failure to store is dropped: the cache only
    /// ever saves work.
    pub(crate) fn put(&self, name: &str, bytes: &[u8]) {
        let _ = self.publish(name, |temporary| std::fs::write(temporary, bytes));
    }

    /// Store a copy of the file `source` as entry `name`, with its permissions.
    pub(crate) fn put_file(&self, name: &str, source: &Path) {
        let _ = self.publish(name, |temporary| std::fs::copy(source, temporary).map(drop));
    }

    fn publish(
        &self,
        name: &str,
        write: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::fs::create_dir_all(&self.root)?;
        let temporary = self.root.join(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let written =
            write(&temporary).and_then(|()| std::fs::rename(&temporary, self.root.join(name)));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        self.reclaim(CAP);
        written
    }

    /// Remove the least recently used entries until the rest fit in `cap` bytes, and
    /// temporary files their writers abandoned.
    pub(crate) fn reclaim(&self, cap: u64) {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        let now = SystemTime::now();
        let age = |modified: SystemTime| now.duration_since(modified).unwrap_or_default();
        let mut kept = Vec::new();
        let mut total = 0;
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            if entry.file_name().to_string_lossy().starts_with('.') {
                if age(modified) > ABANDONED {
                    let _ = std::fs::remove_file(entry.path());
                }
                continue;
            }
            total += meta.len();
            kept.push((modified, meta.len(), entry.path()));
        }
        if total <= cap {
            return;
        }
        kept.sort();
        for (modified, len, path) in kept {
            if total <= cap {
                break;
            }
            if age(modified) < GRACE {
                continue;
            }
            // A run holds its image with a shared lock (`runner::hold`).
            if let Ok(file) = std::fs::File::open(&path)
                && file.try_lock().is_ok()
                && std::fs::remove_file(&path).is_ok()
            {
                total -= len;
            }
        }
    }
}

/// An entry key: lowercase hex of the SHA-256 of `parts` under `domain`.
pub(crate) fn key<'a>(domain: &'a str, parts: impl IntoIterator<Item = &'a [u8]>) -> String {
    hex(&ContentHash::of_parts(domain, parts).0)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What the link reads besides the program's objects, for `target`, by path, size and
/// modification time: the pinned `ld.lld`, the selected runtime archive (with its debug
/// variant), the builtins archive and every file of the link kit. Toolchains come in
/// content-addressed directories, so a changed file has another path or a newer time.
pub(crate) fn toolchain_identity(
    toolchain: &Toolchain,
    triple: &str,
) -> Result<ContentHash, NativeError> {
    let mut files = vec![toolchain.lld.clone()];
    files.extend(toolchain.archive(triple).map(Path::to_path_buf));
    files.extend(
        toolchain
            .runtime_debug
            .get(&(triple.to_owned(), toolchain.runtime_build))
            .cloned(),
    );
    files.push(toolchain.builtins_archive(triple)?.to_path_buf());
    let mut kit = Vec::new();
    walk(&toolchain.kit, &mut kit);
    kit.sort();
    files.extend(kit);
    let mut parts: Vec<Vec<u8>> = vec![
        triple.as_bytes().to_vec(),
        toolchain.runtime_build.variant().into_bytes(),
        vec![u8::from(toolchain.debug_info)],
    ];
    for file in files {
        let path = std::fs::canonicalize(&file).unwrap_or(file);
        let meta = std::fs::metadata(&path)
            .map_err(|e| NativeError::new(StageName::Link, format!("{}: {e}", path.display())))?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        parts.push(path.as_os_str().as_encoded_bytes().to_vec());
        parts.push(meta.len().to_le_bytes().to_vec());
        parts.push(modified.to_le_bytes().to_vec());
    }
    Ok(ContentHash::of_parts(
        "hirsel.program-cache.toolchain.v1",
        parts.iter().map(Vec::as_slice),
    ))
}

fn walk(path: &Path, files: &mut Vec<PathBuf>) {
    match std::fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                walk(&entry.path(), files);
            }
        }
        Err(_) => files.push(path.to_path_buf()),
    }
}

/// What binding a live launch reads from the checked program (`crate::bind_launch`):
/// its semantic revision, launch kind, entry roots and code reach. A cached program
/// binds from these, exactly as `hirsel_engine::bind_run` binds from the program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LaunchFacts {
    pub(crate) revision: hirsel_run_schema::ProgramRevision,
    pub(crate) kind: hirsel_run_schema::LaunchKind,
    pub(crate) roots: Vec<hirsel_run_config::Root>,
    pub(crate) code: hirsel_run_schema::CodeReach,
}

const ROOTS: [(hirsel_run_config::Root, &str); 5] = {
    use hirsel_run_config::Root;
    [
        (Root::Io, "io"),
        (Root::Args, "args"),
        (Root::Env, "env"),
        (Root::Unsafe, "unsafe"),
        (Root::Test, "test"),
    ]
};

impl LaunchFacts {
    /// The facts of `program`'s entry, as `bind_launch` finds them for a live launch.
    pub(crate) fn of(
        program: &mut hirsel_engine::LoadedProgram,
    ) -> Result<Self, hirsel_engine::ToolError> {
        use hirsel_engine::ToolError;
        let signature = crate::launch_signature(program)?;
        let kind = hirsel_engine::entry_launch_kind(&signature).map_err(ToolError::Launch)?;
        let roots = hirsel_engine::entry_roots(&signature, kind).map_err(ToolError::Launch)?;
        let revision = program
            .engine
            .semantic_program_revision(&program.entry)
            .map_err(|e| program.error(e))?;
        let code = match program.unsafe_reach()? {
            None => hirsel_run_schema::CodeReach::Safe,
            Some(reach) => hirsel_run_schema::CodeReach::Escapes(reach.spelled()),
        };
        Ok(Self {
            revision,
            kind,
            roots,
            code,
        })
    }

    /// Bind a live launch's `config`, as `bind_launch` binds it to the program.
    pub(crate) fn bind(
        &self,
        config: crate::LaunchConfig<'_>,
    ) -> Result<hirsel_run_config::BoundRun, hirsel_engine::ToolError> {
        use hirsel_engine::ToolError;
        let requested = config
            .read(&self.revision, self.kind)
            .map_err(|e| ToolError::Runtime(e.to_string()))?;
        requested
            .bind(&hirsel_run_config::ProgramFacts::of(
                &self.revision,
                self.kind,
                self.roots.clone(),
                self.code.clone(),
            ))
            .map_err(ToolError::Launch)
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let roots: Vec<&str> = self
            .roots
            .iter()
            .map(|root| {
                ROOTS
                    .iter()
                    .find(|(r, _)| r == root)
                    .expect("every root is named")
                    .1
            })
            .collect();
        let code = match &self.code {
            hirsel_run_schema::CodeReach::Safe => serde_json::Value::Null,
            hirsel_run_schema::CodeReach::Escapes(path) => serde_json::json!(path),
        };
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "entry": self.revision.entry(),
            "revision": hex(&self.revision.digest()),
            "launch": self.kind,
            "roots": roots,
            "code": code,
        }))
        .expect("launch facts are JSON")
    }

    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        if value["schema"] != 1 {
            return None;
        }
        let digest = value["revision"].as_str()?;
        let digest: Vec<u8> = (0..digest.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(digest.get(i..i + 2)?, 16).ok())
            .collect::<Option<_>>()?;
        let revision = hirsel_run_schema::ProgramRevision::recorded(
            value["entry"].as_str()?,
            digest.try_into().ok()?,
        );
        let kind = serde_json::from_value(value["launch"].clone()).ok()?;
        let roots = value["roots"]
            .as_array()?
            .iter()
            .map(|name| {
                let name = name.as_str()?;
                ROOTS
                    .iter()
                    .find(|(_, n)| *n == name)
                    .map(|(root, _)| *root)
            })
            .collect::<Option<_>>()?;
        let code = match &value["code"] {
            serde_json::Value::Null => hirsel_run_schema::CodeReach::Safe,
            path => {
                hirsel_run_schema::CodeReach::Escapes(serde_json::from_value(path.clone()).ok()?)
            }
        };
        Some(Self {
            revision,
            kind,
            roots,
            code,
        })
    }
}

#[cfg(test)]
mod tests;
