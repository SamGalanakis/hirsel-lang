//! CPython 938144ef Lib/heapq.py:148-169 and Lib/test/test_heapq.py:134-185,243-262.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::observation::Effect;
use hirsel_lir::ir::UnitOrigin;
use std::{collections::BTreeSet, path::PathBuf};

fn corpus(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance")
        .join(relative)
}

#[test]
fn combined_operations_are_ordinary_source_native_units() {
    with_compiler_stack(|| {
        let mut loaded = load_program(&corpus("SEM/SEM-5-pqcombined-vectors-run.hir")).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let mut found = BTreeSet::new();
        for unit in &ready.units {
            let header = &unit.unit().header;
            if header.definition.0.package.name_is("std")
                && header.definition.0.modules == ["priority_queue"]
            {
                assert_eq!(header.origin, UnitOrigin::Source);
                found.insert(header.definition.0.name.as_str());
            }
        }
        for name in ["replace_top", "push_pop", "down"] {
            assert!(
                found.contains(name),
                "missing ordinary source {name}: {found:?}"
            );
        }
    });
}

#[test]
fn owned_returns_ties_and_comparison_failures_finalize_every_owner_once() {
    with_compiler_stack(|| {
        for (relative, failing) in [
            ("DROP/DROP-2-pqcombined-owners-run.hir", false),
            ("FAIL/FAIL-3-pqcombined-push-pop-first-fail.hir", true),
            ("FAIL/FAIL-3-pqcombined-push-pop-late-fail.hir", true),
            ("FAIL/FAIL-3-pqcombined-replace-first-fail.hir", true),
            ("FAIL/FAIL-3-pqcombined-replace-late-fail.hir", true),
        ] {
            let path = corpus(relative);
            let mut loaded = load_program(&path).unwrap();
            loaded.check_for_execution().unwrap();
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
            if failing {
                assert!(
                    matches!(&run.outcome, Ok(RunOutcome::Failed { primary, .. }) if primary.rule.as_str() == "MEM-14"),
                    "{relative}: {:?}",
                    run.outcome
                );
            } else {
                assert!(
                    matches!(
                        run.outcome,
                        Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                    ),
                    "{relative}: {:?}",
                    run.outcome
                );
            }
            let obs = run.observation.unwrap();
            let drops = obs.effects.iter().filter(|e| matches!(&e.effect, Effect::Finalizer { definition } if definition.0.name == "Item")).count();
            let expected = if failing { 8 } else { 7 };
            assert_eq!(drops, expected, "{relative}");
            let labels: Vec<_> = obs
                .output
                .split(|b| *b == b'\n')
                .filter(|s| !s.is_empty())
                .collect();
            assert_eq!(labels.len(), expected, "{relative}: {:?}", obs.output);
            assert_eq!(
                labels.iter().collect::<BTreeSet<_>>().len(),
                expected,
                "duplicate owner: {relative}"
            );
            if failing {
                let actual: BTreeSet<_> = labels
                    .iter()
                    .map(|s| std::str::from_utf8(s).unwrap())
                    .collect();
                assert_eq!(
                    actual,
                    BTreeSet::from(["1", "2", "3", "4", "5", "6", "7", "9"]),
                    "{relative}"
                );
                assert_eq!(obs.failures.len(), 1, "{relative}");
            } else {
                assert_eq!(
                    obs.output,
                    b"empty\nroot\ntie\nsmall\nfirst\nsecond\nretained\n"
                );
            }
        }
    });
}

// Independent BTreeMap-like sorted multiset: no heap operations or std sort oracle
// inside the Hirsel program. Every return, size, minimum and final drain is checked.
#[test]
fn ten_seeded_mixed_streams_match_sorted_multisets() {
    with_compiler_stack(|| {
        for seed in 0..10u64 {
            let mut state = seed + 1;
            let mut bag = Vec::<i32>::new();
            let mut source = String::from(
                r#"@noalloc
fn compare(a: i32, b: i32) -> i32 uses {} {
  if a < b { -2147483648 } else if a > b { 2147483647 } else { 0 }
}
pub fn main(t: Test) -> Result[(), ()] {
  let mut h: unique std@1/priority_queue.Heap[i32] = std@1/priority_queue.empty(compare)
"#,
            );
            for index in 0..64 {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let value = match index % 19 {
                    0 => i32::MIN,
                    1 => i32::MAX,
                    _ => ((state >> 32) % 17) as i32 - 8,
                };
                let call = if index % 5 == 0 {
                    let previous = if bag.is_empty() {
                        "None()".to_string()
                    } else {
                        format!("Some({})", bag.remove(0))
                    };
                    bag.push(value);
                    format!("std@1/priority_queue.replace_top(mut h, {value}) == {previous}")
                } else {
                    bag.push(value);
                    bag.sort_unstable();
                    let previous = bag.remove(0);
                    format!("std@1/priority_queue.push_pop(mut h, {value}) == {previous}")
                };
                bag.sort_unstable();
                source += &format!(
                    "  core@1/test.check(t, {call})\n  core@1/test.check(t, std@1/priority_queue.len(h) == {})\n",
                    bag.len()
                );
                if let Some(minimum) = bag.first() {
                    source += &format!(
                        "  core@1/test.check(t, std@1/priority_queue.peek(h) == Some({minimum}))\n"
                    );
                }
            }
            for value in bag {
                source += &format!(
                    "  core@1/test.check(t, std@1/priority_queue.pop(mut h) == Some({value}))\n"
                );
            }
            source += "  core@1/test.check(t, std@1/priority_queue.len(h) == 0)\n  Ok(())\n}\n";
            let mut loaded = load_program_source(
                &corpus("SEM/SEM-5-pqcombined-vectors-run.hir"),
                Some(&source),
            )
            .unwrap();
            loaded.check_for_execution().unwrap();
            let run = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    entry: loaded.entry,
                    hash_seed: seed,
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
                "seed {seed}: {:?}",
                run.outcome
            );
        }
    });
}
