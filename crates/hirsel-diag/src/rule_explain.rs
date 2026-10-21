//! Read-only rule catalogue, separate from diagnostic-specific repairs.
//!
//! Primary guidance.tsv remains the authority for diagnostic fallbacks. The
//! related table transcribes tools/generate-diagnostics.py's RELATED mapping;
//! reference consistency tests detect drift without changing generated docs.

/// Established guidance. Repairs retain the diagnostic's `help: ` prefix and
/// are conditional advice, never an instruction to apply an automatic edit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuleGuidance {
    pub rule: &'static str,
    pub title: &'static str,
    pub meaning: &'static str,
    pub repair: Option<&'static str>,
    pub related: Vec<RuleGuidance>,
}

fn parse(line: &'static str) -> Option<RuleGuidance> {
    let mut fields = line.splitn(4, '\t');
    Some(RuleGuidance {
        rule: fields.next()?,
        title: fields.next()?,
        meaning: fields.next()?,
        repair: fields.next().filter(|text| !text.is_empty()),
        related: Vec::new(),
    })
}

pub(crate) fn primary_guidance() -> impl Iterator<Item = RuleGuidance> {
    include_str!("guidance.tsv").lines().filter_map(parse)
}

/// The byte-sorted, deduplicated catalogue of existing explanations, including
/// related-rule guidance from the diagnostic reference. An absent rule has no
/// explanation; this is deliberately not a catalogue of every language rule.
pub fn rule_guidance() -> Vec<RuleGuidance> {
    let mut entries: std::collections::BTreeMap<_, _> = primary_guidance()
        .map(|entry| (entry.rule, entry))
        .collect();
    for line in include_str!("rule_explain_related.tsv").lines() {
        let (parent, fields) = line.split_once('\t').expect("related guidance parent");
        let child = parse(fields).expect("related guidance fields");
        entries
            .get_mut(parent)
            .expect("catalogued parent")
            .related
            .push(child.clone());
        entries.entry(child.rule).or_insert(child);
    }
    entries.into_values().collect()
}

#[cfg(test)]
mod rule_explain_tests {
    use super::*;
    use crate::{Diagnostic, RuleId};
    use hirsel_span::{SourceMap, Span};

    #[test]
    fn catalogue_matches_every_primary_and_related_reference_entry() {
        let catalogue = rule_guidance();
        let reference = include_str!("../../../docs/diagnostics.md");
        let mut expected = std::collections::BTreeSet::new();
        let mut count = 0;
        for (level, source) in [
            ("##", include_str!("guidance.tsv")),
            ("###", include_str!("rule_explain_related.tsv")),
        ] {
            for line in source.lines() {
                let (parent, fields) = if level == "###" {
                    let (parent, fields) = line.split_once('\t').unwrap();
                    (Some(parent), fields)
                } else {
                    (None, line)
                };
                let entry = parse(fields).unwrap();
                RuleId::new(entry.rule).unwrap();
                expected.insert(entry.rule);
                let actual = if let Some(parent) = parent {
                    catalogue
                        .iter()
                        .find(|e| e.rule == parent)
                        .unwrap()
                        .related
                        .iter()
                        .find(|e| e.title == entry.title)
                        .unwrap()
                } else {
                    catalogue.iter().find(|e| e.rule == entry.rule).unwrap()
                };
                assert_eq!(
                    (actual.rule, actual.title, actual.meaning, actual.repair),
                    (entry.rule, entry.title, entry.meaning, entry.repair)
                );
                let repair = entry.repair.unwrap().strip_prefix("help: ").unwrap();
                assert!(reference.contains(&format!(
                    "{level} {}: {}\n\n{}\n\nTypical fix: {repair}\n",
                    entry.rule, entry.title, entry.meaning
                )));
                count += 1;
            }
        }
        assert_eq!(
            count,
            reference
                .lines()
                .filter(|s| s.starts_with("## ") || s.starts_with("### "))
                .count()
        );
        assert_eq!(
            catalogue.iter().map(|e| e.rule).collect::<Vec<_>>(),
            expected.into_iter().collect::<Vec<_>>()
        );
    }

    #[test]
    fn related_rules_do_not_change_ordinary_diagnostic_fallbacks() {
        let sources = SourceMap::default();
        for rule in ["CONC-5", "CONC-6", "MEM-11", "OWN-5", "FAIL-1"] {
            let diagnostic =
                Diagnostic::error(RuleId::new(rule).unwrap(), Span::default(), "failed");
            assert_eq!(diagnostic.to_json(&sources)["notes"], serde_json::json!([]));
        }
    }

    #[test]
    fn ordinary_json_keeps_its_fields_and_primary_span() {
        let mut sources = SourceMap::default();
        let file = sources.add("rule-explain-5.hir", "é x\n".into()).unwrap();
        let span = Span::new(sources.file_anchor(file), 3, 4).unwrap();
        let diagnostic = Diagnostic::error(RuleId::of("OWN-2"), span, "moved");
        let json = diagnostic.to_json(&sources);
        assert_eq!(
            json.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "fix",
                "message",
                "notes",
                "primary",
                "rule",
                "schema_version",
                "secondary",
                "severity"
            ]
        );
        assert_eq!(
            json["primary"],
            serde_json::json!({
                "file_id": 0, "file": "rule-explain-5.hir", "start": 3, "end": 4,
                "line": 1, "column": 3, "end_line": 1, "end_column": 4
            })
        );
        assert_eq!(
            json["notes"],
            serde_json::json!([primary_guidance()
                .find(|g| g.rule == "OWN-2")
                .unwrap()
                .repair
                .unwrap()])
        );
    }

    #[test]
    fn absent_repair_is_explicit() {
        assert_eq!(parse("A-1\tTitle\tMeaning\t").unwrap().repair, None);
        assert_eq!(parse("A-1\tTitle\tMeaning").unwrap().repair, None);
    }
}
