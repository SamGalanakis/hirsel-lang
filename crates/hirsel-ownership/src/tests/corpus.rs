use super::super::*;
use super::source_support as support;
use hirsel_diag::{Diagnostic, Severity};
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
    run_with_authority(source, base).map(|(diagnostics, _, map)| (diagnostics, map))
}

fn run_with_authority(
    source: &str,
    base: &support::Registry,
) -> Result<(Vec<Diagnostic>, Vec<Diagnostic>, SourceMap), String> {
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
    let mut authority_diagnostics = vec![];
    for syntax in &file.items {
        let resolution = hirsel_resolve::resolve_item(
            &file,
            syntax,
            &registry.index.clone(),
            support::session(),
            &mut registry,
        );
        if let Some(d) = resolution.query.diagnostics.first() {
            return Err(format!(
                "dependency-owned resolver: {} line {}",
                d.rule,
                map.location(d.primary).unwrap().line
            ));
        }
        let signature = registry.signatures[&syntax.item.definition].clone();
        let output = hirsel_check::body::check_with_loop_protocols(
            hirsel_check::body::LoopInput {
                body: hirsel_check::body::Input {
                    types: support::session(),
                    memo: support::session_memo(),
                    syntax,
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
            .collect::<hirsel_authority::Intrinsics>();
        let mut authority = hirsel_authority::check_file_with_clauses(
            support::session(),
            support::session_memo(),
            syntax,
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
                        && syntax.tree.expressions.iter().any(|expression| {
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
        authority_diagnostics.extend(authority.diagnostics);
        let mut checked = authority.body;
        let output = check_ownership_with_clauses(
            support::session(),
            support::session_memo(),
            syntax,
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
        let (a_at, b_at) = (map.resolve(a.primary), map.resolve(b.primary));
        (
            a_at.start,
            a_at.end,
            a.rule.prefix(),
            suffix(a.rule.as_str()),
        )
            .cmp(&(
                b_at.start,
                b_at.end,
                b.rule.prefix(),
                suffix(b.rule.as_str()),
            ))
    });
    Ok((diagnostics, authority_diagnostics, map))
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
            if rejection.is_some_and(|(rule, _)| !owned(rule)) {
                count.2 += 1;
                pending.insert(name, "outside ownership operative-rule scope".to_string());
                continue;
            }
            count.1 += 1;
            let outcome = run_with_authority(&source, &base).and_then(|(ds, authority, map)| {
                let first_error = ds.iter().find(|d| d.severity == Severity::Error);
                if let Some((rule, line)) = rejection {
                    let line = line.parse::<u32>().unwrap();
                    // J owns the nested receiving-type contracts of these
                    // constructions. Its first error already satisfies the
                    // corpus rule and primary site; do not demand a second
                    // rejection from I or reinterpret these as acceptances.
                    if matches!(
                        name.as_str(),
                        "MODE/MODE-2-copied-part-type-argument-bad.hir"
                            | "MODE/MODE-2-inferred-box-explicit-bad.hir"
                            | "MODE/MODE-2-inferred-tuple-explicit-bad.hir"
                    ) {
                        let first = authority.iter().find(|d| d.severity == Severity::Error);
                        return match first {
                            Some(d)
                                if d.rule.as_str() == rule
                                    && map.location(d.primary).unwrap().line == line =>
                            {
                                Ok(())
                            }
                            _ => Err(format!(
                                "not done: expected J {rule} line {line}; got {first:?}"
                            )),
                        };
                    }
                    match first_error {
                        Some(d)
                            if d.rule.as_str() == rule
                                && map.location(d.primary).unwrap().line == line =>
                        {
                            Ok(())
                        }
                        _ if authority.iter().any(|d| {
                            d.severity == Severity::Error
                                && d.rule.as_str() == rule
                                && map.location(d.primary).unwrap().line == line
                        }) =>
                        {
                            Err(format!("dependency-owned J: {rule} line {line}"))
                        }
                        Some(d) => Err(format!(
                            "not done: expected {rule} line {line}; got {} line {} ({})",
                            d.rule,
                            map.location(d.primary).unwrap().line,
                            d.message
                        )),
                        None => Err(format!("not done: missing {rule} line {line}")),
                    }
                } else if let Some(d) = first_error {
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
                    if reason.starts_with("dependency-owned J:") {
                        count.1 -= 1;
                        count.2 += 1;
                    }
                    pending.insert(name, reason);
                }
            }
        }
        counts.insert(prefix, count);
    }
    let ratchet = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/passing.txt");
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
    let (empty, _, _) = support::parse("", &[]);
    let (diagnostics, map) = run(source, &support::registry(&empty)).unwrap();
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    let arguments: Vec<_> = diagnostics
        .iter()
        .map(|d| {
            assert_eq!(d.rule.as_str(), "MODE-1");
            map.slice(d.secondary[0].span).unwrap()
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

/// The site of a diagnostic: the pass that emitted it (J, the authority walk,
/// or I, the ownership flow) and its message without the explanation.
fn site(pass: &'static str, diagnostic: &Diagnostic) -> (&'static str, String) {
    let message = diagnostic.message.as_str();
    let head = message.split_once(": ").map_or(message, |(head, _)| head);
    (pass, head.to_string())
}

/// Sites that, in every corpus file reaching them, report their rule at the
/// same line as another site. The conformance runner checks only rule and
/// line, so one of them could stop firing unseen; each is pinned here by a
/// witness file on which it must still report.
const WITNESSED_SITES: [(&str, &str, &str); 5] = [
    (
        "MODE/MODE-1-arena-only-closure-call-taken-capture-bad.hir",
        "I",
        "a local result retains a temporary that ends with the function",
    ),
    (
        "MODE/MODE-6-escaping-capture-bad.hir",
        "J",
        "a scoped capability cannot enter an escaping capture",
    ),
    (
        "MODE/MODE-1-written-local-getter-bad.hir",
        "J",
        "an owned result retains a pre-existing local source bound",
    ),
    (
        "CONC/CONC-5-result-sendable-bad.hir",
        "J",
        "the callback result must be sendable",
    ),
    (
        "CONC/CONC-6-destructured-sender-bad.hir",
        "J",
        "channel payload must be sendable; Receivers cross only by task capture",
    ),
];

#[test]
fn every_duplicated_checker_site_has_its_own_witness() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance");
    let (empty, _, _) = support::parse("", &[]);
    let base = support::registry(&empty);
    // Per reject file, the sites reporting its expected rule at its line.
    let mut reporting = BTreeMap::<String, BTreeSet<(&str, String)>>::new();
    for prefix in ["OWN", "MODE", "DROP", "CLO", "MEM", "CONC", "CAP", "PROV"] {
        let mut files: Vec<_> = std::fs::read_dir(root.join(prefix))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "hir"))
            .collect();
        files.sort();
        for path in files {
            let source = std::fs::read_to_string(&path).unwrap();
            let Some((rule, line)) = source
                .lines()
                .find_map(|l| l.strip_prefix("// expect: reject "))
                .and_then(|r| r.split_once(" at line "))
            else {
                continue;
            };
            let line = line.parse::<u32>().unwrap();
            let Ok((ownership, authority, map)) = run_with_authority(&source, &base) else {
                continue;
            };
            let sites = ownership
                .iter()
                .map(|d| ("I", d))
                .chain(authority.iter().map(|d| ("J", d)))
                .filter(|(_, d)| {
                    d.severity == Severity::Error
                        && d.rule.as_str() == rule
                        && map.location(d.primary).unwrap().line == line
                })
                .map(|(pass, d)| site(pass, d))
                .collect();
            let name = path.strip_prefix(&root).unwrap().to_str().unwrap();
            reporting.insert(name.to_string(), sites);
        }
    }
    let sole: BTreeSet<_> = reporting
        .values()
        .filter(|sites| sites.len() == 1)
        .flatten()
        .collect();
    let mut problems = vec![];
    let witnessed: BTreeSet<_> = WITNESSED_SITES
        .iter()
        .map(|(file, pass, message)| {
            let site = (*pass, message.to_string());
            if !reporting
                .get(*file)
                .is_some_and(|sites| sites.contains(&site))
            {
                problems.push(format!(
                    "{pass} site `{message}` no longer reports on its witness {file}: \
                     it stopped firing, or the witness changed"
                ));
            }
            if sole.contains(&site) {
                problems.push(format!(
                    "{pass} site `{message}` now reports alone in some corpus file, which \
                     pins it: remove its WITNESSED_SITES row"
                ));
            }
            site
        })
        .collect();
    for (file, sites) in &reporting {
        for site in sites.iter().filter(|_| sites.len() > 1) {
            if !sole.contains(site) && !witnessed.contains(site) {
                problems.push(format!(
                    "{} site `{}` reports only beside another site (e.g. {file}): delete \
                     the redundant site, or add a WITNESSED_SITES row naming a witness",
                    site.0, site.1
                ));
            }
        }
    }
    problems.sort();
    problems.dedup();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn borrowed_counted_data_cannot_replace_preserved_resource_content() {
    let source = r#"resource type Request { input: Bytes, }
drop(s) {}
@preserves_captures(s)
fn read(bytes: Bytes, mut s: Request) -> () { s.input = copy bytes }
"#;
    let diagnostics = source_diagnostics(source);
    assert!(
        diagnostics.iter().any(|d| d.rule.as_str() == "CLO-4"),
        "{diagnostics:?}"
    );
}

#[test]
fn an_invalid_unique_annotation_does_not_cascade_to_its_binding_use() {
    let (empty, _, _) = support::parse("", &[]);
    let base = support::registry(&empty);
    let source =
        include_str!("../../../../conformance/MODE/MODE-2-annotated-binding-cascade-bad.hir");
    let (diagnostics, _) = run(source, &base).unwrap();
    let rules: Vec<_> = diagnostics.iter().map(|d| d.rule.as_str()).collect();
    assert_eq!(rules, vec!["MODE-2"]);
}

#[test]
fn constant_table_elements_are_not_carried_through_every_flow_state() {
    // Each element of a table literal is live until the literal consumes it,
    // so states and live sets that carried them grew quadratically in its length.
    let rows = |count: usize| {
        let elements: Vec<_> = (0..count).map(|i| format!("{}u64", i * 7 % 1000)).collect();
        let source = format!(
            "const TABLE: Inline[u64, {count}] = [{}]\nfn first() -> u64 {{ TABLE[0] }}\n",
            elements.join(", ")
        );
        let before = work::read(Counter::OwnershipStateRows);
        let diagnostics = source_diagnostics(&source);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        work::read(Counter::OwnershipStateRows) - before
    };
    let small = rows(1024);
    let large = rows(4096);
    assert!(large <= 8 * small + 1024, "small {small}, large {large}");
}

#[test]
fn nested_unique_exports_refuse_counted_aliases() {
    for source in [
        "fn load(take r: Result[unique Bytes, ()]) -> (Result[unique Bytes, ()], Result[unique Bytes, ()]) { (copy r, take r) }",
        "fn load(take r: Result[unique Bytes, ()]) -> Result[unique Bytes, ()] { let alias = copy r; take r }",
        "fn load(take r: (unique Bytes,)) -> ((unique Bytes,), (unique Bytes,)) { (copy r, take r) }",
        "type R[T] { xs: T }\nfn load(take r: R[unique Bytes]) -> (R[unique Bytes], R[unique Bytes]) { (copy r, take r) }",
        "type R { xs: unique Bytes }\nfn make() -> R { R(xs: b\"x\") }\nfn load() -> (unique Bytes, unique Bytes) { let r = make(); let alias = copy r; let R(xs: x) = take r; let R(xs: y) = take alias; (take x, take y) }",
    ] {
        let ds = source_diagnostics(source);
        assert!(
            ds.iter().any(|d| d.rule.as_str() == "MODE-2"),
            "{source}: {ds:?}"
        );
    }
}

#[test]
fn nested_unique_exports_keep_independent_and_shared_twins() {
    for source in [
        "fn load(take r: Result[Bytes, ()]) -> (Result[Bytes, ()], Result[Bytes, ()]) { (copy r, take r) }",
        "type R { xs: unique Bytes }\nfn load(r: R) -> R { unique r }",
        "type R { xs: unique Bytes }\nfn make() -> R { R(xs: b\"x\") }\nfn load() -> unique Bytes { let R(xs: x) = make(); take x }",
        "fn load(take r: Result[unique Bytes, ()]) -> (Result[Bytes, ()], Result[Bytes, ()]) { let shared: Result[Bytes, ()] = take r; (copy shared, take shared) }",
        "fn load(take r: Result[unique Bytes, ()]) -> (unique Result[unique Bytes, ()], Result[unique Bytes, ()]) { (unique r, take r) }",
        "fn load(take r: Result[unique Bytes, ()], take s: Result[unique Bytes, ()]) -> (Result[unique Bytes, ()], Result[unique Bytes, ()]) { (take r, take s) }",
    ] {
        let ds = source_diagnostics(source);
        assert!(ds.is_empty(), "{source}: {ds:?}");
    }
}

#[test]
fn nested_unique_borrowed_views_keep_conditional_nominal_fields() {
    let source = "type R { xs: unique List[Bytes] }\nfn probe(take r: R) { let built = box unique { fn(agree_x: R) { agree_x }(r) }.xs; let frozen = shared.freeze(take built) }";
    let ds = source_diagnostics(source);
    assert!(ds.is_empty(), "{ds:?}");
}

#[test]
fn nested_unique_mutable_handbacks_preserve_payload_contracts() {
    let ds = source_diagnostics("fn share(mut r: (unique Bytes,)) { let alias = copy r }");
    assert!(ds.iter().any(|d| d.rule.as_str() == "MODE-2"), "{ds:?}");
    let ds = source_diagnostics(
        "fn touch(mut r: (unique Bytes,)) {}\nfn load(take r: (unique Bytes,)) -> (unique Bytes,) { let mut held = take r; touch(mut held); take held }",
    );
    assert!(ds.is_empty(), "{ds:?}");
}

#[test]
fn nested_unique_projected_mutation_cannot_retain_unique_payloads() {
    let ds = source_diagnostics(
        "fn weaken(mut bytes: Bytes) {}\nfn load(take r: List[unique Bytes]) -> List[unique Bytes] { let mut held = take r; weaken(mut held[0]); take held }",
    );
    assert!(ds.iter().any(|d| d.rule.as_str() == "MODE-2"), "{ds:?}");
}
