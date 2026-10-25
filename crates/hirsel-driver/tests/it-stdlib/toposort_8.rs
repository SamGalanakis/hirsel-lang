//! CPython graphlib.py:56-85,125-135,189-200 and test_graphlib.py:39-137,
//! 8b7d6c53eee2226b08bed761d5a1742b5adb8507 (PSF-2.0, third-party/NOTICES.md).
//! Independent permutation existence oracle and transitive-closure blocked oracle.
use hirsel_driver::*;
use hirsel_engine::*;
use std::{fmt::Write, path::PathBuf};

fn edges(n: usize, mask: u64) -> Vec<(usize, usize)> {
    (0..n * n)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| (bit / n, bit % n))
        .collect()
}

fn has_order(prefix: &mut Vec<usize>, n: usize, edges: &[(usize, usize)]) -> bool {
    if prefix.len() == n {
        return edges
            .iter()
            .all(|(a, b)| prefix.iter().position(|v| v == a) < prefix.iter().position(|v| v == b));
    }
    for v in 0..n {
        if !prefix.contains(&v) {
            prefix.push(v);
            if has_order(prefix, n, edges) {
                prefix.pop();
                return true;
            }
            prefix.pop();
        }
    }
    false
}

fn blocked(n: usize, edges: &[(usize, usize)]) -> u64 {
    let mut reachable = vec![vec![false; n]; n];
    for &(a, b) in edges {
        reachable[a][b] = true;
    }
    for k in 0..n {
        for a in 0..n {
            for b in 0..n {
                reachable[a][b] |= reachable[a][k] && reachable[k][b];
            }
        }
    }
    (0..n).fold(0, |mask, v| {
        mask | if (0..n).any(|c| reachable[c][c] && reachable[c][v]) {
            1 << v
        } else {
            0
        }
    })
}

fn run(source: &str, measured: Option<&str>, label: &str) {
    with_compiler_stack(|| {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("toposort-8-{label}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.hir");
        std::fs::write(&path, source).unwrap();
        let mut sources = vec![serde_json::json!({"path":"main.hir", "module":[]})];
        if let Some(measured) = measured {
            std::fs::write(dir.join("measured.hir"), measured).unwrap();
            sources.push(serde_json::json!({"path":"measured.hir", "module":["measured"]}));
        }
        let std_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = serde_json::json!({
            "schema":1, "package":{"name":"toposort_8", "version":"1"},
            "sources":sources, "entry":{"module":[], "name":"main"},
            "dependencies":[{"manifest":std_manifest,"package":{"name":"std","version":"1"},"expose":[]}]
        });
        std::fs::write(path.with_extension("hir.json"), manifest.to_string()).unwrap();
        let mut loaded = load_program(&path).unwrap();
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(40_000_000),
                call_depth: 256,
            },
            ..Default::default()
        }));
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                reference.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            reference.outcome
        );
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let ready = native::build(&validated, &options).unwrap();
        assert!(ready.units.iter().any(|unit| {
            let header = &unit.unit().header;
            header.definition.0.package.name_is("std")
                && header.definition.0.modules == ["toposort"]
                && header.definition.0.name == "order"
                && header.origin == hirsel_lir::ir::UnitOrigin::Source
        }));
        let heap = native::run_heap_in(
            &ready,
            &loaded.entry,
            &native::Sites::new(validated.snapshot()),
            &native::HeapWorld {
                steps: Some(40_000_000),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(heap.observation, expected);
        assert_eq!(heap.counters.live_heap_bytes, 0);
        let configs = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
            native::Configuration {
                options,
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
        ];
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        for (config, outcome) in
            native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
        {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{label}: {config}: {outcome:?}"
            );
        }
        eprintln!("toposort-8 {label}: interpreter/H/LLVM/Cranelift agree; no live heap bytes");
    });
}

const CONTROLS: &str = include_str!("../fixtures/toposort-8/controls.hir");

#[test]
fn fixed_reference_and_caps() {
    run(
        &format!(
            "{CONTROLS}\npub fn main() -> Result[(), ()] {{ fixed()\n additional_boundaries()\n Ok(()) }}"
        ),
        None,
        "fixed",
    );
}

#[test]
fn exhaustive_permutation_oracle() {
    let mut source = CONTROLS.to_string();
    source.push_str("\npub fn main() -> Result[(), ()] {\n");
    let mut count = 0;
    for n in 0..=3 {
        let expected: Vec<_> = (0..1u64 << (n * n))
            .map(|mask| {
                let e = edges(n, mask);
                let blocked = blocked(n, &e);
                assert_eq!(has_order(&mut Vec::new(), n, &e), blocked == 0);
                count += 1;
                blocked
            })
            .collect();
        writeln!(source, "{{ let expected: List[u64] = {expected:?}\n for mask in 0u64..{}u64 {{\n let mut edges: List[toposort.Edge] = []\n for a in 0u64..{n}u64 {{ for b in 0u64..{n}u64 {{\n if mask & (1u64 << (a * {n} + b)) != 0 {{ list.push(mut edges, toposort.Edge(from: a, to: b)) }}\n }} }}\n check_graph({n}, edges, expected[mask])\n }} }}", expected.len()).unwrap();
    }
    source.push_str(" Ok(())\n}\n");
    assert_eq!(count, 531);
    run(&source, None, "exhaustive-531");
    eprintln!(
        "toposort-8: {count} exhaustive directed graphs including loops; permutation existence + closure blocked oracle"
    );
}
