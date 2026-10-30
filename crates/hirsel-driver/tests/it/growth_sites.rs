//! The public GEN-5 classifier must preserve the old ordered diagnostic sites.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_span::{Anchor, Span};
use hirsel_syntax::{DefId, DefPath, LexicalOwner, StmtId};
use hirsel_types::{NameSite, SizeParamId, SizeParameterRef, TypeParamId, TypeParameterRef};
use std::collections::{BTreeMap, BTreeSet};

type Vertex = (DefId, GenericParameterRef);

// The former production predicate, deliberately independent of SCC membership.
fn old_sites(edges: &[GrowthEdge]) -> Vec<Span> {
    let mut graph: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for (from, to, _, _) in edges {
        graph.entry(from).or_default().push(to);
    }
    let mut sites = BTreeMap::new();
    for (from, to, _, site) in edges.iter().filter(|edge| edge.2) {
        let mut todo = vec![to];
        let mut seen = BTreeSet::new();
        while let Some(node) = todo.pop() {
            if node == from {
                sites.insert((site.anchor, site.start, site.end), *site);
                break;
            }
            if seen.insert(node) {
                todo.extend(graph.get(node).into_iter().flatten());
            }
        }
    }
    sites.into_values().collect()
}

fn vertex(index: usize) -> Vertex {
    let path = DefPath::core(&format!("gen5_test_{}", index / 4));
    let scope = LexicalOwner::first(&path).site(if index % 4 < 2 {
        NameSite::Item
    } else {
        NameSite::Statement(StmtId(0))
    });
    let parameter = if index.is_multiple_of(2) {
        GenericParameterRef::Type(TypeParameterRef::body(scope, TypeParamId(0)))
    } else {
        GenericParameterRef::Size(SizeParameterRef::body(scope, SizeParamId(0)))
    };
    (DefId::intern(&path), parameter)
}
fn site(anchor: u32, start: u32, end: u32) -> Span {
    Span::new(Anchor(anchor), start, end).unwrap()
}
fn edge(from: usize, to: usize, grows: bool, span: Span) -> GrowthEdge {
    (vertex(from), vertex(to), grows, span)
}
fn compare(edges: &[GrowthEdge]) {
    assert_eq!(growing_sites(edges), old_sites(edges), "{edges:?}");
}
fn random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state >> 32
}
fn shuffle(edges: &mut [GrowthEdge], state: &mut u64) {
    for i in (1..edges.len()).rev() {
        edges.swap(i, random(state) as usize % (i + 1));
    }
}

#[test]
fn ordered_sites_keep_anchors_duplicates_and_parameter_binders() {
    compare(&[]);
    let a = site(2, 10, 12);
    let b = site(1, 10, 12); // Same byte range, different anchor.
    let c = site(1, 10, 11); // Sorting includes the end offset.
    let mut edges = vec![
        edge(0, 0, true, a), // Growing self-loop.
        edge(1, 2, true, b),
        edge(2, 1, false, a),
        edge(2, 1, true, c), // Multiple growing edges in an SCC.
        edge(1, 2, true, b), // Duplicate edge and site.
        edge(4, 5, false, a),
        edge(5, 4, false, a), // Disconnected SCC without growth.
        edge(6, 7, true, a),  // Target without outgoing edges.
        edge(3, 3, false, b), // Isolated, non-growing self-loop.
    ];
    let expected = vec![c, b, a];
    assert_eq!(growing_sites(&edges), expected);
    let mut state = 0x5eed;
    for _ in 0..32 {
        shuffle(&mut edges, &mut state);
        compare(&edges);
        assert_eq!(growing_sites(&edges), expected);
    }
    // Neither equal ordinals nor equal owners merge distinct type/size/local binders.
    for (from, to) in [(0, 1), (0, 2), (0, 4)] {
        let edges = [edge(from, to, true, a), edge(to, to, false, b)];
        compare(&edges);
        assert!(growing_sites(&edges).is_empty());
    }
}

#[test]
fn exhaustive_small_graphs_preserve_ordered_sites() {
    let vertices: Vec<_> = (0..4).map(vertex).collect();
    let mut cases = 0;
    // Every directed topology through four vertices, including self-loops.
    for n in 0..=4 {
        for mask in 0u32..(1 << (n * n)) {
            let mut edges = vec![];
            for bit in 0..n * n {
                if mask & (1 << bit) != 0 {
                    edges.push((
                        vertices[bit / n],
                        vertices[bit % n],
                        true,
                        site((bit % 3 + 1) as u32, bit as u32, bit as u32 + 1),
                    ));
                }
            }
            compare(&edges);
            cases += 1;
        }
    }
    assert_eq!(cases, 66_067); // Includes the empty zero-vertex graph.
    // Also exhaust all absent/stable/growing edge labels through three vertices.
    let mut labeled = 0;
    for n in 0..=3 {
        for mut code in 0..3usize.pow((n * n) as u32) {
            let mut edges = vec![];
            for bit in 0..n * n {
                let label = code % 3;
                code /= 3;
                if label != 0 {
                    edges.push((
                        vertices[bit / n],
                        vertices[bit % n],
                        label == 2,
                        site((bit % 2 + 1) as u32, (bit % 3) as u32, (bit % 3 + 1) as u32),
                    ));
                }
            }
            compare(&edges);
            labeled += 1;
        }
    }
    assert_eq!(labeled, 19_768);
}

pub(super) fn source_compare(engine: &mut QueryEngine) -> Vec<Span> {
    let graph = engine.gen5_source_graph().unwrap();
    let edges = graph.edges.to_vec();
    compare(&edges);
    let expected = old_sites(&edges);
    assert_eq!(growing_sites(&edges), expected);
    let mut shuffled = edges;
    shuffle(&mut shuffled, &mut 0x5eed);
    assert_eq!(growing_sites(&shuffled), expected);
    expected
}

#[test]
fn actual_gen5_fixtures_preserve_verdicts_and_exact_sites() {
    with_compiler_stack(|| {
        for (source, growing) in [
            (
                std::fs::read_to_string("../../conformance/GEN/GEN-5-acyclic-growth.hir").unwrap(),
                false,
            ),
            (
                std::fs::read_to_string("../../conformance/GEN/GEN-5-shrinking-edge.hir").unwrap(),
                false,
            ),
            (
                std::fs::read_to_string("../../conformance/GEN/GEN-5-mutual-growth-ok.hir")
                    .unwrap(),
                false,
            ),
            (
                std::fs::read_to_string("../../conformance/GEN/GEN-5-mutual-growth-bad.hir")
                    .unwrap(),
                true,
            ),
            (
                std::fs::read_to_string(
                    "../../conformance/GEN/GEN-5-generic-callable-growing-ok.hir",
                )
                .unwrap(),
                false,
            ),
            (
                std::fs::read_to_string(
                    "../../conformance/GEN/GEN-5-generic-callable-growing-bad.hir",
                )
                .unwrap(),
                true,
            ),
            ("fn grow[T: copyable](x:T) { grow((copy x,copy x)) }".to_owned(), true),
            (
                "fn stable[n: size](xs: Inline[u8, n]) { stable(xs) }".to_owned(),
                false,
            ),
            (
                "fn outer[n: size](xs: Inline[u8, n]) -> u64 { fn inner[m: size](ys: Inline[u8, m]) -> u64 { let _ = ys; comptime (n * m) }; inner(xs) }\npub fn main() { let xs: Inline[u8, 2] = [1, 2]; let _ = outer(xs) }".to_owned(),
                false,
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("gen5.hir", source.clone()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            let expected = source_compare(&mut engine);
            assert_eq!(!expected.is_empty(), growing, "{source}");
            if growing {
                assert_eq!(output.diagnostics.len(), 1, "{source}");
                assert_eq!(output.diagnostics[0].rule.as_str(), "GEN-5");
                assert_eq!(output.diagnostics[0].primary, expected[0]);
            } else {
                assert!(
                    output.diagnostics.is_empty(),
                    "{source}: {:?}",
                    output.diagnostics
                );
            }
        }
    });
}
