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
use hirsel_span::{FileId, Span};
use hirsel_syntax::DefPath;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The comparison's own version: bump when a field is added or its meaning
/// changes, so stored mismatch artifacts name what they compared.
pub const OBSERVATION_SCHEMA: u32 = 3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub rule: String,
    pub definition: DefPath,
    pub line: u32,
    pub column: u32,
    pub operation: Option<hirsel_interp::observation::SourceSite>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    /// 0 for `Ok`, 1 for `Error`; None when the entry failed or the run is Blocked.
    pub exit_code: Option<i32>,
    /// The run is Blocked (CONC-8, schema 2): it neither returned nor failed.
    pub blocked: bool,
    /// The run spent this budget (`budgets.limits@1`) and stopped as a Blocked run does.
    pub budget: Option<hirsel_run_config::BudgetKind>,
    pub output: Vec<u8>,
    /// The primary failure, then cleanup failures in order.
    pub failures: Vec<Failure>,
}

impl Observation {
    /// The reference interpreter's observation, projected onto this schema.
    pub fn of_reference(observable: &hirsel_interp::observation::Observable) -> Self {
        Self {
            exit_code: observable.exit_code,
            blocked: observable.blocked.is_some(),
            budget: None,
            output: observable.output.clone(),
            failures: observable
                .failures
                .iter()
                .map(|f| Failure {
                    rule: f.rule.as_str().to_string(),
                    definition: f.stable_site.definition.clone(),
                    line: f.stable_site.line,
                    column: f.stable_site.column,
                    operation: f.operation.clone(),
                })
                .collect(),
        }
    }

    /// Resolve this engine's observation into the one host outcome type.
    pub fn outcome(
        &self,
        sites: &Sites<'_>,
        waits: Vec<hirsel_run_config::Wait>,
    ) -> Result<hirsel_run_config::RunOutcome, NativeError> {
        use hirsel_run_config::{Returned, RunOutcome};
        if let Some(kind) = self.budget {
            return Ok(RunOutcome::BudgetExceeded(kind));
        }
        if self.blocked {
            return Ok(RunOutcome::Blocked(waits));
        }
        if let Some(code) = self.exit_code {
            return Ok(RunOutcome::Returned(if code == 0 {
                Returned::Ok
            } else {
                Returned::Error
            }));
        }
        let mut failures = self.failures.iter().map(|failure| {
            let missing = || NativeError::new(StageName::Launch, "failure has no source site");
            Ok(hirsel_run_config::Failure {
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
        let _ = writeln!(out, "output {:?}", String::from_utf8_lossy(&self.output));
        for f in &self.failures {
            let _ = writeln!(
                out,
                "failure {} {} {}:{}",
                f.rule, f.definition.0.name, f.line, f.column
            );
            if let Some(operation) = &f.operation {
                let _ = writeln!(
                    out,
                    "  in {} {}:{}",
                    operation.definition, operation.line, operation.column
                );
            }
        }
        out
    }
}

/// Source positions of the snapshot's definitions: a failure site is a byte
/// range in its definition's file.
pub struct Sites<'a> {
    input: &'a NativeInput,
    files: BTreeMap<&'a DefPath, FileId>,
}

impl<'a> Sites<'a> {
    pub fn new(input: &'a NativeInput) -> Self {
        let files = input
            .source_owners
            .iter()
            .filter_map(|(path, owner)| {
                let item = owner.syntax.tree.items.get(owner.item.0 as usize)?;
                Some((path, item.origin.span.file))
            })
            .collect();
        Self { input, files }
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
            line: location.0,
            column: location.1,
            operation: None,
        })
    }

    pub fn failure_site(&self, failure: &Failure) -> Option<hirsel_run_config::Site> {
        let file = self
            .input
            .sources
            .file(*self.files.get(&failure.definition)?)?;
        Some(hirsel_run_config::Site {
            definition: failure.definition.to_string(),
            file: file.path.to_string_lossy().into_owned(),
            line: failure.line.into(),
            column: failure.column.into(),
        })
    }

    pub(crate) fn report(&self, failure: &Failure, json: bool) -> Result<String, NativeError> {
        let missing = || NativeError::new(StageName::Launch, "failure has no source site");
        let site = self.failure_site(failure).ok_or_else(missing)?;
        let operation = failure
            .operation
            .as_ref()
            .map(|s| self.operation_site(s).ok_or_else(missing))
            .transpose()?;
        Ok(site.report(&failure.rule, json, operation.as_ref()))
    }

    pub fn operation_site(
        &self,
        source: &hirsel_interp::observation::SourceSite,
    ) -> Option<hirsel_run_config::Site> {
        let file = self
            .input
            .sources
            .file(*self.files.get(&source.definition)?)?;
        Some(hirsel_run_config::Site {
            definition: source.definition.to_string(),
            file: file.path.to_string_lossy().into_owned(),
            line: source.line.into(),
            column: source.column.into(),
        })
    }
    /// The one-based line and column a failure site starts at.
    pub fn position(&self, definition: &DefPath, site: SiteRange) -> Option<(u32, u32)> {
        let file = self.files.get(definition)?;
        let span = Span::new(*file, site.start, site.end)?;
        let location = self.input.sources.location(span)?;
        Some((location.line, location.column))
    }
}

/// Every site code emission can pass to `hirsel_rt_v2_failure_begin`, mapped
/// back to its definition and byte range.
pub fn site_codes(program: &ReadyProgram) -> BTreeMap<u64, (DefPath, SiteRange)> {
    let mut codes = BTreeMap::new();
    for unit in &program.units {
        let unit = unit.unit();
        let header = &unit.header;
        let sites = unit
            .tables
            .source_anchors
            .iter()
            .map(|a| a.site)
            .chain(unit.tables.cleanup_anchors.iter().map(|a| a.site));
        for site in sites {
            codes.insert(
                hirsel_lir::emission::site_code(
                    &header.definition,
                    &header.implementation_revision,
                    site,
                ),
                (header.definition.clone(), site),
            );
        }
    }
    codes
}

/// Whether the program catches failures (`task.isolate`'s `Isolate` terminator).
pub fn isolates(program: &ReadyProgram) -> bool {
    program.units.iter().any(|u| {
        u.unit()
            .terminators
            .iter()
            .any(|t| matches!(t, hirsel_lir::ir::Terminator::Isolate { .. }))
    })
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
    /// The table as `hirsel_rt::isolate::FailureTable::decode` reads it: little-endian u64 words, the site
    /// count, then each site's code, line, column and definition length followed by the
    /// definition's bytes and a length-prefixed file path, then the rule count and
    /// each rule's length and bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let word = |out: &mut Vec<u8>, w: u64| out.extend(w.to_le_bytes());
        word(&mut out, self.sites.len() as u64);
        for (code, definition, line, column, file) in &self.sites {
            for w in [
                *code,
                u64::from(*line),
                u64::from(*column),
                definition.len() as u64,
            ] {
                word(&mut out, w);
            }
            out.extend(definition.as_bytes());
            word(&mut out, file.len() as u64);
            out.extend(file.as_bytes());
        }
        word(&mut out, self.rules.len() as u64);
        for rule in &self.rules {
            word(&mut out, rule.len() as u64);
            out.extend(rule.as_bytes());
        }
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
                    .files
                    .get(&definition)
                    .and_then(|id| sites.input.sources.file(*id))
                    .map(|f| f.path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "<core>".into());
                (code, definition.to_string(), line, column, file)
            })
            .collect(),
        rules: super::emit::failure_rules(program),
    }
}
