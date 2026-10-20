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
/// Each lowered site's file offsets and position; synthesized sites
/// (`Span::default()`) have no anchor and no position.
fn spans(engine: &mut QueryEngine, name: &str) -> Vec<(u32, u32, u32, u32)> {
    let definition = engine.lower_to_tir(&path(name)).unwrap().value;
    let site = |span: hirsel_span::Span| {
        let at = engine.sources.resolve(span);
        let loc = engine.sources.location(span);
        assert!(
            loc.is_some() || span == hirsel_span::Span::default(),
            "{span:?}"
        );
        (
            at.start,
            at.end,
            loc.map_or(0, |l| l.line),
            loc.map_or(0, |l| l.column),
        )
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
fn moving_an_item_keeps_cached_sites_without_semantic_recomputation() {
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

fn normalized_sites(
    sources: &hirsel_span::SourceMap,
    owner: &hirsel_syntax::ItemSyntax,
    mut view: String,
) -> String {
    let tree = &owner.tree;
    for (prefix, sites) in [
        (
            "ExprId(",
            tree.expressions
                .iter()
                .map(|n| n.origin.span)
                .collect::<Vec<_>>(),
        ),
        (
            "StmtId(",
            tree.statements.iter().map(|n| n.origin.span).collect(),
        ),
        (
            "PatternId(",
            tree.patterns.iter().map(|n| n.origin.span).collect(),
        ),
        (
            "TypeExprId(",
            tree.types.iter().map(|n| n.origin.span).collect(),
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
            let span = sources.resolve(sites[id]);
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
        for syntax in &parsed.items {
            let item = &syntax.item;
            if !matches!(
                item.kind,
                hirsel_syntax::ItemKind::Function { body: Some(_), .. }
                    | hirsel_syntax::ItemKind::Const { .. }
                    | hirsel_syntax::ItemKind::Type { drop: Some(_), .. }
            ) {
                continue;
            }
            let definition = engine.lower_to_tir(&item.definition).unwrap().value;
            let owner = engine.parse_definition(&item.definition).unwrap().value;
            let printed = normalized_sites(
                &engine.sources,
                &owner,
                hirsel_lower::print::definition(&definition),
            );
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
    // Item-relative spans resolve to the seed's absolute offsets.
    let at = |span| engine.sources.resolve(span);
    let syntaxes: Vec<_> = parsed.items.iter().collect();
    let items: Vec<_> = syntaxes.iter().map(|s| &s.item).collect();
    let target_syntax = syntaxes
        .iter()
        .find(|syntax| {
            syntax.item.definition.0.name != "main"
                && matches!(syntax.item.kind, ItemKind::Function { body: Some(_), .. })
        })
        .unwrap();
    let target = &target_syntax.item;
    let ItemKind::Function {
        body: Some(body), ..
    } = target.kind
    else {
        unreachable!()
    };
    let body = at(target_syntax.tree.expressions[body.0 as usize].origin.span).start as usize;
    let start = at(target.origin.span).start as usize;
    let end = at(target.origin.span).end as usize;
    let ne = at(target.name_span).end as usize;
    let insert = |at, text| format!("{}{}{}", &source[..at], text, &source[at..]);
    let extra = ne + source[ne..].find('(').unwrap() + 1;
    let bad = format!(
        "{}\nfn zz_bad() -> u8 {{ zz_nope }}\n",
        source.trim_end_matches('\n')
    );
    let body_error = insert(body + 1, " let zz_e: u64 = zz_missing;");
    let text = |item: &Item| {
        let span = at(item.origin.span);
        &source[span.start as usize..span.end as usize]
    };
    let reversed = format!(
        "{}{}{}",
        &source[..at(items[0].origin.span).start as usize],
        items
            .iter()
            .rev()
            .map(|i| text(i))
            .collect::<Vec<_>>()
            .join("\n"),
        &source[at(items.last().unwrap().origin.span).end as usize..]
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
        format!("{}\n{}\n", source.trim_end_matches('\n'), text(other)),
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

const SEMANTIC: [QueryKind; 5] = [
    QueryKind::Resolve,
    QueryKind::Signature,
    QueryKind::Check,
    QueryKind::Modes,
    QueryKind::Lower,
];
fn recomputes(engine: &QueryEngine, name: &str) -> [usize; 5] {
    SEMANTIC.map(|kind| engine.recompute_count(&path(name), kind))
}

/// Checks every file in order and observes each executable item, as `observation` does for one.
fn observe_files(
    engine: &mut QueryEngine,
    files: &[(&str, &str)],
) -> (
    Vec<String>,
    Vec<(String, String, Vec<(u32, u32, u32, u32)>)>,
) {
    let ids: Vec<_> = files
        .iter()
        .map(|(name, text)| engine.sources.add(name, (*text).into()).unwrap())
        .collect();
    for id in &ids {
        engine.parse_file(*id, &default_context()).unwrap();
    }
    let mut ds = vec![];
    let mut complete = true;
    for id in &ids {
        let output = check_progress(engine, *id, &default_context()).unwrap();
        complete &= output.completion == CheckCompletion::Complete;
        ds.extend(json_diagnostics(&output));
    }
    let mut bodies = vec![];
    if ds.is_empty() && complete {
        for id in &ids {
            let parsed = engine.parse_file(*id, &default_context()).unwrap().value;
            for syntax in &parsed.items {
                let item = &syntax.item;
                if !matches!(
                    item.kind,
                    hirsel_syntax::ItemKind::Function { body: Some(_), .. }
                ) {
                    continue;
                }
                let definition = engine.lower_to_tir(&item.definition).unwrap().value;
                let printed = normalized_sites(
                    &engine.sources,
                    syntax,
                    hirsel_lower::print::definition(&definition),
                );
                bodies.push((
                    item.definition.0.name.clone(),
                    printed,
                    spans(engine, &item.definition.0.name),
                ));
            }
        }
        bodies.sort_by(|a, b| a.0.cmp(&b.0));
    }
    (ds, bodies)
}

#[test]
fn inserting_an_item_above_keeps_the_other_items_syntax_ids_and_memos() {
    with_compiler_stack(|| {
        let original = "fn a() -> u8 { 1u8 }\nfn b() -> u8 { let x = a()\n x }\n";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        assert!(diagnostics(&mut engine, original).is_empty());
        let syntax = |engine: &mut QueryEngine, name: &str| {
            engine.parse_definition(&path(name)).unwrap().value
        };
        let before = ["a", "b"].map(|name| (syntax(&mut engine, name), recomputes(&engine, name)));
        let lowered = engine.lower_to_tir(&path("b")).unwrap().value;
        for inserted in [
            format!("fn z() -> u8 {{ 2u8 }}\n{original}"),
            format!("// a comment\n\nfn z() -> u8 {{ 2u8 }} // trailing\n{original}"),
        ] {
            assert!(diagnostics(&mut engine, &inserted).is_empty());
            for (name, (old, counts)) in ["a", "b"].iter().zip(&before) {
                let new = syntax(&mut engine, name);
                assert!(std::sync::Arc::ptr_eq(old, &new), "{name}: {inserted}");
                assert_eq!(new.owner(), old.owner());
                assert_eq!(recomputes(&engine, name), *counts, "{name}: {inserted}");
            }
            // The retained TIR is the very same value, at item-relative sites.
            assert_eq!(engine.lower_to_tir(&path("b")).unwrap().value, lowered);
            let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
            assert_eq!(
                observation(&mut engine, &inserted),
                observation(&mut fresh, &inserted),
                "{inserted}"
            );
        }
    });
}

#[test]
fn a_cross_file_move_keeps_ids_and_memos_and_changes_only_the_anchor() {
    with_compiler_stack(|| {
        let a = "fn a() -> u8 { 1u8 }\n";
        let b = "fn b() -> u8 { let x = a()\n x }\n";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let first = format!("{a}{b}");
        observe_files(
            &mut engine,
            &[("one.hir", &first), ("two.hir", "fn c() {}\n")],
        );
        let old = engine.parse_definition(&path("b")).unwrap().value;
        let (old_file, _) = engine.sources.anchor(old.anchor).unwrap();
        let counts = recomputes(&engine, "b");
        let second = format!("fn c() {{}}\n{b}");
        let moved = [("one.hir", a), ("two.hir", second.as_str())];
        let warm = observe_files(&mut engine, &moved);
        let new = engine.parse_definition(&path("b")).unwrap().value;
        assert!(std::sync::Arc::ptr_eq(&old, &new));
        let (new_file, base) = engine.sources.anchor(new.anchor).unwrap();
        assert_ne!(new_file, old_file);
        assert_eq!(
            engine.sources.file(new_file).unwrap().path.to_str(),
            Some("two.hir")
        );
        assert_eq!(base as usize, "fn c() {}\n".len());
        assert_eq!(recomputes(&engine, "b"), counts);
        let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
        assert_eq!(warm, observe_files(&mut fresh, &moved));
    });
}

#[test]
fn duplicate_definitions_report_at_the_later_name_warm_and_cold() {
    with_compiler_stack(|| {
        let a = "fn a() -> u8 { 1u8 }\n";
        let dup = "fn a() -> u8 { 2u8 }\n";
        let b = "fn b() -> u8 { a() }\n";
        let one_a_b = format!("{a}{b}");
        let one_a_dup_b = format!("{a}{dup}{b}");
        let one_dup_a_b = format!("{dup}{a}{b}");
        let two_dup = format!("// two\n{dup}");
        // (one.hir, two.hir) per step: insert, remove and move, in one file and across.
        let steps: [(&str, &str); 8] = [
            (&one_a_b, ""),
            (&one_a_dup_b, ""),
            (&one_dup_a_b, ""),
            (&one_a_b, ""),
            (&one_a_b, &two_dup),
            (b, &two_dup),
            (&one_a_b, &two_dup),
            (&one_a_b, ""),
        ];
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        for (step, (one, two)) in steps.into_iter().enumerate() {
            let files = [("one.hir", one), ("two.hir", two)];
            let warm = observe_files(&mut engine, &files);
            let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
            assert_eq!(warm, observe_files(&mut fresh, &files), "step {step}");
            let duplicates: Vec<serde_json::Value> = warm
                .0
                .iter()
                .map(|d| serde_json::from_str(d).unwrap())
                .filter(|d: &serde_json::Value| d["rule"] == "LANG-7")
                .collect();
            let expected = match step {
                1 => Some(("one.hir", 2)),
                2 => Some(("one.hir", 2)),
                4 | 6 => Some(("two.hir", 2)),
                _ => None,
            };
            assert_eq!(
                duplicates.first().map(|d| (
                    d["primary"]["file"].as_str().unwrap().to_owned(),
                    d["primary"]["line"].as_u64().unwrap()
                )),
                expected.map(|(file, line)| (file.to_owned(), line)),
                "step {step}: {warm:?}"
            );
            if let Some(d) = duplicates.first() {
                assert_eq!(
                    d["primary"]["end"].as_u64().unwrap() - d["primary"]["start"].as_u64().unwrap(),
                    1
                );
            }
        }
    });
}

/// An identical duplicate has the same DefPath and item text as the
/// declaration it repeats, so only `DeclarationCandidates` (the count and
/// each candidate's file and header) tells a cached signature that it
/// appeared, disappeared or moved to another file.
#[test]
fn identical_duplicates_change_declaration_candidates_and_rediagnose_warm_and_cold() {
    with_compiler_stack(|| {
        let a = "fn a() -> u8 { 1u8 }\n";
        let b = "fn b() -> u8 { a() }\n";
        let one_a_b = format!("{a}{b}");
        let one_a_a_b = format!("{a}{a}{b}");
        let one_a_b_a = format!("{a}{b}{a}");
        let two_a = format!("// two\n{a}");
        // (one.hir, two.hir) per step: insert, move within the file, remove,
        // insert in another file, move across files, remove there.
        let steps: [(&str, &str); 7] = [
            (&one_a_b, ""),
            (&one_a_a_b, ""),
            (&one_a_b_a, ""),
            (&one_a_b, ""),
            (&one_a_b, &two_a),
            (b, &format!("{two_a}{a}")),
            (&one_a_b, ""),
        ];
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let mut candidates = vec![];
        for (step, (one, two)) in steps.into_iter().enumerate() {
            let files = [("one.hir", one), ("two.hir", two)];
            let warm = observe_files(&mut engine, &files);
            let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
            assert_eq!(warm, observe_files(&mut fresh, &files), "step {step}");
            assert_eq!(
                engine.declaration_candidates(&path("a")),
                fresh.declaration_candidates(&path("a")),
                "step {step}"
            );
            candidates.push(engine.declaration_candidates(&path("a")).unwrap());
            let duplicates: Vec<serde_json::Value> = warm
                .0
                .iter()
                .map(|d| serde_json::from_str(d).unwrap())
                .filter(|d: &serde_json::Value| d["rule"] == "LANG-7")
                .collect();
            let expected = match step {
                1 => Some(("one.hir", 2)),
                2 => Some(("one.hir", 3)),
                4 => Some(("two.hir", 2)),
                5 => Some(("two.hir", 3)),
                _ => None,
            };
            assert_eq!(
                duplicates.first().map(|d| (
                    d["primary"]["file"].as_str().unwrap().to_owned(),
                    d["primary"]["line"].as_u64().unwrap()
                )),
                expected.map(|(file, line)| (file.to_owned(), line)),
                "step {step}: {warm:?}"
            );
        }
        // One candidate, two in one file (in either order), back to one, two
        // across files, two in the other file, one again.
        assert_eq!(candidates[0], candidates[3]);
        assert_eq!(candidates[0], candidates[6]);
        assert_eq!(candidates[1], candidates[2]);
        for (x, y) in [(0, 1), (1, 4), (4, 5), (0, 4), (0, 5)] {
            assert_ne!(candidates[x], candidates[y], "steps {x} and {y}");
        }
    });
}
