//! zmij 1.0.23, 7b7cc48b src/lib.rs:1369-1504, tests/test.rs:11-115.
//! MIT license in third-party/LICENSE.zmij.
//! Fixed expectations cross-checked with Ryū 4c0618b0 and exact rational intervals.
//! Rendering adapts -6/21 thresholds, e spelling, integral suffix and special values.
use crate::source_std::load_program_source;
use hirsel_driver::*;
use hirsel_engine::*;
use serde_json::Value;
use std::{fmt::Write, path::PathBuf};

const CASES: &[(u64, u64, i32, &str)] = &[
    (4110526470433157393, 662607015, -42, "6.62607015e-34"),
    (
        4827562498382992186,
        5444310685350916,
        -1,
        "544431068535091.6",
    ),
    (1, 5, -324, "5e-324"),
    (2, 1, -323, "1e-323"),
    (24, 12, -323, "1.2e-322"),
    (25, 124, -324, "1.24e-322"),
    (2498, 1234, -323, "1.234e-320"),
    (
        4503599627370494,
        22250738585072004,
        -324,
        "2.2250738585072004e-308",
    ),
    (4607182418800017408, 1, 0, "1"),
    (0, 0, 0, "0"),
    (9223372036854775808, 0, 0, "-0"),
    (9218868437227405312, 0, 0, "inf"),
    (18444492273895866368, 0, 0, "NaN"),
    (
        10459432193471965256,
        4932096661796888,
        -241,
        "-4.932096661796888e-226",
    ),
    (
        5138764384543219154,
        3439070283483335,
        20,
        "3.439070283483335e35",
    ),
    (
        4364844556237552070,
        6606854224493745,
        -32,
        "6.606854224493745e-17",
    ),
    (
        5531241272260666080,
        6079537928711555,
        46,
        "6.079537928711555e61",
    ),
    (
        943434253483863969,
        13588129002659584,
        -261,
        "1.3588129002659584e-245",
    ),
    (
        4494592428115755008,
        29802322387695312,
        -24,
        "2.9802322387695312e-8",
    ),
    (
        4499096027743125504,
        5960464477539063,
        -23,
        "5.960464477539063e-8",
    ),
    (
        5581048456364302559,
        13076622631878654,
        49,
        "1.3076622631878654e65",
    ),
    (
        6999880089232712320,
        903725590277404,
        145,
        "9.03725590277404e159",
    ),
    (
        7014838355096406816,
        903725590277404,
        146,
        "9.03725590277404e160",
    ),
    (
        7030138753015420660,
        903725590277404,
        147,
        "9.03725590277404e161",
    ),
    (
        7044780385476893873,
        903725590277404,
        148,
        "9.03725590277404e162",
    ),
    (
        4513732726532079616,
        5960464477539062,
        -22,
        "5.960464477539062e-7",
    ),
    (4676171550762729472, 4321, 1, "43210"),
    (4676171564506624819, 432101, -1, "43210.1"),
    (4666723172467343360, 1, 4, "10000"),
    (
        14066180773130310682,
        594273647962217,
        1,
        "-5942736479622170",
    ),
    (
        4503599627370495,
        2225073858507201,
        -323,
        "2.225073858507201e-308",
    ),
    (
        4503599627370496,
        22250738585072014,
        -324,
        "2.2250738585072014e-308",
    ),
    (
        4503599627370497,
        2225073858507202,
        -323,
        "2.225073858507202e-308",
    ),
    (
        4607182418800017407,
        9999999999999999,
        -16,
        "0.9999999999999999",
    ),
    (
        4607182418800017409,
        10000000000000002,
        -16,
        "1.0000000000000002",
    ),
    (
        9218868437227405311,
        17976931348623157,
        292,
        "1.7976931348623157e308",
    ),
];

fn run_partition(start: usize, count: usize, narrow_start: usize, narrow_count: usize) {
    with_compiler_stack(|| {
        let rows: Vec<Value> = CASES.iter().enumerate().map(|(index, &(bits, digits, exponent, text))| serde_json::json!({"id": format!("upstream-{index:02}"), "bits": bits, "digits": digits, "exponent": exponent, "text": text})).collect();
        let selected = &rows[start..start + count];
        assert!(!selected.is_empty() || narrow_count > 0);
        let implementation = include_str!("../../../../stdlib/std@1/float_decimal.hir");
        let mut source =
            implementation[..implementation.find("// f2s_intrinsics.h:67").unwrap()].to_owned();
        source.push_str(include_str!(
            "../fixtures/shortest-direct-candidate-28/retained-shortest64.hir"
        ));
        source.push_str("\npub fn main(rt: Io) -> Result[(), OutputError] {\nlet Ok(sink) = io.stdout(rt) else { failure.check(false, TextInvariant()); return Ok(()) }\n");
        writeln!(source, "let rows: Array[(u64, u64, i32, String)] = [").unwrap();
        let mut output = String::new();
        for row in selected {
            let bits = row["bits"].as_u64().unwrap();
            let id = row["id"].as_str().unwrap();
            eprintln!("SHORTEST28 selected {id} {bits:016x}");
            writeln!(
                source,
                "({bits}u64, {}u64, {}, {:?}),",
                row["digits"].as_u64().unwrap_or(0),
                row["exponent"].as_i64().unwrap_or(0),
                row["text"].as_str().unwrap()
            )
            .unwrap();
            writeln!(output, "{}", row["text"].as_str().unwrap()).unwrap();
        }
        source.push_str(r#"]
for (bits, expected_digits, expected_exponent, expected) in rows {
 let fraction=bits & 4503599627370495u64
 let raw=(bits >> 52) & 2047u64
 if raw != 2047 && (fraction != 0 || raw != 0) {
  let direct=shortest64(fraction, raw)
  let retained=retained_shortest64(fraction, raw)
  failure.check(direct == (expected_digits, expected_exponent), TextInvariant())
  failure.check(retained == direct, TextInvariant())
 }
 let value=core@1/f64.from_bits(bits)
 let text=std@1/f64.to_string(value)
 failure.check(text == expected, TextInvariant())
 let json=std@1/derive.json_float(take text)
 let Ok(json_text)=std@1/json.stringify(json) else { failure.check(false, TextInvariant()); return Ok(()) }
 let expected_json=if raw==2047 { f"\"{expected}\"" } else { expected }
 failure.check(json_text == expected_json, TextInvariant())
 out.print(sink, f"{expected}\n")?
}
"#);
        if narrow_count > 0 {
            source.push_str("let narrow: Array[(u32, String)] = [\n");
            for row in include_str!("../fixtures/float-render-13/f32.txt")
                .lines()
                .filter(|r| !r.starts_with('#'))
                .skip(narrow_start)
                .take(narrow_count)
            {
                let (hex, text) = row.split_once(' ').unwrap();
                let bits = u32::from_str_radix(hex, 16).unwrap();
                writeln!(source, "({bits}u32, {text:?}),").unwrap();
                writeln!(output, "{text}").unwrap();
            }
            source.push_str("]\nfor (bits, expected) in narrow {\nfailure.check(std@1/f32.to_string(core@1/f32.from_bits(bits)) == expected, TextInvariant())\nout.print(sink, f\"{expected}\\n\")?\n}\n");
        }
        source.push_str("Ok(())\n}\n");
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
        let mut loaded =
            load_program_source(&path, Some(&source)).unwrap_or_else(|e| panic!("{e:?}"));
        loaded.check_for_execution().unwrap_or_else(|e| {
            if let ToolError::Query {
                error: QueryError::Diagnostics(diagnostics),
                sources,
                ..
            } = &e
            {
                for diagnostic in diagnostics {
                    eprintln!("{}", diagnostic.render_human(sources));
                }
            }
            panic!("{e:?}")
        });
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(100_000_000),
                call_depth: 256,
                source: None,
            },
            ..Default::default()
        }));
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let expected = native::Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: output.into_bytes(),
            failures: vec![],
            roots: vec![],
        };
        {
            eprintln!(
                "SHORTEST28 started interpreter {start} selected_f64={count} selected_f32={narrow_count}"
            );
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
            assert!(
                matches!(
                    run.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{:?}",
                run.outcome
            );
            assert_eq!(
                native::Observation::of_reference(run.observation.as_ref().unwrap()),
                expected
            );
            eprintln!(
                "SHORTEST28 pass interpreter {start} completed_f64={count} completed_f32={narrow_count}"
            );
        }
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let configs = [
            native::Engine::Heap { campaign: false },
            native::Engine::Native {
                opt: native::OptLevel::Speed,
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
            native::Engine::Cranelift {
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| native::Configuration {
            options: options.clone(),
            engine,
        })
        .collect::<Vec<_>>();
        let scratch = tempfile::Builder::new()
            .prefix("shortest-direct-28-")
            .tempdir()
            .unwrap();
        let mut tools =
            native::Toolchain::for_matrix(&scratch.path().join("runtime"), &configs).unwrap();
        tools.launch_timeout = std::time::Duration::from_secs(60);
        for config in configs {
            let name = config.name();
            eprintln!(
                "SHORTEST28 started {name} {start} selected_f64={count} selected_f32={narrow_count}"
            );
            let outcomes = native::differential(
                &validated,
                &loaded.entry,
                &expected,
                &[config],
                &tools,
                scratch.path(),
            );
            assert_eq!(outcomes.len(), 1);
            for (_, outcome) in outcomes {
                assert!(
                    matches!(outcome, native::Outcome::Agree),
                    "{name}: {outcome:?}"
                );
            }
            eprintln!(
                "SHORTEST28 pass {name} {start} completed_f64={count} completed_f32={narrow_count}"
            );
            {
                for row in selected {
                    eprintln!(
                        "SHORTEST28 pass-vector {name} {}",
                        row["id"].as_str().unwrap()
                    );
                }
                for row in include_str!("../fixtures/float-render-13/f32.txt")
                    .lines()
                    .filter(|r| !r.starts_with('#'))
                    .skip(narrow_start)
                    .take(narrow_count)
                {
                    eprintln!(
                        "SHORTEST28 pass-vector {name} f32-{}",
                        row.split_once(' ').unwrap().0
                    );
                }
            }
        }
    });
}

#[test]
fn upstream_binary64_and_boundary_neighbors() {
    run_partition(0, 36, 0, 0);
}
