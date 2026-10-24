//! Composed SEM-1 conversions across branch joins, generic transport and returned storage.
//! Independent decimal expectations: Python exact integers, seed
//! 20261026:integer-conversion-histories-26. Boundary/decimal vector shapes adapted
//! from Rust core/src/convert/num.rs:330-418 and coretests/tests/num/cast.rs:1-92,
//! f6c1107f8be13e48465e12c0fd26e6a39fe18e4f (MIT OR Apache-2.0).
//! Hirsel's named conversion contracts take precedence over Rust casts.
//! Retains rc_legalize and integer_boundary_21 inventories; these 120 histories
//! add composition/transport and four-engine expectations, not a new Cartesian matrix.
use crate::rc_support::{
    self as support,
    history::{Category, Runner},
};
use sha2::{Digest, Sha256};
use std::fmt::Write;

// (source, left, right, chosen arm, operations, expected decimal/Option, failure step).
type Case = (
    &'static str,
    &'static str,
    &'static str,
    bool,
    &'static [(&'static str, &'static str)],
    &'static str,
    usize,
);
#[rustfmt::skip]
const CASES: &[Case] = &[
    ("u8", "0", "126", false, &[("narrow","i8"),("widen","i16")], "126i16", 0),
    ("u8", "127", "0", true, &[("narrow","i8"),("widen","i16")], "127i16", 0),
    ("u8", "255", "0", false, &[("narrow","i8"),("widen","i16")], "0i16", 0),
    ("u8", "255", "0", true, &[("truncate","i8"),("try_narrow","u8")], "None", 0),
    ("u8", "0", "0", false, &[("truncate","i8"),("try_narrow","u8")], "Some(0u8)", 0),
    ("u8", "1", "255", true, &[("truncate","i8"),("try_narrow","u8")], "Some(1u8)", 0),
    ("u8", "85", "0", true, &[("truncate","i8"),("widen","i16"),("try_narrow","u8")], "Some(85u8)", 0),
    ("u8", "0", "0", true, &[("truncate","i8"),("truncate","u8"),("try_narrow","i8")], "Some(0i8)", 0),
    ("u8", "255", "0", true, &[("truncate","i8"),("truncate","u8"),("try_narrow","i8")], "None", 0),
    ("u8", "85", "0", true, &[("truncate","i8"),("truncate","u8"),("try_narrow","i8")], "Some(85i8)", 0),
    ("u8", "255", "0", true, &[("truncate","u8"),("narrow","i8")], "SEM-1", 2),
    ("u8", "255", "127", false, &[("truncate","u8"),("narrow","i8")], "127i8", 0),
    ("u16", "0", "254", false, &[("narrow","u8"),("widen","u16")], "254u16", 0),
    ("u16", "255", "0", true, &[("narrow","u8"),("widen","u16")], "255u16", 0),
    ("u16", "0", "0", false, &[("narrow","u8"),("widen","u16")], "0u16", 0),
    ("u16", "0", "65535", false, &[("truncate","u8"),("try_narrow","i8")], "None", 0),
    ("u16", "0", "65535", true, &[("truncate","u8"),("try_narrow","i8")], "Some(0i8)", 0),
    ("u16", "65535", "1", false, &[("truncate","u8"),("try_narrow","i8")], "Some(1i8)", 0),
    ("u16", "0", "21845", false, &[("truncate","u8"),("widen","u16"),("try_narrow","i8")], "Some(85i8)", 0),
    ("u16", "0", "0", false, &[("truncate","i16"),("truncate","u16"),("try_narrow","i16")], "Some(0i16)", 0),
    ("u16", "65535", "65535", true, &[("truncate","i16"),("truncate","u16"),("try_narrow","i16")], "None", 0),
    ("u16", "0", "21845", false, &[("truncate","i16"),("truncate","u16"),("try_narrow","i16")], "Some(21845i16)", 0),
    ("u16", "0", "65535", false, &[("truncate","u16"),("narrow","i16")], "SEM-1", 2),
    ("u16", "32767", "0", true, &[("truncate","u16"),("narrow","i16")], "32767i16", 0),
    ("u32", "65534", "0", true, &[("narrow","u16"),("widen","u32")], "65534u32", 0),
    ("u32", "0", "65535", false, &[("narrow","u16"),("widen","u32")], "65535u32", 0),
    ("u32", "0", "0", false, &[("narrow","u16"),("widen","u32")], "0u32", 0),
    ("u32", "4294967295", "0", true, &[("truncate","u16"),("try_narrow","i16")], "None", 0),
    ("u32", "4294967295", "0", false, &[("truncate","u16"),("try_narrow","i16")], "Some(0i16)", 0),
    ("u32", "4294967295", "1", false, &[("truncate","u16"),("try_narrow","i16")], "Some(1i16)", 0),
    ("u32", "0", "1431655765", false, &[("truncate","u16"),("widen","u32"),("try_narrow","i16")], "Some(21845i16)", 0),
    ("u32", "0", "0", false, &[("truncate","i32"),("truncate","u32"),("try_narrow","i32")], "Some(0i32)", 0),
    ("u32", "4294967295", "4294967295", true, &[("truncate","i32"),("truncate","u32"),("try_narrow","i32")], "None", 0),
    ("u32", "4294967295", "1431655765", false, &[("truncate","i32"),("truncate","u32"),("try_narrow","i32")], "Some(1431655765i32)", 0),
    ("u32", "4294967295", "0", true, &[("truncate","u32"),("narrow","i32")], "SEM-1", 2),
    ("u32", "0", "2147483647", false, &[("truncate","u32"),("narrow","i32")], "2147483647i32", 0),
    ("u64", "4294967294", "0", true, &[("narrow","u32"),("widen","u64")], "4294967294u64", 0),
    ("u64", "4294967295", "0", true, &[("narrow","u32"),("widen","u64")], "4294967295u64", 0),
    ("u64", "0", "0", false, &[("narrow","u32"),("widen","u64")], "0u64", 0),
    ("u64", "18446744073709551615", "0", true, &[("truncate","u32"),("try_narrow","i32")], "None", 0),
    ("u64", "0", "0", true, &[("truncate","u32"),("try_narrow","i32")], "Some(0i32)", 0),
    ("u64", "1", "0", true, &[("truncate","u32"),("try_narrow","i32")], "Some(1i32)", 0),
    ("u64", "6148914691236517205", "0", true, &[("truncate","u32"),("widen","u64"),("try_narrow","i32")], "Some(1431655765i32)", 0),
    ("u64", "0", "0", true, &[("truncate","i64"),("truncate","u64"),("try_narrow","i64")], "Some(0i64)", 0),
    ("u64", "18446744073709551615", "18446744073709551615", true, &[("truncate","i64"),("truncate","u64"),("try_narrow","i64")], "None", 0),
    ("u64", "6148914691236517205", "0", true, &[("truncate","i64"),("truncate","u64"),("try_narrow","i64")], "Some(6148914691236517205i64)", 0),
    ("u64", "18446744073709551615", "18446744073709551615", true, &[("truncate","u64"),("narrow","i64")], "SEM-1", 2),
    ("u64", "9223372036854775807", "0", true, &[("truncate","u64"),("narrow","i64")], "9223372036854775807i64", 0),
    ("u128", "18446744073709551614", "0", true, &[("narrow","u64"),("widen","u128")], "18446744073709551614u128", 0),
    ("u128", "18446744073709551615", "0", true, &[("narrow","u64"),("widen","u128")], "18446744073709551615u128", 0),
    ("u128", "0", "0", false, &[("narrow","u64"),("widen","u128")], "0u128", 0),
    ("u128", "0", "340282366920938463463374607431768211455", false, &[("truncate","u64"),("try_narrow","i64")], "None", 0),
    ("u128", "340282366920938463463374607431768211455", "0", false, &[("truncate","u64"),("try_narrow","i64")], "Some(0i64)", 0),
    ("u128", "340282366920938463463374607431768211455", "1", false, &[("truncate","u64"),("try_narrow","i64")], "Some(1i64)", 0),
    ("u128", "113427455640312821154458202477256070485", "0", true, &[("truncate","u64"),("widen","u128"),("try_narrow","i64")], "Some(6148914691236517205i64)", 0),
    ("u128", "0", "0", false, &[("truncate","i128"),("truncate","u128"),("try_narrow","i128")], "Some(0i128)", 0),
    ("u128", "340282366920938463463374607431768211455", "0", true, &[("truncate","i128"),("truncate","u128"),("try_narrow","i128")], "None", 0),
    ("u128", "113427455640312821154458202477256070485", "0", true, &[("truncate","i128"),("truncate","u128"),("try_narrow","i128")], "Some(113427455640312821154458202477256070485i128)", 0),
    ("u128", "0", "340282366920938463463374607431768211455", false, &[("truncate","u128"),("narrow","i128")], "SEM-1", 2),
    ("u128", "0", "170141183460469231731687303715884105727", false, &[("truncate","u128"),("narrow","i128")], "170141183460469231731687303715884105727i128", 0),
    ("i8", "126", "-128", true, &[("narrow","u8"),("widen","u16")], "126u16", 0),
    ("i8", "-128", "127", false, &[("narrow","u8"),("widen","u16")], "127u16", 0),
    ("i8", "127", "0", false, &[("narrow","u8"),("widen","u16")], "0u16", 0),
    ("i8", "127", "0", true, &[("truncate","u8"),("try_narrow","i8")], "Some(127i8)", 0),
    ("i8", "0", "-128", false, &[("truncate","u8"),("try_narrow","i8")], "None", 0),
    ("i8", "1", "0", true, &[("truncate","u8"),("try_narrow","i8")], "Some(1i8)", 0),
    ("i8", "85", "-128", true, &[("truncate","u8"),("widen","u16"),("try_narrow","i8")], "Some(85i8)", 0),
    ("i8", "0", "-128", true, &[("truncate","u8"),("narrow","i8")], "0i8", 0),
    ("i8", "-1", "-128", true, &[("truncate","u8"),("narrow","i8")], "SEM-1", 2),
    ("i8", "-128", "-128", true, &[("truncate","u8"),("narrow","i8")], "SEM-1", 2),
    ("i8", "0", "-1", false, &[("truncate","i8"),("narrow","u8")], "SEM-1", 2),
    ("i8", "-128", "0", false, &[("truncate","i8"),("narrow","u8")], "0u8", 0),
    ("i16", "-32768", "126", false, &[("narrow","i8"),("widen","i16")], "126i16", 0),
    ("i16", "127", "32767", true, &[("narrow","i8"),("widen","i16")], "127i16", 0),
    ("i16", "-32768", "-128", false, &[("narrow","i8"),("widen","i16")], "-128i16", 0),
    ("i16", "32767", "32767", true, &[("truncate","i8"),("try_narrow","u8")], "None", 0),
    ("i16", "0", "-32768", false, &[("truncate","i8"),("try_narrow","u8")], "Some(0u8)", 0),
    ("i16", "1", "-32768", true, &[("truncate","i8"),("try_narrow","u8")], "Some(1u8)", 0),
    ("i16", "21845", "-32768", true, &[("truncate","i8"),("widen","i16"),("try_narrow","u8")], "Some(85u8)", 0),
    ("i16", "0", "0", false, &[("truncate","u16"),("narrow","i16")], "0i16", 0),
    ("i16", "-32768", "-1", false, &[("truncate","u16"),("narrow","i16")], "SEM-1", 2),
    ("i16", "32767", "-32768", false, &[("truncate","u16"),("narrow","i16")], "SEM-1", 2),
    ("i16", "0", "-1", false, &[("truncate","i16"),("narrow","u16")], "SEM-1", 2),
    ("i16", "0", "-32768", true, &[("truncate","i16"),("narrow","u16")], "0u16", 0),
    ("i32", "32766", "0", true, &[("narrow","i16"),("widen","i32")], "32766i32", 0),
    ("i32", "0", "32767", false, &[("narrow","i16"),("widen","i32")], "32767i32", 0),
    ("i32", "0", "-32768", false, &[("narrow","i16"),("widen","i32")], "-32768i32", 0),
    ("i32", "2147483647", "2147483647", true, &[("truncate","i16"),("try_narrow","u16")], "None", 0),
    ("i32", "-2147483648", "-2147483648", true, &[("truncate","i16"),("try_narrow","u16")], "Some(0u16)", 0),
    ("i32", "1", "-2147483648", true, &[("truncate","i16"),("try_narrow","u16")], "Some(1u16)", 0),
    ("i32", "1431655765", "-2147483648", true, &[("truncate","i16"),("widen","i32"),("try_narrow","u16")], "Some(21845u16)", 0),
    ("i32", "0", "0", true, &[("truncate","u32"),("narrow","i32")], "0i32", 0),
    ("i32", "-1", "2147483647", true, &[("truncate","u32"),("narrow","i32")], "SEM-1", 2),
    ("i32", "-2147483648", "-2147483648", false, &[("truncate","u32"),("narrow","i32")], "SEM-1", 2),
    ("i32", "-1", "2147483647", true, &[("truncate","i32"),("narrow","u32")], "SEM-1", 2),
    ("i32", "2147483647", "0", false, &[("truncate","i32"),("narrow","u32")], "0u32", 0),
    ("i64", "2147483646", "0", true, &[("narrow","i32"),("widen","i64")], "2147483646i64", 0),
    ("i64", "2147483647", "-9223372036854775808", true, &[("narrow","i32"),("widen","i64")], "2147483647i64", 0),
    ("i64", "0", "-2147483648", false, &[("narrow","i32"),("widen","i64")], "-2147483648i64", 0),
    ("i64", "9223372036854775807", "0", true, &[("truncate","i32"),("try_narrow","u32")], "None", 0),
    ("i64", "-9223372036854775808", "-9223372036854775808", false, &[("truncate","i32"),("try_narrow","u32")], "Some(0u32)", 0),
    ("i64", "1", "0", true, &[("truncate","i32"),("try_narrow","u32")], "Some(1u32)", 0),
    ("i64", "6148914691236517205", "0", true, &[("truncate","i32"),("widen","i64"),("try_narrow","u32")], "Some(1431655765u32)", 0),
    ("i64", "9223372036854775807", "0", false, &[("truncate","u64"),("narrow","i64")], "0i64", 0),
    ("i64", "-9223372036854775808", "-1", false, &[("truncate","u64"),("narrow","i64")], "SEM-1", 2),
    ("i64", "9223372036854775807", "-9223372036854775808", false, &[("truncate","u64"),("narrow","i64")], "SEM-1", 2),
    ("i64", "-1", "-9223372036854775808", true, &[("truncate","i64"),("narrow","u64")], "SEM-1", 2),
    ("i64", "0", "0", false, &[("truncate","i64"),("narrow","u64")], "0u64", 0),
    ("i128", "170141183460469231731687303715884105727", "9223372036854775806", false, &[("narrow","i64"),("widen","i128")], "9223372036854775806i128", 0),
    ("i128", "9223372036854775807", "170141183460469231731687303715884105727", true, &[("narrow","i64"),("widen","i128")], "9223372036854775807i128", 0),
    ("i128", "-9223372036854775808", "0", true, &[("narrow","i64"),("widen","i128")], "-9223372036854775808i128", 0),
    ("i128", "0", "170141183460469231731687303715884105727", false, &[("truncate","i64"),("try_narrow","u64")], "None", 0),
    ("i128", "170141183460469231731687303715884105727", "-170141183460469231731687303715884105728", false, &[("truncate","i64"),("try_narrow","u64")], "Some(0u64)", 0),
    ("i128", "1", "0", true, &[("truncate","i64"),("try_narrow","u64")], "Some(1u64)", 0),
    ("i128", "-170141183460469231731687303715884105728", "113427455640312821154458202477256070485", false, &[("truncate","i64"),("widen","i128"),("try_narrow","u64")], "Some(6148914691236517205u64)", 0),
    ("i128", "0", "-170141183460469231731687303715884105728", true, &[("truncate","u128"),("narrow","i128")], "0i128", 0),
    ("i128", "-170141183460469231731687303715884105728", "-1", false, &[("truncate","u128"),("narrow","i128")], "SEM-1", 2),
    ("i128", "-170141183460469231731687303715884105728", "0", true, &[("truncate","u128"),("narrow","i128")], "SEM-1", 2),
    ("i128", "-1", "0", true, &[("truncate","i128"),("narrow","u128")], "SEM-1", 2),
    ("i128", "0", "170141183460469231731687303715884105727", true, &[("truncate","i128"),("narrow","u128")], "0u128", 0),
];

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

const PRELUDE: &str = "fn keep[T: plain](value: T) -> T uses {} { value }\n";

fn helper(index: usize) -> (String, Option<(u32, u32)>, String) {
    let (from, _, _, _, ops, _, failure) = CASES[index];
    let final_type = if ops.last().unwrap().0 == "try_narrow" {
        format!("Option[{}]", ops.last().unwrap().1)
    } else {
        ops.last().unwrap().1.to_string()
    };
    let record = index % 2 == 0;
    let returned = if record {
        format!("Returned_{index}")
    } else {
        format!("({final_type}, bool)")
    };
    let mut source = String::new();
    if record {
        writeln!(source, "type Returned_{index} {{ value: {final_type} }}").unwrap();
    }
    writeln!(source, "type Saved_{index} {{ value: {} }}", ops[0].1).unwrap();
    writeln!(
        source,
        "fn h{index}(a: {from}, b: {from}, flag: bool) -> {returned} {{"
    )
    .unwrap();
    source.push_str("  let v0 = if flag { a } else { b }\n");
    let mut site = None;
    for (step, (op, to)) in ops.iter().enumerate() {
        let step = step + 1;
        let t = if *op == "try_narrow" {
            format!("Option[{to}]")
        } else {
            to.to_string()
        };
        let operand = if step == 2 {
            "transported".to_string()
        } else {
            format!("v{}", step - 1)
        };
        let statement = format!("  let v{step}: {t} = int.{op}({operand})");
        if step == failure {
            site = Some((
                source.lines().count() as u32 + 1,
                statement.find("int.").unwrap() as u32 + 1,
            ));
        }
        writeln!(source, "{statement}").unwrap();
        if step == 1 {
            // Return from generic storage before feeding the next conversion.
            if record {
                writeln!(
                    source,
                    "  let saved = keep(Saved_{index}(value: v1))\n  let transported = saved.value"
                )
                .unwrap();
            } else {
                source.push_str(
                    "  let saved = keep((v1, flag))\n  let (transported, chosen) = saved\n",
                );
            }
        }
    }
    let last = ops.len();
    if record {
        writeln!(source, "  Returned_{index}(value: v{last})\n}}").unwrap();
    } else {
        writeln!(source, "  (v{last}, flag)\n}}").unwrap();
    }
    let (from, a, b, flag, _, _, _) = CASES[index];
    let call = format!("h{index}({a}{from}, {b}{from}, {flag})");
    (source, site, call)
}

fn observe(runner: &Runner, ids: &[usize], source: &str, site: Option<(usize, u32, u32)>) {
    for i in ids {
        eprintln!("CONVERSION_STARTED {}-{:02}", CASES[*i].0, i % 12);
    }
    let name = format!("history-{}", ids[0]);
    let report = runner.run(&name, source);
    let mut accepted = report.validated && report.cells.len() == 4;
    for cell in &report.cells {
        // Each engine is judged against the mathematical expectation itself.
        // Agreement with the interpreter alone cannot make a cell pass.
        let valid = cell.category == Category::Observation
            && cell.observation.as_ref().is_some_and(|o| {
                o.output.is_empty()
                    && !o.blocked
                    && o.budget.is_none()
                    && o.signaled.is_none()
                    && if let Some((index, line, column)) = site {
                        o.exit_code.is_none() && o.failures.len() == 1 && {
                            let f = &o.failures[0];
                            f.rule == "SEM-1"
                                && f.definition == support::path(&format!("h{index}"))
                                && f.file == "main.hir"
                                && f.line == line
                                && f.column == column
                                && f.operation.is_none()
                        }
                    } else {
                        o.exit_code == Some(0) && o.failures.is_empty()
                    }
            });
        accepted &= valid;
        for i in ids {
            let case = CASES[*i];
            eprintln!(
                "CONVERSION_RECEIPT {}",
                serde_json::json!({
                    "id":format!("{}-{:02}",case.0,i%12), "engine":cell.engine,
                    "source_hash":hash(source.as_bytes()), "expected":case.5,
                    "expected_hash":hash(case.5.as_bytes()), "site":site,
                    "category":format!("{:?}",cell.category), "pass":valid,
                    "observation":cell.observation.as_ref().map(|o|o.render()), "detail":cell.detail
                })
            );
        }
    }
    eprintln!(
        "CONVERSION_SOURCE {}",
        serde_json::json!({"ids":ids,"source":source,"site":site})
    );
    assert!(accepted, "{name}: {source}\n{report:#?}");
}

fn partition(kind: usize, chunk: usize) {
    assert_eq!(CASES.len(), 120);
    let start = kind * 12 + chunk * 4;
    let ids: Vec<_> = (start..start + 4).collect();
    eprintln!("CONVERSION_SELECTED {ids:?}");
    let directory = tempfile::tempdir().unwrap();
    let runner = Runner::new(directory.path()).unwrap();
    let mut source = PRELUDE.to_string();
    let mut checks = String::new();
    let mut successes = Vec::new();
    for index in ids {
        let (helper, site, call) = helper(index);
        if let Some((line, column)) = site {
            let prefix_lines = PRELUDE.lines().count() as u32;
            let text = format!(
                "{PRELUDE}{helper}pub fn main() -> Result[(), u8] {{\n  let value = {call}\n  Ok(())\n}}\n"
            );
            observe(
                &runner,
                &[index],
                &text,
                Some((index, line + prefix_lines, column)),
            );
        } else {
            source.push_str(&helper);
            if index % 2 == 0 {
                writeln!(checks, "  let result_{index} = {call}\n  if result_{index}.value != {} {{ return Error(1u8) }}", CASES[index].5).unwrap();
            } else {
                writeln!(checks, "  let (result_{index}, chosen_{index}) = {call}\n  if result_{index} != {} {{ return Error(1u8) }}", CASES[index].5).unwrap();
            }
            successes.push(index);
        }
    }
    if !successes.is_empty() {
        write!(
            source,
            "pub fn main() -> Result[(), u8] {{\n{checks}  Ok(())\n}}\n"
        )
        .unwrap();
        observe(&runner, &successes, &source, None);
    }
}

fn illegal(kind: usize) {
    let from = CASES[kind * 12].0;
    let width: u32 = from[1..].parse().unwrap();
    let flip = if from.starts_with('i') { 'u' } else { 'i' };
    for oversized in [false, true] {
        // No integer wider than 128 exists: those two extra oversized controls
        // use 8->128 (distinct from the adjacent 8->16 controls).
        let (from, to, legal, illegal) = if oversized {
            let source_width = if width == 128 { 8 } else { width };
            let target_width = if width == 128 { 128 } else { width * 2 };
            (
                format!("{}{source_width}", &from[..1]),
                format!("{}{target_width}", &from[..1]),
                "widen",
                "truncate",
            )
        } else {
            (
                from.to_string(),
                format!("{flip}{width}"),
                "truncate",
                "widen",
            )
        };
        let id = format!("static-{kind}-{illegal}");
        let text = format!(
            "fn convert(x: {from}) -> {to} {{ int.{illegal}(x) }}\npub fn main() -> Result[(), u8] {{ let value = convert(0{from}); Ok(()) }}\n"
        );
        let checked = support::try_compile(&text).expect_err("illegal conversion accepted");
        assert!(
            checked.contains("diagnostic:") && checked.contains("SEM-1"),
            "{id}: {checked}"
        );
        let positive = text.replace(&format!("int.{illegal}"), &format!("int.{legal}"));
        let (program, lowered) = support::lower_valid(&positive);
        let mut ready = Vec::new();
        for (i, unit) in lowered.units.iter().enumerate() {
            let id = hirsel_rc::instance::InstanceId(i as u32);
            let options = hirsel_rc::placement::Options::for_instance(true, &lowered.instances, id);
            let placed =
                hirsel_rc::placement::place(unit.clone(), &lowered.table, &options).unwrap();
            let proofs = hirsel_rc::insert::Proofs::for_instance(&lowered.instances, id);
            let rc = hirsel_rc::insert::insert(placed, &lowered.table, &proofs).unwrap();
            ready.push(hirsel_rc::elide::elide(rc, &lowered.table).unwrap());
        }
        let finalizers = hirsel_rc::cleanup::Finalizers::new(&lowered.instances, &ready);
        let manifest = support::manifest(&program);
        let target = support::target();
        let operations = lowered.operation_registry(&program);
        let inputs = hirsel_lir::validate::Inputs {
            memo: Default::default(),
            discharge: None,
            operations: &operations,
            target: &target,
            layouts: &lowered.table,
            manifest: &manifest,
        };
        let mut changed = 0;
        let mut violations = Vec::new();
        for unit in ready {
            let mut unit = hirsel_rc::ready::ready(unit, &lowered.table, &finalizers).unwrap();
            assert!(hirsel_lir::validate::check(&unit, &inputs).is_empty());
            for inst in &mut unit.instructions {
                if let hirsel_lir::ir::Inst::Convert {
                    mode, flag: None, ..
                } = inst
                {
                    *mode = if oversized {
                        hirsel_lir::ir::ConvertMode::Truncate
                    } else {
                        hirsel_lir::ir::ConvertMode::Widen
                    };
                    changed += 1;
                }
            }
            violations.extend(hirsel_lir::validate::check(&unit, &inputs));
        }
        assert!(changed > 0, "{id}: no lowered conversion");
        assert!(
            violations
                .iter()
                .any(|v| v.kind == hirsel_lir::validate::ViolationKind::Type("conversion")),
            "{id}: {violations:?}"
        );
        eprintln!(
            "CONVERSION_STATIC {}",
            serde_json::json!({"id":id,"source":text,"source_hash":hash(text.as_bytes()),"checker":"reject","lowering_positive":"validated","independent_lir_mutation":"reject conversion","checker_detail":checked,"violations":format!("{violations:?}")})
        );
    }
}

macro_rules! partitions {
    ($($name:ident:$index:literal),*) => {$(mod $name {
        #[test] fn c0() { super::partition($index, 0); }
        #[test] fn c1() { super::partition($index, 1); }
        #[test] fn c2() { super::partition($index, 2); }
        #[test] fn illegal() { super::illegal($index); }
    })*};
}
partitions! { u8:0, u16:1, u32:2, u64:3, u128:4, i8:5, i16:6, i32:7, i64:8, i128:9 }
