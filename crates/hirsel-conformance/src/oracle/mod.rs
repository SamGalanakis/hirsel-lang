//! The compiler-architecture oracles (compiler-arch design §4): O1 jobs and salt
//! invariance, O2 warm equals cold over scripted edits, O3 recompute counts and
//! O4 the verified result store; and O5, analysis annotation invariance (PS-19). Each runs against today's compiler where it can;
//! a check that needs a switch the compiler lacks reports `Verdict::Todo` naming
//! the switch and its lane (`switches`), never a pass.
pub mod edits;
pub mod o1;
pub mod o2;
pub mod o3;
pub mod o4;
pub mod o5;
pub mod switches;

use hirsel_driver::native::{Backend, OptLevel, Options, Partition, TargetSpec};
use hirsel_engine::store::{DirStore, NoneStore, ResultStore, VerifyStore};
use hirsel_engine::{CheckCompletion, StdImage, StdImages, ToolError};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};
pub use switches::Switch;

/// The O2 and O3 edit scenarios, by oracle, relative to the repository.
pub const SCENARIOS: &str = "crates/hirsel-conformance/oracles";

/// One run's outputs by name: `diagnostics`, `tir`, and with `native`,
/// `receipts` and `objects/<backend>/<n>.o`. Two runs agree when the maps are
/// byte-identical.
pub type Artifacts = BTreeMap<String, Vec<u8>>;

/// How one run compiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub jobs: usize,
    pub salt: Option<u64>,
    /// `dir@1` at this path; `None` is a cold `none@1` run.
    pub store: Option<PathBuf>,
    pub verify_store: bool,
    pub verify: bool,
    /// Comptime limits (`steps`, `call_depth`), else the engine's defaults.
    pub limits: Option<Limits>,
    /// Also build at Ready and emit unoptimized objects with both backends.
    pub native: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub steps: Option<u64>,
    pub call_depth: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            jobs: 1,
            salt: None,
            store: None,
            verify_store: false,
            verify: true,
            limits: None,
            native: false,
        }
    }
}

impl Config {
    fn compile(&self, store: Arc<dyn ResultStore>) -> hirsel_engine::CompileConfig {
        hirsel_engine::CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::new(self.jobs),
            verify: if self.verify {
                hirsel_engine::Verify::On
            } else {
                hirsel_engine::Verify::Off
            },
            retention: hirsel_engine::memo::Retention::Resident,
            lowering: hirsel_engine::LoweringPolicy::Reachable,
            store,
            verify_store: if self.verify_store {
                VerifyStore::Recompute
            } else {
                VerifyStore::Off
            },
            ..Default::default()
        }
    }

    fn result_store(&self) -> Arc<dyn ResultStore> {
        match &self.store {
            Some(path) => Arc::new(DirStore::new(path.clone())),
            None => Arc::new(NoneStore),
        }
    }

    /// The switches this run needs beyond today's compiler.
    pub fn switches(&self) -> Vec<Switch> {
        let mut wanted = vec![];
        if self.jobs != 1 {
            wanted.push(Switch::Jobs);
        }
        if self.salt.is_some() {
            wanted.push(Switch::IdSalt);
        }
        if self.store.is_some() {
            wanted.push(Switch::DirStore);
        }
        if self.verify_store {
            wanted.push(Switch::VerifyStore);
        }
        if !self.verify {
            wanted.push(Switch::VerifyOff);
        }
        wanted
    }

    /// The `produce` arguments that select this configuration in a child process.
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["--jobs".into(), self.jobs.to_string()];
        if let Some(salt) = self.salt {
            args.extend(["--salt".into(), salt.to_string()]);
        }
        if let Some(store) = &self.store {
            args.extend(["--store".into(), format!("dir:{}", store.display())]);
        }
        if self.verify_store {
            args.push("--verify-store".into());
        }
        if !self.verify {
            args.extend(["--verify".into(), "off".into()]);
        }
        if let Some(limits) = self.limits {
            args.extend(["--limits".into(), limits.to_string()]);
        }
        if self.native {
            args.push("--native".into());
        }
        args
    }
}

impl std::fmt::Display for Limits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.steps {
            Some(steps) => write!(f, "steps={steps},depth={}", self.call_depth),
            None => write!(f, "steps=unbounded,depth={}", self.call_depth),
        }
    }
}

impl std::str::FromStr for Limits {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, String> {
        let (mut steps, mut depth) = (None, None);
        for part in text.split(',') {
            match part.split_once('=') {
                Some(("steps", "unbounded")) => steps = Some(None),
                Some(("steps", n)) => steps = Some(Some(n.parse().map_err(|_| part.to_string())?)),
                Some(("depth", n)) => depth = Some(n.parse().map_err(|_| part.to_string())?),
                _ => {
                    return Err(format!(
                        "limits: expected steps=<n|unbounded>,depth=<n>: {text}"
                    ));
                }
            }
        }
        match (steps, depth) {
            (Some(steps), Some(call_depth)) => Ok(Limits { steps, call_depth }),
            _ => Err(format!(
                "limits: expected steps=<n|unbounded>,depth=<n>: {text}"
            )),
        }
    }
}

/// A cold run's images: only the toolchain's std image; nothing else checked
/// is shared between runs.
pub fn fresh_images() -> StdImages {
    static IMAGES: std::sync::OnceLock<Vec<Arc<StdImage>>> = std::sync::OnceLock::new();
    let images = IMAGES.get_or_init(|| {
        let path = std::env::var_os("HIRSEL_IMAGE")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .map(|exe| exe.with_file_name("hirsel-std.image"))
            });
        path.and_then(|path| StdImage::open(&path).ok())
            .into_iter()
            .collect()
    });
    StdImages::of(images.clone())
}

/// Compile `program` under `config` with `cache`, as `hirsel check --json`
/// renders it, then export the entry's TIR and, with `native`, its Ready
/// receipts and objects. Refuses before compiling when a switch is missing.
pub fn produce(program: &Path, config: &Config, cache: &StdImages) -> Result<Artifacts, Switch> {
    produce_with_store(program, config, cache, config.result_store())
}

fn produce_with_store(
    program: &Path,
    config: &Config,
    cache: &StdImages,
    store: Arc<dyn ResultStore>,
) -> Result<Artifacts, Switch> {
    switches::missing(config.switches())?;
    Ok(hirsel_engine::with_compiler_stack(|| {
        artifacts(program, config, cache, store)
    }))
}

fn artifacts(
    path: &Path,
    config: &Config,
    cache: &StdImages,
    store: Arc<dyn ResultStore>,
) -> Artifacts {
    let mut out = Artifacts::new();
    let compiler = hirsel_driver::COMPILER.with_config(config.compile(store));
    let mut program = match compiler.load_program_source_with_images(path, None, cache) {
        Ok(program) => program,
        Err(error) => {
            out.insert("diagnostics".into(), tool_error(&error).into_bytes());
            return out;
        }
    };
    program_artifacts(&mut program, config)
}

fn program_artifacts(program: &mut hirsel_engine::LoadedProgram, config: &Config) -> Artifacts {
    let mut out = Artifacts::new();
    program.engine.verify = if config.verify {
        hirsel_engine::Verify::On
    } else {
        hirsel_engine::Verify::Off
    };
    if let Some(limits) = config.limits {
        program.engine.comptime_limits.steps = limits.steps;
        program.engine.comptime_limits.call_depth = limits.call_depth;
    }
    let output = match program.check(hirsel_engine::LoweringPolicy::Reachable) {
        Ok(output) => output,
        Err(error) => {
            let error = program.error(error);
            out.insert("diagnostics".into(), tool_error(&error).into_bytes());
            return out;
        }
    };
    let mut diagnostics = String::new();
    for d in &output.diagnostics {
        writeln!(diagnostics, "{}", d.to_json(&output.sources)).unwrap();
    }
    let rejected = output
        .diagnostics
        .iter()
        .any(|d| d.severity.as_str() == "error");
    if let CheckCompletion::Incomplete { phase } = output.completion {
        writeln!(diagnostics, "incomplete: {}", phase.phase_name()).unwrap();
    }
    out.insert("diagnostics".into(), diagnostics.into_bytes());
    if rejected || output.completion != CheckCompletion::Complete {
        return out;
    }
    let entry = program.entry.clone();
    let tir = program.check_for_execution().and_then(|()| {
        program
            .engine
            .export_tir(&entry)
            .map_err(|e| program.error(e))
    });
    out.insert(
        "tir".into(),
        tir.unwrap_or_else(|error| format!("export: {}", tool_error(&error)))
            .into_bytes(),
    );
    if config.native {
        native(program, &mut out);
    }
    out
}

/// Ready receipts, then the singleton objects of both backends at O0.
fn native(program: &mut hirsel_engine::LoadedProgram, out: &mut Artifacts) {
    let engine = &mut program.engine;
    let validated = match engine
        .export_native_input()
        .map(|input| engine.import_native_input(input))
    {
        Ok(Ok(validated)) => validated,
        Ok(Err(error)) => {
            out.insert(
                "receipts".into(),
                format!("import: {error:?}\n").into_bytes(),
            );
            return;
        }
        Err(error) => {
            out.insert(
                "receipts".into(),
                format!("export: {error:?}\n").into_bytes(),
            );
            return;
        }
    };
    let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    let ready = match hirsel_driver::native::build(&validated, &options) {
        Ok(ready) => ready,
        Err(error) => {
            out.insert("receipts".into(), format!("build: {error}\n").into_bytes());
            return;
        }
    };
    let mut receipts = String::new();
    for unit in &ready.units {
        writeln!(receipts, "{:?}", unit.receipt().raw()).unwrap();
    }
    out.insert("receipts".into(), receipts.into_bytes());
    for backend in [Backend::Llvm, Backend::Cranelift] {
        let objects = hirsel_driver::native::emit_backend_objects(
            &ready,
            backend,
            OptLevel::None,
            Partition::Singleton,
            false,
            None,
        );
        match objects {
            Ok(objects) => {
                for (index, object) in objects.into_iter().enumerate() {
                    out.insert(
                        format!("objects/{}/{index:04}.o", backend.name()),
                        object.bytes,
                    );
                }
            }
            Err(error) => {
                out.insert(
                    format!("objects/{}/error", backend.name()),
                    error.to_string().into_bytes(),
                );
            }
        }
    }
}

/// A tool error as `hirsel check --json` reports it: the diagnostics it carries,
/// else its message.
fn tool_error(error: &ToolError) -> String {
    match error {
        ToolError::Query {
            error: hirsel_engine::QueryError::Diagnostics(diagnostics),
            sources,
            ..
        } => diagnostics
            .iter()
            .map(|d| format!("{}\n", d.to_json(sources)))
            .collect(),
        error => format!("tool error: {error}\n"),
    }
}

/// Write `artifacts` under `dir`, one file per name.
pub fn write_artifacts(dir: &Path, artifacts: &Artifacts) -> std::io::Result<()> {
    for (name, bytes) in artifacts {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, bytes)?;
    }
    Ok(())
}

/// The artifacts `write_artifacts` wrote under `dir`.
pub fn read_artifacts(dir: &Path) -> std::io::Result<Artifacts> {
    fn walk(root: &Path, dir: &Path, out: &mut Artifacts) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(root, &path, out)?;
            } else {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(name, std::fs::read(&path)?);
            }
        }
        Ok(())
    }
    let mut out = Artifacts::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// `None` when `actual` equals `expected` byte for byte, else where they first differ.
pub fn difference(expected: &Artifacts, actual: &Artifacts) -> Option<String> {
    for name in expected.keys().chain(actual.keys()) {
        match (expected.get(name), actual.get(name)) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => return Some(format!("{name}: {}", first_difference(a, b))),
            (Some(_), None) => return Some(format!("{name}: missing")),
            (None, _) => return Some(format!("{name}: unexpected")),
        }
    }
    None
}

fn first_difference(expected: &[u8], actual: &[u8]) -> String {
    fn text(bytes: &[u8]) -> Option<&str> {
        std::str::from_utf8(bytes)
            .ok()
            .filter(|t| !t.contains('\0'))
    }
    match (text(expected), text(actual)) {
        (Some(expected), Some(actual)) => {
            let mut lines = expected.lines().zip(actual.lines()).enumerate();
            match lines.find(|(_, (a, b))| a != b) {
                Some((index, (a, b))) => format!("line {}: expected {a:?}, got {b:?}", index + 1),
                None => format!(
                    "{} lines expected, {} got",
                    expected.lines().count(),
                    actual.lines().count()
                ),
            }
        }
        _ => {
            let at = expected.iter().zip(actual).position(|(a, b)| a != b);
            format!(
                "bytes differ at offset {} ({} expected, {} got)",
                at.unwrap_or(expected.len().min(actual.len())),
                expected.len(),
                actual.len()
            )
        }
    }
}

/// One oracle check's outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail(String),
    Todo(Switch),
}

#[derive(Clone, Debug)]
pub struct Check {
    pub name: String,
    pub verdict: Verdict,
}

/// An oracle's checks. It passes when every check passes; a `Todo` passes only
/// under `allow_todo`.
#[derive(Clone, Debug)]
pub struct Report {
    pub oracle: &'static str,
    pub checks: Vec<Check>,
}

impl Report {
    pub fn new(oracle: &'static str) -> Self {
        Self {
            oracle,
            checks: vec![],
        }
    }
    pub fn add(&mut self, name: impl Into<String>, verdict: Verdict) {
        self.checks.push(Check {
            name: name.into(),
            verdict,
        });
    }
    pub fn count(&self, kind: fn(&Verdict) -> bool) -> usize {
        self.checks.iter().filter(|c| kind(&c.verdict)).count()
    }
    pub fn failures(&self) -> Vec<&Check> {
        self.checks
            .iter()
            .filter(|c| matches!(c.verdict, Verdict::Fail(_)))
            .collect()
    }
    /// The missing switches, each once.
    pub fn todo(&self) -> std::collections::BTreeSet<Switch> {
        self.checks
            .iter()
            .filter_map(|c| match c.verdict {
                Verdict::Todo(switch) => Some(switch),
                _ => None,
            })
            .collect()
    }
    pub fn passed(&self, allow_todo: bool) -> bool {
        self.failures().is_empty() && (allow_todo || self.todo().is_empty())
    }
    pub fn json(&self) -> serde_json::Value {
        let checks: Vec<_> = self
            .checks
            .iter()
            .map(|c| match &c.verdict {
                Verdict::Pass => serde_json::json!({"name": c.name, "verdict": "pass"}),
                Verdict::Fail(why) => {
                    serde_json::json!({"name": c.name, "verdict": "fail", "why": why})
                }
                Verdict::Todo(switch) => serde_json::json!({
                    "name": c.name, "verdict": "todo", "switch": format!("{switch:?}"),
                    "lane": switch.lane(),
                }),
            })
            .collect();
        serde_json::json!({
            "oracle": self.oracle,
            "pass": self.count(|v| *v == Verdict::Pass),
            "fail": self.failures().len(),
            "todo": self.count(|v| matches!(v, Verdict::Todo(_))),
            "checks": checks,
        })
    }
    /// Failures and missing switches, one per line, then the counts.
    pub fn text(&self) -> String {
        let mut text = String::new();
        for check in self.failures() {
            if let Verdict::Fail(why) = &check.verdict {
                writeln!(text, "{}: FAIL {}: {why}", self.oracle, check.name).unwrap();
            }
        }
        for switch in self.todo() {
            let cells = self
                .checks
                .iter()
                .filter(|c| c.verdict == Verdict::Todo(switch))
                .count();
            writeln!(text, "{}: {switch} ({cells} checks)", self.oracle).unwrap();
        }
        writeln!(
            text,
            "{}: {} pass, {} fail, {} todo",
            self.oracle,
            self.count(|v| *v == Verdict::Pass),
            self.failures().len(),
            self.count(|v| matches!(v, Verdict::Todo(_)))
        )
        .unwrap();
        text
    }
}

/// The repository-relative corpus files under `prefixes`, sorted, in `shard`,
/// then `limit` of them spread evenly over that list.
pub fn corpus(
    root: &Path,
    prefixes: &[String],
    shard: Option<hirsel_cli_args::Shard>,
    limit: Option<usize>,
) -> Result<Vec<PathBuf>, String> {
    let files = crate::Selection {
        shard,
        paths: prefixes,
    }
    .files(root)?;
    Ok(match limit {
        Some(limit) if limit < files.len() => (0..limit)
            .map(|i| files[i * files.len() / limit].clone())
            .collect(),
        _ => files,
    })
}
