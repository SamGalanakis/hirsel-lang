//! Bounded shadow oracle: explicit graph edits plus fresh checked executions.
use hirsel_driver::{compiler_adapters, testing::impact::*};
use hirsel_engine::*;
use hirsel_run_schema::ProgramNode;
use hirsel_syntax::ContentHash;
use std::collections::{BTreeMap, BTreeSet};

fn node(id: &str, body: &str, deps: &[&str]) -> ProgramNode {
    ProgramNode {
        identity: id.into(),
        content: vec![body.as_bytes().to_vec()],
        dependencies: deps.iter().map(|s| s.to_string()).collect(),
    }
}
fn selected(id: &str, hash: ContentHash) -> SelectedTest {
    SelectedTest {
        id: id.into(),
        root: id.into(),
        inputs: Inputs {
            snapshot: hash,
            root: id.into(),
            values: Input::ALL
                .into_iter()
                .map(|f| (f, b"pinned fixture v1".to_vec()))
                .collect(),
            failure: None,
            environment: Ok(b"no host effects; deterministic fixture v1".to_vec()),
        },
    }
}
/// Only the bounded fixture producer establishes all classes. This is not a
/// production adapter and must not infer closure completeness from no authority.
fn snapshot(salt: u8, nodes: Vec<ProgramNode>, tests: &[&str]) -> Snapshot {
    let hash = ContentHash([salt; 32]);
    let evidence = nodes
        .iter()
        .map(|n| {
            (
                n.identity.clone(),
                Evidence {
                    snapshot: hash,
                    established: DependencyClass::ALL.into_iter().collect(),
                    failure: None,
                },
            )
        })
        .collect();
    Snapshot::new(
        hash,
        nodes,
        evidence,
        tests.iter().map(|t| selected(t, hash)),
    )
}
fn disjoint() -> Vec<ProgramNode> {
    vec![
        node("A", "test", &["a"]),
        node("B", "test", &["b"]),
        node("a", "1", &[]),
        node("b", "2", &[]),
    ]
}
fn verdicts(report: &Report) -> Vec<Verdict> {
    report.results.iter().map(|r| r.verdict).collect()
}

#[test]
fn disjoint_changed_same_signature_and_same_output_still_affected() {
    let old = snapshot(1, disjoint(), &["A", "B"]);
    let mut nodes = disjoint();
    nodes[2].content = vec![b"1 + 0".to_vec()];
    let new = snapshot(2, nodes, &["A", "B"]);
    let mut calls = vec![];
    let (report, outcomes) = validate(&old, &new, |side, test| {
        calls.push((side, test.to_string()));
        Ok(7)
    });
    assert_eq!(calls.len(), 4);
    assert_eq!(outcomes.len(), 2);
    assert_eq!(verdicts(&report), [Verdict::Affected, Verdict::Unaffected]);
    assert_eq!(
        report.results[0].changed_dependencies,
        BTreeSet::from(["a".into()])
    );
    assert_eq!(report.results[0].witnesses[0].path, ["A", "a"]);
    assert_ne!(report.old_graph, report.new_graph);
    assert!(!report.results[1].rerun());
}

#[test]
fn both_graphs_cover_removed_edges_targets_deletions_and_cycles() {
    for edit in 0..5 {
        let mut nodes = disjoint();
        nodes.push(node("cycle", "x", &["a"]));
        nodes[2].dependencies.push("cycle".into());
        let old = snapshot(1, nodes.clone(), &["A", "B"]);
        match edit {
            0 => nodes[0].dependencies.clear(),
            1 => nodes[0].dependencies = vec!["b".into()],
            2 => {
                nodes[2].content = vec![b"edited cycle".to_vec()];
            }
            3 => {
                nodes.remove(0);
            }
            _ => {
                nodes[4].content = vec![b"edited other member".to_vec()];
            }
        }
        let new = snapshot(2, nodes, if edit == 3 { &["B"] } else { &["A", "B"] });
        let report = compare(&old, &new);
        assert_eq!(
            verdicts(&report),
            [Verdict::Affected, Verdict::Unaffected],
            "{edit}: {report:?}"
        );
        assert!(report.old_index.components < report.old_index.nodes);
    }
}

#[test]
fn every_required_dependency_class_and_input_has_conservative_fallback() {
    for class in DependencyClass::ALL {
        let nodes = disjoint();
        let hash = ContentHash([2; 32]);
        let mut evidence: BTreeMap<_, _> = nodes
            .iter()
            .map(|n| {
                (
                    n.identity.clone(),
                    Evidence {
                        snapshot: hash,
                        established: DependencyClass::ALL.into_iter().collect(),
                        failure: None,
                    },
                )
            })
            .collect();
        evidence.get_mut("a").unwrap().established.remove(&class);
        let new = Snapshot::new(
            hash,
            nodes,
            evidence,
            [selected("A", hash), selected("B", hash)],
        );
        let report = compare(&snapshot(1, disjoint(), &["A", "B"]), &new);
        assert_eq!(verdicts(&report), [Verdict::Unknown, Verdict::Unaffected]);
        assert!(
            report.results[0]
                .reasons
                .iter()
                .any(|r| r.contains(&format!("{class:?}")))
        );
    }
    for field in Input::ALL {
        let old = snapshot(1, disjoint(), &["A", "B"]);
        let mut test = selected("A", ContentHash([2; 32]));
        test.inputs.values.insert(field, b"changed".to_vec());
        let mut new = snapshot(2, disjoint(), &["A", "B"]);
        // Reconstruct the public product with the edited input record.
        let nodes = disjoint();
        let evidence = nodes
            .iter()
            .map(|n| {
                (
                    n.identity.clone(),
                    Evidence {
                        snapshot: new.identity,
                        established: DependencyClass::ALL.into_iter().collect(),
                        failure: None,
                    },
                )
            })
            .collect();
        new = Snapshot::new(
            new.identity,
            nodes,
            evidence,
            [test.clone(), selected("B", new.identity)],
        );
        assert_eq!(
            verdicts(&compare(&old, &new)),
            [Verdict::Affected, Verdict::Unaffected]
        );
        test.inputs.values.remove(&field);
        let nodes = disjoint();
        let hash = new.identity;
        let evidence = nodes
            .iter()
            .map(|n| {
                (
                    n.identity.clone(),
                    Evidence {
                        snapshot: hash,
                        established: DependencyClass::ALL.into_iter().collect(),
                        failure: None,
                    },
                )
            })
            .collect();
        let missing = Snapshot::new(hash, nodes, evidence, [test]);
        assert_eq!(compare(&old, &missing).results[0].verdict, Verdict::Unknown);
    }
}

#[test]
fn missing_stale_corrupt_failed_cancelled_and_live_records_never_unaffected() {
    for mode in 0..11 {
        let old = snapshot(1, disjoint(), &["A"]);
        let hash = ContentHash([2; 32]);
        let mut nodes = disjoint();
        let mut evidence: BTreeMap<_, _> = nodes
            .iter()
            .map(|n| {
                (
                    n.identity.clone(),
                    Evidence {
                        snapshot: hash,
                        established: DependencyClass::ALL.into_iter().collect(),
                        failure: None,
                    },
                )
            })
            .collect();
        let mut test = selected("A", hash);
        match mode {
            0 => {
                evidence.remove("a");
            }
            1 => evidence.get_mut("a").unwrap().snapshot = ContentHash([0; 32]),
            2 => test.inputs.snapshot = ContentHash([0; 32]),
            3 => test.inputs.root = "wrong".into(),
            4 => evidence.get_mut("a").unwrap().failure = Some("operational failure".into()),
            5 => evidence.get_mut("a").unwrap().failure = Some("cancelled".into()),
            6 => test.inputs.environment = Err("live filesystem/network/time or FFI".into()),
            7 => {
                nodes.remove(2);
            }
            8 => nodes.push(node("a", "conflicting", &[])),
            9 => test.inputs.failure = Some("corrupt input record".into()),
            _ => test.inputs.environment = Ok(vec![]),
        }
        let new = Snapshot::new(hash, nodes, evidence, [test]);
        let report = compare(&old, &new);
        assert_eq!(
            report.results[0].verdict,
            Verdict::Unknown,
            "{mode}: {report:?}"
        );
        assert!(!report.results[0].reasons.is_empty());
        assert!(report.results[0].rerun());
    }
    let old = snapshot(1, disjoint(), &["A"]);
    let new = snapshot(2, disjoint(), &["A"]);
    let mut calls = 0;
    let (report, _) = validate(&old, &new, |_, _| {
        calls += 1;
        Err::<(), _>("cancelled oracle".into())
    });
    assert_eq!(calls, 2);
    assert_eq!(report.results[0].verdict, Verdict::Unknown);
}

#[test]
fn selected_add_delete_rename_and_filter_change_bind_the_report() {
    for tests in [&["A", "B"][..], &["B"][..], &["B", "renamed"][..]] {
        let old = snapshot(1, disjoint(), &["A"]);
        let mut nodes = disjoint();
        nodes.push(node("renamed", "test", &["a"]));
        let new = snapshot(2, nodes, tests);
        let mut calls = 0;
        let (report, _) = validate(&old, &new, |_, _| {
            calls += 1;
            Ok(())
        });
        assert_eq!(calls, report.results.len() * 2);
        for result in &report.results {
            assert_eq!(
                result.verdict,
                if result.test == "A" && tests.contains(&"A") {
                    Verdict::Unaffected
                } else {
                    Verdict::Affected
                }
            );
        }
        assert_eq!(report.old_snapshot, old.identity);
        assert_eq!(report.new_selection.len(), tests.len());
    }
}

#[test]
fn stable_ordered_bfs_shuffled_edges_nodes_salts_and_jobs() {
    let Some(runs) = hirsel_intern::under_salts(
        "test_impact::stable_ordered_bfs_shuffled_edges_nodes_salts_and_jobs",
        &[1, 7, 99],
        || {
            let mut nodes = vec![
                node("A", "", &["z", "b"]),
                node("b", "", &["x"]),
                node("z", "", &["x"]),
                node("x", "old", &[]),
            ];
            let old = snapshot(1, nodes.clone(), &["A"]);
            nodes[3].content = vec![b"new".to_vec()];
            let expected = compare(&old, &snapshot(2, nodes.clone(), &["A"]));
            assert_eq!(expected.results[0].witnesses[0].path, ["A", "b", "x"]);
            for _ in 0..8 {
                nodes.rotate_left(1);
                for n in &mut nodes {
                    n.dependencies.reverse();
                }
                let report = compare(&old, &snapshot(2, nodes.clone(), &["A"]));
                assert_eq!(expected, report);
            }
            // Independent parallel callers cannot alter traversal order.
            std::thread::scope(|s| {
                for _ in 0..4 {
                    s.spawn(|| {
                        assert_eq!(expected, compare(&old, &snapshot(2, nodes.clone(), &["A"])))
                    });
                }
            });
            let source_report = with_compiler_stack(|| {
                let mut before = engine(PURE);
                let mut after = engine(&PURE.replacen("{ 1u64 }", "{ 3u64 }", 1));
                format!(
                    "{:?}",
                    compare(&capture(&mut before, 1), &capture(&mut after, 2))
                )
            });
            format!("{expected:?}\n{source_report}")
        },
    ) else {
        return;
    };
    assert!(runs.windows(2).all(|w| w[0] == w[1]));
}

fn path(name: &str) -> DefPath {
    let mut p = entry_path(&default_context());
    p.0.name = name.into();
    p
}
fn engine(source: &str) -> QueryEngine {
    let mut e = QueryEngine::new(default_environment(), compiler_adapters());
    let file = e.sources.add("shadow-fixture.hir", source.into()).unwrap();
    let checked = check_progress(&mut e, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    e
}
const PURE: &str = "fn a() -> u64 { 1u64 }\nfn b() -> u64 { 2u64 }\n@test\npub fn test_a() -> Result[(), ()] { if a() == 1u64 { Ok(()) } else { Error(()) } }\n@test\npub fn test_b() -> Result[(), ()] { if b() == 2u64 { Ok(()) } else { Error(()) } }\n";
fn capture(e: &mut QueryEngine, salt: u8) -> Snapshot {
    let nodes = e
        .semantic_program_nodes([path("test_a"), path("test_b")])
        .unwrap();
    snapshot(
        salt,
        nodes,
        &[&path("test_a").to_string(), &path("test_b").to_string()],
    )
}
fn observe(e: &mut QueryEngine, name: &str) -> String {
    let config = RunConfiguration {
        entry: path(name),
        hash_seed: 0,
        inputs: vec![],
    };
    let run = run_observed_snapshot(e, &config, SimulationConfig::default()).unwrap();
    format!("{:?}", run.observation.unwrap())
}

#[test]
fn source_shadow_runs_both_tests_and_checks_real_edges_and_fresh_outcomes() {
    with_compiler_stack(|| {
        let mut old_engine = engine(PURE);
        let mut new_engine = engine(&PURE.replacen("{ 1u64 }", "{ 3u64 }", 1));
        assert_eq!(
            old_engine
                .item_signature(&path("a"))
                .unwrap()
                .value
                .fingerprint,
            new_engine
                .item_signature(&path("a"))
                .unwrap()
                .value
                .fingerprint
        );
        assert_eq!(
            old_engine
                .semantic_program_revision(&path("test_b"))
                .unwrap(),
            new_engine
                .semantic_program_revision(&path("test_b"))
                .unwrap()
        );
        assert_ne!(
            old_engine
                .semantic_program_revision(&path("test_a"))
                .unwrap(),
            new_engine
                .semantic_program_revision(&path("test_a"))
                .unwrap()
        );
        let old = capture(&mut old_engine, 1);
        let new = capture(&mut new_engine, 2);
        let nodes = old_engine
            .semantic_program_nodes([path("test_a"), path("test_b")])
            .unwrap();
        let a = nodes
            .iter()
            .find(|n| n.identity == path("test_a").to_string())
            .unwrap();
        assert!(a.dependencies.contains(&path("a").to_string()));
        assert!(!a.dependencies.contains(&path("b").to_string()));
        let mut calls = 0;
        let (report, observations) = validate(&old, &new, |side, id| {
            calls += 1;
            let name = if id == path("test_a").to_string() {
                "test_a"
            } else {
                "test_b"
            };
            Ok(observe(
                if side == Side::Old {
                    &mut old_engine
                } else {
                    &mut new_engine
                },
                name,
            ))
        });
        assert_eq!(calls, 4);
        assert_eq!(verdicts(&report), [Verdict::Affected, Verdict::Unaffected]);
        assert_ne!(observations[0].old, observations[0].new);
        assert_eq!(observations[1].old, observations[1].new);
        // A seeded edit that leaves the value unchanged still changes a's content.
        let mut same = engine(&PURE.replacen("{ 1u64 }", "{ 1u64 + 0u64 }", 1));
        assert_eq!(
            verdicts(&compare(&old, &capture(&mut same, 3))),
            [Verdict::Affected, Verdict::Unaffected]
        );
    });
}

const OWNED: &str = "pub opaque type Hidden: () uses {} { value: u64 }\nresource type Token { value: u64 }\ndrop(token) { let n = 1u64; }\nfn a() { let h = Hidden(value: 1); let t = Token(value: 0u64); }\nfn b() { let h = Hidden(value: 2); let t = Token(value: 0u64); }\npub fn test_a() -> Result[(), ()] { a(); Ok(()) }\npub fn test_b() -> Result[(), ()] { b(); Ok(()) }\n";
const CALLABLES: &str = "fn a() -> u64 { 1u64 }\nfn b() -> u64 { 2u64 }\npub fn test_a() -> Result[(), ()] { let f = a; let n = f(); Ok(()) }\npub fn test_b() -> Result[(), ()] { let f = a; let n = f(); Ok(()) }\n";
const GENERATED: &str = "type Seed { x: u32 }\nfn body_text() -> String { \"fn()->u32 uses {}{11u32}\" }\nfn make(s: TypeShape[Seed]) -> Result[Body[fn()->u32 uses {}],String] { Ok(codegen.body(body_text())) }\nfn chosen() -> u32 uses {} = comptime make(shape(Seed))\npub fn test_a() -> Result[(), ()] { let n = chosen(); Ok(()) }\npub fn test_b() -> Result[(), ()] { let n = chosen(); Ok(()) }\n";

#[test]
fn source_finalizer_private_representation_generator_and_callable_matrix() {
    with_compiler_stack(|| {
        for (label, source, from, to, dependency) in [
            (
                "implicit finalizer",
                OWNED,
                "let n = 1u64",
                "let n = 2u64",
                "Token",
            ),
            (
                "private representation",
                OWNED,
                "Hidden: () uses {} { value: u64 }",
                "Hidden: () uses {} { value: u32 }",
                "Hidden",
            ),
            ("generated body", GENERATED, "11u32", "23u32", "chosen"),
            ("generator input", GENERATED, "x: u32", "x: u64", "Seed"),
            ("callable targets", CALLABLES, "let f = a", "let f = b", "a"),
        ] {
            let mut before = engine(source);
            let mut after = engine(&source.replace(from, to));
            let nodes = before
                .semantic_program_nodes([path("test_a"), path("test_b")])
                .unwrap();
            let dep = path(dependency).to_string();
            assert!(
                nodes.iter().any(|n| n.identity == dep),
                "{label}: missing {dep}"
            );
            if label == "implicit finalizer" {
                // Drop clauses are content of their owning type, not a separate
                // named graph node. An implicit drop must reach that owner.
                for consumer in ["a", "b"] {
                    let caller = nodes
                        .iter()
                        .find(|n| n.identity == path(consumer).to_string())
                        .unwrap();
                    assert!(
                        caller.dependencies.contains(&dep),
                        "{consumer} must reach drop owner"
                    );
                }
                let signature = before.item_signature(&path("Token")).unwrap().value;
                let hirsel_types::SignatureKind::Type(decl) = &signature.kind else {
                    panic!("type");
                };
                assert!(decl.drop.is_some());
            }
            let old = capture(&mut before, 1);
            let new = capture(&mut after, 2);
            let mut runs = 0;
            let (report, observations) = validate(&old, &new, |side, id| {
                runs += 1;
                let name = if id == path("test_a").to_string() {
                    "test_a"
                } else {
                    "test_b"
                };
                Ok(observe(
                    if side == Side::Old {
                        &mut before
                    } else {
                        &mut after
                    },
                    name,
                ))
            });
            assert_eq!(runs, 4);
            assert_eq!(
                verdicts(&report),
                [Verdict::Affected, Verdict::Affected],
                "{label}: {report:?}"
            );
            // Each edit preserves the result but still changes the closure.
            assert!(observations.iter().all(|p| p.old.is_ok() && p.new.is_ok()));
            assert!(
                observations.iter().all(|p| p.old == p.new),
                "{label}: same output edit"
            );
            assert_ne!(report.old_graph, report.new_graph, "{label}");
        }
    });
}

#[test]
fn source_native_and_interpreter_fresh_observations_agree() {
    use hirsel_driver::native::*;
    with_compiler_stack(|| {
        let config = Configuration {
            options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        };
        let matrix = [config];
        let root =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shadow-test-impact-native");
        let toolchain = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        for (index, source) in [
            PURE.to_owned(),
            PURE.replacen("{ 1u64 }", "{ 3u64 }", 1),
            OWNED.to_owned(),
            OWNED.replace("let n = 1u64", "let n = 2u64"),
            GENERATED.to_owned(),
            GENERATED.replace("11u32", "23u32"),
            OWNED.replace(
                "Hidden: () uses {} { value: u64 }",
                "Hidden: () uses {} { value: u32 }",
            ),
            GENERATED.replace("x: u32", "x: u64"),
            CALLABLES.to_owned(),
            CALLABLES.replace("let f = a", "let f = b"),
        ]
        .iter()
        .enumerate()
        {
            let mut e = engine(source);
            // Export once for the batched roots; no selected oracle is omitted.
            e.semantic_program_nodes([path("test_a"), path("test_b")])
                .unwrap();
            let validated = e
                .native_input_for(&[path("test_a"), path("test_b")])
                .unwrap();
            for name in ["test_a", "test_b"] {
                let entry = path(name);
                let reference = run_observed_snapshot(
                    &mut e,
                    &RunConfiguration {
                        entry: entry.clone(),
                        hash_seed: 0,
                        inputs: vec![],
                    },
                    SimulationConfig::default(),
                )
                .unwrap();
                let observation =
                    Observation::of_reference(reference.observation.as_ref().unwrap());
                let dir = root.join(format!("{index}-{name}"));
                std::fs::create_dir_all(&dir).unwrap();
                for (configuration, outcome) in
                    differential(&validated, &entry, &observation, &matrix, &toolchain, &dir)
                {
                    assert!(
                        matches!(outcome, Outcome::Agree),
                        "{index} {name} {configuration}: {outcome:?}"
                    );
                }
            }
        }
    });
}

/// Ignored measurement entry, run explicitly through tools/capped and perf.
/// Each mode performs the same 100 batched operations; setup is outside the loop.
#[test]
#[ignore = "explicit instruction measurement, not a timing assertion"]
fn shadow_measurement() {
    use std::hint::black_box;
    with_compiler_stack(|| {
        let mode = std::env::var("HIRSEL_IMPACT_MEASURE").unwrap_or_else(|_| "index".into());
        let mut e = engine(PURE);
        let old = capture(&mut e, 1);
        let mut edited = engine(&PURE.replacen("{ 1u64 }", "{ 3u64 }", 1));
        let new = capture(&mut edited, 2);
        let report = compare(&old, &new);
        eprintln!(
            "SHADOW_INDEX {:?} {:?}; verdicts {:?}; payload {} {}",
            report.old_index,
            report.new_index,
            verdicts(&report),
            old.payload_bytes(),
            new.payload_bytes()
        );
        for iteration in 0..100 {
            match mode.as_str() {
                "index" => {
                    black_box(compare(black_box(&old), black_box(&new)));
                }
                "execute" => {
                    black_box(observe(&mut e, "test_a"));
                    black_box(observe(&mut e, "test_b"));
                }
                "native" => {
                    if iteration == 0 {
                        source_native_and_interpreter_fresh_observations_agree();
                    }
                }
                "setup" => {
                    black_box(&old);
                }
                _ => panic!("unknown measure mode"),
            }
        }
    });
}

#[test]
fn exhaustive_small_graph_edits_have_no_false_unaffected_closure() {
    // A deliberately independent forward-closure oracle, bounded to 3 nodes.
    fn closure(nodes: &[ProgramNode]) -> BTreeMap<String, ProgramNode> {
        let map: BTreeMap<_, _> = nodes
            .iter()
            .map(|n| (n.identity.clone(), n.clone()))
            .collect();
        let mut reached = BTreeMap::new();
        let mut todo = vec!["0".to_string()];
        while let Some(id) = todo.pop() {
            if reached.contains_key(&id) {
                continue;
            }
            let node = map[&id].clone();
            todo.extend(node.dependencies.clone());
            reached.insert(id, node);
        }
        reached
    }
    let mut unaffected = 0;
    for bits in 0..512 {
        let nodes: Vec<_> = (0..3)
            .map(|i| {
                node(
                    &i.to_string(),
                    "v1",
                    &(0..3)
                        .filter(|j| bits & (1 << (i * 3 + j)) != 0)
                        .map(|j| j.to_string())
                        .collect::<Vec<_>>()
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let old = snapshot(1, nodes.clone(), &["0"]);
        for changed in 0..3 {
            let mut edited = nodes.clone();
            edited[changed].content = vec![b"v2".to_vec()];
            let report = compare(&old, &snapshot(2, edited.clone(), &["0"]));
            let same = closure(&nodes) == closure(&edited);
            assert_eq!(
                report.results[0].verdict,
                if same {
                    Verdict::Unaffected
                } else {
                    Verdict::Affected
                }
            );
            unaffected += usize::from(same);
        }
        for edge in 0..9 {
            let mut edited = nodes.clone();
            let deps = &mut edited[edge / 3].dependencies;
            let target = (edge % 3).to_string();
            if deps.contains(&target) {
                deps.retain(|d| d != &target);
            } else {
                deps.push(target);
                deps.sort();
            }
            let report = compare(&old, &snapshot(2, edited.clone(), &["0"]));
            assert_eq!(
                report.results[0].verdict,
                if closure(&nodes) == closure(&edited) {
                    Verdict::Unaffected
                } else {
                    Verdict::Affected
                }
            );
        }
    }
    assert!(unaffected > 0);
}
