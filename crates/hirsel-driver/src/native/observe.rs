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
pub const OBSERVATION_SCHEMA: u32 = 4;

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
            return Ok(RunOutcome::Returned(if code == 0 {
                Returned::Ok
            } else {
                Returned::Error
            }));
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
        for line in &self.roots {
            let _ = writeln!(out, "roots {line}");
        }
        out
    }
}

/// Source positions of the snapshot's definitions: a failure site is a byte
/// range relative to its definition's anchor. A cached program's sites come
/// from its `SiteTable` instead, which holds the same answers.
pub struct Sites<'a> {
    source: Source<'a>,
}

enum Source<'a> {
    Input {
        input: &'a NativeInput,
        anchors: BTreeMap<&'a DefPath, hirsel_span::Anchor>,
    },
    Table(&'a SiteTable),
}

impl<'a> Sites<'a> {
    pub fn new(input: &'a NativeInput) -> Self {
        let anchors = input
            .source_owners
            .iter()
            .map(|(path, owner)| (path, owner.syntax.anchor))
            .collect();
        Self {
            source: Source::Input { input, anchors },
        }
    }

    /// The sites `table` recorded.
    pub(crate) fn table(table: &'a SiteTable) -> Self {
        Self {
            source: Source::Table(table),
        }
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

    /// The path and the logical path of the file a definition's source stands in.
    fn file(&self, definition: &DefPath) -> Option<(String, String)> {
        match &self.source {
            Source::Input { input, anchors } => {
                let (file, _) = input.sources.anchor(*anchors.get(definition)?)?;
                let file = input.sources.file(file)?;
                Some((
                    file.path.to_string_lossy().into_owned(),
                    file.logical_path().to_string_lossy().into_owned(),
                ))
            }
            Source::Table(table) => table.files.get(definition).cloned(),
        }
    }
    pub fn failure_site(&self, failure: &Failure) -> Option<hirsel_run_schema::Site> {
        let (file, _) = self.file(&failure.definition)?;
        Some(hirsel_run_schema::Site {
            definition: failure.definition.to_string(),
            file,
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
    ) -> Option<hirsel_run_schema::Site> {
        let (file, _) = self.file(&source.definition)?;
        Some(hirsel_run_schema::Site {
            definition: source.definition.to_string(),
            file,
            line: source.line.into(),
            column: source.column.into(),
        })
    }
    /// The one-based line and column a failure site starts at.
    pub fn position(&self, definition: &DefPath, site: SiteRange) -> Option<(u32, u32)> {
        match &self.source {
            Source::Input { input, anchors } => {
                let anchor = anchors.get(definition)?;
                let span = Span::new(*anchor, site.start, site.end)?;
                let location = input.sources.location(span)?;
                Some((location.line, location.column))
            }
            Source::Table(table) => {
                *table
                    .positions
                    .get(&(definition.clone(), site.start, site.end))?
            }
        }
    }
}

/// What observing a program's runs reads from it and its sources: its rules by
/// code (`failure_rules`), its site codes (`site_codes`) and each site's position
/// and file as `Sites` answers them. The program cache keeps it beside the image
/// (`program_cache`), so a cached run reports exactly what a fresh one does.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SiteTable {
    pub(crate) rules: Vec<String>,
    pub(crate) codes: BTreeMap<u64, (DefPath, SiteRange)>,
    positions: BTreeMap<(DefPath, u32, u32), Option<(u32, u32)>>,
    files: BTreeMap<DefPath, (String, String)>,
}

impl SiteTable {
    pub(crate) fn new(program: &ReadyProgram, sites: &Sites<'_>) -> Self {
        let codes = site_codes(program);
        let mut table = Self {
            rules: super::emit::failure_rules(program),
            ..Self::default()
        };
        for (definition, range) in codes.values() {
            table.positions.insert(
                (definition.clone(), range.start, range.end),
                sites.position(definition, *range),
            );
            if let Some(file) = sites.file(definition) {
                table.files.insert(definition.clone(), file);
            }
        }
        table.codes = codes;
        table
    }

    /// The table as JSON: `rules`, then each code as `[code, definition, start, end,
    /// line, column]` (line and column `null` outside the source), then each
    /// definition's `[path, logical path]`.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let codes: Vec<_> = self
            .codes
            .iter()
            .map(|(code, (definition, range))| {
                let position = self.positions[&(definition.clone(), range.start, range.end)];
                serde_json::json!([
                    code,
                    definition.to_string(),
                    range.start,
                    range.end,
                    position.map(|p| p.0),
                    position.map(|p| p.1),
                ])
            })
            .collect();
        let files: serde_json::Map<_, _> = self
            .files
            .iter()
            .map(|(definition, (path, logical))| {
                (definition.to_string(), serde_json::json!([path, logical]))
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "rules": self.rules,
            "codes": codes,
            "files": files,
        }))
        .expect("a site table is JSON")
    }

    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        if value["schema"] != 1 {
            return None;
        }
        let mut table = Self::default();
        for rule in value["rules"].as_array()? {
            table.rules.push(rule.as_str()?.to_owned());
        }
        for code in value["codes"].as_array()? {
            let [code, definition, start, end, line, column] = code.as_array()?.as_slice() else {
                return None;
            };
            let definition = crate::def_path(definition.as_str()?)?;
            let number = |v: &serde_json::Value| u32::try_from(v.as_u64()?).ok();
            let range = SiteRange {
                start: number(start)?,
                end: number(end)?,
            };
            let position = match (line, column) {
                (serde_json::Value::Null, serde_json::Value::Null) => None,
                (line, column) => Some((number(line)?, number(column)?)),
            };
            table
                .positions
                .insert((definition.clone(), range.start, range.end), position);
            table.codes.insert(code.as_u64()?, (definition, range));
        }
        for (definition, file) in value["files"].as_object()? {
            let [path, logical] = file.as_array()?.as_slice() else {
                return None;
            };
            table.files.insert(
                crate::def_path(definition)?,
                (path.as_str()?.to_owned(), logical.as_str()?.to_owned()),
            );
        }
        Some(table)
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
                    .file(&definition)
                    .map(|(_, logical)| logical)
                    .unwrap_or_else(|| "<core>".into());
                (code, definition.to_string(), line, column, file)
            })
            .collect(),
        rules: super::emit::failure_rules(program),
    }
}
