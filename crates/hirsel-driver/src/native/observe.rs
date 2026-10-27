//! The observation every engine is compared on: the entry's exit
//! code, the bytes it printed and the ordered failure chain by rule,
//! definition, line and column. It holds no address, allocation or count.
//!
//! The reference interpreter's `Observable` also orders resource, finalizer
//! and capability effects; neither the heap executor nor the launcher reports
//! those yet, so they are outside this comparison.
use super::stages::{NativeError, ReadyProgram, StageName};
use hirsel_engine::NativeInput;
use hirsel_lir::ir::SiteRange;
use hirsel_span::Span;
use hirsel_syntax::DefPath;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The comparison's own version: bump when a field is added or its meaning
/// changes, so stored mismatch artifacts name what they compared.
pub const OBSERVATION_SCHEMA: u32 = 5;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub rule: String,
    pub definition: DefPath,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub operation: Option<hirsel_interp::observation::SourceSite>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    /// The returned unsigned byte status; None when the entry failed or did not return.
    pub exit_code: Option<i32>,
    /// The run is Blocked (CONC-8, schema 2): it neither returned nor failed.
    pub blocked: bool,
    /// The run spent this budget (`budgets.limits@2`) and stopped as a Blocked run does.
    pub budget: Option<hirsel_run_schema::BudgetKind>,
    /// The run ended by this signal's default action (schema 4): a native executable is
    /// terminated by the signal itself, which an ordinary exit status never matches.
    pub signaled: Option<hirsel_run_schema::SignalKind>,
    pub output: Vec<u8>,
    /// The primary failure, then cleanup failures in order.
    pub failures: Vec<Failure>,
    /// `--trace-roots` (S-14): every live `Shared` root's count at every scheduling point,
    /// one line per point; empty unless the fixture traces roots.
    pub roots: Vec<String>,
}

impl Observation {
    /// The reference interpreter's observation, projected onto this schema.
    pub fn of_reference(observable: &hirsel_interp::observation::Observable) -> Self {
        Self {
            exit_code: observable.exit_code,
            blocked: observable.blocked.is_some(),
            budget: None,
            signaled: observable.signaled,
            output: observable.output.clone(),
            failures: observable
                .failures
                .iter()
                .map(|f| Failure {
                    rule: f.rule.as_str().to_string(),
                    definition: f.stable_site.definition.clone(),
                    file: f.stable_site.file.clone(),
                    line: f.stable_site.line,
                    column: f.stable_site.column,
                    operation: f.operation.clone(),
                })
                .collect(),
            roots: observable
                .final_state
                .as_ref()
                .map(|state| state.root_trace.clone())
                .unwrap_or_default(),
        }
    }

    /// Resolve this engine's observation into the one host outcome type.
    pub fn outcome(
        &self,
        sites: &Sites<'_>,
        waits: Vec<hirsel_run_schema::Wait>,
    ) -> Result<hirsel_run_schema::RunOutcome, NativeError> {
        use hirsel_run_schema::{Returned, RunOutcome};
        if let Some(kind) = self.budget {
            return Ok(RunOutcome::BudgetExceeded(kind));
        }
        if let Some(kind) = self.signaled {
            return Ok(RunOutcome::Signaled(kind));
        }
        if self.blocked {
            return Ok(RunOutcome::Blocked(waits));
        }
        if let Some(code) = self.exit_code {
            return Ok(RunOutcome::Returned(Returned::from_exit_code(code as u8)));
        }
        let mut failures = self.failures.iter().map(|failure| {
            let missing = || NativeError::new(StageName::Launch, "failure has no source site");
            Ok(hirsel_run_schema::Failure {
                rule: failure.rule.clone(),
                site: sites.failure_site(failure).ok_or_else(missing)?,
                operation: failure
                    .operation
                    .as_ref()
                    .map(|site| sites.operation_site(site).ok_or_else(missing))
                    .transpose()?,
            })
        });
        Ok(RunOutcome::Failed {
            primary: failures.next().ok_or_else(|| {
                NativeError::new(StageName::Launch, "neither a return nor a failure")
            })??,
            causes: failures.collect::<Result<_, NativeError>>()?,
        })
    }

    /// A stable text form, for mismatch artifacts and reports.
    pub fn render(&self) -> String {
        let mut out = format!("observation {OBSERVATION_SCHEMA}\n");
        match self.exit_code {
            Some(code) => {
                let _ = writeln!(out, "exit {code}");
            }
            None => out.push_str("exit none\n"),
        }
        if self.blocked {
            out.push_str("blocked\n");
        }
        if let Some(kind) = self.signaled {
            let _ = writeln!(out, "signaled {}", kind.name());
        }
        let _ = writeln!(out, "output {:?}", String::from_utf8_lossy(&self.output));
        for f in &self.failures {
            let _ = writeln!(
                out,
                "failure {} {} {:?} {}:{}",
                f.rule, f.definition.0.name, f.file, f.line, f.column
            );
            if let Some(operation) = &f.operation {
                let _ = writeln!(
                    out,
                    "  in {} {:?} {}:{}",
                    operation.definition, operation.file, operation.line, operation.column
                );
            }
        }
        for line in &self.roots {
            let _ = writeln!(out, "roots {line}");
        }
        out
    }
}

/// Source positions of the snapshot's definitions: a failure site is a byte
/// range relative to its selected existing source anchor.
pub struct Sites<'a> {
    sources: &'a hirsel_span::SourceMap,
    anchors: BTreeMap<&'a DefPath, Vec<hirsel_span::Anchor>>,
}

impl<'a> Sites<'a> {
    pub fn new(input: &'a NativeInput) -> Self {
        let anchors = input
            .source_owners
            .iter()
            .map(|(path, owner)| {
                (
                    path,
                    input.definitions.get(path).map_or_else(
                        || vec![owner.syntax.anchor],
                        |d| hirsel_tir::source_anchors(d),
                    ),
                )
            })
            .collect();
        Self::from_anchors(&input.sources, anchors)
    }

    /// Bind already selected source anchors without retaining a NativeInput or
    /// any source Definition. Generated sources preserve their anchor order.
    pub fn from_anchors(
        sources: &'a hirsel_span::SourceMap,
        anchors: BTreeMap<&'a DefPath, Vec<hirsel_span::Anchor>>,
    ) -> Self {
        Self { sources, anchors }
    }

    pub fn failure(
        &self,
        rule: &str,
        definition: &DefPath,
        site: SiteRange,
    ) -> Result<Failure, NativeError> {
        let location = self.position(definition, site).ok_or_else(|| {
            NativeError::new(
                StageName::Launch,
                format!("a failure site outside the source of {}", definition.0.name),
            )
        })?;
        Ok(Failure {
            rule: rule.to_string(),
            definition: definition.clone(),
            file: self
                .file(definition, site.source)
                .ok_or_else(|| NativeError::new(StageName::Launch, "missing failure source"))?
                .path
                .to_string_lossy()
                .into_owned(),
            line: location.0,
            column: location.1,
            operation: None,
        })
    }

    /// The file a definition's source stands in.
    fn file(&self, definition: &DefPath, source: u32) -> Option<&hirsel_span::SourceFile> {
        let (file, _) = self
            .sources
            .anchor(*self.anchors.get(definition)?.get(source as usize)?)?;
        self.sources.file(file)
    }
    pub fn failure_site(&self, failure: &Failure) -> Option<hirsel_run_schema::Site> {
        Some(hirsel_run_schema::Site {
            definition: failure.definition.to_string(),
            file: self.outcome_file(&failure.definition, &failure.file)?,
            line: failure.line.into(),
            column: failure.column.into(),
        })
    }

    pub(crate) fn report(&self, failure: &Failure, json: bool) -> Result<String, NativeError> {
        let missing = || NativeError::new(StageName::Launch, "failure has no source site");
        let mut site = self.failure_site(failure).ok_or_else(missing)?;
        site.file = failure.file.clone();
        let operation = failure
            .operation
            .as_ref()
            .map(|source| {
                let mut site = self.operation_site(source).ok_or_else(missing)?;
                site.file = source.file.clone();
                Ok(site)
            })
            .transpose()?;
        Ok(site.report(&failure.rule, json, operation.as_ref()))
    }

    pub fn operation_site(
        &self,
        source: &hirsel_interp::observation::SourceSite,
    ) -> Option<hirsel_run_schema::Site> {
        Some(hirsel_run_schema::Site {
            definition: source.definition.to_string(),
            file: self.outcome_file(&source.definition, &source.file)?,
            line: source.line.into(),
            column: source.column.into(),
        })
    }

    /// Host outcomes bind the same logical file as the compiled failure table.
    /// The observation retains its physical spelling for source diagnostics.
    fn outcome_file(&self, definition: &DefPath, path: &str) -> Option<String> {
        (0..self.anchors.get(definition)?.len()).find_map(|source| {
            let file = self.file(definition, source as u32)?;
            (file.path == std::path::Path::new(path)
                || file.logical_path() == std::path::Path::new(path))
            .then(|| file.logical_path().to_string_lossy().into_owned())
        })
    }
    /// The one-based line and column a failure site starts at.
    pub fn position(&self, definition: &DefPath, site: SiteRange) -> Option<(u32, u32)> {
        let anchor = self.anchors.get(definition)?.get(site.source as usize)?;
        let span = Span::new(*anchor, site.start, site.end)?;
        let location = self.sources.location(span)?;
        Some((location.line, location.column))
    }
}

/// Generated definitions keep their source owner's anchors and diagnostic name.
/// This is the explicit mapping produced by import and bound to the run identity.
pub(super) fn source_definition<'a>(
    program: &'a ReadyProgram,
    definition: &'a DefPath,
) -> &'a DefPath {
    program
        .semantic_owners
        .get(definition)
        .and_then(Option::as_ref)
        .unwrap_or(definition)
}

/// Every site code emission can pass to `hirsel_rt_v2_failure_begin`, mapped
/// back to its definition and byte range.
pub fn site_codes(program: &ReadyProgram) -> BTreeMap<u64, (DefPath, SiteRange)> {
    site_codes_from(
        program.summaries.iter().map(|summary| {
            (
                &summary.definition,
                &summary.implementation_revision,
                summary.sites.as_slice(),
            )
        }),
        |definition| source_definition(program, definition).clone(),
    )
}

pub(crate) fn site_codes_from<'a, T>(
    summaries: impl IntoIterator<Item = (&'a DefPath, &'a hirsel_syntax::ContentHash, &'a [SiteRange])>,
    owner: impl Fn(&DefPath) -> T,
) -> BTreeMap<u64, (T, SiteRange)> {
    let mut codes = BTreeMap::new();
    for (definition, revision, sites) in summaries {
        for &site in sites {
            codes.insert(
                hirsel_lir::emission::site_code(definition, revision, site),
                (owner(definition), site),
            );
        }
    }
    codes
}

/// Whether the program catches failures (`task.isolate`'s `Isolate` terminator).
pub fn isolates(program: &ReadyProgram) -> bool {
    program.summaries.iter().any(|s| s.isolates)
}

/// The program's failure table (`hirsel_rt::isolate`): each site code's definition path and
/// one-based line and column (0 and 0 inside a library body), in code order, and each program
/// rule code's rule ID.
#[derive(Clone, Debug)]
pub struct FailureTable {
    pub sites: Vec<(u64, String, u32, u32, String)>,
    pub rules: Vec<String>,
}

impl FailureTable {
    /// The table as `hirsel_rt::isolate::FailureTable::new` reads it in place: little-endian,
    /// a u32 site count and a u32 rule count, then each site's 32-byte record (u64 code, u32
    /// line, u32 column, then the u32 offset and length in the string pool of its definition
    /// path and of its file path) in code order, each rule's u32 offset and length, then the
    /// string pool, which holds each distinct string once, in first-use order.
    pub fn encode(&self) -> Vec<u8> {
        let mut pool: Vec<u8> = Vec::new();
        let mut placed = std::collections::HashMap::new();
        let mut place = |text: &str| -> [u32; 2] {
            let offset = *placed.entry(text.to_owned()).or_insert_with(|| {
                pool.extend(text.as_bytes());
                pool.len() - text.len()
            });
            [offset, text.len()].map(|n| u32::try_from(n).expect("a failure table over 4 GiB"))
        };
        let count = |n: usize| u32::try_from(n).expect("a failure table over 4 GiB");
        let mut out = Vec::new();
        out.extend(count(self.sites.len()).to_le_bytes());
        out.extend(count(self.rules.len()).to_le_bytes());
        for (code, definition, line, column, file) in &self.sites {
            out.extend(code.to_le_bytes());
            for word in [[*line, *column], place(definition), place(file)].concat() {
                out.extend(word.to_le_bytes());
            }
        }
        for rule in &self.rules {
            for word in place(rule) {
                out.extend(word.to_le_bytes());
            }
        }
        out.extend(pool);
        out
    }
}

pub fn failure_table(program: &ReadyProgram, sites: &Sites<'_>) -> FailureTable {
    FailureTable {
        sites: site_codes(program)
            .into_iter()
            .map(|(code, (definition, range))| {
                // A site inside a library body has no source position: line and column 0.
                let (line, column) = sites.position(&definition, range).unwrap_or((0, 0));
                let file = sites
                    .file(&definition, range.source)
                    .map(|f| f.logical_path().to_string_lossy().into_owned())
                    .unwrap_or_else(|| "<core>".into());
                (code, definition.to_string(), line, column, file)
            })
            .collect(),
        rules: super::emit::failure_rules(program),
    }
}
