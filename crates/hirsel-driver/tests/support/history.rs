//! Test-only shared runner for bounded ownership, placement, cleanup and scalar histories.
//! Uses only public driver adapters; unavailable tools never count as agreement.
use hirsel_driver::{compiler_adapters, native::*};
use hirsel_engine::*;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Observation,
    SourceRejection,
    VRejection,
    HTrap,
    ToolingLimit,
    Timeout,
    ToolError,
}

#[derive(Debug)]
pub struct Cell {
    pub engine: String,
    pub category: Category,
    pub observation: Option<Observation>,
    pub detail: String,
    pub disagreement: String,
}

#[derive(Debug)]
pub struct Report {
    pub validated: bool,
    pub cells: Vec<Cell>,
}

impl Report {
    pub fn agrees(&self) -> bool {
        self.validated
            && self.cells.len() > 1
            && self.cells.iter().all(|cell| {
                cell.category == Category::Observation
                    && cell.observation == self.cells[0].observation
            })
    }

    /// Signatures omit source coordinates, which deletion can move, but retain
    /// the engine and the differing observation field. A different failure kind
    /// or a newly introduced rejection cannot replace an output disagreement.
    pub fn disagreements(&self, output: &[u8], failure: bool) -> Vec<(String, String)> {
        let reference = self.cells.iter().find(|c| c.engine == "reference");
        let oracle = reference.and_then(|c| c.observation.as_ref());
        let mut differences = Vec::new();
        for cell in &self.cells {
            if cell.category != Category::Observation {
                differences.push((
                    cell.engine.clone(),
                    format!("{:?}:{}", cell.category, cell.disagreement),
                ));
                continue;
            }
            let observed = cell.observation.as_ref().unwrap();
            if cell.engine == "reference" {
                if observed.output != output {
                    differences.push((cell.engine.clone(), "model-output".into()));
                }
                let expected_exit = if failure { None } else { Some(0) };
                if observed.exit_code != expected_exit
                    || observed.failures.len() != usize::from(failure)
                    || observed.failures.iter().any(|f| f.rule != "SEM-1")
                    || observed.blocked
                    || observed.budget.is_some()
                    || observed.signaled.is_some()
                {
                    differences.push((cell.engine.clone(), "model-outcome".into()));
                }
            } else if let Some(reference) = oracle {
                for (field, different) in [
                    ("output", observed.output != reference.output),
                    ("exit", observed.exit_code != reference.exit_code),
                    ("failures", observed.failures != reference.failures),
                    ("blocked", observed.blocked != reference.blocked),
                    ("budget", observed.budget != reference.budget),
                    ("signal", observed.signaled != reference.signaled),
                    ("roots", observed.roots != reference.roots),
                ] {
                    if different {
                        differences.push((cell.engine.clone(), field.into()));
                    }
                }
            }
        }
        differences
    }
}

fn error_cell(engine: &str, category: Category, detail: impl std::fmt::Debug) -> Cell {
    Cell {
        engine: engine.into(),
        category,
        observation: None,
        detail: format!("{detail:?}"),
        disagreement: format!("{category:?}"),
    }
}

pub fn tooling(message: &str) -> Category {
    let message = message.to_ascii_lowercase();
    if message.contains("timeout") || message.contains("timed out") {
        Category::Timeout
    } else if ["limit", "budget", "exhaust", "out of memory"]
        .iter()
        .any(|s| message.contains(s))
    {
        Category::ToolingLimit
    } else {
        Category::ToolError
    }
}

pub fn query_error(engine: &str, error: QueryError) -> Cell {
    let category = tooling(&format!("{error:?}"));
    let category = if category != Category::ToolError {
        category
    } else {
        match &error {
            QueryError::Diagnostics(_) if engine == "source" => Category::SourceRejection,
            QueryError::Diagnostics(_) => Category::VRejection,
            QueryError::Operational(failure)
                if matches!(
                    failure.operation.as_str(),
                    "TIR validation" | "native snapshot import"
                ) =>
            {
                Category::VRejection
            }
            _ => Category::ToolError,
        }
    };
    let mut cell = error_cell(engine, category, &error);
    cell.disagreement = match error {
        QueryError::Operational(failure) => failure.operation,
        QueryError::Diagnostics(diagnostics) => format!(
            "{:?}",
            diagnostics.iter().map(|d| &d.rule).collect::<Vec<_>>()
        ),
        QueryError::NotImplemented => "not-implemented".into(),
    };
    cell
}

pub fn native_error(engine: &str, error: NativeError) -> Cell {
    let category = tooling(&error.message);
    let category = if category == Category::ToolError
        && error.stage == StageName::Heap
        && !error.message.starts_with("Unsupported:")
    {
        Category::HTrap
    } else {
        category
    };
    let mut cell = error_cell(engine, category, &error);
    // H errors begin with the physical violation kind. Source coordinates,
    // counts and instruction indices may change under deletion; the kind must not.
    cell.disagreement = format!(
        "{:?}:{}:{}",
        error.stage,
        error.validator,
        error.message.split(':').next().unwrap_or(&error.message)
    );
    cell
}

pub struct Runner {
    matrix: Vec<Configuration>,
    toolchain: Toolchain,
    directory: PathBuf,
}

impl Runner {
    pub fn new(directory: &Path) -> Result<Self, NativeError> {
        let matrix: Vec<_> = [
            Engine::Heap { campaign: true },
            Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| Configuration {
            options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            engine,
        })
        .collect();
        Self::with_matrix(directory, matrix)
    }

    pub fn with_matrix(directory: &Path, matrix: Vec<Configuration>) -> Result<Self, NativeError> {
        std::fs::create_dir_all(directory).unwrap();
        let mut toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix)?;
        toolchain.launch_timeout = std::time::Duration::from_secs(60);
        Ok(Self {
            matrix,
            toolchain,
            directory: directory.into(),
        })
    }

    pub fn run(&self, name: &str, source: &str) -> Report {
        with_compiler_stack(|| {
            let mut report = Report {
                validated: false,
                cells: Vec::new(),
            };
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("main.hir", source.into()).unwrap();
            let checked = match check_progress(&mut engine, file, &context) {
                Ok(checked) => checked,
                Err(e) => {
                    report.cells.push(query_error("source", e));
                    return report;
                }
            };
            let errors: Vec<_> = checked
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            if !errors.is_empty() {
                report
                    .cells
                    .push(error_cell("source", Category::SourceRejection, errors));
                return report;
            }
            let input = match engine.export_native_input() {
                Ok(input) => input,
                Err(e) => {
                    report.cells.push(query_error("V-export", e));
                    return report;
                }
            };
            let validated = match engine.import_native_input(input) {
                Ok(validated) => validated,
                Err(e) => {
                    report.cells.push(query_error("V-import", e));
                    return report;
                }
            };
            report.validated = true;
            let entry = entry_path(&context);
            let reference = match run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            ) {
                Ok(run) => match run.observation {
                    Some(observation) => Observation::of_reference(&observation),
                    None => {
                        report
                            .cells
                            .push(error_cell("reference", Category::ToolingLimit, run));
                        return report;
                    }
                },
                Err(e) => {
                    report
                        .cells
                        .push(error_cell("reference", tooling(&format!("{e:?}")), e));
                    return report;
                }
            };
            self.compare(&validated, &entry, &reference, name)
        })
    }

    /// Reuse an already checked source/V snapshot and reference execution.
    pub fn compare(
        &self,
        validated: &ValidatedNativeInput,
        entry: &hirsel_syntax::DefPath,
        reference: &Observation,
        name: &str,
    ) -> Report {
        let mut report = Report {
            validated: true,
            cells: vec![Cell {
                engine: "reference".into(),
                category: Category::Observation,
                observation: Some(reference.clone()),
                detail: String::new(),
                disagreement: String::new(),
            }],
        };
        let directory = self.directory.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        for (configuration, outcome) in differential(
            validated,
            entry,
            reference,
            &self.matrix,
            &self.toolchain,
            &directory,
        ) {
            report.cells.push(match outcome {
                Outcome::Agree => Cell {
                    engine: configuration,
                    category: Category::Observation,
                    observation: Some(reference.clone()),
                    detail: String::new(),
                    disagreement: String::new(),
                },
                Outcome::Mismatch(observation) => Cell {
                    engine: configuration,
                    category: Category::Observation,
                    observation: Some(observation),
                    detail: String::new(),
                    disagreement: String::new(),
                },
                Outcome::Unavailable(error) => native_error(&configuration, error),
            });
        }
        report
    }
}

/// Deterministic, deletion-only, one-minimal reduction. Revisit earlier deletions
/// after each successful pass. The caller must check model/source/V validity and
/// the original disagreement, using the same runner as the unreduced history.
pub fn minimize<T: Clone>(original: &[T], mut preserves: impl FnMut(&[T]) -> bool) -> Vec<T> {
    let mut kept = original.to_vec();
    loop {
        let mut changed = false;
        for index in 0..kept.len() {
            let mut candidate = kept.clone();
            candidate.remove(index);
            if preserves(&candidate) {
                kept = candidate;
                changed = true;
                break;
            }
        }
        if !changed {
            return kept;
        }
    }
}

pub fn persist(directory: &Path, name: &str, seed: u64, source: &str, detail: &str) {
    let directory = directory.join(name);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("main.hir"), source).unwrap();
    std::fs::write(directory.join("hirsel.json"), r#"{"schema":1,"package":{"name":"standalone","version":"0"},"sources":[{"path":"main.hir","module":[]}],"entry":{"module":[],"name":"main"},"dependencies":[]}"#).unwrap();
    let receipt = format!(
        "seed={seed}\nsource_hash={:?}\n{detail}\n",
        hirsel_syntax::ContentHash::of_bytes(source.as_bytes())
    );
    let manifest = std::fs::read_to_string(directory.join("hirsel.json")).unwrap();
    let receipt = format!(
        "{receipt}manifest_hash={:?}\n",
        hirsel_syntax::ContentHash::of_bytes(manifest.as_bytes())
    );
    std::fs::write(directory.join("receipt.txt"), &receipt).unwrap();
    // A remote action's scratch is disposable. Retain a machine-readable copy in
    // captured stderr as well, so the caller can restore every witness locally.
    eprintln!(
        "HISTORY_ARTIFACT {}",
        serde_json::json!({
            "name": name, "seed": seed, "source": source,
            "manifest": manifest,
            "receipt": receipt,
        })
    );
}
