//! Packed walk-only truncate/fill controls and physical allocator evidence.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir")
}

fn check(path: &Path, source: Option<&str>, stdout: &[u8], failure: Option<&str>, campaign: bool) {
    let mut loaded = load_program_source(path, source).unwrap();
    loaded.check_for_execution().unwrap_or_else(|e| match e {
        ToolError::Query { error, .. } => panic!("{error:?}"),
        e => panic!("{e}"),
    });
    loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
        limits: hirsel_interp::Limits {
            steps: Some(20_000_000),
            call_depth: 256,
        },
        ..Default::default()
    }));
    let run = run_observed_snapshot(
        &mut loaded.engine,
        &RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
    )
    .unwrap();
    let observation = run.observation.as_ref().unwrap();
    assert_eq!(observation.output, stdout);
    match failure {
        Some(rule) => assert!(
            matches!(run.outcome,
            Ok(RunOutcome::Failed { primary: hirsel_run_schema::Failure { rule: ref actual, .. }, .. })
            if actual.as_str() == rule),
            "{:?}",
            run.outcome
        ),
        None => assert!(
            matches!(
                run.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            run.outcome
        ),
    }
    let expected = native::Observation::of_reference(observation);
    let input = loaded.engine.export_native_input().unwrap();
    let validated = loaded.engine.import_native_input(input).unwrap();
    let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
    let ready = native::build(&validated, &options).unwrap();
    let sites = native::Sites::new(validated.snapshot());
    let world = native::HeapWorld {
        steps: Some(20_000_000),
        ..Default::default()
    };
    let heap = if campaign {
        native::heap_campaign_in(&ready, &loaded.entry, &sites, &world)
    } else {
        native::run_heap_in(&ready, &loaded.entry, &sites, &world)
    }
    .unwrap();
    assert_eq!(heap.observation, expected);
    assert_eq!(heap.counters.live_heap_bytes, 0);
    let configs = [
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
    // Several independent tests share the package-resolution anchor path.
    // Their native link/record files must belong to distinct source identities.
    let source_identity = hirsel_syntax::ContentHash::of_bytes(source.unwrap_or("").as_bytes());
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "packed-list-9-{}-{source_identity:?}",
        path.file_stem().unwrap().to_str().unwrap()
    ));
    let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
    for (configuration, outcome) in
        native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
    {
        assert!(
            matches!(outcome, native::Outcome::Agree),
            "{configuration}: {outcome:?}"
        );
    }
}

const TYPES: &str = "type Inner = Num(n: u64) | Pair(a: u64, b: u64)
type V = Empty() | Small(x: u8) | Big(x: u64, y: u64) | Node(inner: Packed[Inner])
";

#[test]
fn alternating_widths_alias_noop_zero_and_regrowth_on_all_engines() {
    with_compiler_stack(|| {
        let source = format!(
            r#"{TYPES}
fn inspect(xs: List[Packed[V]], n: u64) -> () {{
  failure.check(list.len(xs) == n, Bounds())
  let mut i = 0u64
  for x in xs {{
    if i % 4 == 1 {{ failure.check(x.Small.x == 7u8, Bounds()) }}
    if i % 4 == 2 {{ failure.check(x.Big.x == 11 && x.Big.y == 13, Bounds()) }}
    if i % 4 == 3 {{ failure.check(x.Node.inner.Pair.a == 17 && x.Node.inner.Pair.b == 19, Bounds()) }}
    i += 1
  }}
}}
pub fn main() -> Result[(), ()] {{
  let mut xs: List[Packed[V]] = []
  for i in 0u64..32u64 {{
    if i % 4 == 0 {{ list.push(mut xs, Empty()) }}
    else if i % 4 == 1 {{ list.push(mut xs, Small(x: 7u8)) }}
    else if i % 4 == 2 {{ list.push(mut xs, Big(x: 11, y: 13)) }}
    else {{ list.push(mut xs, Node(inner: Pair(a: 17, b: 19))) }}
  }}
  let alias = copy xs
  list.truncate(mut xs, 18446744073709551615)
  inspect(xs, 32)
  list.truncate(mut xs, 32)
  inspect(xs, 32)
  list.truncate(mut xs, 15)
  inspect(xs, 15)
  inspect(alias, 32)
  list.push(mut xs, Node(inner: Pair(a: 17, b: 19)))
  inspect(xs, 16)
  list.truncate(mut xs, 1)
  inspect(xs, 1)
  list.truncate(mut xs, 0)
  inspect(xs, 0)
  list.push(mut xs, Empty())
  list.push(mut xs, Small(x: 7u8))
  inspect(xs, 2)
  inspect(alias, 32)
  Ok(())
}}
"#
        );
        check(&fixture(), Some(&source), b"", None, true);
    });
}

#[test]
fn repeat_each_variant_and_zero_on_all_engines() {
    with_compiler_stack(|| {
        // Rust's repeat constructor vectors, with a zero-payload arm and a
        // nested flat payload. A copied seed has no owned child allocation.
        let mut cases = String::new();
        for (seed, assertion) in [
            ("Empty()", "true"),
            ("Small(x: 7u8)", "x.Small.x == 7u8"),
            ("Big(x: 11, y: 13)", "x.Big.x == 11 && x.Big.y == 13"),
            (
                "Node(inner: Pair(a: 17, b: 19))",
                "x.Node.inner.Pair.a == 17 && x.Node.inner.Pair.b == 19",
            ),
        ] {
            for n in [0, 1, 2, 3, 64] {
                cases.push_str(&format!("{{ let seed: Packed[V] = {seed}\nlet xs = std@1/list.filled({n}, take seed)\nfailure.check(list.len(xs) == {n}, Bounds())\nfor x in xs {{ failure.check({assertion}, Bounds()) }} }}\n"));
            }
        }
        let source = format!("{TYPES}pub fn main() -> Result[(), ()] {{\n{cases}Ok(())\n}}\n");
        check(&fixture(), Some(&source), b"", None, false);
    });
}

#[test]
fn impossible_packed_extent_fails_before_filling_on_all_engines() {
    with_compiler_stack(|| {
        let source = format!(
            "{TYPES}pub fn main() -> Result[(), ()] {{ let seed: Packed[V] = Big(x: 1, y: 2)\nlet xs = std@1/list.filled(18446744073709551615, take seed)\nfailure.check(list.len(xs) == 0, Bounds())\nOk(()) }}"
        );
        check(&fixture(), Some(&source), b"", Some("SEM-1"), false);
    });
}

#[test]
fn changing_a_packed_active_tag_stays_rejected() {
    with_compiler_stack(|| {
        let source = format!(
            "{TYPES}pub fn main() -> Result[(), ()] {{\nlet mut xs: List[Packed[V]] = [Small(x: 7u8)]\nlist.truncate(mut xs, 1)\nfor x in mut xs {{ x = Big(x: 1, y: 2) }}\nOk(()) }}"
        );
        let mut loaded = load_program_source(&fixture(), Some(&source)).unwrap();
        let result = loaded
            .check(hirsel_engine::LoweringPolicy::Reachable)
            .unwrap();
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "MEM-13"),
            "{:?}",
            result.diagnostics
        );
    });
}

#[test]
fn physical_outer_allocations_and_detach_cost_on_both_backends() {
    with_compiler_stack(|| {
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("packed-list-9-counts");
        let tools = native::Toolchain::in_tree_with_runtime(
            &dir.join("runtime"),
            &[&target],
            native::RuntimeBuild {
                measurement: true,
                ..Default::default()
            },
        )
        .unwrap();
        for (name, n, tail, expected) in [
            ("empty", 0, "", 0),
            // The first minimum-width request is 32 bytes; Big needs 40.
            ("one", 1, "", 2),
            ("original64", 64, "", 2),
            ("exact_noop", 64, "list.truncate(mut xs, 64)", 2),
            (
                "excess_noop",
                64,
                "list.truncate(mut xs, 18446744073709551615)",
                2,
            ),
            ("unique_prefix", 64, "list.truncate(mut xs, 31)", 3),
            (
                "shared_prefix",
                64,
                "let alias = copy xs\nlist.truncate(mut xs, 31)\nfailure.check(list.len(alias) == 64, Bounds())\nfor x in alias { failure.check(x.Big.x == 1 && x.Big.y == 2, Bounds()) }",
                3,
            ),
            ("clear", 64, "list.truncate(mut xs, 0)", 2),
        ] {
            let source = format!(
                r#"
type V = Small(x: u8) | Big(x: u64, y: u64)
fn build(n: u64) -> List[Packed[V]] {{
  let seed: Packed[V] = Big(x: 1, y: 2)
  std@1/list.filled(n, take seed)
}}
pub fn main() -> Result[(), ()] {{
  let mut xs = build({n})
  failure.check(list.len(xs) == {n}, Bounds())
  {tail}
  for x in xs {{ failure.check(x.Big.x == 1 && x.Big.y == 2, Bounds()) }}
  Ok(())
}}
"#
            );
            let mut loaded = load_program_source(&fixture(), Some(&source)).unwrap();
            loaded
                .check_for_execution()
                .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input).unwrap();
            let mut options = native::Options::new(target.clone());
            options.regions = false;
            let ready =
                native::build(&validated, &options).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            let heap = native::run_heap_in(
                &ready,
                &loaded.entry,
                &native::Sites::new(validated.snapshot()),
                &native::HeapWorld {
                    steps: Some(20_000_000),
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(heap.counters.live_heap_bytes, 0, "{name}");
            eprintln!(
                "packed-list-9 H {name}: {} outer allocations; seed=0",
                heap.counters.allocations
            );
            assert_eq!(heap.counters.allocations, expected, "H {name}");
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                let case = dir.join(format!("{backend:?}-{name}"));
                std::fs::create_dir_all(&case).unwrap();
                let objects = native::emit_backend_objects(
                    &ready,
                    backend,
                    native::OptLevel::Speed,
                    native::Partition::Whole,
                    true,
                    None,
                )
                .unwrap();
                let exe =
                    native::link_objects(&ready, &loaded.entry, &objects, &tools, &case).unwrap();
                let totals = case.join("totals");
                let run = std::process::Command::new(exe)
                    .env_clear()
                    .env("HIRSEL_RT_MEASUREMENT", &totals)
                    .output()
                    .unwrap();
                assert!(
                    run.status.success(),
                    "{backend:?} {name}: {}",
                    String::from_utf8_lossy(&run.stderr)
                );
                let report = std::fs::read_to_string(totals).unwrap();
                let allocations: u64 = report
                    .lines()
                    .filter_map(|line| line.strip_prefix("class "))
                    .map(|line| {
                        line.split_whitespace()
                            .nth(1)
                            .unwrap()
                            .parse::<u64>()
                            .unwrap()
                    })
                    .sum();
                let frees: u64 = report
                    .lines()
                    .filter_map(|line| line.strip_prefix("class "))
                    .map(|line| {
                        line.split_whitespace()
                            .nth(2)
                            .unwrap()
                            .parse::<u64>()
                            .unwrap()
                    })
                    .sum();
                assert_eq!(
                    frees, allocations,
                    "{backend:?} {name} leaked backing\n{report}"
                );
                assert!(
                    report.lines().any(|line| line == "live_bytes 0"),
                    "{backend:?} {name}\n{report}"
                );
                eprintln!(
                    "packed-list-9 {backend:?} {name}: {allocations} outer allocations/{frees} frees; live_bytes=0; seed=0"
                );
                assert_eq!(allocations, expected, "{backend:?} {name}\n{report}");
            }
        }
    });
}
