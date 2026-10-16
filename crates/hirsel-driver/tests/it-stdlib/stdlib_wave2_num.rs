//! Independent host text and rational oracles for ordinary numeric std source.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_lir::ir::UnitOrigin;
use std::{collections::BTreeSet, path::Path};

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir")
}

fn execute(source: &str) {
    let mut loaded = load_program_source(&fixture(), Some(source)).unwrap();
    loaded.check_for_execution().unwrap();
    loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
        limits: hirsel_interp::Limits {
            steps: 100_000_000,
            call_depth: 256,
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
        matches!(run.outcome, Ok(RunOutcome::Exit(0))),
        "{:?}",
        run.outcome
    );
}

#[test]
fn source_parser_rounds_midpoints_and_unbounded_decimal_suffixes_like_rust() {
    with_compiler_stack(|| {
        let tie = "1.00000000000000011102230246251565404236316680908203125";
        let halfway_f32 = "1.000000059604644775390625";
        let mut source = String::from("pub fn main(t: Test) -> Result[(), ()] {\n");
        for text in [
            tie.into(),
            format!("{tie}{}1", "0".repeat(1000)),
            halfway_f32.into(),
            format!("{halfway_f32}{}1", "0".repeat(1000)),
            "2.4703282292062327e-324".into(),
            "2.4703282292062328e-324".into(),
            "7.006492321624085e-46".into(),
            "7.006492321624086e-46".into(),
            "-1e-10000".into(),
            "1.7976931348623158e308".into(),
            "1.7976931348623159e308".into(),
            "3.4028235e38".into(),
            "3.4028236e38".into(),
        ] {
            for (family, bits, finite) in [
                {
                    let value: f32 = text.parse().unwrap();
                    ("f32", u64::from(value.to_bits()), value.is_finite())
                },
                {
                    let value: f64 = text.parse().unwrap();
                    ("f64", value.to_bits(), value.is_finite())
                },
            ] {
                if finite {
                    let uint = if family == "f32" { "u32" } else { "u64" };
                    source += &format!(
                        "match std@1/{family}.parse(\"{text}\") {{ Ok(value) => core@1/test.check(t, core@1/{family}.bits(value) == {bits}{uint}), Error(_) => core@1/test.check(t, false) }}\n"
                    );
                } else {
                    source += &format!(
                        "core@1/test.check(t, std@1/{family}.parse(\"{text}\") == Error(OutOfRange))\n"
                    );
                }
            }
        }
        source += "Ok(())\n}\n";
        execute(&source);
    });
}

#[test]
fn source_exact_sum_matches_all_256_independent_rational_vectors_in_both_orders() {
    with_compiler_stack(|| {
        let rows: Vec<_> = include_str!("../../../../native-tests/library/exact-sums.txt")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect();
        assert_eq!(rows.len(), 256);
        for batch in rows.chunks(16) {
            let mut source = String::from(
                "pub fn main(t: Test) -> Result[(), ()] {\nlet cases: Array[(Array[f64], u64)] = [\n",
            );
            for line in batch {
                let (input, expected) = line.split_once(" / ").unwrap();
                let mut words: Vec<_> = input.split_whitespace().collect();
                for _ in 0..2 {
                    let items = words
                        .iter()
                        .map(|word| format!("core@1/f64.from_bits(0x{word}u64)"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    source += &format!("([{items}], 0x{expected}u64),\n");
                    words.reverse();
                }
            }
            source += "]\nfor (values, expected) in cases { core@1/test.check(t, core@1/f64.bits(std@1/f64.sum_exact(values)) == expected) }\nOk(())\n}\n";
            execute(&source);
        }
    });
}

#[test]
fn every_numeric_catalogue_function_compiles_as_an_ordinary_source_unit() {
    with_compiler_stack(|| {
        let mut source = String::from(
            "pub fn main(t: Test) -> Result[(), ()] {\nlet mut buffer: Inline[u8, 40] = [",
        );
        source += &vec!["0u8"; 40].join(", ");
        source += "]\n";
        let families = [
            "bool", "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128", "f32",
            "f64",
        ];
        for family in families {
            let literal = if family == "bool" {
                "true".into()
            } else {
                format!("1{family}")
            };
            let text = if family == "bool" { "true" } else { "1" };
            source += &format!(
                "std@1/{family}.to_string({literal});\nstd@1/{family}.format({literal}, mut buffer);\nstd@1/{family}.parse(\"{text}\");\n"
            );
        }
        for family in ["f32", "f64"] {
            source += &format!(
                "let values_{family}: Array[{family}] = [1{family}]\nstd@1/{family}.sum(values_{family});\nstd@1/{family}.sum_exact(values_{family});\n"
            );
        }
        source += "Ok(())\n}\n";
        let mut loaded = load_program_source(&fixture(), Some(&source)).unwrap();
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
            if header.definition.0.package.name == "std" {
                assert_eq!(header.origin, UnitOrigin::Source);
                found.insert((
                    header.definition.0.modules.join("/"),
                    header.definition.0.name.clone(),
                ));
            }
        }
        for family in families {
            for name in ["to_string", "format", "parse"] {
                assert!(
                    found.contains(&(family.into(), name.into())),
                    "{family}.{name}"
                );
            }
        }
        for family in ["f32", "f64"] {
            for name in ["sum", "sum_exact"] {
                assert!(
                    found.contains(&(family.into(), name.into())),
                    "{family}.{name}"
                );
            }
        }
    });
}

#[test]
fn adjacent_pair_reductions_match_an_independent_host_tree_at_both_widths() {
    with_compiler_stack(|| {
        let mut source = String::from("pub fn main(t: Test) -> Result[(), ()] {\n");
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for len in [0, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 100] {
            let mut values = Vec::new();
            for _ in 0..len {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let exponent = (seed >> 58) as i32 - 32;
                let sign = if seed & 1 == 0 { 1.0 } else { -1.0 };
                values.push(sign * (seed >> 11) as f64 / (1u64 << 53) as f64 * 2f64.powi(exponent));
            }
            let mut wide = values.clone();
            let mut narrow: Vec<f32> = values.iter().map(|&v| v as f32).collect();
            while wide.len() > 1 {
                wide = wide
                    .chunks(2)
                    .map(|c| if c.len() == 2 { c[0] + c[1] } else { c[0] })
                    .collect();
                narrow = narrow
                    .chunks(2)
                    .map(|c| if c.len() == 2 { c[0] + c[1] } else { c[0] })
                    .collect();
            }
            for (family, uint, words, expected) in [
                (
                    "f64",
                    "u64",
                    values.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    wide.first().copied().unwrap_or(0.0).to_bits(),
                ),
                (
                    "f32",
                    "u32",
                    values
                        .iter()
                        .map(|&v| (v as f32).to_bits() as u64)
                        .collect(),
                    narrow.first().copied().unwrap_or(0.0).to_bits() as u64,
                ),
            ] {
                let inputs = words
                    .iter()
                    .map(|word| format!("core@1/{family}.from_bits({word}{uint})"))
                    .collect::<Vec<_>>()
                    .join(",");
                source += &format!(
                    "let v_{family}_{len}: Array[{family}] = [{inputs}]\ncore@1/test.check(t, core@1/{family}.bits(std@1/{family}.sum(v_{family}_{len})) == {expected}{uint})\n"
                );
            }
        }
        source += "Ok(())\n}\n";
        execute(&source);
    });
}
