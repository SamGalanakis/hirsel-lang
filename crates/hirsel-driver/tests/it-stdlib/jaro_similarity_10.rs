//! strsim dacc84c0dc61eff0ee0ff66962bcf2e17018ad26, src/lib.rs:824-984.
//! MIT notice and semantic adaptations: docs/text-similarity.md.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{Integer, Limits, Outcome, Value, library::Library};
use hirsel_syntax::{DefPath, ItemPath, PackageId};
use hirsel_types::Primitive;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/jaro-similarity-10-{name}.hir"))
}

// Independent tiny matcher: remove candidates from a pool, record matched
// positions, sort by the right position, then compare the two sequences.
// It uses neither the port's flags nor its transposition cursor.
fn oracle(a: &str, b: &str) -> (f64, f64) {
    let left: Vec<_> = a.chars().collect();
    let right: Vec<_> = b.chars().collect();
    let n = left.len();
    let m = right.len();
    if n == 0 || m == 0 {
        let value = if n == m { 1.0 } else { 0.0 };
        return (value, value);
    }
    let radius = (n.max(m) / 2).saturating_sub(1);
    let mut available: Vec<_> = right.iter().copied().enumerate().collect();
    let mut pairs = Vec::new();
    for (i, ch) in left.iter().enumerate() {
        if let Some(at) = available
            .iter()
            .position(|(j, candidate)| i.abs_diff(*j) <= radius && ch == candidate)
        {
            let (j, matched) = available.remove(at);
            pairs.push((j, matched));
        }
    }
    if pairs.is_empty() {
        return (0.0, 0.0);
    }
    let ordered_left: Vec<_> = pairs.iter().map(|(_, ch)| *ch).collect();
    pairs.sort_by_key(|(j, _)| *j);
    let mismatches = ordered_left
        .iter()
        .zip(&pairs)
        .filter(|(a, (_, b))| **a != *b)
        .count();
    let matches = pairs.len();
    let similarity = (matches as f64 / n as f64
        + matches as f64 / m as f64
        + (matches - mismatches / 2) as f64 / matches as f64)
        / 3.0;
    let prefix = left
        .iter()
        .zip(&right)
        .take(4)
        .take_while(|(a, b)| a == b)
        .count();
    let winkler = if similarity > 0.7 {
        similarity + 0.1 * prefix as f64 * (1.0 - similarity)
    } else {
        similarity
    };
    (similarity, winkler)
}

#[test]
fn independent_frozen_oracle() {
    let mut words = vec![String::new()];
    for len in 1..=4 {
        for bits in 0..1 << len {
            words.push(
                (0..len)
                    .rev()
                    .map(|shift| if bits & (1 << shift) == 0 { 'a' } else { 'é' })
                    .collect(),
            );
        }
    }
    let source = std::fs::read_to_string(fixture("exhaustive")).unwrap();
    let expected = source
        .split("let expected: List[f64] = [")
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap();
    let frozen: Vec<f64> = expected
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
    let mut calculated = Vec::new();
    for a in &words {
        for b in &words {
            let (jaro, winkler) = oracle(a, b);
            calculated.extend([jaro, winkler]);
        }
    }
    assert_eq!(frozen.len(), 1922);
    assert_eq!(frozen, calculated);
    // Three ordered mismatches must be integer-halved to one transposition.
    assert_eq!(
        oracle("aaaabc", "aaabca"),
        (0.9444444444444445, 0.9611111111111111)
    );
    assert_eq!(oracle("é", "e\u{301}"), (0.0, 0.0));
}

fn run(name: &str, backend: Option<native::Backend>) {
    with_compiler_stack(|| {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let outcome = if let Some(backend) = backend {
            native::run_path_native_backend_with_validation(
                &fixture(name),
                HostLaunch::empty(),
                None,
                backend,
                native::Output::Streams {
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                Verify::On,
                native::Validation::EveryStage,
            )
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap()
        } else {
            run_path_with_step_budget(
                &fixture(name),
                SimulationConfig::default(),
                Some(20_000_000),
                Verify::On,
                &mut stdout,
                &mut stderr,
            )
            .unwrap_or_else(|error| panic!("{error}"))
        };
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{name}: {outcome:?}; {}",
            String::from_utf8_lossy(&stderr)
        );
        assert!(stdout.is_empty());
    });
}

macro_rules! cases {
    ($name:ident, $backend:expr) => {
        mod $name {
            use super::*;
            #[test]
            fn vectors() {
                run("vectors", $backend);
            }
            #[test]
            fn bounds() {
                run("bounds", $backend);
            }
            #[test]
            fn exhaustive() {
                run("exhaustive", $backend);
            }
        }
    };
}
cases!(interpreter, None);
cases!(llvm, Some(native::Backend::Llvm));
cases!(cranelift, Some(native::Backend::Cranelift));

mod h {
    use super::*;
    fn audit(name: &str) {
        with_compiler_stack(|| {
            let mut loaded = load_program(&fixture(name)).unwrap_or_else(|error| panic!("{error}"));
            loaded
                .check_for_execution()
                .unwrap_or_else(|error| panic!("{error}"));
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input.clone()).unwrap();
            let ready = native::build(
                &validated,
                &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            )
            .unwrap();
            let run = native::run_heap_in(
                &ready,
                &loaded.entry,
                &native::Sites::new(&input),
                &native::HeapWorld {
                    steps: Some(20_000_000),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                run.observation.exit_code,
                Some(0),
                "{name}: {:?}",
                run.observation
            );
        });
    }
    #[test]
    fn vectors() {
        audit("vectors");
    }
    #[test]
    fn bounds() {
        audit("bounds");
    }
    #[test]
    fn exhaustive() {
        audit("exhaustive");
    }
}

fn integer(n: u64) -> Value {
    Value::Integer(Integer::new(Primitive::U64, u128::from(n)).unwrap())
}

fn invoke(loaded: &mut LoadedProgram, name: &str, args: Vec<Value>) -> Value {
    let config = RunConfiguration {
        entry: DefPath(ItemPath {
            package: PackageId {
                name: "std".into(),
                version: "1".into(),
            },
            modules: vec!["text_similarity".into()],
            name: name.into(),
        }),
        hash_seed: 0,
        inputs: vec![],
    };
    let runtime = loaded.engine.runtime_types().unwrap();
    let sources = loaded.engine.sources.clone();
    let mut library = Library::new(&config, Default::default()).unwrap();
    let execution = hirsel_interp::run_with_runtime(
        &mut loaded.engine,
        &sources,
        &config,
        args,
        Limits::default(),
        &mut library,
        &runtime,
    )
    .unwrap();
    let Outcome::Returned { value, .. } = execution.outcome else {
        panic!("{:?}", execution.outcome)
    };
    value
}

#[test]
fn synthetic_preflight_overflow_and_threshold() {
    with_compiler_stack(|| {
        let mut loaded = load_program(&fixture("vectors")).unwrap();
        loaded.check_for_execution().unwrap();
        let accepted = invoke(
            &mut loaded,
            "preflight",
            vec![integer(1), integer(1), integer(1)],
        );
        let refused = invoke(
            &mut loaded,
            "preflight",
            vec![integer(1), integer(1), integer(0)],
        );
        assert_ne!(accepted, refused);
        for (n, m, cells, ok) in [
            (0, u64::MAX, 0, true),
            (u64::MAX, 0, 0, true),
            (u64::MAX, 2, u64::MAX, false),
            (u64::MAX, 1, u64::MAX, false),
            (1, u64::MAX, u64::MAX, false),
            (u64::MAX / 2, 3, u64::MAX, false),
            (1_152_921_504_606_846_971, 1, u64::MAX, true),
            (1_152_921_504_606_846_972, 1, u64::MAX, false),
            (1, 1_152_921_504_606_846_971, u64::MAX, true),
            (1, 1_152_921_504_606_846_972, u64::MAX, false),
            (6, 6, 36, true),
            (6, 6, 35, false),
        ] {
            let value = invoke(
                &mut loaded,
                "preflight",
                vec![integer(n), integer(m), integer(cells)],
            );
            assert_eq!(
                value,
                if ok { &accepted } else { &refused }.clone(),
                "{n},{m},{cells}"
            );
        }
        for (score, expected) in [
            (
                f64::from_bits(0.7f64.to_bits() - 1),
                f64::from_bits(0.7f64.to_bits() - 1),
            ),
            (0.7, 0.7),
            (f64::from_bits(0.7f64.to_bits() + 1), 0.8200000000000001),
            (0.8, 0.88),
        ] {
            let value = invoke(
                &mut loaded,
                "boost",
                vec![Value::F64(score.to_bits()), integer(4)],
            );
            assert_eq!(value, Value::F64(expected.to_bits()));
        }
    });
}

#[test]
fn new_module_importer_selection() {
    with_compiler_stack(|| {
        let mut loaded = load_program(&fixture("vectors")).unwrap();
        loaded.check_for_execution().unwrap();
        for name in ["jaro", "jaro_winkler"] {
            assert!(matches!(
                invoke(
                    &mut loaded,
                    name,
                    vec![
                        Value::String("💚".into()),
                        Value::String("💚".into()),
                        integer(1),
                        integer(1)
                    ]
                ),
                Value::Variant { .. }
            ));
        }
    });
}
