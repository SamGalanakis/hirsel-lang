//! Chrono-free independent integer/calendar vectors for ordinary std source.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_lir::ir::UnitOrigin;
use std::{collections::BTreeSet, path::PathBuf};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/SEM/SEM-1-pathtime-time-run.hir")
}

fn execute(source: &str) {
    let mut loaded = load_program_source(&fixture(), Some(source)).unwrap();
    loaded.check_for_execution().unwrap();
    loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
        limits: hirsel_interp::Limits {
            steps: Some(500_000_000),
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

fn duration_text(nanos: i128) -> String {
    let magnitude = nanos.unsigned_abs();
    let fraction = magnitude % 1_000_000_000;
    format!(
        "{}{}{}s",
        if nanos < 0 { "-" } else { "" },
        magnitude / 1_000_000_000,
        if fraction == 0 {
            String::new()
        } else {
            format!(".{fraction:09}").trim_end_matches('0').into()
        }
    )
}

#[test]
fn duration_text_matches_independent_unsigned_integer_vectors() {
    with_compiler_stack(|| {
        let mut source = String::from("pub fn main(t: Test) -> Result[(), ()] {\n");
        let mut values = vec![i128::MIN, i128::MIN + 1, i128::MAX - 1, i128::MAX];
        values.extend((-10..=10).flat_map(|v| [v, v * 1_000_000_000, v * 1_000_000_000 + 1]));
        for nanos in values {
            let text = duration_text(nanos);
            source += &format!(
                "core@1/test.check(t, std@1/duration.to_string(std@1/time.Duration(nanoseconds: {nanos}i128)) == \"{text}\")\n"
            );
            source += &format!(
                "match std@1/duration.parse(\"{text}\") {{ Ok(value) => core@1/test.check(t, value.nanoseconds == {nanos}i128), Error(_) => core@1/test.check(t, false) }}\n"
            );
        }
        source += "Ok(())\n}\n";
        execute(&source);
    });
}

// Count years and months directly, independently of the source's cycle/year/month decomposition.
fn leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}
fn unix_days(year: i32, month: usize, day: i32) -> i128 {
    let years = |end: i32| {
        (0..end)
            .map(|y| if leap(y) { 366i128 } else { 365 })
            .sum::<i128>()
    };
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    years(year) - years(1970)
        + months[..month - 1]
            .iter()
            .map(|&d| i128::from(d))
            .sum::<i128>()
        + i128::from(day - 1)
}

#[test]
fn rfc3339_matches_independent_gregorian_and_offset_vectors() {
    with_compiler_stack(|| {
        let mut source = String::from("pub fn main(t: Test) -> Result[(), ()] {\n");
        for year in [0, 1, 4, 100, 400, 1600, 1900, 1969, 1970, 2000, 2024, 9999] {
            for (month, day) in [
                (1, 1),
                (2, if leap(year) { 29 } else { 28 }),
                (3, 1),
                (12, 31),
            ] {
                let nanos = unix_days(year, month, day) * 86_400_000_000_000 + 12_345_678_901;
                let text = format!("{year:04}-{month:02}-{day:02}T00:00:12.345678901Z");
                source += &format!(
                    "match std@1/time.parse(\"{text}\") {{ Ok(value) => core@1/test.check(t, value.unix_nanoseconds == {nanos}i128), Error(_) => core@1/test.check(t, false) }}\n"
                );
                source += &format!(
                    "match std@1/time.to_string(std@1/time.Timestamp(unix_nanoseconds: {nanos}i128)) {{ Ok(text) => core@1/test.check(t, text == \"{text}\"), Error(_) => core@1/test.check(t, false) }}\n"
                );
            }
        }
        for (text, nanos) in [
            ("1970-01-01T23:59:00+23:59", 0i128),
            ("1970-01-01T00:00:00-23:59", 86_340_000_000_000),
            ("1969-12-31T23:59:59.999999999Z", -1),
            ("0000-01-01T00:00:00Z", -62_167_219_200_000_000_000),
            (
                "9999-12-31T23:59:59.999999999Z",
                253_402_300_799_999_999_999,
            ),
        ] {
            source += &format!(
                "match std@1/time.parse(\"{text}\") {{ Ok(value) => core@1/test.check(t, value.unix_nanoseconds == {nanos}i128), Error(_) => core@1/test.check(t, false) }}\n"
            );
        }
        // Even an actual UTC leap-second date rejects 60 under Hirsel's contract.
        source += "match std@1/time.parse(\"2016-12-31T23:59:60Z\") { Error(Invalid()) => (), _ => core@1/test.check(t, false) }\n";
        let previous_second = unix_days(2016, 12, 31) * 86_400_000_000_000 + 86_399_000_000_000;
        source += &format!(
            "match std@1/time.parse(\"2016-12-31T23:59:59Z\") {{ Ok(value) => core@1/test.check(t, value.unix_nanoseconds == {previous_second}i128), Error(_) => core@1/test.check(t, false) }}\n"
        );
        for nanos in [i128::MIN, i128::MAX] {
            source += &format!(
                "match std@1/time.to_string(std@1/time.Timestamp(unix_nanoseconds: {nanos}i128)) {{ Error(OutOfRange()) => (), Ok(_) => core@1/test.check(t, false) }}\n"
            );
        }
        source += "Ok(())\n}\n";
        execute(&source);
    });
}

#[test]
#[ignore = "full gate"]
fn full_gate_path_and_time_public_functions_compile_as_source_units() {
    with_compiler_stack(|| {
        let mut found = BTreeSet::new();
        for name in [
            "SEM-3-pathtime-path-run",
            "SEM-1-pathtime-duration-run",
            "SEM-1-pathtime-time-run",
        ] {
            let path = fixture().parent().unwrap().join(format!("{name}.hir"));
            let mut loaded = load_program(&path).unwrap();
            loaded.check_for_execution().unwrap();
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input).unwrap();
            let ready = native::build(
                &validated,
                &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            )
            .unwrap();
            for unit in &ready.units {
                let header = &unit.unit().header;
                if header.definition.0.package.name_is("std") {
                    assert_eq!(header.origin, UnitOrigin::Source);
                    found.insert((
                        header.definition.0.modules.join("/"),
                        header.definition.0.name.clone(),
                    ));
                }
            }
        }
        for (module, names) in [
            (
                "path",
                vec![
                    "normalize",
                    "join",
                    "parent",
                    "filename",
                    "extension",
                    "components",
                ],
            ),
            ("duration", vec!["from_parts", "parse", "to_string"]),
            ("time", vec!["parse", "to_string"]),
        ] {
            for name in names {
                assert!(
                    found.contains(&(module.into(), name.into())),
                    "missing {module}.{name}"
                );
                assert!(
                    registry::BuiltinRegistry::new()
                        .get(&hirsel_syntax::DefPath::core(&format!("{module}.{name}")))
                        .is_none()
                );
            }
        }
    });
}

#[test]
#[ignore = "full gate"]
fn full_gate_every_four_digit_year_matches_a_chrono_free_calendar_checksum() {
    with_compiler_stack(|| {
        let checksum: i128 = (0..=9999)
            .map(|year| unix_days(year, 1, 1) * 86_400_000_000_000)
            .sum();
        let source = format!(
            r#"pub fn main(t: Test) -> Result[(), ()] {{
  let mut year: i128 = 0
  let mut checksum: i128 = 0
  let mut expected: i128 = -62167219200000000000
  while year <= 9999 {{
    let prefix: String = if year < 10 {{ "000" }} else if year < 100 {{ "00" }} else if year < 1000 {{ "0" }} else {{ "" }}
    let text = std@1/string.join([prefix, std@1/i128.to_string(year), "-01-01T00:00:00Z"], "")
    match std@1/time.parse(text) {{
      Ok(value) => {{
        test.check(t, value.unix_nanoseconds == expected)
        checksum += value.unix_nanoseconds
        match std@1/time.to_string(value) {{ Ok(formatted) => test.check(t, formatted == text), Error(_) => test.check(t, false) }}
      }},
      Error(_) => test.check(t, false),
    }}
    let days: i128 = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {{ 366 }} else {{ 365 }}
    expected += days * 86400000000000
    year += 1
  }}
  test.check(t, checksum == {checksum}i128)
  Ok(())
}}"#
        );
        execute(&source);
    });
}
