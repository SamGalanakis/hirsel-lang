//! SEM-2 finite model: iterative adjacent rounds, independent of pair_range's recursion.
use crate::source_std::load_program_source;
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn source_error(error: ToolError) -> ! {
    if let ToolError::Query {
        error: QueryError::Diagnostics(diagnostics),
        sources,
        ..
    } = &error
    {
        for diagnostic in diagnostics {
            eprintln!("{}", diagnostic.render_human(sources));
        }
    }
    panic!("{error}")
}

/// Shared with the original wave2 oracle. The odd tail is carried, never added to zero.
pub(super) fn pair_rounds<T: Clone>(values: &[T], add: impl Fn(&T, &T) -> T) -> Option<T> {
    let mut round = values.to_vec();
    while round.len() > 1 {
        round = round
            .chunks(2)
            .map(|pair| {
                if pair.len() == 2 {
                    add(&pair[0], &pair[1])
                } else {
                    pair[0].clone()
                }
            })
            .collect();
    }
    round.pop()
}

/// Leaves are 0..len; every later index names exactly one rounded addition.
fn dag(len: usize) -> (Vec<(usize, usize)>, Option<usize>) {
    let mut edges = Vec::new();
    let leaves: Vec<_> = (0..len).collect();
    // Build using the same round combinator, with interior mutability only for node IDs.
    let nodes = std::cell::RefCell::new(&mut edges);
    let root = pair_rounds(&leaves, |&a, &b| {
        let mut nodes = nodes.borrow_mut();
        let id = len + nodes.len();
        nodes.push((a, b));
        id
    });
    (edges, root)
}

// Hand-derived IEEE words, not decimal conversion or sum_exact results. For p=24/53,
// A=2^(p+1): (A+1)+(-A+1)=0, a left fold gives 1, exact sum gives 2.
// (1+half_ulp)+half_ulp=1, while adding the two half ulps first gives nextafter(1).
fn fixed(narrow: bool) -> Vec<(Vec<u64>, u64)> {
    let (one, a, half, sign, inf, nan, normal, max) = if narrow {
        (
            0x3f800000, 0x4c000000, 0x33800000, 0x80000000, 0x7f800000, 0x7fc00000, 0x00800000,
            0x7f7fffff,
        )
    } else {
        (
            0x3ff0000000000000,
            0x4350000000000000,
            0x3ca0000000000000,
            0x8000000000000000,
            0x7ff0000000000000,
            0x7ff8000000000000,
            0x0010000000000000,
            0x7fefffffffffffff,
        )
    };
    vec![
        (vec![], 0),
        (vec![sign], sign),
        (vec![nan], nan),
        (vec![a, one, a | sign, one], 0),
        (
            vec![a, a | sign, one, one],
            one + (if narrow { 1 << 23 } else { 1 << 52 }),
        ),
        (vec![a, one, one, a | sign], 0),
        (vec![one, half, half], one),
        (vec![half, half, one], one + 1),
        (vec![one + 1, half], one + 2),
        (vec![one, half, one | sign], 0),
        (vec![1, 1], 2),
        (vec![normal - 1, 1], normal),
        (vec![normal, (normal - 1) | sign], 1),
        (vec![normal, normal | sign, 1], 1),
        (vec![max, max], inf),
        (vec![max, max, max | sign], inf),
        (vec![sign, sign], sign),
        (vec![sign, sign, sign], sign),
        (vec![sign, 0], 0),
        (vec![one, one | sign], 0),
        (vec![inf], inf),
        (vec![inf | sign], inf | sign),
        (vec![inf, one], inf),
        (vec![inf | sign, one], inf | sign),
        (vec![inf, inf | sign], nan),
        (vec![inf, inf | sign, one], nan),
        (vec![nan, one], nan),
        (vec![one, nan, one], nan),
    ]
}

fn model(words: &[u64], narrow: bool) -> u64 {
    if narrow {
        let values: Vec<_> = words.iter().map(|&w| f32::from_bits(w as u32)).collect();
        let result = pair_rounds(&values, |a, b| a + b).unwrap_or(0.0);
        if result.is_nan() {
            0x7fc00000
        } else {
            u64::from(result.to_bits())
        }
    } else {
        let values: Vec<_> = words.iter().map(|&w| f64::from_bits(w)).collect();
        let result = pair_rounds(&values, |a, b| a + b).unwrap_or(0.0);
        if result.is_nan() {
            0x7ff8000000000000
        } else {
            result.to_bits()
        }
    }
}

fn cases(narrow: bool) -> Vec<(Vec<u64>, u64)> {
    let mut cases = fixed(narrow);
    for (words, expected) in &cases {
        assert_eq!(
            model(words, narrow),
            *expected,
            "manual IEEE vector {words:x?}"
        );
    }
    let mut seed = 0x9e3779b97f4a7c15u64;
    for len in 0..=65 {
        // Track the leaf-pair DAG for every generated length, before executing it.
        assert_eq!(dag(len).0.len(), len.saturating_sub(1));
        for pattern in 0..3 {
            let words: Vec<_> = (0..len)
                .map(|i| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    if pattern == 0 {
                        let exponent = (seed >> 58) as i32 - 32;
                        let value = (if seed & 1 == 0 { 1.0 } else { -1.0 }) * (seed >> 11) as f64
                            / (1u64 << 53) as f64
                            * 2f64.powi(exponent);
                        if narrow {
                            u64::from((value as f32).to_bits())
                        } else {
                            value.to_bits()
                        }
                    } else {
                        let (a, one, sign) = if narrow {
                            (0x4c000000, 0x3f800000, 0x80000000)
                        } else {
                            (0x4350000000000000, 0x3ff0000000000000, 0x8000000000000000)
                        };
                        [a, one, a | sign, one][(i + pattern - 1) % 4]
                    }
                })
                .collect();
            let expected = model(&words, narrow);
            cases.push((words, expected));
        }
    }
    assert_eq!(cases.len(), 226);
    assert!(cases.len() <= 256);
    cases
}

fn source(narrow: bool, batch: usize) -> String {
    let mut source = String::from("pub fn main(t: Test) -> Result[(), ()] {\n");
    let (family, uint) = if narrow {
        ("f32", "u32")
    } else {
        ("f64", "u64")
    };
    source += &format!("let cases: Array[(Array[{family}], {uint})] = [\n");
    let rows = cases(narrow);
    let rows: Vec<_> = rows.chunks(16).nth(batch).unwrap().to_vec();
    eprintln!(
        "{family} batch {batch}: {} exact retained cases",
        rows.len()
    );
    for (words, expected) in rows {
        let inputs = words
            .iter()
            .map(|word| format!("core@1/{family}.from_bits(0x{word:x}{uint})"))
            .collect::<Vec<_>>()
            .join(",");
        source += &format!("([{inputs}], 0x{expected:x}{uint}),\n");
    }
    source += &format!(
        "]\nfor (values, expected) in cases {{ core@1/test.check(t, core@1/{family}.bits(std@1/{family}.sum(values)) == expected) }}\n"
    );
    source += include_str!("../../../../conformance/SEM/SEM-2-float-reduction-model-16-run.hir")
        .split("// CONTROLS\n")
        .nth(1)
        .unwrap();
    source
}

macro_rules! batches {
    ($narrow:literal; $($name:ident: $batch:literal),+ $(,)?) => {
        $(#[test] fn $name() {
            all_engines(stringify!($name), &source($narrow, $batch), true);
        })+
    };
}
batches!(true;
    f32_batch_00: 0, f32_batch_01: 1, f32_batch_02: 2, f32_batch_03: 3,
    f32_batch_04: 4, f32_batch_05: 5, f32_batch_06: 6, f32_batch_07: 7,
    f32_batch_08: 8, f32_batch_09: 9, f32_batch_10: 10, f32_batch_11: 11,
    f32_batch_12: 12, f32_batch_13: 13, f32_batch_14: 14);
batches!(false;
    f64_batch_00: 0, f64_batch_01: 1, f64_batch_02: 2, f64_batch_03: 3,
    f64_batch_04: 4, f64_batch_05: 5, f64_batch_06: 6, f64_batch_07: 7,
    f64_batch_08: 8, f64_batch_09: 9, f64_batch_10: 10, f64_batch_11: 11,
    f64_batch_12: 12, f64_batch_13: 13, f64_batch_14: 14);

fn all_engines(name: &str, source: &str, success: bool) {
    with_compiler_stack(|| {
        let mut loaded = load_program_source(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir"),
            Some(source),
        )
        .unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| source_error(error));
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(10_000_000),
                call_depth: 256,
                source: None,
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
        assert_eq!(
            matches!(
                reference.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            success,
            "{name}: {:?}",
            reference.outcome
        );
        if !success {
            assert!(
                matches!(&reference.outcome, Ok(RunOutcome::Failed { primary, .. }) if primary.rule == "LANG-9"),
                "{:?}",
                reference.outcome
            );
        }
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let ready = native::build(&validated, &options).unwrap();
        if success {
            // Inspect only arithmetic flags, never compare unstable LLVM text snapshots.
            let ir = native::emit_ir(&ready, native::OptLevel::Speed).unwrap();
            let mut additions = 0;
            for line in ir.lines() {
                let words: Vec<_> = line.split_whitespace().collect();
                if let Some(op) = words
                    .iter()
                    .position(|w| matches!(*w, "fadd" | "fsub" | "fmul" | "fdiv" | "frem"))
                {
                    additions += usize::from(words[op] == "fadd");
                    for flag in [
                        "fast", "reassoc", "contract", "nnan", "ninf", "nsz", "arcp", "afn",
                    ] {
                        assert!(
                            !words[op + 1..].contains(&flag),
                            "relaxed arithmetic: {line}"
                        );
                    }
                }
            }
            assert!(additions >= 1, "audit must see ordinary sum additions");
        }
        let sites = native::Sites::new(validated.snapshot());
        let heap = native::run_heap_in(
            &ready,
            &loaded.entry,
            &sites,
            &native::HeapWorld {
                steps: Some(10_000_000),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(heap.observation, expected);
        assert_eq!(heap.counters.live_heap_bytes, 0);
        assert!(heap.counters.peak_live_heap_bytes < 4_000_000);
        assert!(
            heap.counters.allocations < 100_000,
            "{name}: {:?}",
            heap.counters
        );
        eprintln!(
            "{name}: {} heap allocations, zero live bytes",
            heap.counters.allocations
        );
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
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("float-reduction-model-16-{name}"));
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        let mut count = 0;
        for (config, outcome) in
            native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
        {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{name}: {config}: {outcome:?}"
            );
            count += 1;
        }
        assert_eq!(count, 2);
        eprintln!(
            "float-reduction-model-16 {name}: interpreter + checked heap + LLVM Speed + Cranelift agree"
        );
    });
}
