//! Rule-linked diagnostics and stable JSON (FAIL-1, LANG-6).

#[cfg(test)]
hirsel_guard::install!();

use hirsel_span::{SourceMap, Span};
use std::{borrow::Cow, fmt};
use unicode_width::UnicodeWidthStr;
/// A syntactically valid specification ID (FAIL-1); membership is checked against the spec.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct RuleId(Cow<'static, str>);
impl serde::Serialize for RuleId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
/// Decoded IDs are validated like any other external data.
impl<'de> serde::Deserialize<'de> for RuleId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(|_| serde::de::Error::custom("invalid rule ID"))
    }
}

impl RuleId {
    /// A fixed compiler rule, validated during constant evaluation.
    ///
    /// ```
    /// use hirsel_diag::RuleId;
    /// const RULE: RuleId = RuleId::of("OWN-6");
    /// assert_eq!(RULE.as_str(), "OWN-6");
    /// ```
    ///
    /// Invalid fixed IDs fail to compile:
    ///
    /// ```compile_fail
    /// use hirsel_diag::RuleId;
    /// const RULE: RuleId = RuleId::of("OWN-0");
    /// ```
    pub const fn of(value: &'static str) -> Self {
        assert!(Self::valid(value), "invalid rule ID; expected PREFIX-N");
        Self(Cow::Borrowed(value))
    }

    /// Validate an ID read from external data.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidRuleId> {
        let value = value.into();
        if Self::valid(&value) {
            Ok(Self(Cow::Owned(value)))
        } else {
            Err(InvalidRuleId(value))
        }
    }

    const fn valid(value: &str) -> bool {
        let bytes = value.as_bytes();
        let mut i = 0;
        while i < bytes.len() && bytes[i].is_ascii_uppercase() {
            i += 1;
        }
        if i == 0 || i >= bytes.len() || bytes[i] != b'-' {
            return false;
        }
        i += 1;
        if i >= bytes.len() || bytes[i] < b'1' || bytes[i] > b'9' {
            return false;
        }
        while i < bytes.len() {
            if !bytes[i].is_ascii_digit() {
                return false;
            }
            i += 1;
        }
        true
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn prefix(&self) -> &str {
        self.0.split_once('-').unwrap().0
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Invalid rule spelling (FAIL-1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidRuleId(String);
impl fmt::Display for InvalidRuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid rule ID {:?}; expected PREFIX-N", self.0)
    }
}
impl std::error::Error for InvalidRuleId {}

/// Diagnostic level; errors prevent execution (FAIL-1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}
impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Note => "note",
        }
    }
}
/// A related source site with an explanation (OWN-3, PROV-2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Label {
    pub span: Span,
    pub message: String,
}
/// One optional replacement; consumers must verify the source revision first (LANG-1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fix {
    pub span: Span,
    pub replacement: String,
    pub message: String,
}
/// What a diagnostic reports, beyond its rule, for section 23 selection and
/// cause folding. Producers set it; renderers and JSON v1 do not show it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Obligation {
    /// A required multiplicity: a once function where a many one is required (MODE-3).
    Multiplicity,
    /// A generic copy proof (GEN-1).
    CopyProof,
    /// A failed generic bound; `arguments` are the call arguments whose
    /// instantiation failed, so their parameter modes are not imposed (spec #38).
    GenericBound { arguments: Vec<Span> },
    /// A mode requirement that a callee parameter imposes on the argument at `argument`.
    ParameterMode { argument: Span },
}
/// Every language diagnostic cites exactly one rule (spec introduction, FAIL-1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub rule: RuleId,
    pub severity: Severity,
    pub message: String,
    pub primary: Span,
    pub secondary: Vec<Label>,
    pub notes: Vec<String>,
    pub fix: Option<Fix>,
    pub obligations: Vec<Obligation>,
    /// An opaque definition-local root-cause key (section 23: the same provided
    /// cause at the same site regardless of the failed requirement). Diagnostics with
    /// an equal key fold into the one section 23 selects first, and the others
    /// become its notes. Not shown by renderers or JSON v1.
    pub root_cause: Option<u64>,
}
impl Diagnostic {
    pub fn error(rule: RuleId, primary: Span, message: impl Into<String>) -> Self {
        Self {
            rule,
            primary,
            message: message.into(),
            severity: Severity::Error,
            secondary: vec![],
            notes: vec![],
            fix: None,
            obligations: vec![],
            root_cause: None,
        }
    }
    pub fn with_obligation(mut self, obligation: Obligation) -> Self {
        self.obligations.push(obligation);
        self
    }
    /// Prefer repairs proved by the producer; otherwise show the rule's typical
    /// repair. Human and JSON output consume exactly the same notes.
    fn rendered_notes(&self) -> impl Iterator<Item = &str> {
        let typical = (self.fix.is_none()
            && !self.notes.iter().any(|note| note.starts_with("help: ")))
        .then(|| {
            include_str!("guidance.tsv").lines().find_map(|line| {
                let mut fields = line.splitn(4, '\t');
                (fields.next()? == self.rule.as_str())
                    .then(|| fields.nth(2))
                    .flatten()
            })
        })
        .flatten();
        self.notes.iter().map(String::as_str).chain(typical)
    }
    /// Versioned object, with null locations for invalid spans; never panics on stale IDs.
    pub fn to_json(&self, sources: &SourceMap) -> serde_json::Value {
        use serde_json::json;
        json!({"schema_version": 1, "rule": self.rule.as_str(), "severity": self.severity.as_str(),
            "message": self.message, "primary": span_json(self.primary, sources),
            "secondary": self.secondary.iter().map(|s| json!({"span": span_json(s.span, sources), "message": s.message})).collect::<Vec<_>>(),
            "notes": self.rendered_notes().collect::<Vec<_>>(), "fix": self.fix.as_ref().map(|f| json!({"span": span_json(f.span, sources), "replacement": f.replacement, "message": f.message}))})
    }
    pub fn render_human(&self, sources: &SourceMap) -> String {
        let mut out = format!(
            "{}[{}]: {}\n",
            self.severity.as_str(),
            self.rule,
            self.message
        );
        render_site(&mut out, self.primary, "", sources);
        for label in &self.secondary {
            render_site(&mut out, label.span, &label.message, sources);
        }
        for note in self.rendered_notes() {
            // A note that begins `help: ` is a suggested repair.
            match note.strip_prefix("help: ") {
                Some(help) => out.push_str(&format!("  = help: {help}\n")),
                None => out.push_str(&format!("  = note: {note}\n")),
            }
        }
        if let Some(fix) = &self.fix {
            out.push_str(&format!(
                "  = help: {} (replace with {:?})\n",
                fix.message, fix.replacement
            ));
            render_site(&mut out, fix.span, "suggested replacement", sources);
        }
        out
    }
}
fn span_json(span: Span, sources: &SourceMap) -> serde_json::Value {
    use serde_json::json;
    let location = sources.location(span);
    let span = sources.resolve(span);
    let file = sources.file(span.file);
    let end = sources
        .file_slice(span)
        .and_then(|_| file?.location(span.end));
    json!({"file_id": file.map_or(span.file.0, |f| f.ordinal()), "file": file.map(|f| f.path.to_string_lossy()),
        "start": span.start, "end": span.end,
        "line": location.map(|l| l.line), "column": location.map(|l| l.column),
        "end_line": end.map(|l| l.line), "end_column": end.map(|l| l.column)})
}
fn render_site(out: &mut String, span: Span, message: &str, sources: &SourceMap) {
    let site = sources.resolve(span);
    if let (Some(file), Some(loc)) = (sources.file(site.file), sources.location(span)) {
        out.push_str(&format!(
            " --> {}:{}:{}\n",
            file.path.display(),
            loc.line,
            loc.column
        ));
        if let Some(line) = file.line(loc.line) {
            let width = sources
                .slice(span)
                .unwrap_or("")
                .split('\n')
                .next()
                .unwrap_or("")
                .width()
                .max(1);
            let prefix: String = line.chars().take((loc.column - 1) as usize).collect();
            let padding = prefix
                .split('\t')
                .map(|part| " ".repeat(part.width()))
                .collect::<Vec<_>>()
                .join("	");
            out.push_str(&format!(
                "{:>4} | {}\n     | {}{} {}\n",
                loc.line,
                line,
                padding,
                "^".repeat(width),
                message
            ));
        }
    } else {
        out.push_str(&format!(
            " --> <unavailable>:{}..{} {message}\n",
            site.start, site.end
        ));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rule_ids_are_validated() {
        const RULE: RuleId = RuleId::of("OWN-6");
        assert_eq!(RULE.prefix(), "OWN");
        assert_eq!(RULE, RuleId::new("OWN-6").unwrap());
        for valid in ["A-1", "LANG-14", "EXAMPLE-12345678901234567890"] {
            assert_eq!(RuleId::of(valid), RuleId::new(valid).unwrap());
        }
        for invalid in [
            "",
            "own-6",
            "OWN-0",
            "OWN-01",
            "OWN--6",
            "OWN-6x",
            "-6",
            "OWN-",
            "OWN-6-1",
            "\u{d6}WN-6",
            "OWN-６",
        ] {
            assert!(RuleId::new(invalid).is_err());
            assert!(std::panic::catch_unwind(|| RuleId::of(invalid)).is_err());
        }
    }
    #[test]
    fn repair_guidance_matches_json_human_and_reference() {
        let mut seen = std::collections::BTreeSet::new();
        let reference = include_str!("../../../docs/diagnostics.md");
        for line in include_str!("guidance.tsv").lines() {
            let fields = line.split('\t').collect::<Vec<_>>();
            let [rule, title, meaning, help] = fields.as_slice() else {
                panic!("invalid guidance: {line}");
            };
            let rule = RuleId::new(*rule).unwrap();
            assert!(seen.insert(rule.clone()), "duplicate {rule}");
            let fix = help.strip_prefix("help: ").unwrap();
            assert!(reference.contains(&format!(
                "## {rule}: {title}\n\n{meaning}\n\nTypical fix: {fix}\n"
            )));
            let sources = SourceMap::default();
            let diagnostic = Diagnostic::error(rule, Span::default(), "failed");
            assert_eq!(
                diagnostic.to_json(&sources)["notes"],
                serde_json::json!([help])
            );
            assert!(
                diagnostic
                    .render_human(&sources)
                    .contains(&format!("= help: {fix}"))
            );
        }
        assert_eq!(reference.matches("\n## ").count(), seen.len());
        assert!(seen.len() >= 15);
    }
    #[test]
    fn specific_repairs_supersede_typical_guidance() {
        let sources = SourceMap::default();
        let mut d = Diagnostic::error(RuleId::of("OWN-2"), Span::default(), "failed");
        d.notes.push("help: write `take xs` here".into());
        assert_eq!(
            d.to_json(&sources)["notes"],
            serde_json::json!(["help: write `take xs` here"])
        );
        d.notes.clear();
        d.fix = Some(Fix {
            span: Span::default(),
            replacement: "take xs".into(),
            message: "move the last use".into(),
        });
        assert_eq!(d.to_json(&sources)["notes"], serde_json::json!([]));
        let d = Diagnostic::error(RuleId::of("FAIL-1"), Span::default(), "failed");
        assert_eq!(d.to_json(&sources)["notes"], serde_json::json!([]));
    }
    #[test]
    fn stable_json_and_unicode_locations() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.hir", "é x\n".into()).unwrap();
        let span = Span::new(sources.file_anchor(file), 3, 4).unwrap();
        let mut d = Diagnostic::error(RuleId::new("OWN-2").unwrap(), span, "moved");
        d.secondary.push(Label {
            span,
            message: "taken here".into(),
        });
        d.fix = Some(Fix {
            span,
            replacement: "copy x".into(),
            message: "share".into(),
        });
        let json = d.to_json(&sources);
        assert_eq!(json["primary"]["column"], 3);
        assert_eq!(json["rule"], "OWN-2");
        assert_eq!(json["fix"]["replacement"], "copy x");
        assert!(d.render_human(&sources).contains("error[OWN-2]"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json.to_string()).unwrap(),
            json
        );
    }
    #[test]
    fn human_carets_follow_tabs_wide_text_and_combining_scalars() {
        for (source, padding, column) in [
            ("\tlet x: u8 = nope", "\t            ", 14),
            ("\t\"日本語\" + nope", "\t           ", 10),
            ("e\u{301} + nope", "    ", 6),
            ("👩‍💻 + nope", "     ", 7),
        ] {
            let mut sources = SourceMap::default();
            let file = sources.add("caret.hir", source.into()).unwrap();
            let start = source.find("nope").unwrap() as u32;
            let diagnostic = Diagnostic::error(
                RuleId::new("LANG-4").unwrap(),
                Span::new(sources.file_anchor(file), start, start + 4).unwrap(),
                "unresolved name",
            );
            let human = diagnostic.render_human(&sources);
            assert!(
                human.contains(&format!("     | {padding}^^^^ ")),
                "{source}: {human:?}"
            );
            assert_eq!(diagnostic.to_json(&sources)["primary"]["column"], column);
        }
        let mut sources = SourceMap::default();
        let file = sources.add("wide.hir", "日".into()).unwrap();
        let diagnostic = Diagnostic::error(
            RuleId::new("LANG-1").unwrap(),
            Span::new(sources.file_anchor(file), 0, 3).unwrap(),
            "unexpected character",
        );
        assert!(diagnostic.render_human(&sources).contains("     | ^^ "));
    }
}
