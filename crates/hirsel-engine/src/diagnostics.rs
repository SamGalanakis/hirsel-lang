//! docs/spec/diagnostics.md: prerequisite guards live at query boundaries; select each operation's
//! property obligation before ordering independent sites by source path and byte.
use hirsel_diag::{Diagnostic, Obligation, Severity};
use hirsel_span::SourceMap;

/// Read a std schema converter's explanatory comments only when the primary
/// is within its following single-line adapter. These are diagnostic hints,
/// never semantic metadata; malformed or absent comments add nothing.
pub(crate) fn schema_hook_context(
    text: &str,
    start: usize,
    end: usize,
) -> Option<(&str, &str, &str)> {
    schema_hook_contexts(text)
        .find(|(range, _, _, _)| start >= range.start && end <= range.end)
        .map(|(_, parent, member, contract)| (parent, member, contract))
}

pub(crate) fn schema_hook_contexts(
    text: &str,
) -> impl Iterator<Item = (std::ops::Range<usize>, &str, &str, &str)> {
    let mut lines = text.split_inclusive('\n');
    let mut offset = 0usize;
    std::iter::from_fn(move || {
        loop {
            let line = lines.next()?;
            offset += line.len();
            let Some(context) = line.strip_prefix("// schema-hook ").map(str::trim) else {
                continue;
            };
            let contract = lines.next()?;
            offset += contract.len();
            let Some(contract) = contract.strip_prefix("// schema-contract ").map(str::trim) else {
                continue;
            };
            let converter = lines.next()?;
            let start = offset;
            offset += converter.len();
            let Some((parent, member)) = context.split_once('\t') else {
                continue;
            };
            return Some((start..offset, parent, member, contract));
        }
    })
}

fn stage(d: &Diagnostic) -> u8 {
    let rule = d.rule.as_str();
    let has = |wanted: fn(&Obligation) -> bool| d.obligations.iter().any(wanted);
    // A required multiplicity is a property of the operation, ordered after
    // identity and mode; a repeated once invocation is an availability prerequisite.
    if rule == "MODE-3" && has(|o| matches!(o, Obligation::Multiplicity)) {
        return 8;
    }
    // A generic copy proof is a mode obligation; other bounds are properties.
    if rule == "GEN-1" && !has(|o| matches!(o, Obligation::CopyProof)) {
        return 10;
    }
    if rule == "CAP-2" && has(|o| matches!(o, Obligation::GenericBound { .. })) {
        return 10;
    }
    match rule {
        "OWN-2" | "MODE-3" => 0,
        "OWN-1" | "LANG-15" | "LANG-5" | "OWN-8" => 1,
        "OWN-5" | "GEN-1" => 2,
        "OWN-3" => 3,
        "MEM-9" | "CAP-4" | "CAP-1" | "CAP-2" => 4,
        "PROV-5" | "PROV-2" | "PROV-1" | "PROV-4" => 5,
        "MODE-1" => 6,
        "MODE-2" => 7,
        "MODE-4" => 9,
        "MEM-5" => 11,
        _ => 10,
    }
}
fn specialized(special: &str, general: &str) -> bool {
    matches!(
        (special, general),
        ("DROP-3", "OWN-1" | "OWN-5" | "LANG-7")
            | ("MODE-6", "MODE-1" | "CLO-1")
            | ("CLO-3", "MODE-4" | "OWN-5" | "CAP-2")
            | ("CONC-5", "MODE-2" | "MODE-4" | "CAP-2" | "GEN-1")
            | ("CONC-6", "MODE-4" | "CAP-2" | "GEN-1")
            | ("PROV-5", "PROV-1" | "PROV-4")
            | ("PROV-2", "PROV-1")
            | ("CAP-3", "CAP-1" | "CAP-2" | "CLO-2")
            | ("CLO-2", "CAP-1" | "CAP-2")
            | ("MEM-4" | "MEM-12" | "MEM-13" | "MEM-11", "GEN-1" | "LANG-7")
            | ("MEM-11", "MODE-2" | "CAP-2")
            | ("MEM-13", "MODE-1")
            | ("SEM-4" | "FACT-1" | "FACT-2", "LANG-7")
    )
}
/// Spec #38: an invalid type-argument instantiation
/// cannot impose parameter mode defaults on the arguments it names.
fn failed_instantiation(bound: &Diagnostic, mode: &Diagnostic) -> bool {
    bound.primary == mode.primary
        && bound.obligations.iter().any(|b| {
            let Obligation::GenericBound { arguments } = b else {
                return false;
            };
            mode.obligations.iter().any(|m| {
                matches!(m, Obligation::ParameterMode { argument } if arguments.contains(argument))
            })
        })
}
/// Diagnostics of one definition with an equal root cause report one defect:
/// the first that section 23 selects is reported, and the others become its notes.
fn fold_root_causes(diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut folded: Vec<Diagnostic> = Vec::with_capacity(diagnostics.len());
    for diagnostic in diagnostics {
        let first = diagnostic.root_cause.and_then(|root| {
            folded
                .iter()
                .position(|d| d.root_cause == Some(root) && d.severity == diagnostic.severity)
        });
        let Some(first) = first else {
            folded.push(diagnostic);
            continue;
        };
        let first = &mut folded[first];
        first.notes.push(format!(
            "this also fails: {}: {}",
            diagnostic.rule, diagnostic.message
        ));
        first.secondary.push(hirsel_diag::Label {
            span: diagnostic.primary,
            message: format!(
                "this also fails here for the same cause ({})",
                diagnostic.rule
            ),
        });
        // Each folded obligation keeps its requirement label and any causal
        // steps or verified repairs that differ from the representative.
        for label in diagnostic.secondary {
            if !first.secondary.contains(&label) {
                first.secondary.push(label);
            }
        }
        for note in diagnostic.notes {
            if !first.notes.contains(&note) {
                first.notes.push(note);
            }
        }
    }
    folded
}
/// Where a diagnostic's primary site stands. Without sources, an anchor stands
/// for its own file.
fn site(sources: Option<&SourceMap>, d: &Diagnostic) -> hirsel_span::FileSpan {
    sources.map_or(
        hirsel_span::FileSpan {
            file: hirsel_span::FileId(d.primary.anchor.0),
            start: d.primary.start,
            end: d.primary.end,
        },
        |s| s.resolve(d.primary),
    )
}
/// Equal current sites can have different retained anchors after a declaration
/// is removed and introduced again. Compare every diagnostic field, locating
/// its spans for deduplication just as ordering already locates the primary.
fn located(
    mut diagnostic: Diagnostic,
    sources: &SourceMap,
    files: &mut std::collections::BTreeMap<std::path::PathBuf, hirsel_span::Anchor>,
) -> Diagnostic {
    let mut span = |span| {
        let at = sources.resolve(span);
        if let Some(file) = sources.file(at.file) {
            hirsel_span::Span {
                anchor: *files.entry(file.path.clone()).or_insert(file.anchor),
                start: at.start,
                end: at.end,
            }
        } else {
            span
        }
    };
    diagnostic.primary = span(diagnostic.primary);
    for label in &mut diagnostic.secondary {
        label.span = span(label.span);
    }
    if let Some(fix) = &mut diagnostic.fix {
        fix.span = span(fix.span);
    }
    for obligation in &mut diagnostic.obligations {
        match obligation {
            Obligation::GenericBound { arguments } => {
                for argument in arguments {
                    *argument = span(*argument);
                }
            }
            Obligation::ParameterMode { argument } => *argument = span(*argument),
            Obligation::Multiplicity | Obligation::CopyProof => {}
        }
    }
    diagnostic
}
pub fn select_diagnostics(
    mut diagnostics: Vec<Diagnostic>,
    sources: Option<&SourceMap>,
) -> Vec<Diagnostic> {
    for diagnostic in &mut diagnostics {
        if diagnostic.rule.as_str() == "SEM-3"
            && diagnostic
                .message
                .starts_with("f-strings format through std@1, which this program does not select")
            && let Some(source) = sources.and_then(|s| s.file(s.resolve(diagnostic.primary).file))
        {
            let mut sidecar = source.path.as_os_str().to_os_string();
            sidecar.push(".json");
            let note = format!(
                "source sidecar: {}",
                std::path::Path::new(&sidecar).display()
            );
            if !diagnostic.notes.contains(&note) {
                diagnostic.notes.push(note);
            }
        }
    }
    let bounds = diagnostics.clone();
    diagnostics.retain(|d| {
        !matches!(d.rule.as_str(), "MODE-1" | "MODE-2" | "MODE-3" | "MODE-4")
            || !bounds.iter().any(|bound| failed_instantiation(bound, d))
    });
    let original = diagnostics.clone();
    diagnostics.retain(|d| {
        !original.iter().any(|other| {
            site(sources, other) == site(sources, d)
                && specialized(other.rule.as_str(), d.rule.as_str())
        })
    });
    let selected = diagnostics.clone();
    diagnostics.retain(|d| {
        !selected.iter().any(|prerequisite| {
            let (p, d_at) = (site(sources, prerequisite), site(sources, d));
            stage(prerequisite) == 0
                && stage(d) > 0
                && p.file == d_at.file
                && d_at.start <= p.start
                && d_at.end >= p.end
        })
    });
    diagnostics.sort_by(|a, b| {
        let path = |d: &Diagnostic| {
            sources
                .and_then(|s| s.file(s.resolve(d.primary).file))
                .map(|f| f.path.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let (a_at, b_at) = (site(sources, a), site(sources, b));
        // Warnings never change which error is reported first.
        let warning = |d: &Diagnostic| d.severity != Severity::Error;
        (warning(a), path(a), a_at.start, a_at.end)
            .cmp(&(warning(b), path(b), b_at.start, b_at.end))
            .then_with(|| stage(a).cmp(&stage(b)))
            .then_with(|| a.rule.prefix().cmp(b.rule.prefix()))
            .then_with(|| {
                a.rule
                    .as_str()
                    .split_once('-')
                    .unwrap()
                    .1
                    .parse::<u32>()
                    .unwrap()
                    .cmp(
                        &b.rule
                            .as_str()
                            .split_once('-')
                            .unwrap()
                            .1
                            .parse::<u32>()
                            .unwrap(),
                    )
            })
    });
    diagnostics.dedup_by(|a, b| {
        a == b
            || sources.is_some_and(|sources| {
                let mut files = std::collections::BTreeMap::new();
                located(a.clone(), sources, &mut files) == located(b.clone(), sources, &mut files)
            })
    });
    fold_root_causes(diagnostics)
}
#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_diag::{RuleId, Severity};
    use hirsel_span::{Anchor, FileId, Span};
    fn diag(rule: &str, start: u32) -> Diagnostic {
        Diagnostic {
            rule: RuleId::new(rule).unwrap(),
            severity: Severity::Error,
            message: rule.into(),
            primary: Span::new(Anchor(0), start, start + 1).unwrap(),
            secondary: vec![],
            notes: vec![],
            fix: None,
            obligations: vec![],
            root_cause: None,
        }
    }
    #[test]
    fn duplicate_current_sites_ignore_retained_anchor_identity() {
        let mut sources = SourceMap::default();
        let file = sources.add("main.hir", "abc\n".into()).unwrap();
        let first = sources.item_anchor(file, 0);
        let snapshot = sources.add_snapshot(file, "abc\n".into()).unwrap();
        let second = sources.item_anchor(snapshot, 0);
        let mut a = diag("LANG-7", 0);
        a.primary.anchor = first;
        let mut b = a.clone();
        b.primary.anchor = second;
        assert_eq!(
            select_diagnostics(vec![a.clone(), b.clone()], Some(&sources)).len(),
            1
        );
        sources.move_anchor(second, file, 1);
        assert_eq!(select_diagnostics(vec![a, b], Some(&sources)).len(), 2);
    }
    #[test]
    fn section_23_specialized_owners_override_restatements() {
        for (special, general) in [
            ("DROP-3", "LANG-7"),
            ("MODE-6", "MODE-1"),
            ("CLO-3", "MODE-4"),
            ("CONC-5", "CAP-2"),
            ("CONC-5", "MODE-2"),
            ("MEM-11", "MODE-2"),
            ("MEM-11", "CAP-2"),
            ("MEM-13", "MODE-1"),
            ("CONC-6", "GEN-1"),
            ("SEM-4", "LANG-7"),
            ("FACT-2", "LANG-7"),
            ("PROV-5", "PROV-1"),
            ("CAP-3", "CLO-2"),
        ] {
            let ds = select_diagnostics(vec![diag(general, 1), diag(special, 1)], None);
            assert_eq!(ds.len(), 1);
            assert_eq!(ds[0].rule.as_str(), special);
        }
    }
    #[test]
    fn independent_obligations_use_source_order_not_property_stage() {
        let ds = select_diagnostics(vec![diag("OWN-2", 8), diag("MEM-5", 2)], None);
        assert_eq!(ds[0].rule.as_str(), "MEM-5");
    }
    #[test]
    fn manifest_paths_and_numeric_rule_suffixes_break_ties() {
        let mut sources = SourceMap::default();
        sources.add("z.hir", "x".into());
        sources.add("a.hir", "x".into());
        let mut a = diag("MEM-10", 0);
        let mut b = diag("MEM-2", 0);
        a.primary.anchor = sources.file_anchor(FileId(0));
        b.primary.anchor = sources.file_anchor(FileId(1));
        let first = select_diagnostics(vec![a, b], Some(&sources))[0].primary;
        assert_eq!(sources.resolve(first).file, FileId(1));
        assert_eq!(
            select_diagnostics(vec![diag("MEM-10", 0), diag("MEM-2", 0)], None)[0]
                .rule
                .as_str(),
            "MEM-2"
        );
    }
}

#[cfg(test)]
mod obligation_tests {
    use super::*;
    use hirsel_diag::{Diagnostic, RuleId};
    use hirsel_span::{Anchor, Span};
    #[test]
    fn section_23_operand_read_precedes_enclosing_call_properties() {
        let call = Diagnostic::error(
            const { RuleId::of("CLO-3") },
            Span::new(Anchor(0), 1, 20).unwrap(),
            "crossing",
        );
        let read = Diagnostic::error(
            const { RuleId::of("OWN-2") },
            Span::new(Anchor(0), 8, 9).unwrap(),
            "use after move",
        );
        assert_eq!(
            select_diagnostics(vec![call, read], None)[0].rule.as_str(),
            "OWN-2"
        );
    }
    #[test]
    fn section_23_required_modes_follow_identity_and_mode_order() {
        let site = Span::new(Anchor(0), 1, 2).unwrap();
        let multiplicity = Diagnostic::error(const { RuleId::of("MODE-3") }, site, "any text")
            .with_obligation(Obligation::Multiplicity);
        for rule in ["PROV-1", "MODE-1", "MODE-2"] {
            let other = Diagnostic::error(RuleId::new(rule).unwrap(), site, "required property");
            assert_eq!(
                select_diagnostics(vec![multiplicity.clone(), other], None)[0]
                    .rule
                    .as_str(),
                rule
            );
        }
    }
}

#[cfg(test)]
mod instantiation_tests {
    use super::*;
    use hirsel_diag::RuleId;
    use hirsel_span::{Anchor, Span};
    #[test]
    fn failed_generic_bounds_suppress_dependent_modes_but_preserve_operand_errors() {
        let call = Span::new(Anchor(0), 0, 30).unwrap();
        let argument = Span::new(Anchor(0), 5, 10).unwrap();
        for rule in ["MODE-4", "CAP-2", "GEN-1"] {
            let bound = Diagnostic::error(RuleId::new(rule).unwrap(), call, "failed bound")
                .with_obligation(Obligation::GenericBound {
                    arguments: vec![argument],
                });
            for mode in ["MODE-1", "MODE-2", "MODE-3", "MODE-4"] {
                let derivative =
                    Diagnostic::error(RuleId::new(mode).unwrap(), call, "required mode")
                        .with_obligation(Obligation::ParameterMode { argument });
                assert_eq!(
                    select_diagnostics(vec![bound.clone(), derivative.clone()], None),
                    vec![bound.clone()]
                );
                // Another argument's mode requirement is independent of the failed bound.
                let other = Span::new(Anchor(0), 12, 14).unwrap();
                // A required multiplicity carries its obligation, as the checker reports it.
                let mut independent =
                    Diagnostic::error(RuleId::new(mode).unwrap(), call, "required mode")
                        .with_obligation(Obligation::ParameterMode { argument: other });
                if mode == "MODE-3" {
                    independent = independent.with_obligation(Obligation::Multiplicity);
                }
                assert_eq!(
                    select_diagnostics(vec![bound.clone(), independent], None).len(),
                    2
                );
            }
            let availability = Diagnostic::error(const { RuleId::of("OWN-2") }, argument, "moved");
            assert_eq!(
                select_diagnostics(vec![bound, availability.clone()], None),
                vec![availability]
            );
        }
    }
}

#[cfg(test)]
mod fold_tests {
    use super::*;
    use hirsel_diag::RuleId;
    use hirsel_span::{Anchor, Span};
    fn rooted(rule: &str, start: u32, root: Option<u64>) -> Diagnostic {
        let mut d = Diagnostic::error(
            RuleId::new(rule).unwrap(),
            Span::new(Anchor(0), start, start + 1).unwrap(),
            format!("{rule} message"),
        );
        d.root_cause = root;
        d
    }
    #[test]
    fn folded_requirements_keep_their_labels_and_the_first_headline() {
        let first = rooted("MODE-1", 10, Some(7));
        let mut later = rooted("CLO-4", 20, Some(7));
        later.secondary.push(hirsel_diag::Label {
            span: Span::new(Anchor(0), 30, 31).unwrap(),
            message: "required: an escaping closure owns global captures".into(),
        });
        let ds = select_diagnostics(vec![later.clone(), first.clone()], None);
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].message, first.message);
        assert_eq!(ds[0].primary, first.primary);
        assert!(ds[0].secondary.contains(&later.secondary[0]));
        assert_eq!(ds[0].notes, vec!["this also fails: CLO-4: CLO-4 message"]);
    }
    #[test]
    fn folding_follows_section_23_selection_across_rules() {
        // The stage-0 availability error precedes the property at the same
        // site; the property folds into it rather than replacing it.
        let ds = select_diagnostics(
            vec![rooted("MODE-1", 5, Some(1)), rooted("OWN-2", 5, Some(1))],
            None,
        );
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].rule.as_str(), "OWN-2");
        // A diagnostic a filter removes never becomes a fold representative.
        let mut special = rooted("MODE-6", 9, None);
        special.root_cause = None;
        let ds = select_diagnostics(
            vec![
                rooted("MODE-1", 9, Some(2)),
                special,
                rooted("MODE-1", 12, Some(2)),
            ],
            None,
        );
        assert_eq!(
            ds.iter()
                .map(|d| (d.rule.as_str(), d.primary.start))
                .collect::<Vec<_>>(),
            vec![("MODE-6", 9), ("MODE-1", 12)]
        );
    }
    #[test]
    fn distinct_or_absent_root_causes_never_fold_and_warnings_follow_errors() {
        let ds = select_diagnostics(
            vec![
                rooted("MODE-1", 1, Some(1)),
                rooted("MODE-1", 2, Some(2)),
                rooted("MODE-1", 3, None),
                rooted("MODE-1", 4, None),
            ],
            None,
        );
        assert_eq!(ds.len(), 4);
        let mut warning = rooted("OWN-5", 0, None);
        warning.severity = Severity::Warning;
        let ds = select_diagnostics(vec![warning, rooted("MODE-1", 9, None)], None);
        assert_eq!(ds[0].severity, Severity::Error);
        assert_eq!(ds[1].severity, Severity::Warning);
    }
}
