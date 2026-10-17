//! Corpus headers, driver classification and an additive passing ratchet.
use hirsel_diag::{RuleId, Severity};
use hirsel_engine::CheckOutput;
use hirsel_engine::{NotImplemented, RunOutcome};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Component, Path},
};

pub mod agree;
mod compiler;
pub mod corpus_format;
pub mod native;
pub use compiler::Observation;

pub type Rules = BTreeSet<RuleId>;

/// The specification's directory, relative to the repository root.
pub const SPEC_DIR: &str = "docs/spec";

/// The rule IDs one Markdown part of the spec defines: every `### ID: Title`
/// heading outside fenced code blocks. A heading whose text before `: ` is one
/// word must be a valid rule ID, and an ID may be defined only once.
pub fn extract_rules(markdown: &str) -> Result<Rules, String> {
    let mut rules = Rules::new();
    for id in rule_headings(markdown) {
        let id = RuleId::new(id).map_err(|e| e.to_string())?;
        if !rules.insert(id.clone()) {
            return Err(format!("spec rule {id} is defined twice"));
        }
    }
    if rules.is_empty() {
        return Err("spec defines no `### ID: Title` rules".into());
    }
    Ok(rules)
}

fn rule_headings(markdown: &str) -> impl Iterator<Item = &str> {
    let mut fenced = false;
    markdown.lines().filter_map(move |line| {
        if line.starts_with("```") {
            fenced = !fenced;
        }
        let (id, _) = line
            .strip_prefix("### ")
            .filter(|_| !fenced)?
            .split_once(": ")?;
        (!id.is_empty() && !id.contains(char::is_whitespace)).then_some(id)
    })
}

/// Every rule the spec defines, each with the part (`ownership.md`) defining
/// it. An ID defined in two parts is an error.
pub fn spec_definitions(root: &Path) -> Result<BTreeMap<RuleId, String>, String> {
    let dir = root.join(SPEC_DIR);
    let mut parts = fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    parts.retain(|path| path.extension().is_some_and(|e| e == "md"));
    parts.sort();
    let mut definitions = BTreeMap::new();
    for path in parts {
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for id in rule_headings(&text) {
            let id = RuleId::new(id).map_err(|e| format!("{name}: {e}"))?;
            if let Some(first) = definitions.insert(id.clone(), name.clone()) {
                return Err(format!("spec rule {id} is defined in {first} and {name}"));
            }
        }
    }
    if definitions.is_empty() {
        return Err(format!(
            "{} defines no `### ID: Title` rules",
            dir.display()
        ));
    }
    Ok(definitions)
}

/// The rule IDs the spec defines.
pub fn spec_rules(root: &Path) -> Result<Rules, String> {
    Ok(spec_definitions(root)?.into_keys().collect())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expect {
    Accept,
    Reject {
        rule: RuleId,
        line: usize,
    },
    Run {
        stdout: String,
        exit: i32,
    },
    Fail {
        rule: RuleId,
        line: usize,
        stdout: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Header {
    pub rule: RuleId,
    pub expect: Expect,
}

fn known_rule(value: &str, rules: &Rules) -> Result<RuleId, String> {
    let id = RuleId::new(value).map_err(|e| e.to_string())?;
    if !rules.contains(&id) {
        return Err(format!("unknown spec rule ID {id}"));
    }
    Ok(id)
}

/// Parse the leading comment block. Stdout directives each represent a literal line.
pub fn parse_header(path: &Path, source: &str, rules: &Rules) -> Result<Header, String> {
    let parse = || -> Result<Header, String> {
        let mut rule = None;
        let mut expect = None;
        let mut stdout = String::new();
        let mut saw_stdout = false;
        let mut exit = None;
        let mut entry = None;
        let mut in_header = true;
        let line_count = source.lines().count();
        for (index, raw) in source.lines().enumerate() {
            let line = raw.trim_start();
            if line.trim().is_empty() {
                continue;
            }
            let Some(comment) = line.strip_prefix("//") else {
                in_header = false;
                continue;
            };
            let comment = comment.trim_start();
            let Some((key, value)) = comment.split_once(':') else {
                if in_header
                    && ["rule", "expect", "stdout", "exit"]
                        .iter()
                        .any(|k| comment.split_whitespace().next() == Some(k))
                {
                    return Err(format!(
                        "line {}: malformed directive (missing ':')",
                        index + 1
                    ));
                }
                continue;
            };
            let directive = matches!(key, "rule" | "expect" | "stdout" | "exit");
            if !in_header {
                if directive {
                    return Err(format!("line {}: directive after source code", index + 1));
                }
                continue;
            }
            let value_trimmed = value.trim();
            match key {
                "rule" => {
                    if rule.is_some() {
                        return Err("duplicate rule directive".into());
                    }
                    rule = Some(known_rule(value_trimmed, rules)?);
                }
                "expect" => {
                    if expect.is_some() {
                        return Err("duplicate expect directive".into());
                    }
                    let words: Vec<_> = value_trimmed.split_whitespace().collect();
                    expect = Some(match words.as_slice() {
                        ["accept"] => Expect::Accept,
                        ["run"] => Expect::Run {
                            stdout: String::new(),
                            exit: 0,
                        },
                        [kind @ ("reject" | "fail"), id, "at", "line", number] => {
                            let rule = known_rule(id, rules)?;
                            if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
                                return Err("expected a positive line number".into());
                            }
                            let line: usize = number
                                .parse()
                                .map_err(|_| "expected a positive line number")?;
                            // Runtime failures may belong to an ordinary dependency
                            // source. The current header carries a line, not a file;
                            // execution still compares the actual failure site exactly.
                            // Static rejection diagnostics belong to this fixture.
                            if line == 0 || (*kind == "reject" && line > line_count) {
                                return Err(format!(
                                    "cited line {line} is outside file (1..={line_count})"
                                ));
                            }
                            if *kind == "reject" {
                                Expect::Reject { rule, line }
                            } else {
                                Expect::Fail {
                                    rule,
                                    line,
                                    stdout: None,
                                }
                            }
                        }
                        _ => return Err(format!("malformed expect directive: {value_trimmed:?}")),
                    });
                }
                "entry" => {
                    if entry.is_some()
                        || value_trimmed.is_empty()
                        || !value_trimmed.bytes().enumerate().all(|(i, b)| {
                            b.is_ascii_alphabetic() || b == b'_' || (i > 0 && b.is_ascii_digit())
                        })
                    {
                        return Err("entry must name one function".into());
                    }
                    entry = Some(value_trimmed.to_string());
                }
                "stdout" => {
                    if !matches!(expect, Some(Expect::Run { .. } | Expect::Fail { .. })) {
                        return Err("stdout must follow expect: run or fail".into());
                    }
                    saw_stdout = true;
                    stdout.push_str(value.strip_prefix(' ').unwrap_or(value));
                    stdout.push('\n');
                }
                "exit" => {
                    if !matches!(expect, Some(Expect::Run { .. })) {
                        return Err("exit must follow expect: run".into());
                    }
                    if exit.is_some() {
                        return Err("duplicate exit directive".into());
                    }
                    exit = Some(
                        value_trimmed
                            .parse::<i32>()
                            .map_err(|_| "exit must be an integer")?,
                    );
                }
                "spec"
                    if value_trimmed.strip_prefix('#').is_some_and(|n| {
                        !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
                    }) => {}
                _ => return Err(format!("line {}: unknown directive {key:?}", index + 1)),
            }
        }
        let rule = rule.ok_or("missing rule directive")?;
        let mut expect = expect.ok_or("missing expect directive")?;
        let prefix_dir = path
            .components()
            .nth(1)
            .and_then(|c| c.as_os_str().to_str());
        if path.components().next().map(|c| c.as_os_str()) != Some("conformance".as_ref())
            || prefix_dir != Some(rule.prefix())
        {
            return Err(format!(
                "rule {rule} must live under conformance/{}/",
                rule.prefix()
            ));
        }
        if let Expect::Run {
            stdout: output,
            exit: code,
        } = &mut expect
        {
            *output = stdout;
            *code = exit.unwrap_or(0);
        } else if let Expect::Fail { stdout: output, .. } = &mut expect {
            *output = saw_stdout.then_some(stdout);
        } else if saw_stdout || exit.is_some() {
            return Err("stdout/exit directives are only valid for expect: run".into());
        }
        Ok(Header { rule, expect })
    };
    parse().map_err(|error| format!("{}: {error}", path.display()))
}

/// Ordinary selected entry for main and Test corpus programs.
pub(crate) fn selected_entry(
    source: &str,
    context: &hirsel_engine::FileContext,
) -> hirsel_engine::DefPath {
    let mut entry = hirsel_engine::entry_path(context);
    for line in source
        .lines()
        .take_while(|line| line.trim().is_empty() || line.trim_start().starts_with("//"))
    {
        if let Some(name) = line.trim_start().strip_prefix("// entry:") {
            entry.0.name = name.trim().into();
        }
    }
    entry
}

/// Narrow adapter for the real driver and runner-owned fixtures.
pub trait Driver {
    fn evaluate(&self, path: &Path, expect: &Expect) -> (Status, Observation) {
        (classify(self, path, expect), Observation::default())
    }
    fn complete(&self) -> bool {
        true
    }
    fn check(&self, path: &Path) -> Result<CheckOutput, NotImplemented>;
    fn run(&self, path: &Path, stdout: &mut dyn Write) -> Result<RunOutcome, NotImplemented>;
}
/// A corpus driver varying only placement; capability inputs keep their seeds.
pub struct SeededCompiler(pub u64);
impl Driver for SeededCompiler {
    fn evaluate(&self, path: &Path, expect: &Expect) -> (Status, Observation) {
        compiler::evaluate_with_hash_seed(path, expect, self.0)
    }
    fn check(&self, path: &Path) -> Result<CheckOutput, NotImplemented> {
        Compiler.check(path)
    }
    fn run(&self, path: &Path, stdout: &mut dyn Write) -> Result<RunOutcome, NotImplemented> {
        Compiler.run(path, stdout)
    }
}
pub struct Compiler;
impl Driver for Compiler {
    fn evaluate(&self, path: &Path, expect: &Expect) -> (Status, Observation) {
        compiler::evaluate(path, expect)
    }
    fn check(&self, path: &Path) -> Result<CheckOutput, NotImplemented> {
        hirsel_driver::check_file(path)
    }
    fn run(&self, path: &Path, stdout: &mut dyn Write) -> Result<RunOutcome, NotImplemented> {
        hirsel_driver::run_file(path, stdout)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Status {
    Pass,
    Fail(String),
    NotImplemented,
}

pub fn classify(driver: &(impl Driver + ?Sized), path: &Path, expect: &Expect) -> Status {
    if !matches!(expect, Expect::Reject { .. }) && !driver.complete() {
        return Status::NotImplemented;
    }
    let output = match driver.check(path) {
        Ok(diagnostics) => diagnostics,
        Err(NotImplemented) => return Status::NotImplemented,
    };
    if (!matches!(expect, Expect::Reject { .. }) || output.diagnostics.is_empty())
        && output.completion != hirsel_engine::CheckCompletion::Complete
    {
        return Status::NotImplemented;
    }
    let diagnostics = &output.diagnostics;
    if let Expect::Reject { rule, line } = expect {
        return if diagnostics.first().is_some_and(|d| {
            &d.rule == rule
                && output
                    .sources
                    .location(d.primary)
                    .is_some_and(|p| p.line as usize == *line)
        }) {
            Status::Pass
        } else {
            Status::Fail(format!(
                "expected first rule diagnostic {rule} at line {line}; got {diagnostics:?}"
            ))
        };
    }
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Status::Fail(format!("unexpected error diagnostics: {diagnostics:?}"));
    }
    if matches!(expect, Expect::Accept) {
        return Status::Pass;
    }
    let mut stdout = Vec::new();
    let outcome = match driver.run(path, &mut stdout) {
        Ok(outcome) => outcome,
        Err(NotImplemented) => return Status::NotImplemented,
    };
    match (expect, &outcome) {
        (
            Expect::Run {
                stdout: expected,
                exit,
            },
            RunOutcome::Exit(actual),
        ) if expected.as_bytes() == stdout && exit == actual => Status::Pass,
        (
            Expect::Fail {
                rule,
                line,
                stdout: expected,
            },
            RunOutcome::Failure {
                rule: actual,
                line: site,
            },
        ) if rule == actual
            && line == site
            && expected
                .as_ref()
                .is_none_or(|expected| expected.as_bytes() == stdout) =>
        {
            Status::Pass
        }
        _ => Status::Fail(format!(
            "expected {expect:?}; got {outcome:?}, stdout {:?}",
            String::from_utf8_lossy(&stdout)
        )),
    }
}

#[derive(Clone, Debug)]
pub struct TestResult {
    pub path: String,
    pub prefix: String,
    pub status: Status,
}

#[derive(Default, Clone, Debug)]
pub struct Counts {
    pub validated: usize,
    pub passing: usize,
    pub failing: usize,
    pub not_implemented: usize,
}
impl Counts {
    fn add(&mut self, status: &Status) {
        self.validated += 1;
        match status {
            Status::Pass => self.passing += 1,
            Status::Fail(_) => self.failing += 1,
            Status::NotImplemented => self.not_implemented += 1,
        }
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"validated": self.validated, "passing": self.passing, "failing": self.failing, "not_implemented": self.not_implemented})
    }
}
impl std::fmt::Display for Counts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} validated / {} passing / {} failing / {} not implemented",
            self.validated, self.passing, self.failing, self.not_implemented
        )
    }
}

#[derive(Default, Debug)]
pub struct Report {
    pub counts: Counts,
    pub in_gate: Counts,
    pub exclusions: BTreeMap<String, Vec<String>>,
    pub observations: BTreeMap<String, Observation>,
    pub prefixes: BTreeMap<String, Counts>,
    pub tests: Vec<TestResult>,
    pub errors: Vec<String>,
    pub format_exemptions: BTreeMap<String, String>,
}
impl Report {
    pub fn success(&self) -> bool {
        self.errors.is_empty()
    }
    pub fn json(&self) -> serde_json::Value {
        // Retain the external unchecked counters as zero; validation is complete.
        serde_json::json!({
            "summary": self.counts.json(), "in_gate": self.in_gate.json(),
            "unchecked": 0,
            "format_exemptions": self.format_exemptions,
            "prefixes": self.prefixes.iter().map(|(p,c)| (p.clone(), c.json())).collect::<BTreeMap<_,_>>(),
            "tests": self.tests.iter().map(|t| {
                let (status, reason) = match &t.status { Status::Pass => ("pass", None), Status::Fail(reason) => ("fail", Some(reason)), Status::NotImplemented => ("not_implemented", None) };
                serde_json::json!({"path": t.path, "prefix": t.prefix, "status": status, "reason": reason, "exclusions": self.exclusions.get(&t.path).cloned().unwrap_or_default(), "in_gate": !self.exclusions.contains_key(&t.path), "actual": self.observations.get(&t.path).map(|o| &o.actual), "primary_external": self.observations.get(&t.path).map(|o| o.primary_external), "primary": self.observations.get(&t.path).and_then(|o| o.primary.as_ref()).map(|p| p.json()), "phase": self.observations.get(&t.path).map(|o| &o.phase), "unchecked": self.observations.get(&t.path).map(|_| 0)})
            }).collect::<Vec<_>>(),
            "errors": self.errors, "success": self.success()
        })
    }
}

fn discover(dir: &Path, files: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if kind.is_dir() {
            if entry.file_name() != "reports" {
                discover(&path, files)?;
            }
        } else if path.extension().is_some_and(|s| s == "hir") {
            if !kind.is_file() {
                return Err(format!("{}: test must be a regular file", path.display()));
            }
            files.push(path);
        }
    }
    Ok(())
}

/// Only regressions in the listed set gate execution. Updates never forget old paths.
pub fn ratchet(existing: &str, tests: &[TestResult], update: bool) -> Result<String, Vec<String>> {
    ratchet_within(existing, tests, update, None)
}

use hirsel_cli_args::Shard;
fn in_shard(shard: Option<Shard>, path: &str) -> bool {
    shard.is_none_or(|shard| shard.holds(path))
}

/// The ratchet over one shard: every listed path is still validated, but only
/// the shard's listed paths must be present and passing.
pub fn ratchet_within(
    existing: &str,
    tests: &[TestResult],
    update: bool,
    shard: Option<Shard>,
) -> Result<String, Vec<String>> {
    let mut listed = BTreeSet::new();
    let mut errors = Vec::new();
    let mut previous = None;
    for line in existing.lines() {
        let path = Path::new(line);
        if line.is_empty()
            || !line.starts_with("conformance/")
            || path.extension().is_none_or(|s| s != "hir")
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || line.contains('\\')
        {
            errors.push(format!("conformance/passing.txt: invalid path {line:?}"));
        }
        if previous.is_some_and(|p| p >= line) {
            errors.push("conformance/passing.txt: paths must be sorted and unique".into());
        }
        previous = Some(line);
        listed.insert(line.to_owned());
    }
    let statuses: BTreeMap<_, _> = tests.iter().map(|t| (t.path.as_str(), &t.status)).collect();
    for path in listed.iter().filter(|path| in_shard(shard, path)) {
        match statuses.get(path.as_str()) {
            Some(Status::Pass) => {}
            Some(status) => errors.push(format!("{path}: ratchet requires pass, got {status:?}")),
            None => errors.push(format!("{path}: ratchet path missing or failed validation")),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    if update {
        listed.extend(
            tests
                .iter()
                .filter(|t| t.status == Status::Pass)
                .map(|t| t.path.clone()),
        );
    }
    Ok(listed.into_iter().map(|p| p + "\n").collect())
}

pub fn run_suite(root: &Path, driver: &impl Driver, update: bool) -> Result<Report, String> {
    run_suite_shard(root, driver, update, None)
}

/// `run_suite` over one shard of the corpus. A shard never updates the ratchet.
pub fn run_suite_shard(
    root: &Path,
    driver: &impl Driver,
    update: bool,
    shard: Option<Shard>,
) -> Result<Report, String> {
    if update && shard.is_some() {
        return Err("--update-passing needs the whole corpus, not a shard".into());
    }
    let rules = spec_rules(root)?;
    let mut files = Vec::new();
    discover(&root.join("conformance"), &mut files)?;
    files.retain(|file| in_shard(shard, &relative(root, file)));
    files.sort();
    let mut report = Report::default();
    let runtime =
        fs::read_to_string(root.join("conformance/m2-runtime.txt")).map_err(|e| e.to_string())?;
    let runtime: BTreeSet<_> = runtime
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| line.split('\t').next().unwrap().to_owned())
        .collect();
    // Validate the whole corpus before invoking any compiler entry point.
    let mut headers = Vec::new();
    for file in files {
        let path = file.strip_prefix(root).map_err(|e| e.to_string())?;
        let source = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        match parse_header(path, &source, &rules) {
            Ok(header) => headers.push((
                file.clone(),
                path.to_string_lossy().replace('\\', "/"),
                header,
            )),
            Err(error) => report.errors.push(error),
        }
    }
    for (file, path, header) in headers {
        let (status, observation) = driver.evaluate(&file, &header.expect);
        if let Some(divergence) = &observation.checked {
            report.errors.push(format!("{path}: {divergence}"));
        }
        report.observations.insert(path.clone(), observation);
        let exclusions = gate_exclusions(
            &runtime,
            &path,
            &fs::read_to_string(&file).map_err(|e| e.to_string())?,
        );
        if exclusions.is_empty() {
            report.in_gate.add(&status);
        } else {
            report.exclusions.insert(path.clone(), exclusions);
        }
        let prefix = header.rule.prefix().to_owned();
        report.counts.add(&status);
        report
            .prefixes
            .entry(prefix.clone())
            .or_default()
            .add(&status);
        report.tests.push(TestResult {
            path,
            prefix,
            status,
        });
    }
    let passing = root.join("conformance/passing.txt");
    let existing =
        fs::read_to_string(&passing).map_err(|e| format!("{}: {e}", passing.display()))?;
    match ratchet_within(&existing, &report.tests, update, shard) {
        Ok(updated) if update && report.success() => {
            fs::write(&passing, updated).map_err(|e| format!("{}: {e}", passing.display()))?
        }
        Ok(_) => {}
        Err(errors) => report.errors.extend(errors),
    }
    Ok(report)
}

/// The repository-relative path the reports, the ratchet and shards key on.
pub(crate) fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Checked-in issue states; test execution never contacts GitHub.
fn gate_exclusions(runtime: &BTreeSet<String>, path: &str, source: &str) -> Vec<String> {
    let mut reasons = Vec::new();
    if runtime.contains(path) {
        reasons.push("M2 runtime".into());
    }
    for (offset, marker) in source.match_indices("// spec: #") {
        // Each occurrence, including comments indented inside a fixture.
        let start = offset + marker.len();
        let number: String = source[start..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if include_str!("open-spec-issues.txt")
            .lines()
            .any(|n| n == number)
        {
            reasons.push(format!("open spec issue #{number}"));
        }
    }
    reasons.sort();
    reasons.dedup();
    reasons
}
