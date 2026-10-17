use super::super::*;
use super::source_support as support;
use hirsel_diag::Diagnostic;
use hirsel_resolve::{ManifestFixture, ManifestIndex};
use hirsel_span::SourceMap;
use hirsel_syntax as ast;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

fn owned(rule: &str) -> bool {
    matches!(
        rule,
        "OWN-1"
            | "OWN-2"
            | "OWN-3"
            | "OWN-4"
            | "OWN-5"
            | "OWN-7"
            | "OWN-8"
            | "MODE-1"
            | "MODE-2"
            | "MODE-3"
            | "MODE-7"
            | "CLO-1"
            | "CLO-4"
    )
}

pub(super) fn run(
    source: &str,
    base: &support::Registry,
) -> Result<(Vec<Diagnostic>, SourceMap), String> {
    let (file, map, ds) = support::parse(source, &[]);
    if let Some(d) = ds.first() {
        return Err(format!(
            "dependency-owned lexer/parser: {} line {}",
            d.rule,
            map.location(d.primary).unwrap().line
        ));
    }
    let (index, ds) = ManifestIndex::new(
        ManifestFixture::from_files(
            support::context().package,
            [&file],
            ast::ContentHash::default(),
        )
        .with_dependency_signatures(base.signatures.values()),
    );
    if let Some(d) = ds.first() {
        return Err(format!("dependency-owned resolver index: {}", d.rule));
    }
    let mut registry = support::Registry {
        signatures: base.signatures.clone(),
        index,
        reads: vec![],
        builtins: base.builtins.clone(),
    };
    support::normalize_file(&file, &mut registry)
        .map_err(|e| format!("dependency-owned signature: {e}"))?;
    let mut diagnostics = vec![];
    for item in &file.roots {
        let resolution = hirsel_resolve::resolve_item(
            &file,
            *item,
            &file.symbols,
            &registry.index.clone(),
            &mut registry,
        );
        if let Some(d) = resolution.query.diagnostics.first() {
            return Err(format!(
                "dependency-owned resolver: {} line {}",
                d.rule,
                map.location(d.primary).unwrap().line
            ));
        }
        let signature = registry.signatures[&file.tree.items[item.0 as usize].definition].clone();
        let output = hirsel_check::body::check_with_loop_protocols(
            hirsel_check::body::LoopInput {
                body: hirsel_check::body::Input {
                    tree: &file.tree,
                    item: *item,
                    symbols: &file.symbols,
                    context: &support::context(),
                    resolution: &resolution,
                    signature: &signature,
                },
                protocols: &hirsel_types::core_roles::loop_protocols(),
            },
            &mut registry,
        )
        .map_err(|e| format!("dependency-owned body checker/provider: {e:?}"))?;
        if let Some(d) = output.checked.diagnostics.first() {
            return Err(format!(
                "dependency-owned body checker: {} line {}: {}",
                d.rule,
                map.location(d.primary).unwrap().line,
                d.message
            ));
        }

        let clauses: Vec<_> = output
            .plans
            .contracts
            .iter()
            .map(|clause| (clause.expression, clause.function))
            .collect();
        let loops: Vec<_> = output
            .plans
            .loops
            .iter()
            .filter_map(|plan| plan.cursor().cloned())
            .collect();
        let intrinsics = hirsel_types::core_roles::intrinsic_roles()
            .into_iter()
            .collect::<crate::authority::Intrinsics>();
        let mut authority = crate::authority::check_file_with_clauses(
            &file,
            *item,
            &signature,
            output.checked.value,
            &mut registry,
            &intrinsics,
            &loops,
            &clauses,
        );
        if let Some(failure) = authority.failure {
            return Err(format!("dependency-owned authority/provider: {failure:?}"));
        }

        // An implicit mapper has no ordinary argument in I's CFG. J checks
        // its taken payload's receiving contract; keep that MODE-2 judgment
        // in this rule/site oracle instead of excluding the corpus program.
        diagnostics.extend(
            authority
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.rule.as_str() == "MODE-2"
                        && file.tree.expressions.iter().any(|expression| {
                            expression.origin.span == diagnostic.primary
                                && matches!(
                                    expression.kind,
                                    ast::ExprKind::Try {
                                        map_error: Some(_),
                                        ..
                                    }
                                )
                        })
                })
                .cloned(),
        );

        let rows: Vec<_> = authority
            .captures
            .iter()
            .flat_map(|table| {
                let context = table.context;
                table
                    .captures
                    .iter()
                    .enumerate()
                    .map(move |(ordinal, capture)| CaptureMode {
                        expression: table.expression,
                        ordinal: ordinal as u32,
                        owner: capture.owner.clone(),
                        usage: capture.usage,
                        context,
                    })
            })
            .collect();
        // This oracle checks I's diagnostics. J's judgments supply real capture
        // layouts, but its independent diagnostics belong to its own ratchet.
        let mut checked = authority.body;
        let output = check_ownership_with_clauses(
            &file.tree,
            &file.symbols,
            *item,
            &signature,
            &mut checked,
            &rows,
            &clauses,
            &mut authority.result_types,
            &[],
            &[],
            &mut registry,
        )
        .map_err(|e| format!("ownership provider: {e:?}"))?;
        diagnostics.extend(output.diagnostics);
    }
    diagnostics.sort_by(|a, b| {
        (
            a.primary.start,
            a.primary.end,
            a.rule.prefix(),
            suffix(a.rule.as_str()),
        )
            .cmp(&(
                b.primary.start,
                b.primary.end,
                b.rule.prefix(),
                suffix(b.rule.as_str()),
            ))
    });
    Ok((diagnostics, map))
}
#[test]
fn source_corpus_ratchets_rule_and_primary_line() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance");
    let (empty, _, _) = support::parse("", &[]);
    let base = support::registry(&empty);
    let mut passing = BTreeSet::new();
    let mut pending = BTreeMap::new();
    let mut counts = BTreeMap::new();
    for prefix in ["OWN", "MODE", "DROP", "CLO"] {
        let mut files: Vec<_> = std::fs::read_dir(root.join(prefix))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "hir"))
            .collect();
        files.sort();
        let mut count = (0, 0, 0);
        for path in files {
            let source = std::fs::read_to_string(&path).unwrap();
            let name = path
                .strip_prefix(&root)
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let expected = source
                .lines()
                .find_map(|l| l.strip_prefix("// expect: "))
                .unwrap();
            let rejection = expected
                .strip_prefix("reject ")
                .map(|r| r.split_once(" at line ").unwrap());
            if matches!(
                name.as_str(),
                "CLO/CLO-4-capture-authority-declared-bad.hir"
                    | "MODE/MODE-5-authority-union-bad.hir"
            ) {
                count.2 += 1;
                pending.insert(
                    name,
                    "dependency-owned J: declared escaping authority (CLO-4)".to_string(),
                );
                continue;
            }
            if matches!(
                name.as_str(),
                "CLO/CLO-2-annotation-nested-uses-part-bad.hir"
                    | "CLO/CLO-4-annotated-tuple-callable-return-bad.hir"
                    | "CLO/CLO-4-contract6-held-selection-bad.hir"
            ) {
                count.2 += 1;
                pending.insert(
                    name,
                    "dependency-owned J: written annotation contract (CLO-4)".to_string(),
                );
                continue;
            }
            if name == "MODE/MODE-2-callable-parameter-unique-bad.hir" {
                count.2 += 1;
                pending.insert(
                    name,
                    "dependency-owned J: instantiated callable header (MODE-2)".to_string(),
                );
                continue;
            }
            // Mutable callable multiplicity is a fixed receiving contract
            // checked by J, not I's current ownership evidence. The full
            // source pipeline and independent V control cover this case.
            if name == "MODE/MODE-3-mut-callable-multiplicity-bad.hir" {
                count.2 += 1;
                pending.insert(
                    name,
                    "dependency-owned J: invariant mutable callable contract (MODE-3)".to_string(),
                );
                continue;
            }
            // Nested type-graph contracts are J's shared view relation, not
            // I's current root ownership evidence. Their producer tests and
            // integrated corpus independently check these new diagnostics.
            if matches!(
                name.as_str(),
                "MODE/MODE-2-nested-view-copy-kept-bad.hir"
                    | "MODE/MODE-2-nested-view-mut-bad.hir"
                    | "MODE/MODE-2-nested-view-strengthen-bad.hir"
                    | "MODE/MODE-2-nested-view-phantom-bad.hir"
            ) {
                count.2 += 1;
                pending.insert(
                    name,
                    "dependency-owned J: nested receiving contract (MODE-2)".to_string(),
                );
                continue;
            }
            if rejection.is_some_and(|(rule, _)| !owned(rule)) {
                count.2 += 1;
                pending.insert(name, "outside ownership operative-rule scope".to_string());
                continue;
            }
            count.1 += 1;
            let outcome = run(&source, &base).and_then(|(ds, map)| {
                if let Some((rule, line)) = rejection {
                    let line = line.parse::<u32>().unwrap();
                    match ds.first() {
                        Some(d)
                            if d.rule.as_str() == rule
                                && map.location(d.primary).unwrap().line == line =>
                        {
                            Ok(())
                        }
                        Some(d) => Err(format!(
                            "not done: expected {rule} line {line}; got {} line {} ({})",
                            d.rule,
                            map.location(d.primary).unwrap().line,
                            d.message
                        )),
                        None => Err(format!("not done: missing {rule} line {line}")),
                    }
                } else if let Some(d) = ds.first() {
                    Err(format!(
                        "not done: unexpected {} line {} ({})",
                        d.rule,
                        map.location(d.primary).unwrap().line,
                        d.message
                    ))
                } else {
                    Ok(())
                }
            });
            match outcome {
                Ok(()) => {
                    count.0 += 1;
                    passing.insert(name);
                }
                Err(reason) => {
                    pending.insert(name, reason);
                }
            }
        }
        counts.insert(prefix, count);
    }
    let ratchet = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ownership/tests/passing.txt");
    eprintln!(
        "ownership corpus (passing, eligible, other operative rules): {counts:?}; {} passing, {} pending",
        passing.len(),
        pending.len()
    );
    for (name, reason) in &pending {
        if !reason.starts_with("outside ownership") {
            eprintln!("ownership pending {name}: {reason}");
        }
    }
    let mut regressions = vec![];
    for line in std::fs::read_to_string(&ratchet)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        if !passing.contains(line) {
            regressions.push(format!("{line}: {:?}", pending.get(line)));
        }
    }
    assert!(
        regressions.is_empty(),
        "ownership corpus regressions:\n{}",
        regressions.join("\n")
    );
    if std::env::var_os("HIRSEL_BLESS").is_some() {
        std::fs::write(
            &ratchet,
            passing.iter().cloned().collect::<Vec<_>>().join("\n") + "\n",
        )
        .unwrap();
        std::fs::write(
            ratchet.with_file_name("pending.txt"),
            pending
                .iter()
                .map(|(name, reason)| format!("{name} # {reason}"))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
    }
    for (name, reason) in &pending {
        assert!(
            !reason.starts_with("not done:"),
            "unresolved ownership obligation: {name}: {reason}"
        );
    }
    assert!(!passing.is_empty());
}

pub(super) fn source_diagnostics(source: &str) -> Vec<Diagnostic> {
    let (empty, _, _) = support::parse("", &[]);
    let base = support::registry(&empty);
    run(source, &base).unwrap().0
}

#[test]
fn call_mode_obligations_identify_their_independent_arguments() {
    let source = "fn need(take x:Bytes, take y:Bytes) {}\nfn example(take x:Bytes in x, take y:Bytes in y) { need(take x, take y) }";
    let diagnostics = source_diagnostics(source);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    let arguments: Vec<_> = diagnostics
        .iter()
        .map(|d| {
            assert_eq!(d.rule.as_str(), "MODE-1");
            let span = d.secondary[0].span;
            &source[span.start as usize..span.end as usize]
        })
        .collect();
    assert_eq!(arguments, vec!["take x", "take y"]);
}

#[test]
fn finalizers_cannot_move_or_copy_their_whole_self_but_can_read_fields() {
    for use_self in ["take x", "copy x", "unique x"] {
        let source = format!("resource type Value {{ n:u8 }} drop(x) {{ let _ = {use_self} }}");
        let diagnostics = source_diagnostics(&source);
        assert_eq!(
            diagnostics.first().map(|d| d.rule.as_str()),
            Some("DROP-3"),
            "{diagnostics:?}"
        );
    }
    let diagnostics = source_diagnostics(
        "fn keep(take v:Value) {}\nresource type Value { n:u8 } drop(x) { let action = fn() { keep(take x) } }",
    );
    assert_eq!(
        diagnostics.first().map(|d| d.rule.as_str()),
        Some("DROP-3"),
        "{diagnostics:?}"
    );
    let diagnostics = source_diagnostics(
        "type Stop = StopNow()\nresource type Value { status:Result[(),Stop] } drop(x) { let _ = x.status }",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn packed_variant_payloads_overlap_but_same_variant_fields_are_disjoint() {
    let (empty, _, _) = support::parse("", &[]);
    let base = support::registry(&empty);
    let header = "type Choice = A(x:u32,y:u32) | B(z:u32)\nfn pair(mut a:u32,mut b:u32) {}\n";
    let source = format!(
        "{header}fn example(mut value:Packed[Choice]) {{\n  pair(mut value.A.x,mut value.B.z)\n}}"
    );
    let (diagnostics, map) = run(&source, &base).unwrap();
    assert_eq!(diagnostics[0].rule.as_str(), "OWN-3");
    assert_eq!(map.location(diagnostics[0].primary).unwrap().line, 4);
    for body in [
        "fn example(mut value:Packed[Choice]) { pair(mut value.A.x,mut value.A.y) }",
        "fn example(mut first:Packed[Choice],mut second:Packed[Choice]) { pair(mut first.A.x,mut second.B.z) }",
    ] {
        let (diagnostics, _) = run(&format!("{header}{body}"), &base).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}

#[test]
fn checked_nested_payloads_and_sequence_roots_preserve_overlap_owners() {
    let header = "type Inner = A(x:u32,y:u32) | B(z:u32)\ntype Outer = Wrap(payload:Packed[Inner]) | Other(n:u32)\nfn pair(mut a:u32,mut b:u32) {}\n";
    let (empty, _, _) = support::parse("", &[]);
    let base = support::registry(&empty);
    let bad = format!(
        "{header}fn example(mut value:Packed[Outer]) {{\n pair(mut value.Wrap.payload.A.x,mut value.Wrap.payload.B.z)\n}}"
    );
    let (diagnostics, map) = run(&bad, &base).unwrap();
    assert_eq!(diagnostics[0].rule.as_str(), "OWN-3");
    assert_eq!(map.location(diagnostics[0].primary).unwrap().line, 5);
    let good = format!(
        "{header}fn example(mut value:Packed[Outer]) {{ pair(mut value.Wrap.payload.A.x,mut value.Wrap.payload.A.y) }}"
    );
    assert!(run(&good, &base).unwrap().0.is_empty());
    for (source, rule, line) in [
        (
            "fn pair(mut elem:u32,mut all:Array[u32]) {}\nfn example(mut value:Array[u32]) {\n pair(mut value[0],mut value)\n}",
            "OWN-3",
            3,
        ),
        (
            "type Rec { n:u32 }\nfn pair(mut a:u32,mut b:u32) {}\nfn example(mut value:Rec) {\n let alias=value\n pair(mut value.n,mut alias.n)\n}",
            "LANG-5",
            5,
        ),
    ] {
        let (diagnostics, map) = run(source, &base).unwrap();
        assert_eq!(diagnostics[0].rule.as_str(), rule);
        assert_eq!(map.location(diagnostics[0].primary).unwrap().line, line);
    }
}
