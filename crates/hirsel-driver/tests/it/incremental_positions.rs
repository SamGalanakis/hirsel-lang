use hirsel_driver::*;
use hirsel_engine::*;
fn path(name: &str) -> DefPath {
    let mut p = entry_path(&default_context());
    p.0.name = name.into();
    p
}
fn checked(engine: &mut QueryEngine, source: &str) -> (hirsel_span::FileId, CheckOutput) {
    let file = engine.sources.add("edits.hir", source.into()).unwrap();
    (
        file,
        check_progress(engine, file, &default_context()).unwrap(),
    )
}
fn json_diagnostics(output: &CheckOutput) -> Vec<String> {
    output
        .diagnostics
        .iter()
        .map(|d| {
            let mut json = d.to_json(&output.sources);
            json["primary"].as_object_mut().unwrap().remove("file_id");
            for label in json["secondary"].as_array_mut().unwrap() {
                label["span"].as_object_mut().unwrap().remove("file_id");
            }
            if let Some(fix) = json["fix"].as_object_mut() {
                fix["span"].as_object_mut().unwrap().remove("file_id");
            }
            json.to_string()
        })
        .collect()
}
fn diagnostics(engine: &mut QueryEngine, source: &str) -> Vec<String> {
    json_diagnostics(&checked(engine, source).1)
}
fn spans(engine: &mut QueryEngine, name: &str) -> Vec<(u32, u32, u32, u32)> {
    let definition = engine.lower_to_tir(&path(name)).unwrap().value;
    let site = |span: hirsel_span::Span| {
        let loc = engine.sources.location(span).unwrap();
        (span.start, span.end, loc.line, loc.column)
    };
    definition
        .bodies
        .iter()
        .flat_map(|b| {
            b.locals
                .iter()
                .map(|l| l.span)
                .chain(b.scopes.iter().map(|s| s.span))
                .chain(b.blocks.iter().flat_map(|b| {
                    std::iter::once(b.span).chain(b.instructions.iter().map(|i| i.span))
                }))
        })
        .map(site)
        .collect()
}
#[test]
fn moved_reused_diagnostics_and_lowered_sites_equal_a_fresh_session() {
    with_compiler_stack(|| {
        let initial = "fn a() -> u8 { 1u8 }\nfn b() -> u8 { a() }\n";
        let bad = format!("{initial}fn bad() -> u8 {{ nope }}\n");
        let versions = [
            initial.to_string(),
            bad.clone(),
            format!("// moved\n\n{bad}"),
            initial.to_string(),
            format!("\n\n{initial}"),
            "fn b() -> u8 { a() }\nfn a() -> u8 { 1u8 }\n".into(),
            initial.to_string(),
        ];
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        for source in versions {
            let actual = diagnostics(&mut engine, &source);
            let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
            assert_eq!(actual, diagnostics(&mut fresh, &source), "{source}");
            for name in ["a", "b"] {
                assert_eq!(
                    spans(&mut engine, name),
                    spans(&mut fresh, name),
                    "{name}: {source}"
                );
            }
        }
    });
}
#[test]
fn moving_an_item_rebases_cached_sites_without_semantic_recomputation() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let original = "fn a() -> u8 { 1u8 }\nfn b() -> u8 { a() }\n";
        diagnostics(&mut engine, original);
        let before = [
            QueryKind::Resolve,
            QueryKind::Signature,
            QueryKind::Check,
            QueryKind::Modes,
            QueryKind::Lower,
        ]
        .map(|k| engine.recompute_count(&path("b"), k));
        let retained = engine.lower_to_tir(&path("b")).unwrap().value;
        let retained_view = hirsel_lower::print::definition(&retained);
        let shifted = format!("// shift\n\n{original}");
        diagnostics(&mut engine, &shifted);
        let after = [
            QueryKind::Resolve,
            QueryKind::Signature,
            QueryKind::Check,
            QueryKind::Modes,
            QueryKind::Lower,
        ]
        .map(|k| engine.recompute_count(&path("b"), k));
        assert_eq!(before, after);
        assert_eq!(hirsel_lower::print::definition(&retained), retained_view);
        let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
        diagnostics(&mut fresh, &shifted);
        assert_eq!(spans(&mut engine, "b"), spans(&mut fresh, "b"));
    });
}

fn normalized_sites(file: &hirsel_syntax::SyntaxFile, mut view: String) -> String {
    for (prefix, sites) in [
        (
            "ExprId(",
            file.tree
                .expressions
                .iter()
                .map(|n| n.origin.span)
                .collect::<Vec<_>>(),
        ),
        (
            "StmtId(",
            file.tree.statements.iter().map(|n| n.origin.span).collect(),
        ),
        (
            "PatternId(",
            file.tree.patterns.iter().map(|n| n.origin.span).collect(),
        ),
        (
            "TypeExprId(",
            file.tree.types.iter().map(|n| n.origin.span).collect(),
        ),
        (
            "ItemId(",
            file.tree.items.iter().map(|n| n.origin.span).collect(),
        ),
    ] {
        let mut normalized = String::new();
        let mut remaining = view.as_str();
        while let Some((start, _)) = remaining
            .match_indices(prefix)
            .find(|(i, _)| *i == 0 || !remaining.as_bytes()[i - 1].is_ascii_alphabetic())
        {
            normalized.push_str(&remaining[..start]);
            remaining = &remaining[start + prefix.len()..];
            let end = remaining.find(')').unwrap();
            let id: usize = remaining[..end].parse().unwrap();
            let span = sites[id];
            normalized.push_str(&format!("{prefix}{}:{})", span.start, span.end));
            remaining = &remaining[end + 1..];
        }
        normalized.push_str(remaining);
        view = normalized;
    }
    view
}

fn observation(
    engine: &mut QueryEngine,
    source: &str,
) -> (
    String,
    Vec<String>,
    Vec<(String, String, Vec<(u32, u32, u32, u32)>)>,
) {
    let (file, output) = checked(engine, source);
    let ds = json_diagnostics(&output);
    let mut bodies = vec![];
    if ds.is_empty() && output.completion == CheckCompletion::Complete {
        let parsed = engine.parse_file(file, &default_context()).unwrap().value;
        for id in parsed.roots {
            let item = &parsed.tree.items[id.0 as usize];
            if !matches!(
                item.kind,
                hirsel_syntax::ItemKind::Function { body: Some(_), .. }
                    | hirsel_syntax::ItemKind::Const { .. }
                    | hirsel_syntax::ItemKind::Type { drop: Some(_), .. }
            ) {
                continue;
            }
            let definition = engine.lower_to_tir(&item.definition).unwrap().value;
            let (owner, _) = engine.parse_definition(&item.definition).unwrap().value;
            let printed = normalized_sites(&owner, hirsel_lower::print::definition(&definition));
            bodies.push((
                item.definition.0.name.clone(),
                printed,
                spans(engine, &item.definition.0.name),
            ));
        }
        bodies.sort_by(|a, b| a.0.cmp(&b.0));
    }
    (format!("{:?}", output.completion), ds, bodies)
}

fn edit_sequence(source: &str) -> Vec<String> {
    use hirsel_syntax::*;
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("seed.hir", source.into()).unwrap();
    let parsed = engine.parse_file(file, &default_context()).unwrap().value;
    let items: Vec<_> = parsed
        .roots
        .iter()
        .map(|id| &parsed.tree.items[id.0 as usize])
        .collect();
    let target = items
        .iter()
        .copied()
        .find(|item| {
            item.definition.0.name != "main"
                && matches!(item.kind, ItemKind::Function { body: Some(_), .. })
        })
        .unwrap();
    let ItemKind::Function {
        body: Some(body), ..
    } = target.kind
    else {
        unreachable!()
    };
    let body = parsed.tree.expressions[body.0 as usize].origin.span.start as usize;
    let start = target.origin.span.start as usize;
    let end = target.origin.span.end as usize;
    let ne = target.name_span.end as usize;
    let insert = |at, text| format!("{}{}{}", &source[..at], text, &source[at..]);
    let extra = ne + source[ne..].find('(').unwrap() + 1;
    let bad = format!(
        "{}\nfn zz_bad() -> u8 {{ zz_nope }}\n",
        source.trim_end_matches('\n')
    );
    let body_error = insert(body + 1, " let zz_e: u64 = zz_missing;");
    let reversed = format!(
        "{}{}{}",
        &source[..items[0].origin.span.start as usize],
        items
            .iter()
            .rev()
            .map(|i| &source[i.origin.span.start as usize..i.origin.span.end as usize])
            .collect::<Vec<_>>()
            .join("\n"),
        &source[items.last().unwrap().origin.span.end as usize..]
    );
    let other = items
        .iter()
        .copied()
        .find(|i| i.definition != target.definition)
        .unwrap();
    vec![
        source.into(),
        insert(body + 1, " let zz_probe_local: u64 = 7u64;"),
        source.into(),
        insert(body + 1, " /* note */"),
        format!("\n\n\n{source}"),
        source.into(),
        insert(extra, "zz_extra: u64, "),
        source.into(),
        format!("{}{}", &source[..end - 1], &source[end..]),
        source.into(),
        format!("{}{}", &source[..start], &source[end..]),
        source.into(),
        insert(ne, "_zz"),
        source.into(),
        reversed,
        source.into(),
        bad.clone(),
        format!("// shifted\n\n{bad}"),
        format!("// shifted\n\n{source}"),
        body_error.clone(),
        format!("\n\n{body_error}"),
        source.into(),
        format!(
            "{}\n{}\n",
            source.trim_end_matches('\n'),
            &source[other.origin.span.start as usize..other.origin.span.end as usize]
        ),
        source.into(),
    ]
}

#[test]
fn review_corpus_edit_matrix_equals_fresh_including_all_source_positions() {
    with_compiler_stack(|| {
        // The review's fourteen seeds and twenty-four edits, kept in-process.
        let seeds = [
            include_str!("../../../../conformance/CAP/CAP-2-plain-wrapper-inert-ok.hir"),
            include_str!("../../../../conformance/CLO/CLO-1-library-callback-forms.hir"),
            include_str!("../../../../conformance/CONC/CONC-6-last-sender-closes-receive.hir"),
            include_str!("../../../../conformance/CT/CT-2-dependent-constant.hir"),
            include_str!("../../../../conformance/DROP/DROP-2-move-argument.hir"),
            include_str!("../../../../conformance/FAIL/FAIL-3-unwind-resources.hir"),
            include_str!("../../../../conformance/GEN/GEN-1-flat-bytes-record-ok.hir"),
            include_str!("../../../../conformance/LANG/LANG-8-mapping-arity-ok.hir"),
            include_str!("../../../../conformance/MEM/MEM-1-projection-survives-owner-move.hir"),
            include_str!("../../../../conformance/MODE/MODE-2-sharing-counted-part-ok.hir"),
            include_str!("../../../../conformance/OWN/OWN-2-callee-owns-on-error.hir"),
            include_str!("../../../../conformance/PROV/PROV-1-branch-expiry-match-ok.hir"),
            include_str!("../../../../conformance/SEM/SEM-4-call-sources.hir"),
            include_str!("../../../../conformance/CAP/CAP-2-resource-drop-summary-ok.hir"),
        ];
        for (seed, source) in seeds.into_iter().enumerate() {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            for (step, source) in edit_sequence(source).into_iter().enumerate() {
                let actual = observation(&mut engine, &source);
                let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
                assert_eq!(
                    actual,
                    observation(&mut fresh, &source),
                    "seed {seed}, step {step}: {source}"
                );
            }
        }
    });
}

#[test]
fn restoring_a_renamed_definition_cannot_revive_old_positions() {
    with_compiler_stack(|| {
        let original = "type Wrap[T:plain]{value:T}\nfn need[T:inert](x:T){}\nfn probe(x:Wrap[u32]){need(x)}\n";
        let renamed = original.replace("fn need[", "fn need_zz[");
        let reordered = "fn probe(x:Wrap[u32]){need(x)}\nfn need[T:inert](x:T){}\ntype Wrap[T:plain]{value:T}\n";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        for source in [original, &renamed, original, reordered] {
            let actual = observation(&mut engine, source);
            let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
            assert_eq!(actual, observation(&mut fresh, source), "{source}");
        }
    });
}
