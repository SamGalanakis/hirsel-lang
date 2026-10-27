use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::{DefPath, ItemPath};
fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: default_context().package,
        modules: vec![],
        name: name.into(),
    })
}
fn parse(engine: &mut QueryEngine, text: &str) -> hirsel_span::FileId {
    let file = engine.sources.add("semantic.hir", text.into()).unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    file
}
#[test]
fn real_signature_reads_only_headers_and_real_resolver_retains_failed_reads() {
    hirsel_engine::with_compiler_stack(
        real_signature_reads_only_headers_and_real_resolver_retains_failed_reads_on_compiler_thread,
    );
}
fn real_signature_reads_only_headers_and_real_resolver_retains_failed_reads_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    parse(&mut engine, "fn main() -> u32 { unknown() }\n");
    let signature = engine.item_signature(&path("main")).unwrap();
    assert!(signature.diagnostics.is_empty());
    assert!(matches!(
        engine.resolve_definition(&path("main")),
        Err(QueryError::Diagnostics(_))
    ));
    let reads = engine
        .dependency_reads(&path("main"), QueryKind::Resolve)
        .unwrap();
    assert!(!reads.resolver.is_empty());
    assert!(reads.resolver.iter().any(|r| matches!(
        (r).as_ref(),
        hirsel_resolve::DependencyRead::Lookup { result: Err(_), .. }
    )));
    assert!(
        !engine
            .resolver_snapshot(&path("main"), QueryKind::Resolve)
            .unwrap()
            .reads
            .is_empty()
    );
}
#[test]
fn real_adapters_track_sibling_signatures_across_body_and_signature_edits() {
    hirsel_engine::with_compiler_stack(
        real_adapters_track_sibling_signatures_across_body_and_signature_edits_on_compiler_thread,
    );
}
fn real_adapters_track_sibling_signatures_across_body_and_signature_edits_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    parse(&mut engine, "fn a() -> u32 { 1 }\nfn b() -> u32 { a() }\n");
    engine.resolve_definition(&path("b")).unwrap();
    let reads = engine
        .dependency_reads(&path("b"), QueryKind::Resolve)
        .unwrap();
    assert!(
        reads
            .signatures
            .iter()
            .any(|r| *r.definition.path() == path("a"))
    );
    parse(
        &mut engine,
        "fn a() -> u32 { 123 }\nfn b() -> u32 { a() }\n",
    );
    engine.resolve_definition(&path("b")).unwrap();
    assert_eq!(engine.recompute_count(&path("b"), QueryKind::Resolve), 1);
    assert_eq!(engine.recompute_count(&path("a"), QueryKind::Signature), 1);
    parse(
        &mut engine,
        "fn a() -> u64 { 123 }\nfn b() -> u32 { a() }\n",
    );
    engine.resolve_definition(&path("b")).unwrap();
    assert_eq!(engine.recompute_count(&path("b"), QueryKind::Resolve), 2);
}
#[test]
fn real_signature_phase_reports_duplicate_at_exact_later_name() {
    hirsel_engine::with_compiler_stack(
        real_signature_phase_reports_duplicate_at_exact_later_name_on_compiler_thread,
    );
}
fn real_signature_phase_reports_duplicate_at_exact_later_name_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = parse(&mut engine, "fn value(x: u8) {}\nfn value(x: u32) {}\n");
    let Err(QueryError::Diagnostics(errors)) = engine.item_signature(&path("value")) else {
        panic!("duplicates must reject");
    };
    assert_eq!(errors[0].rule.as_str(), "LANG-7");
    assert_eq!(engine.sources.slice(errors[0].primary), Some("value"));
    assert_eq!(engine.sources.location(errors[0].primary).unwrap().line, 2);
    assert_eq!(engine.sources.resolve(errors[0].primary).file, file);
}
#[test]
fn self_nominal_signature_probes_are_recorded_without_recomputing_forever() {
    hirsel_engine::with_compiler_stack(
        self_nominal_signature_probes_are_recorded_without_recomputing_forever_on_compiler_thread,
    );
}
fn self_nominal_signature_probes_are_recorded_without_recomputing_forever_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    parse(&mut engine, "type Node { next: box Node }\n");
    engine.item_signature(&path("Node")).unwrap();
    engine.item_signature(&path("Node")).unwrap();
    assert_eq!(
        engine.recompute_count(&path("Node"), QueryKind::Signature),
        1
    );
    let reads = engine
        .dependency_reads(&path("Node"), QueryKind::Signature)
        .unwrap();
    // Lexical self names resolve from their own header, without publishing
    // a transient failed-signature dependency (1d15243c).
    assert!(!reads.failed_signatures.contains(&path("Node")));
    let first = engine.item_signature(&path("Node")).unwrap().value;
    parse(&mut engine, "type Node { next: box Node, value: u64 }\n");
    let edited = engine.item_signature(&path("Node")).unwrap().value;
    assert_ne!(first.fingerprint, edited.fingerprint);
    let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
    parse(&mut fresh, "type Node { next: box Node, value: u64 }\n");
    assert_eq!(
        edited.fingerprint,
        fresh
            .item_signature(&path("Node"))
            .unwrap()
            .value
            .fingerprint
    );
    engine.item_signature(&path("Node")).unwrap();
    assert_eq!(
        engine.recompute_count(&path("Node"), QueryKind::Signature),
        2
    );
}

#[test]
fn v20_signature_forms_reach_shared_semantics_through_real_adapters() {
    hirsel_engine::with_compiler_stack(
        v20_signature_forms_reach_shared_semantics_through_real_adapters_on_compiler_thread,
    );
}
fn v20_signature_forms_reach_shared_semantics_through_real_adapters_on_compiler_thread() {
    use hirsel_types::{
        Allocation, AuthoritySummary, Bound, DropAllocation, SignatureKind, TypeKind,
    };
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    parse(
        &mut engine,
        "flat type Packet { items: List[u32] @ growable }\nresource type Token { n: u32 }\n@noalloc drop(x) { () }\npub opaque type Marker: () uses {}\nfn discard[T: noalloc_drop](take x: T) {}\n",
    );
    let packet = engine.item_signature(&path("Packet")).unwrap().value;
    let SignatureKind::Type(def) = &packet.kind else {
        panic!()
    };
    let TypeKind::Record(fields) = &engine
        .types()
        .get(def.representation.unwrap())
        .unwrap()
        .kind
    else {
        panic!()
    };
    assert!(fields[0].growable);
    assert_eq!(
        engine.types().get(fields[0].ty).unwrap().drop_allocation,
        DropAllocation::NoAlloc
    );
    let token = engine.item_signature(&path("Token")).unwrap().value;
    let SignatureKind::Type(def) = &token.kind else {
        panic!()
    };
    assert_eq!(def.destructor_allocation, Allocation::NoAlloc);
    assert_eq!(def.authority, AuthoritySummary::inert());
    assert!(def.bounds.contains(&Bound::NoallocDrop));
    let marker = engine.item_signature(&path("Marker")).unwrap().value;
    let SignatureKind::Type(def) = &marker.kind else {
        panic!()
    };
    assert!(def.bounds.is_empty());
    assert_eq!(def.authority, AuthoritySummary::inert());
    engine.item_signature(&path("discard")).unwrap();
}

#[test]
fn v20_mem_rejections_reach_their_owner_or_report_an_incomplete_pipeline() {
    hirsel_engine::with_compiler_stack(
        v20_mem_rejections_reach_their_owner_or_report_an_incomplete_pipeline_on_compiler_thread,
    );
}
fn v20_mem_rejections_reach_their_owner_or_report_an_incomplete_pipeline_on_compiler_thread() {
    for relative in [
        "MEM/MEM-12-growable-last-bad.hir",
        "MEM/MEM-12-growable-list-bad.hir",
        "MEM/MEM-12-growable-owner-bad.hir",
        "MEM/MEM-5-destructor-body-bad.hir",
        "MEM/MEM-5-destructor-field-cleanup-bad.hir",
    ] {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance")
                .join(relative),
        )
        .unwrap();
        let expected = source
            .lines()
            .find_map(|line| line.strip_prefix("// expect: reject "))
            .unwrap();
        let (rule, line) = expected.split_once(" at line ").unwrap();
        let mut adapters = compiler_adapters();
        // This test retains the deliberately partial pipeline until P is installed.
        adapters.checker = None;
        let mut engine = QueryEngine::new(default_environment(), adapters);
        let file = parse(&mut engine, &source);
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        if let Some(first) = checked.diagnostics.first() {
            assert_eq!(first.rule.as_str(), rule, "{relative}");
            assert_eq!(
                checked.sources.location(first.primary).unwrap().line,
                line.parse::<u32>().unwrap(),
                "{relative}"
            );
        } else {
            assert!(
                matches!(checked.completion, CheckCompletion::Incomplete { .. }),
                "{relative}"
            );
        }
    }
}

#[test]
fn a_partial_check_keeps_the_exact_opaque_clause_diagnostic_and_completion() {
    hirsel_engine::with_compiler_stack(
        a_partial_check_keeps_the_exact_opaque_clause_diagnostic_and_completion_on_compiler_thread,
    );
}
fn a_partial_check_keeps_the_exact_opaque_clause_diagnostic_and_completion_on_compiler_thread() {
    let mut adapters = compiler_adapters();
    // This test retains the deliberately partial pipeline until P is installed.
    adapters.checker = None;
    let mut engine = QueryEngine::new(default_environment(), adapters);
    let file = parse(&mut engine, "pub opaque type Marker uses {}\n");
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(checked.diagnostics[0].rule.as_str(), "LANG-6");
    assert_eq!(
        checked.sources.slice(checked.diagnostics[0].primary),
        Some("Marker")
    );
    assert!(matches!(
        checked.completion,
        CheckCompletion::Incomplete { .. }
    ));
}

#[test]
fn duplicate_diagnostics_keep_candidate_order_when_the_first_snapshot_changes() {
    hirsel_engine::with_compiler_stack(duplicate_diagnostics_keep_candidate_order_when_the_first_snapshot_changes_on_compiler_thread);
}
fn duplicate_diagnostics_keep_candidate_order_when_the_first_snapshot_changes_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    parse(&mut engine, "fn value(x: u8) {}\nfn value(x: u32) {}\n");
    parse(&mut engine, "fn value(x: u16) {}\nfn value(x: u32) {}\n");
    let Err(QueryError::Diagnostics(errors)) = engine.item_signature(&path("value")) else {
        panic!("the later retained candidate must still be a duplicate");
    };
    assert_eq!(errors[0].rule.as_str(), "LANG-7");
    assert_eq!(engine.sources.slice(errors[0].primary), Some("value"));
    assert_eq!(engine.sources.location(errors[0].primary).unwrap().line, 2);
}

#[test]
fn missing_handle_capabilities_own_the_incomplete_call() {
    hirsel_engine::with_compiler_stack(
        missing_handle_capabilities_own_the_incomplete_call_on_compiler_thread,
    );
}
fn missing_handle_capabilities_own_the_incomplete_call_on_compiler_thread() {
    for (source, name) in [
        (
            "fn operation(tasks: Tasks, mut s: Sender[u32], take value: u32) -> Result[(), (u32, ChannelError)] { channel.send(mut s, take value) }",
            "operation",
        ),
        (
            "fn write(dir: Dir, mut f: AtomicFile of dir, data: Bytes) -> Result[(), FileError] { file.write_all(mut f, data) }",
            "write",
        ),
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        parse(&mut engine, source);
        let diagnostics = match engine.mode_definition(&path(name)) {
            Ok(out) => out.diagnostics,
            Err(QueryError::Diagnostics(ds)) => ds,
            Err(e) => panic!("{e:?}"),
        };
        assert_eq!(diagnostics[0].rule.as_str(), "CAP-2", "{diagnostics:?}");
        assert_eq!(
            engine.sources.slice(diagnostics[0].primary),
            Some(if name == "operation" {
                "channel.send(mut s, take value)"
            } else {
                "file.write_all(mut f, data)"
            })
        );
    }
}

#[test]
fn unicode_position_loop_binders_retain_the_written_string_origin() {
    hirsel_engine::with_compiler_stack(
        unicode_position_loop_binders_retain_the_written_string_origin_on_compiler_thread,
    );
}
fn unicode_position_loop_binders_retain_the_written_string_origin_on_compiler_thread() {
    for same in [false, true] {
        let source = format!(
            "fn example(unused: Io) {{ let s: String = \"é😀\"; let other: String = \"é😀\"; for pos in string.positions(s) {{ let empty = string.slice({}, pos, pos); }} }}",
            if same { "s" } else { "other" }
        );
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        parse(&mut engine, &source);
        let diagnostics = match engine.mode_definition(&path("example")) {
            Ok(out) => out.diagnostics,
            Err(QueryError::Diagnostics(ds)) => ds,
            Err(e) => panic!("{e:?}"),
        };
        if same {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
        } else {
            assert_eq!(diagnostics[0].rule.as_str(), "PROV-1", "{diagnostics:?}");
        }
    }
}

#[test]
fn position_loops_preserve_union_origins_until_the_branded_use() {
    hirsel_engine::with_compiler_stack(
        position_loops_preserve_union_origins_until_the_branded_use_on_compiler_thread,
    );
}
fn position_loops_preserve_union_origins_until_the_branded_use_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    parse(
        &mut engine,
        "fn example(c: bool) { let s: String = \"a\"; let other: String = \"b\"; let view = if c { string.positions(s) } else { string.positions(other) }; for pos in view { let empty = string.slice(s, pos, pos); } }",
    );
    let diagnostics = match engine.mode_definition(&path("example")) {
        Ok(out) => out.diagnostics,
        Err(QueryError::Diagnostics(ds)) => ds,
        Err(e) => panic!("{e:?}"),
    };
    assert_eq!(diagnostics[0].rule.as_str(), "PROV-2", "{diagnostics:?}");
}

#[test]
fn failed_sendable_instantiation_suppresses_only_dependent_parameter_modes() {
    hirsel_engine::with_compiler_stack(
        failed_sendable_instantiation_suppresses_only_dependent_parameter_modes_on_compiler_thread,
    );
}
fn failed_sendable_instantiation_suppresses_only_dependent_parameter_modes_on_compiler_thread() {
    for independent in [false, true] {
        let source = if independent {
            "flat type Item { sku:String, qty:u32 }\nfn cross[T:sendable](take x:T, take y:Bytes) {}\nfn example(take x:Item in x, take y:Bytes in y) { cross(take x, take y) }"
        } else {
            "flat type Item { sku:String, qty:u32 }\nfn cross[T:sendable](take x:T) {}\nfn example(take x:Item in x) { cross(take x) }"
        };
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        parse(&mut engine, source);
        let diagnostics = match engine.mode_definition(&path("example")) {
            Ok(out) => out.diagnostics,
            Err(QueryError::Diagnostics(ds)) => ds,
            Err(e) => panic!("{e:?}"),
        };
        assert_eq!(
            diagnostics[0].rule.as_str(),
            if independent { "MODE-1" } else { "MODE-4" },
            "{diagnostics:?}"
        );
    }
}

#[test]
fn header_views_retain_the_original_syntax_and_exclude_execution_reads() {
    hirsel_engine::with_compiler_stack(
        header_views_retain_the_original_syntax_and_exclude_execution_reads_on_compiler_thread,
    );
}
fn header_views_retain_the_original_syntax_and_exclude_execution_reads_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    parse(
        &mut engine,
        "fn predicate(x: u32) -> bool { x > 0 }\n\
         fn main(x: u32) -> u32 requires predicate(x) { missing() }\n\
         const c: u32 = missing()\n\
         resource type Token { n: u32 } drop(self) { missing() }\n\
         distinct type Positive(n: u32) where predicate(n)\n",
    );
    for name in ["main", "c", "Token", "Positive"] {
        engine.item_signature(&path(name)).unwrap();
        let original = engine.parse_definition(&path(name)).unwrap().value;
        let header = engine
            .resolver_snapshot(&path(name), QueryKind::Signature)
            .unwrap();
        assert!(std::sync::Arc::ptr_eq(&original, &header.syntax));
        assert!(!header.reads.iter().any(|read| matches!(
            (read).as_ref(),
            hirsel_types::DependencyRead::Lookup { segments, .. }
                | hirsel_types::DependencyRead::ProviderLookup { segments, .. }
                if segments.iter().any(|name| name == "missing")
        )));
        if matches!(name, "main" | "Positive") {
            assert!(
                header
                    .output
                    .dependencies
                    .iter()
                    .any(|d| *d.definition.path() == path("predicate"))
            );
        }
    }
    // The body view sees the very same tree and still diagnoses the excluded use.
    assert!(matches!(
        engine.resolve_definition(&path("main")),
        Err(QueryError::Diagnostics(_))
    ));
    let original = engine.parse_definition(&path("main")).unwrap().value;
    assert!(std::sync::Arc::ptr_eq(
        &original,
        &engine
            .resolver_snapshot(&path("main"), QueryKind::Resolve)
            .unwrap()
            .syntax
    ));
}

#[test]
fn header_dependencies_ignore_sibling_bodies_but_track_contract_changes() {
    hirsel_engine::with_compiler_stack(
        header_dependencies_ignore_sibling_bodies_but_track_contract_changes_on_compiler_thread,
    );
}
fn header_dependencies_ignore_sibling_bodies_but_track_contract_changes_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let source = |body: &str, contract: &str| {
        format!(
            "fn predicate(x: u32) -> bool {contract} {{ {body} }}\n\
         fn main(x: u32) -> u32 requires predicate(x) {{ x }}\n"
        )
    };
    parse(&mut engine, &source("x > 0", ""));
    let before = engine.item_signature(&path("main")).unwrap();
    let reads = engine
        .dependency_reads(&path("main"), QueryKind::Signature)
        .unwrap()
        .clone();
    parse(&mut engine, &source("x > 1", ""));
    let body_edit = engine.item_signature(&path("main")).unwrap();
    assert_eq!(before.value.fingerprint, body_edit.value.fingerprint);
    assert_eq!(before.dependencies, body_edit.dependencies);
    assert_eq!(
        format!("{reads:?}"),
        format!(
            "{:?}",
            engine
                .dependency_reads(&path("main"), QueryKind::Signature)
                .unwrap()
        )
    );
    assert_eq!(
        engine.recompute_count(&path("main"), QueryKind::Signature),
        1
    );
    parse(&mut engine, &source("x > 1", "requires x > 0"));
    let contract_edit = engine.item_signature(&path("main")).unwrap();
    assert_ne!(before.value.fingerprint, contract_edit.value.fingerprint);
    assert_ne!(before.dependencies, contract_edit.dependencies);
    assert_eq!(
        engine.recompute_count(&path("main"), QueryKind::Signature),
        2
    );
}

#[test]
fn shared_phase_adapters_keep_success_and_failure_products_with_each_engine() {
    use std::sync::Arc;
    let mut first = QueryEngine::new(default_environment(), compiler_adapters());
    let shared = first.adapters.phases.as_ref().unwrap().clone();
    let mut second = QueryEngine::new(default_environment(), PhaseAdapters::compiler(shared));
    // The resolver itself can also be shared: it owns no per-engine snapshots.
    second.adapters.resolver = first.adapters.resolver.clone();
    assert!(Arc::ptr_eq(
        first.adapters.resolver.as_ref().unwrap(),
        second.adapters.resolver.as_ref().unwrap()
    ));
    let good = "fn main(value: u64) -> u64 { value }\n";
    let bad = "fn main(other: u8) -> u8 { missing() }\n";
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(move || {
            with_compiler_stack(move || {
                parse(&mut first, good);
                first.resolve_definition(&path("main")).unwrap();
                first
            })
        });
        let second = scope.spawn(move || {
            with_compiler_stack(move || {
                parse(&mut second, bad);
                assert!(matches!(
                    second.resolve_definition(&path("main")),
                    Err(QueryError::Diagnostics(_))
                ));
                second
            })
        });
        (first.join().unwrap(), second.join().unwrap())
    });
    let good = first
        .resolver_snapshot(&path("main"), QueryKind::Resolve)
        .unwrap();
    let bad = second
        .resolver_snapshot(&path("main"), QueryKind::Resolve)
        .unwrap();
    assert!(good.output.diagnostics.is_empty());
    assert!(!bad.output.diagnostics.is_empty());
    assert!(!Arc::ptr_eq(&good.syntax, &bad.syntax));
    assert_ne!(good.syntax.text_hash, bad.syntax.text_hash);
    assert!(!bad.reads.is_empty());
}
