//! Minimum scripts checked against independent dynamic-programming LCS lengths.
use hirsel_driver::native::*;
use hirsel_engine::{RunConfiguration, SimulationConfig};
use std::path::PathBuf;

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cli/fixtures/diff-minimal-43.hir")
}
fn witness() -> (String, String) {
    (
        "x\nc\n".repeat(8) + &"x\n".repeat(17),
        "y\nc\n".repeat(8) + &"y\n".repeat(17),
    )
}
// This oracle shares neither preprocessing nor a middle-snake search with std.
fn minimum(a: &[&str], b: &[&str]) -> usize {
    let mut row = vec![0; b.len() + 1];
    for x in a {
        let mut diagonal = 0;
        for (j, y) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if x == y {
                diagonal + 1
            } else {
                row[j].max(above)
            };
            diagonal = above;
        }
    }
    a.len() + b.len() - 2 * row[b.len()]
}
fn validate(old: &str, new: &str, output: &[u8]) -> usize {
    let a: Vec<_> = old.split_inclusive('\n').collect();
    let b: Vec<_> = new.split_inclusive('\n').collect();
    let (mut i, mut j, mut changed) = (0, 0, 0);
    let mut rebuilt = String::new();
    for line in std::str::from_utf8(output).unwrap().lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[1].parse::<usize>().unwrap(), i);
        assert_eq!(fields[2].parse::<usize>().unwrap(), j);
        let n = fields[3].parse::<usize>().unwrap();
        assert!(n > 0);
        match fields[0] {
            "K" => {
                assert_eq!(a[i..i + n], b[j..j + n]);
                rebuilt.extend(a[i..i + n].iter().copied());
                i += n;
                j += n;
            }
            "D" => {
                i += n;
                changed += n;
            }
            "I" => {
                rebuilt.extend(b[j..j + n].iter().copied());
                j += n;
                changed += n;
            }
            _ => panic!("invalid edit {line}"),
        }
    }
    assert_eq!((i, j), (a.len(), b.len()));
    assert_eq!(rebuilt, new);
    assert_eq!(
        changed,
        minimum(&a, &b),
        "old={old:?} new={new:?} script={}",
        String::from_utf8_lossy(output)
    );
    changed
}
fn pair(loaded: &mut hirsel_engine::LoadedProgram, old: &str, new: &str) -> Observation {
    let config = RunConfiguration {
        entry: loaded.entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let world = SimulationConfig {
        args: vec![old.into(), new.into()],
        ..Default::default()
    };
    let mut observations = Vec::new();
    for checked in [false, true] {
        let result = if checked {
            hirsel_engine::run_observed_checked_bounded(
                &mut loaded.engine,
                &config,
                world.clone(),
                1_000_000,
            )
        } else {
            hirsel_engine::run_observed_snapshot_bounded(
                &mut loaded.engine,
                &config,
                world.clone(),
                1_000_000,
            )
        }
        .unwrap();
        eprintln!("DIFF43 checked={checked} steps={:?}", result.step_usage);
        let observed = result
            .observation
            .unwrap_or_else(|| panic!("{:?}", result.outcome));
        assert_eq!(observed.exit_code, Some(0), "{observed:?}");
        assert!(observed.failures.is_empty(), "{observed:?}");
        observations.push(observed);
    }
    assert_eq!(observations[0], observations[1]);
    validate(old, new, &observations[0].output);
    Observation::of_reference(&observations[0])
}
#[test]
fn public_witness() {
    hirsel_engine::with_compiler_stack(|| {
        let mut loaded = hirsel_driver::load_program(&path()).unwrap();
        loaded.check_for_execution().unwrap();
        let (old, new) = witness();
        let observed = pair(&mut loaded, &old, &new);
        assert_eq!(validate(&old, &new, &observed.output), 50);
    });
}
#[test]
fn lcs_property_reference_and_checked_h() {
    hirsel_engine::with_compiler_stack(|| {
        let mut loaded = hirsel_driver::load_program(&path()).unwrap();
        loaded.check_for_execution().unwrap();
        let mut words = vec![String::new()];
        for n in 1..=3 {
            for bits in 0..1 << n {
                words.push(
                    (0..n)
                        .map(|i| if bits & (1 << i) == 0 { "a\n" } else { "b\n" })
                        .collect(),
                );
            }
        }
        let mut count = 0;
        for a in &words {
            for b in &words {
                pair(&mut loaded, a, b);
                count += 1;
            }
        }
        // Fixed seed, unequal lengths, repeated/absent tokens, and missing final LF.
        let mut seed = 43u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            seed
        };
        for _ in 0..96 {
            let mut inputs = [String::new(), String::new()];
            for text in &mut inputs {
                for _ in 0..next() % 25 {
                    text.push_str(["a\n", "b\n", "c\n", "d\n"][(next() >> 32) as usize % 4]);
                }
                if next() & 1 == 0 {
                    text.pop();
                }
            }
            pair(&mut loaded, &inputs[0], &inputs[1]);
            count += 1;
        }
        // Small random words alone don't reliably trigger Common pruning.
        for anchors in [1, 2, 4, 8] {
            for tail in [1, 9, 17] {
                let a = "x\nc\n".repeat(anchors) + &"x\n".repeat(tail);
                let b = "y\nc\n".repeat(anchors) + &"y\n".repeat(tail);
                pair(&mut loaded, &a, &b);
                count += 1;
            }
        }
        assert_eq!(count, 333);
    });
}
#[test]
fn public_witness_four_engines() {
    hirsel_engine::with_compiler_stack(|| {
        let mut loaded = hirsel_driver::load_program(&path()).unwrap();
        loaded.check_for_execution().unwrap();
        let (old, new) = witness();
        let expected = pair(&mut loaded, &old, &new);
        let input = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let configs = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Default,
                    partition: Partition::Whole,
                    runtime: Runtime::Default,
                },
            },
            Configuration {
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Default,
                },
            },
        ];
        let dir = tempfile::tempdir().unwrap();
        let tools = Toolchain::for_matrix(&dir.path().join("runtime"), &configs).unwrap();
        let fixture = Fixture {
            args: vec![old, new],
            steps: Some(1_000_000),
            ..Default::default()
        };
        for (name, outcome) in differential_in(
            &input,
            &loaded.entry,
            &expected,
            &configs,
            &tools,
            dir.path(),
            &fixture,
        ) {
            assert!(matches!(outcome, Outcome::Agree), "{name}: {outcome:?}");
        }
    });
}
