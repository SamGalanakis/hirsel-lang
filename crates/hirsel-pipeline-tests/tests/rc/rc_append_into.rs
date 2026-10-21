//! Results built only to be appended (format-into): a call whose Bytes or
//! String result is only copied onto a list appends to that list instead.
//! Every rewritten and derived definition passes the IR checker, every unit
//! passes C at each stage, and H agrees with the interpreter under its fault
//! campaign. Shapes whose fusion would reorder an observable effect stay calls.
use crate::rc_support as support;

use hirsel_driver::native::{
    Configuration, Engine, Observation, Options, Outcome, Toolchain, differential,
};
use hirsel_engine::{
    RunConfiguration, SimulationConfig, run_observed_snapshot, with_compiler_stack,
};
use hirsel_rc::append_into::{INTO_SUFFIX, fuse};
use hirsel_rc::instance::Program;
use hirsel_syntax::DefPath;
use hirsel_tir::{Callee, Terminator};
use std::path::PathBuf;

use support::*;

/// `main` prints `value()`.
const ENTRY: &str = "fn output_error[E](take e: E) -> () {}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let mut line: List[u8] = []
  std@1/u64.format_into(value(), mut line)
  list.push(mut line, 10u8)
  match string.from_bytes(bytes.from_list(take line)) {
    Ok(text) => out.print(io.stdout(rt) ? grant_error, text) ? output_error,
    Error(_) => (),
  }
  Ok(())
}
@noalloc
fn grant_error(refusal: GrantError) -> () uses {} { () }
";

/// The frozen benchmarks' `number` and `append`, and a checksum.
const SHARED: &str = "fn number(n: u64) -> Bytes {
  let mut out: List[u8] = []
  let s = std@1/u64.to_string(n)
  let b = string.as_bytes(s)
  for i in 0u64..bytes.len(b) {
    list.push(mut out, b[i])
  }
  list.push(mut out, 10u8)
  bytes.from_list(take out)
}
fn append(mut out: List[u8], data: Bytes) {
  for i in 0u64..bytes.len(data) { list.push(mut out, data[i]) }
}
fn checksum(b: Bytes) -> u64 {
  let mut h: u64 = 14695981039346656037u64
  for i in 0u64..bytes.len(b) {
    let product: u128 = int.widen(h ^ int.widen(b[i])) * 1099511628211u128
    h = int.truncate(product)
  }
  h
}
";

fn source(body: &str) -> String {
    format!("{SHARED}{body}{ENTRY}")
}

fn named(program: &Program, name: &str) -> Option<DefPath> {
    program
        .definitions
        .keys()
        .find(|p| p.0.name == name && !p.0.package.name_is("std"))
        .cloned()
}

/// The callee names of `name`'s entry body calls.
fn calls(program: &Program, name: &str) -> Vec<String> {
    let def = &program.definitions[&named(program, name).unwrap_or_else(|| panic!("{name}"))];
    def.bodies[def.entry.0 as usize]
        .blocks
        .iter()
        .filter_map(|b| match &b.terminator {
            Terminator::Call {
                callee: Callee::Definition { path, .. },
                ..
            } => Some(
                path.0
                    .modules
                    .iter()
                    .chain([&path.0.name])
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("."),
            ),
            _ => None,
        })
        .collect()
}

/// The IR checker accepts every definition the fusion changed or added.
fn checked(source: &str, before: &Program, after: &Program) {
    let mut signatures = hirsel_validate::SignatureMap::new(after.types.clone());
    signatures.extend_signatures(after.signatures.values().map(|s| (**s).clone()));
    for (path, def) in &after.definitions {
        if before.definitions.get(path) == Some(def) {
            continue;
        }
        let v = hirsel_validate::validate(def, &after.types, &signatures, &mut vec![]);
        assert!(
            v.violations.is_empty(),
            "{source}\n{}: {:#?}",
            path.0.name,
            v.violations
        );
    }
}

/// The driver's build (C at every stage) agrees with the interpreter on H,
/// under its failure campaign.
fn agree(source: &str, name: &str) {
    let source = source.to_owned();
    let name = name.to_owned();
    with_compiler_stack(move || {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../hirsel-driver/tests/fixtures/std_numeric.hir");
        let mut loaded = hirsel_driver::load_program_source(&path, Some(&source)).unwrap();
        loaded.check_for_execution().unwrap();
        let entry = loaded.entry.clone();
        let execution = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let reference = Observation::of_reference(execution.observation.as_ref().unwrap());
        assert!(!reference.output.is_empty(), "{source}");
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let target = hirsel_driver::native::TargetSpec::x86_64_unknown_linux_gnu();
        let configurations = vec![Configuration {
            options: Options::new(target.clone()),
            engine: Engine::Heap { campaign: true },
        }];
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        let tools =
            Toolchain::in_tree_diagnostic(&root.join("append-into-runtime"), &[&target]).unwrap();
        std::fs::create_dir_all(root.join(&name)).unwrap();
        for (configuration, outcome) in differential(
            &validated,
            &entry,
            &reference,
            &configurations,
            &tools,
            &root.join(&name),
        ) {
            assert_eq!(outcome, Outcome::Agree, "{source}\n{configuration}");
        }
    });
}

/// Fuse `body`'s program, check it with the IR checker, run it through the
/// driver on H against the interpreter, and return the fused program.
fn check(body: &str, name: &str) -> Program {
    let source = source(body);
    let program = compile(&source);
    let fused = fuse(&program);
    checked(&source, &program, &fused);
    agree(&source, name);
    fused
}

/// The frozen persistent-list and wordfreq emitters, reduced.
const EMIT: &str = "fn value() -> u64 {
  let mut output: List[u8] = []
  list.reserve(mut output, 64u64)
  for v in 0u64..3u64 {
    let line = number(v * 9770000977u64)
    for j in 0u64..bytes.len(line) { list.push(mut output, line[j]) }
    append(mut output, number(v))
    list.push(mut output, 9u8)
  }
  checksum(bytes.from_list(take output))
}
";

#[test]
fn copied_results_append_into_the_list_they_were_copied_onto() {
    let fused = check(EMIT, "append-into-emit");
    let into = format!("number{INTO_SUFFIX}");
    // Both emitters call the appending form; nothing calls number.
    let value = calls(&fused, "value");
    assert_eq!(value.iter().filter(|c| **c == into).count(), 2, "{value:?}");
    assert!(!value.iter().any(|c| c == "number"), "{value:?}");
    // The builder and its form append the text: no to_string remains.
    for f in ["number", into.as_str()] {
        let c = calls(&fused, f);
        assert!(c.iter().any(|c| c == "u64.format_into"), "{f}: {c:?}");
        assert!(!c.iter().any(|c| c == "u64.to_string"), "{f}: {c:?}");
        assert!(!c.iter().any(|c| c == "string.as_bytes"), "{f}: {c:?}");
    }
    assert!(!calls(&fused, &into).iter().any(|c| c == "bytes.from_list"));
}

/// A package consumer that names only to_string must still select its appender.
/// ENTRY names format_into directly, which used to hide the missing dependency.
#[test]
fn to_string_only_consumers_select_the_appending_implementation() {
    let entry = ENTRY.replace(
        "  std@1/u64.format_into(value(), mut line)",
        "  let digits = string.as_bytes(std@1/u64.to_string(value()))\n  for i in 0u64..bytes.len(digits) { list.push(mut line, digits[i]) }",
    );
    let source = format!("{SHARED}{EMIT}{entry}");
    let program = compile(&source);
    let fused = fuse(&program);
    checked(&source, &program, &fused);
    for name in ["number", "number$into"] {
        let c = calls(&fused, name);
        assert!(c.iter().any(|c| c == "u64.format_into"), "{name}: {c:?}");
        assert!(!c.iter().any(|c| c == "u64.to_string"), "{name}: {c:?}");
        assert!(!c.iter().any(|c| c == "string.as_bytes"), "{name}: {c:?}");
    }
    agree(&source, "to-string-only-append");
}

/// Fusing these would reorder or drop an observable effect, or read bytes
/// the copy no longer holds: each call stays as it was.
#[test]
fn results_read_otherwise_or_lists_touched_first_stay_calls() {
    let fused = check(
        "// The result is read besides its copy.
fn reread() -> u64 {
  let mut output: List[u8] = []
  for v in 0u64..2u64 {
    let line = number(v)
    for j in 0u64..bytes.len(line) { list.push(mut output, line[j]) }
    list.push(mut output, line[0])
  }
  checksum(bytes.from_list(take output))
}
// The list is written between the call and the copy.
fn written() -> u64 {
  let mut output: List[u8] = []
  for v in 0u64..2u64 {
    let line = number(v)
    list.push(mut output, 35u8)
    for j in 0u64..bytes.len(line) { list.push(mut output, line[j]) }
  }
  checksum(bytes.from_list(take output))
}
// A partial copy is no copy.
fn partial() -> u64 {
  let mut output: List[u8] = []
  for v in 0u64..2u64 {
    let line = number(v)
    for j in 1u64..bytes.len(line) { list.push(mut output, line[j]) }
  }
  checksum(bytes.from_list(take output))
}
// The list is read between the call and the copy.
fn measured() -> u64 {
  let mut output: List[u8] = []
  for v in 0u64..2u64 {
    let line = number(v)
    let n = list.len(output)
    for j in 0u64..bytes.len(line) { list.push(mut output, line[j]) }
    list.push(mut output, int.narrow(n % 200u64))
  }
  checksum(bytes.from_list(take output))
}
fn value() -> u64 {
  reread() ^ written() ^ partial() ^ measured()
}
",
        "append-into-declined",
    );
    for f in ["reread", "written", "partial", "measured"] {
        let c = calls(&fused, f);
        assert!(c.iter().any(|c| c == "number"), "{f}: {c:?}");
        assert!(!c.iter().any(|c| c.ends_with(INTO_SUFFIX)), "{f}: {c:?}");
    }
}

/// A builder that can fail after its first append is not given an appending
/// form: the caller's list would hold the partial text when cleanup ran.
/// Failing before the first append is the same in both forms.
#[test]
fn builders_failing_after_their_first_append_stay_calls() {
    let fused = check(
        "fn late(n: u64) -> Bytes {
  let mut out: List[u8] = []
  list.push(mut out, 40u8)
  failure.check(n < 1000u64, Arithmetic())
  list.push(mut out, 41u8)
  bytes.from_list(take out)
}
fn early(n: u64) -> Bytes {
  failure.check(n < 1000u64, Arithmetic())
  let mut out: List[u8] = []
  list.push(mut out, 40u8)
  list.push(mut out, 41u8)
  bytes.from_list(take out)
}
fn value() -> u64 {
  let mut output: List[u8] = []
  for v in 0u64..2u64 {
    let line = late(v)
    for j in 0u64..bytes.len(line) { list.push(mut output, line[j]) }
    let other = early(v)
    for j in 0u64..bytes.len(other) { list.push(mut output, other[j]) }
  }
  checksum(bytes.from_list(take output))
}
",
        "append-into-failing",
    );
    let value = calls(&fused, "value");
    assert!(value.iter().any(|c| c == "late"), "{value:?}");
    assert!(named(&fused, &format!("late{INTO_SUFFIX}")).is_none());
    assert!(
        value.iter().any(|c| *c == format!("early{INTO_SUFFIX}")),
        "{value:?}"
    );
}

/// The digit emitters manipulate only scalar/inline data and a borrowed output.
/// A counted table used to add generic release calls around indexed reads.
#[test]
fn decimal_digit_emitters_need_no_reference_count_or_release_work() {
    let program = compile(
        "fn main() -> u64 {
  let mut out: List[u8] = []
  std@1/u64.format_into(18446744073709551615u64, mut out)
  list.len(out)
}
",
    );
    for target in hirsel_lir::layout_types::TargetSpec::executed() {
        let built = hirsel_driver::native::build_program(
            &program,
            manifest(&program),
            &Options::new(target),
        )
        .unwrap();
        let mut emitters = 0;
        for unit in &built.units {
            let unit = unit.unit();
            let path = &unit.header.definition.0;
            if path.package.package_is("std", "1")
                && path.modules.last().is_some_and(|name| name == "decimal")
                && matches!(
                    path.name.as_str(),
                    "append" | "leading_digits" | "sixteen_digits"
                )
            {
                emitters += 1;
                let count_work: Vec<_> = unit
                    .instructions
                    .iter()
                    .filter(|inst| {
                        matches!(
                            inst,
                            hirsel_lir::ir::Inst::Dup { .. } | hirsel_lir::ir::Inst::Release { .. }
                        )
                    })
                    .collect();
                assert!(count_work.is_empty(), "{}: {count_work:#?}", path.name);
            }
        }
        assert_eq!(emitters, 3);
    }
}

#[test]
fn inline_format_builders_append_into_the_destination() {
    let input = source(
        "fn value() -> u64 { let mut output: List[u8] = []\n std@1/text.append(mut output, f\"{42u64},\")\n checksum(bytes.from_list(take output)) }
",
    );
    let program = compile(&input);
    let fused = fuse(&program);
    checked(&input, &program, &fused);
    assert!(
        !calls(&fused, "value")
            .iter()
            .any(|c| c == "text.finish_valid"),
        "{:?}",
        calls(&fused, "value")
    );
    agree(&input, "inline-format-append");
}

#[test]
fn string_byte_borrows_need_no_runtime_bridge_or_holder_operations() {
    with_compiler_stack(|| {
        let input = "fn view_size(s: String) -> u64 { let data = string.as_bytes(s)\n bytes.len(data) }\npub fn main() -> Result[(), u64] { let mut b: List[u8] = []\n std@1/text.append(mut b, \"é😀\")\n Error(view_size(std@1/text.finish_valid(take b))) }";
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../hirsel-driver/tests/fixtures/std_numeric.hir");
        let mut loaded = hirsel_driver::load_program_source(&path, Some(input)).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        for target in hirsel_lir::layout_types::TargetSpec::executed() {
            let built = hirsel_driver::native::build(&validated, &Options::new(target)).unwrap();
            let mut checked = 0;
            for unit in &built.units {
                let unit = unit.unit();
                let path = &unit.header.definition.0;
                if path.name == "view_size"
                    || (path.package.package_is("std", "1")
                        && path.modules == ["text"]
                        && path.name == "append")
                {
                    checked += 1;
                    assert!(
                        !unit.instructions.iter().any(|i| {
                            let value = match i {
                                hirsel_lir::ir::Inst::Dup { value, .. }
                                | hirsel_lir::ir::Inst::Release { value, .. } => *value,
                                _ => return false,
                            };
                            let layout = unit.types[unit.values[value.index()].ty.index()].layout;
                            matches!(
                                built.table.layouts[layout.0 as usize].kind,
                                hirsel_lir::layout_types::LayoutKind::Slice { .. }
                            )
                        }),
                        "{}: {:#?}",
                        path.name,
                        unit.instructions
                    );
                    assert!(
                        !unit
                            .tables
                            .operations
                            .iter()
                            .any(|op| op.definition.is_core("string.as_bytes"))
                    );
                }
            }
            assert_eq!(checked, 2);
        }
    });
}
