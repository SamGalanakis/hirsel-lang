//! Go sort/search_test.go:15-62, ebe2feefc400f813aefdc8a036e66d2619da6649.
//! Linear oracles and full interpreter traces are independent of the binary search.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{Integer, Limits, Outcome, TraceEvent, TraceMode, Value, library::Library};
use hirsel_syntax::{DefPath, ItemPath, PackageId};
use hirsel_types::Primitive;
use std::{fmt::Write, path::PathBuf};

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn integer(n: u64) -> Value {
    Value::Integer(Integer::new(Primitive::U64, u128::from(n)).unwrap())
}

fn trace(
    loaded: &mut LoadedProgram,
    package: &str,
    module: &[&str],
    name: &str,
    args: Vec<Value>,
) -> hirsel_interp::Execution {
    let entry = DefPath(ItemPath {
        package: PackageId {
            name: package.into(),
            version: "1".into(),
        },
        modules: module.iter().map(|m| (*m).into()).collect(),
        name: name.into(),
    });
    let mut runtime = loaded.engine.runtime_types().unwrap();
    runtime.trace_mode = TraceMode::Full;
    let sources = loaded.engine.sources.clone();
    let config = RunConfiguration {
        entry,
        hash_seed: 0,
        inputs: vec![],
    };
    let mut library = Library::new(&config, Default::default()).unwrap();
    hirsel_interp::run_with_runtime(
        &mut loaded.engine,
        &sources,
        &config,
        args,
        Limits {
            steps: Some(1_000_000),
            ..Default::default()
        },
        &mut library,
        &runtime,
    )
    .unwrap()
}

#[test]
fn comparator_counts_are_logarithmic_including_empty() {
    with_compiler_stack(|| {
        let mut loaded =
            load_program(&fixture("tests/fixtures/sorted_bounds_7/trace.hir")).unwrap();
        loaded.check_for_execution().unwrap();
        let mut runs = 0;
        for n in [0u64, 1, 2, 3, 16, 65, 1024] {
            for value in [0, n / 6, n / 3, n + 1] {
                for op in 0..6 {
                    let execution = trace(
                        &mut loaded,
                        "application",
                        &[],
                        "probe",
                        vec![integer(n), integer(value), integer(op)],
                    );
                    let lower = (0..n)
                        .filter(|i| (if op >= 3 { 0 } else { i / 3 }) < value)
                        .count() as u64;
                    let upper = (0..n)
                        .filter(|i| (if op >= 3 { 0 } else { i / 3 }) <= value)
                        .count() as u64;
                    let pair = match op % 3 {
                        0 => (lower, lower),
                        1 => (upper, upper),
                        _ => (lower, upper),
                    };
                    assert!(
                        matches!(execution.outcome, Outcome::Returned { ref value, .. }
                        if *value == Value::Tuple(vec![Some(integer(pair.0)), Some(integer(pair.1))])),
                        "{n}/{value}/{op}: {:?}",
                        execution.outcome
                    );
                    let calls = execution
                        .trace
                        .iter()
                        .filter(|event| {
                            matches!(event,
                        TraceEvent::Enter { path, .. } if path.0.name.as_str() == "compare")
                        })
                        .count();
                    let per_search = (u64::BITS - n.leading_zeros()) as usize;
                    assert!(
                        calls <= per_search * if op % 3 == 2 { 2 } else { 1 },
                        "n={n}, op={op}, calls={calls}"
                    );
                    assert_eq!(calls == 0, n == 0);
                    runs += 1;
                }
            }
        }
        assert_eq!(runs, 168);
    });
}

#[test]
fn midpoint_and_go_large_domains_without_large_allocations() {
    with_compiler_stack(|| {
        let mut loaded =
            load_program(&fixture("tests/fixtures/sorted_bounds_7/trace.hir")).unwrap();
        loaded.check_for_execution().unwrap();
        // Call the actual private arithmetic helper, including max+max overflow controls.
        for (lo, hi) in [
            (0, u64::MAX),
            (u64::MAX - 2, u64::MAX),
            (u64::MAX - 1, u64::MAX),
            (991, 1_000_000_000),
            (1_999_999_999, 2_000_000_000),
        ] {
            let execution = trace(
                &mut loaded,
                "std",
                &["sorted"],
                "midpoint",
                vec![integer(lo), integer(hi)],
            );
            let expected = ((u128::from(lo) + u128::from(hi)) / 2) as u64;
            assert!(
                matches!(execution.outcome, Outcome::Returned { ref value, .. } if *value == integer(expected)),
                "{:?}",
                execution.outcome
            );
        }
        // Go's synthetic 1e9/2e9 vectors and full u64 domain, using the source helper.
        for (n, boundary) in [
            (1_000_000_000, 991),
            (1_000_000_000, 0),
            (1_000_000_000, 1_000_000_000 - 7),
            (2_000_000_000, 2_000_000_000),
            (u64::MAX, u64::MAX),
        ] {
            let (mut lo, mut hi, mut calls) = (0, n, 0);
            while lo < hi {
                let execution = trace(
                    &mut loaded,
                    "std",
                    &["sorted"],
                    "midpoint",
                    vec![integer(lo), integer(hi)],
                );
                let Outcome::Returned {
                    value: Value::Integer(mid),
                    ..
                } = execution.outcome
                else {
                    panic!("{:?}", execution.outcome)
                };
                let mid = mid.bits() as u64;
                assert!(lo <= mid && mid < hi);
                if mid < boundary {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
                calls += 1;
            }
            assert_eq!(lo, boundary);
            assert!(calls <= 64);
        }
    });
}

#[test]
fn generated_small_lists_match_independent_linear_oracle() {
    with_compiler_stack(|| {
        let template =
            include_str!("../../../../conformance/SEM/SEM-5-sorted-bounds-7-vectors-run.hir");
        let mut source = template.split("pub fn main()").next().unwrap().to_owned();
        source.push_str("pub fn main() -> Result[(), ()] {\n");
        let mut seed = 7u64;
        let mut queries = 0;
        for len in 0..=24 {
            let mut xs = Vec::new();
            for _ in 0..len {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                xs.push(((seed >> 32) % 9) as i32 - 4);
            }
            xs.sort_unstable();
            for descending in [false, true] {
                if descending {
                    xs.reverse();
                }
                writeln!(source, "{{ let xs: List[i32] = {xs:?}").unwrap();
                for value in -5..=5 {
                    writeln!(
                        source,
                        "verify(xs, {value}, {})",
                        if descending { "descending" } else { "compare" }
                    )
                    .unwrap();
                    queries += 1;
                }
                source.push_str("}\n");
            }
        }
        source.push_str("Ok(())\n}\n");
        let path = fixture("../../conformance/SEM/SEM-5-sorted-bounds-7-vectors-run.hir");
        let mut loaded = load_program_source(&path, Some(&source)).unwrap();
        loaded.check_for_execution().unwrap();
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: Limits {
                steps: Some(5_000_000),
                ..Default::default()
            },
            ..Default::default()
        }));
        let run = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry,
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                run.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            run.outcome
        );
        assert_eq!(queries, 550);
    });
}

fn resource_case(op: &str) {
    let fail = op != "owned";
    let path = if fail {
        format!("../../conformance/MEM/MEM-14-sorted-bounds-7-{op}.hir")
    } else {
        "../../conformance/SEM/SEM-5-sorted-bounds-7-owned-run.hir".into()
    };
    let mut output = vec![];
    let result = run_path(&fixture(&path), &mut output).unwrap();
    if fail {
        assert!(
            matches!(result, RunOutcome::Failed { primary: hirsel_run_schema::Failure { ref rule, ref site, .. }, .. }
            if rule.as_str() == "MEM-14" && site.line == 15 && site.file.ends_with(&format!("MEM-14-sorted-bounds-7-{op}.hir"))),
            "{result:?}"
        );
    } else {
        assert!(
            matches!(
                result,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{result:?}"
        );
    }
    let output = String::from_utf8(output).unwrap();
    let mut lines = output.lines();
    if !fail {
        assert_eq!(lines.next(), Some("H 2 6"));
    }
    let mut ids: Vec<u64> = lines
        .map(|line| line.strip_prefix("D ").unwrap().parse().unwrap())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, (0..8).collect::<Vec<_>>());
}

#[test]
fn first_comparator_failure_and_upper_phase_failure_clean_up() {
    with_compiler_stack(|| {
        for op in ["lower", "upper", "equal"] {
            resource_case(op);
        }
    });
}

#[test]
fn duplicate_resource_range_borrows_without_cloning_or_reordering() {
    with_compiler_stack(|| resource_case("owned"));
}
