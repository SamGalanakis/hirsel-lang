use hirsel_driver::*;
use hirsel_engine::*;

fn check(source: &str) -> CheckOutput {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("graph.hir", source.into()).unwrap();
    check_progress(&mut engine, file, &default_context()).unwrap()
}

#[test]
fn growing_cycles_cite_the_earliest_growing_call() {
    hirsel_engine::with_compiler_stack(
        growing_cycles_cite_the_earliest_growing_call_on_compiler_thread,
    );
}
fn growing_cycles_cite_the_earliest_growing_call_on_compiler_thread() {
    for source in [
        "fn grow[T: copyable](x:T) { grow((copy x,copy x)) }",
        "fn first[T: copyable](x:T) { second((copy x,copy x)) }\nfn second[U: copyable](x:U) { first(x) }",
    ] {
        let output = check(source);
        assert_eq!(output.diagnostics[0].rule.as_str(), "GEN-5");
        assert!(
            output
                .sources
                .slice(output.diagnostics[0].primary)
                .unwrap()
                .contains("(copy x,copy x)")
        );
    }
}

#[test]
fn stable_shrinking_acyclic_and_reset_edges_are_not_growth_cycles() {
    hirsel_engine::with_compiler_stack(
        stable_shrinking_acyclic_and_reset_edges_are_not_growth_cycles_on_compiler_thread,
    );
}
fn stable_shrinking_acyclic_and_reset_edges_are_not_growth_cycles_on_compiler_thread() {
    for source in [
        "fn grow[T: copyable](x:T) { grow(x) }",
        "fn first[T: copyable](x:T) { second((copy x,copy x)) }\nfn second[U](x:U) {}",
        "fn first[T: copyable](x:T) { second((copy x,copy x)) }\nfn second[U: copyable](x:(U,U)) { let (a,b)=x; first(a) }",
        "fn first[T: copyable](x:T) { second((copy x,copy x)) }\nfn second[U: copyable](x:U) { first(1u32) }",
    ] {
        assert!(check(source).diagnostics.is_empty(), "{source}");
    }
}

#[test]
fn graph_verdict_updates_after_same_signature_body_edits() {
    hirsel_engine::with_compiler_stack(
        graph_verdict_updates_after_same_signature_body_edits_on_compiler_thread,
    );
}
fn graph_verdict_updates_after_same_signature_body_edits_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    for (body, rejected) in [("grow((copy x,copy x))", true), ("grow(x)", false)] {
        let file = engine
            .sources
            .add(
                "graph.hir",
                format!("fn grow[T: copyable](x:T) {{ {body} }}"),
            )
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "GEN-5"),
            rejected
        );
    }
}

#[test]
fn graph_source_order_does_not_depend_on_file_discovery() {
    hirsel_engine::with_compiler_stack(
        graph_source_order_does_not_depend_on_file_discovery_on_compiler_thread,
    );
}
fn graph_source_order_does_not_depend_on_file_discovery_on_compiler_thread() {
    for reverse in [false, true] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let mut files = vec![
            (
                "a.hir",
                "fn first[T:copyable](x:T) { second((copy x,copy x)) }",
            ),
            ("b.hir", "fn second[U:copyable](x:U) { first(x) }"),
        ];
        if reverse {
            files.reverse();
        }
        let mut first = None;
        for (name, source) in files {
            let file = engine.sources.add(name, source.into()).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            if name == "a.hir" {
                first = Some(file);
            }
        }
        let output = check_progress(&mut engine, first.unwrap(), &default_context()).unwrap();
        assert_eq!(output.diagnostics[0].rule.as_str(), "GEN-5");
        assert_eq!(
            output
                .sources
                .file(output.sources.resolve(output.diagnostics[0].primary).file)
                .unwrap()
                .path
                .to_str(),
            Some("a.hir")
        );
    }
}

fn native_engine(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("graph.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    engine
}
fn path(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}
fn find<'a>(
    input: &'a NativeInput,
    owner: &str,
    kind: InstanceKind,
    callee: &str,
) -> &'a InstanceEdge {
    input
        .instances
        .edges
        .iter()
        .find(|e| e.owner.0.name == owner && e.kind == kind && e.callee.0.name == callee)
        .unwrap_or_else(|| panic!("no {owner} {kind:?} {callee} edge"))
}
const EDGES: &str = "resource type Holder[T: copyable] { v: T }\ndrop(x) { keep(copy x.v) }\ntype Pair[T: copyable] { first: Holder[(T,T)] }\ntype Wrap[T] { v: T }\nfn keep[T: copyable](x:T) {}\nfn total[n: size](xs: Inline[u32, n]) -> u64 { 0u64 }\nfn sizes[m: size](xs: Inline[u32, m]) -> u64 { total(xs) }\nfn outer[T: copyable](x:T) { let f = fn(y: T) { keep(y) }; f(x) }\nfn pairs[T: copyable](x:T) { let p = Pair(first: Holder(v: (copy x, copy x))) }\nconst W: Wrap[u32] = Wrap(v: 1u32)\n";

#[test]
fn exported_instance_edges_keep_owner_store_binder_and_round_trip() {
    hirsel_engine::with_compiler_stack(
        exported_instance_edges_keep_owner_store_binder_and_round_trip_on_compiler_thread,
    );
}
fn exported_instance_edges_keep_owner_store_binder_and_round_trip_on_compiler_thread() {
    use hirsel_types::{BinderRef, GenericArgument, NameSite, Size, TypeKind};
    let mut engine = native_engine(EDGES);
    let input = engine.export_native_input().unwrap();
    let item = |name: &str| {
        input.source_owners[&path(name)]
            .syntax
            .owner()
            .site(NameSite::Item)
    };
    for edge in &input.instances.edges {
        assert_eq!(edge.binder, item(&edge.callee.0.name), "{edge:?}");
    }
    // Finalizer and closure bodies call through their own owner's binder.
    find(&input, "Holder", InstanceKind::Call, "keep");
    find(&input, "outer", InstanceKind::Call, "keep");
    // A dropped nominal instantiates its finalizer and its representation.
    find(&input, "pairs", InstanceKind::Drop, "Pair");
    find(&input, "pairs", InstanceKind::Drop, "Holder");
    let representation = find(&input, "Pair", InstanceKind::Representation, "Holder");
    assert_eq!(representation.owner_binder, Some(item("Pair")));
    let materialized = find(&input, "W", InstanceKind::Materialized, "Wrap");
    let GenericArgument::Type(actual) = materialized.arguments[0] else {
        panic!("type argument")
    };
    assert!(matches!(
        materialized.types.get(actual).unwrap().kind,
        TypeKind::Primitive(_)
    ));
    // A size argument is an edge in the caller's own lexical binder.
    let size = find(&input, "sizes", InstanceKind::Call, "total");
    assert!(matches!(
        size.arguments[..],
        [GenericArgument::Size(Size::Parameter(BinderRef::Body { scope, .. }))] if scope == item("sizes")
    ));
    let validated = engine.import_native_input(input.clone()).unwrap();
    assert!(validated.snapshot().instances.same(&input.instances));
    assert!(input.instances.growing_sites().is_empty());
}

#[test]
fn native_import_rejects_forged_and_dropped_instance_edges() {
    hirsel_engine::with_compiler_stack(
        native_import_rejects_forged_and_dropped_instance_edges_on_compiler_thread,
    );
}
fn native_import_rejects_forged_and_dropped_instance_edges_on_compiler_thread() {
    use hirsel_types::{GenericArgument, NameSite, TypeKind};
    let mut engine = native_engine(EDGES);
    let base = engine.export_native_input().unwrap();
    engine.import_native_input(base.clone()).unwrap();
    let index = |input: &NativeInput, owner: &str, kind, callee: &str| {
        let edge = find(input, owner, kind, callee) as *const InstanceEdge;
        input
            .instances
            .edges
            .iter()
            .position(|e| std::ptr::eq(e, edge))
            .unwrap()
    };
    let rejected = |engine: &mut QueryEngine, input: NativeInput| {
        let error = engine.import_native_input(input).err().expect("rejected");
        assert!(format!("{error:?}").contains("instance graph"), "{error:?}");
    };
    // Dropping the size edge.
    let mut bad = base.clone();
    let size = index(&bad, "sizes", InstanceKind::Call, "total");
    bad.instances.edges.remove(size);
    rejected(&mut engine, bad);
    // Dropping the finalizer and representation requirements.
    for (owner, kind, callee) in [
        ("pairs", InstanceKind::Drop, "Holder"),
        ("Pair", InstanceKind::Representation, "Holder"),
    ] {
        let mut bad = base.clone();
        let at = index(&bad, owner, kind, callee);
        bad.instances.edges.remove(at);
        rejected(&mut engine, bad);
    }
    // A forged argument: the same store's primitive row instead of `(T,T)`.
    let mut bad = base.clone();
    let at = index(&bad, "pairs", InstanceKind::Drop, "Holder");
    let edge = &mut bad.instances.edges[at];
    let primitive = edge
        .types
        .iter()
        .find(|(_, row)| matches!(row.kind, TypeKind::Primitive(_)))
        .unwrap()
        .0;
    edge.arguments[0] = GenericArgument::Type(primitive);
    rejected(&mut engine, bad);
    // A forged binder, kind and owner.
    let at = index(&base, "outer", InstanceKind::Call, "keep");
    let forgeries: [fn(&mut InstanceEdge); 3] = [
        |e| e.binder = hirsel_types::LexicalOwner::first(&path("absent")).site(NameSite::Item),
        |e| e.kind = InstanceKind::Reference,
        |e| e.owner.0.name = "pairs".into(),
    ];
    for forge in forgeries {
        let mut bad = base.clone();
        forge(&mut bad.instances.edges[at]);
        rejected(&mut engine, bad);
    }
    // An extra edge.
    let mut bad = base;
    let extra = bad.instances.edges[at].clone();
    bad.instances.edges.push(extra);
    rejected(&mut engine, bad);
}

/// Every order of the items, cut into every sequence of contiguous units.
fn partitions(items: &[&str]) -> Vec<Vec<String>> {
    fn permutations(items: &[&str]) -> Vec<Vec<String>> {
        if items.is_empty() {
            return vec![vec![]];
        }
        let mut out = vec![];
        for i in 0..items.len() {
            let mut rest = items.to_vec();
            let first = rest.remove(i);
            for mut tail in permutations(&rest) {
                tail.insert(0, first.to_string());
                out.push(tail);
            }
        }
        out
    }
    let mut out = vec![];
    for order in permutations(items) {
        for cuts in 0..1u32 << (order.len() - 1) {
            let mut units = vec![order[0].clone()];
            for (i, item) in order.iter().enumerate().skip(1) {
                if cuts & (1 << (i - 1)) != 0 {
                    units.push(item.clone());
                } else {
                    let last = units.last_mut().unwrap();
                    last.push('\n');
                    last.push_str(item);
                }
            }
            out.push(units);
        }
    }
    out
}
fn site_texts(engine: &QueryEngine, sites: Vec<hirsel_span::Span>) -> Vec<String> {
    sites
        .into_iter()
        .map(|site| engine.sources.slice(site).unwrap().to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}
const FINITE: [&str; 3] = [
    "fn first[T: copyable](x:T) { second((copy x,copy x)) }\nfn second[U: copyable](x:(U,U)) { let (a,b)=x; first(a) }\nfn third[V: copyable](x:V) { first(x) }",
    "fn a[n: size](xs: Inline[u32, n]) -> u64 { b(xs) }\nfn b[m: size](ys: Inline[u32, m]) -> u64 { a(ys) }\nfn c[k: size](zs: Inline[u32, k]) -> u64 { a(zs) }",
    "resource type Holder[T: copyable] { v: T }\ndrop(x) { grow(copy x.v) }\nfn grow[T: copyable](x:T) { let h = Holder(v: copy x) }",
];
const GROWING: [&str; 2] = [
    "fn first[T: copyable](x:T) { second((copy x,copy x)) }\nfn second[U: copyable](x:U) { third(x) }\nfn third[V: copyable](x:V) { first(x) }",
    "fn a[n: size, T: copyable](xs: Inline[u32, n], x: T) { b(xs, (copy x, copy x)) }\nfn b[m: size, U: copyable](ys: Inline[u32, m], y: U) { c(ys, y) }\nfn c[k: size, V: copyable](zs: Inline[u32, k], z: V) { a(zs, z) }",
];
// spec #87: a finalizer closes this cycle, so only native import rejects it.
const FINALIZER_GROWING: &str = "resource type Holder[T: copyable] { v: T }\ndrop(x) { grow(copy x.v) }\nfn grow[T: copyable](x:T) { let h = Holder(v: (copy x, copy x)) }";

fn definitions(program: &str) -> Vec<&str> {
    let mut items: Vec<&str> = vec![];
    for line in program.lines() {
        if line.starts_with("drop(") {
            let type_line = items.pop().unwrap();
            let start = program.find(type_line).unwrap();
            items.push(&program[start..start + type_line.len() + 1 + line.len()]);
        } else {
            items.push(line);
        }
    }
    items
}
fn source_verdict(units: &[String]) -> (QueryEngine, Vec<String>) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    for (i, unit) in units.iter().enumerate() {
        let file = engine
            .sources
            .add(format!("unit{i}.hir"), unit.clone())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
    }
    let sites = engine.gen5_source_graph().unwrap().growing_sites();
    let texts = site_texts(&engine, sites);
    (engine, texts)
}

#[test]
fn finite_cycles_pass_and_growing_cycles_fail_over_every_partition_order() {
    hirsel_engine::with_compiler_stack(
        finite_cycles_pass_and_growing_cycles_fail_over_every_partition_order_on_compiler_thread,
    );
}
fn finite_cycles_pass_and_growing_cycles_fail_over_every_partition_order_on_compiler_thread() {
    for (program, growing) in FINITE
        .iter()
        .map(|p| (*p, false))
        .chain(GROWING.iter().map(|p| (*p, true)))
    {
        let (_, whole) = source_verdict(&[program.to_string()]);
        assert_eq!(!whole.is_empty(), growing, "{program}: {whole:?}");
        for partition in partitions(&definitions(program)) {
            assert_eq!(source_verdict(&partition).1, whole, "{partition:?}");
        }
        if growing {
            let output = check(program);
            assert_eq!(output.diagnostics[0].rule.as_str(), "GEN-5");
            assert!(
                whole.contains(
                    &output
                        .sources
                        .slice(output.diagnostics[0].primary)
                        .unwrap()
                        .to_string()
                )
            );
            continue;
        }
        // The native graph's verdict is the same over every order in which
        // definition units contribute their edges.
        let mut engine = native_engine(program);
        let input = engine.export_native_input().unwrap();
        let mut owners: Vec<_> = input.instances.edges.iter().map(|e| &e.owner).collect();
        owners.dedup();
        let names: Vec<_> = owners.iter().map(|o| o.0.name.as_str()).collect();
        for partition in partitions(&names) {
            let mut graph = InstanceGraph::default();
            for unit in &partition {
                for name in unit.lines() {
                    graph.edges.extend(
                        input
                            .instances
                            .edges
                            .iter()
                            .filter(|e| e.owner.0.name == name)
                            .cloned(),
                    );
                }
            }
            assert!(graph.growing_sites().is_empty(), "{program}: {partition:?}");
        }
        engine.import_native_input(input).unwrap();
    }
}

#[test]
fn a_growing_finalizer_cycle_is_native_unavailable_over_every_partition_order() {
    hirsel_engine::with_compiler_stack(
        a_growing_finalizer_cycle_is_native_unavailable_over_every_partition_order_on_compiler_thread,
    );
}
fn a_growing_finalizer_cycle_is_native_unavailable_over_every_partition_order_on_compiler_thread() {
    // Source verdicts stay unchanged until spec #87 rules.
    let (_, source) = source_verdict(&[FINALIZER_GROWING.to_string()]);
    assert!(source.is_empty());
    let mut engine = native_engine(FINALIZER_GROWING);
    let input = engine.export_native_input().unwrap();
    let whole = site_texts(&engine, input.instances.growing_sites());
    assert!(whole.contains(&"h".to_string()), "{whole:?}");
    let mut owners: Vec<_> = input.instances.edges.iter().map(|e| &e.owner).collect();
    owners.sort();
    owners.dedup();
    let names: Vec<_> = owners.iter().map(|o| o.0.name.as_str()).collect();
    for partition in partitions(&names) {
        let mut graph = InstanceGraph::default();
        for name in partition.iter().flat_map(|unit| unit.lines()) {
            graph.edges.extend(
                input
                    .instances
                    .edges
                    .iter()
                    .filter(|e| e.owner.0.name == name)
                    .cloned(),
            );
        }
        assert_eq!(site_texts(&engine, graph.growing_sites()), whole);
    }
    let error = engine.import_native_input(input).err().expect("rejected");
    assert!(format!("{error:?}").contains("#87"), "{error:?}");
}

mod verdict {
    use hirsel_driver::*;
    use hirsel_engine::*;
    use hirsel_intern::work::{Counter, read};
    use hirsel_syntax::ContentHash;

    #[test]
    fn unchanged_program_audit_does_not_rewalk_checked_bodies() {
        with_compiler_stack(|| {
            for (body, rejected) in [("grow((copy x,copy x))", true), ("grow(x)", false)] {
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine
                    .sources
                    .add(
                        "graph.hir",
                        format!("fn grow[T:copyable](x:T) {{ {body} }}"),
                    )
                    .unwrap();
                engine.parse_file(file, &default_context()).unwrap();
                let first = engine.generic_graph_diagnostics().unwrap();
                assert_eq!(!first.is_empty(), rejected);
                let before = read(Counter::CacheValidationRows);
                let second = engine.generic_graph_diagnostics().unwrap();
                assert_eq!(first, second);
                assert_eq!(read(Counter::CacheValidationRows) - before, 0);
                engine.comptime_limits.steps = engine.comptime_limits.steps.map(|steps| steps - 1);
                let before = read(Counter::CacheValidationRows);
                assert_eq!(engine.generic_graph_diagnostics().unwrap(), first);
                assert!(read(Counter::CacheValidationRows) > before);
                engine
                    .adapters
                    .compiler_state
                    .as_ref()
                    .unwrap()
                    .borrow_mut()
                    .manifest_revision = ContentHash::of_bytes(b"edited manifest");
                let before = read(Counter::CacheValidationRows);
                assert_eq!(engine.generic_graph_diagnostics().unwrap(), first);
                assert!(read(Counter::CacheValidationRows) > before);
            }
        });
    }
}
