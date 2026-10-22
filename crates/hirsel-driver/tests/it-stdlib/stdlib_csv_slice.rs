//! The borrowed CSV cursor's differential and allocation contracts.
use hirsel_driver::native::{
    self, OptLevel, Options, Partition, RuntimeBuild, TargetSpec, Toolchain,
};
use hirsel_driver::*;
use hirsel_engine::with_compiler_stack;
use std::{path::PathBuf, process::Command};

fn measured(source: &str, label: &str) -> String {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/SEM/SEM-3-csv-slice-run.hir");
    let mut loaded = load_program_source(&fixture, Some(source)).unwrap();
    loaded.check_for_execution().unwrap();
    let input = loaded.engine.export_native_input().unwrap();
    let validated = loaded.engine.import_native_input(input).unwrap();
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let ready = native::build(&validated, &Options::new(target.clone())).unwrap();
    let dir = std::env::temp_dir().join(format!("hirsel-csv-{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tools = Toolchain::in_tree_with_runtime(
        &dir.join("runtime"),
        &[&target],
        RuntimeBuild {
            measurement: true,
            ..Default::default()
        },
    )
    .unwrap();
    let exe = native::link(
        &ready,
        &loaded.entry,
        OptLevel::Speed,
        Partition::Whole,
        &tools,
        &dir,
    )
    .unwrap();
    let totals = dir.join("totals");
    let result = Command::new(exe)
        .env_clear()
        .env("HIRSEL_RT_MEASUREMENT", &totals)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let report = std::fs::read_to_string(totals).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    report
}

fn allocations(report: &str) -> u64 {
    report
        .lines()
        .filter_map(|line| line.strip_prefix("class "))
        .map(|row| {
            row.split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<u64>()
                .unwrap()
        })
        .sum()
}

#[test]
fn unchanged_csv_fields_and_later_records_do_not_allocate() {
    with_compiler_stack(|| {
        let source = |row: &str, columns: u64| {
            let row = row.repeat(256);
            format!(
                r#"pub fn main() -> Result[(), ()] {{
  let mut cursor = match std@1/csv.slice_reader("{row}", std@1/csv.default_config()) {{
    Ok(cursor) => take cursor,
    Error(_) => return Error(()),
  }}
  for iteration in 0u64..257u64 {{
    failure.check(std@1/csv.next_record(mut cursor) == Ok(iteration < 256), TextInvariant())
    if iteration < 256 {{
      failure.check(std@1/csv.field_count(cursor) == {columns}, TextInvariant())
      for i in 0u64..{columns}u64 {{
        failure.check(std@1/csv.get_field(cursor, i) == Some("123"), TextInvariant())
      }}
    }}
  }}
  Ok(())
}}
"#
            )
        };
        let narrow = measured(&source("123\\n", 1), "narrow");
        let wide = measured(&source("123,123,123,123,123,123\\n", 6), "wide");
        eprintln!("narrow:\n{narrow}wide:\n{wide}");
        // Only the span array's geometric growth may allocate as width grows.
        assert!(
            allocations(&wide) <= allocations(&narrow) + 4,
            "{narrow}\n{wide}"
        );
        for report in [&narrow, &wide] {
            assert!(
                report.lines().any(|line| line == "deep_copies 0"),
                "{report}"
            );
            assert!(
                report.lines().any(|line| line == "live_bytes 0"),
                "{report}"
            );
        }
    });
}

#[test]
fn short_csv_inputs_match_the_strict_parser_at_every_record_limit() {
    with_compiler_stack(|| {
        let fixture = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/SEM/SEM-3-csv-slice-run.hir"),
        )
        .unwrap();
        let helpers = fixture.split("pub fn main()").next().unwrap();
        let source = format!(
            r#"{helpers}
pub fn main() -> Result[(), ()] {{
  let alphabet = b"a,\"\r\n"
  let mut config = std@1/csv.default_config()
  for code in 0u64..19531u64 {{
    let mut digits = code
    let mut out: List[u8] = []
    while digits > 0 {{
      list.push(mut out, alphabet[(digits - 1) % 5])
      digits = (digits - 1) / 5
    }}
    let source = text.finish_valid(take out)
    for limit in 0u64..8u64 {{
      config.max_record_bytes = limit
      failure.check(parse(source, config) == std@1/csv.parse(copy source, config), TextInvariant())
    }}
  }}
  Ok(())
}}
"#
        );
        measured(&source, "differential");
    });
}
