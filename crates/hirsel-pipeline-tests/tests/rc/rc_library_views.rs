//! SEM-3 string views through the validated compiler and executed native ABI.
use hirsel_driver::native::{
    Configuration, Engine, Observation, OptLevel, Options, Outcome, Partition, Runtime, TargetSpec,
    Toolchain, differential,
};
use hirsel_engine::{
    InterpreterAdapter, RunConfiguration, RunOutcome, SimulationConfig, check_progress,
    default_context, entry_path, run_observed_snapshot, with_compiler_stack,
};
use hirsel_interp::Limits;
use std::path::PathBuf;

fn agree(source: String, configurations: Vec<Configuration>, name: &str, limits: Limits) {
    let name = name.to_owned();
    with_compiler_stack(move || {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../hirsel-driver/tests/fixtures/std_source/main.hir");
        let mut engine = hirsel_driver::load_program_source(&path, Some(""))
            .unwrap()
            .engine;
        let context = default_context();
        let file = engine.sources.add("views.hir", source.clone()).unwrap();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(
            checked.diagnostics.is_empty(),
            "{source}\n{:?}",
            checked.diagnostics
        );
        let entry = entry_path(&context);
        engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits,
            ..Default::default()
        }));
        let execution = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        if let Ok(RunOutcome::ToolingLimit(limit)) = &execution.outcome {
            panic!(
                "{name}: the reference interpreter stopped at its {:?} limit of {}, before the \
                 program finished; remeasure its step count and resize the budget",
                limit.kind, limit.limit
            );
        }
        let Some(observation) = execution.observation.as_ref() else {
            panic!(
                "{name}: the reference run has no observation: {:?}",
                execution.outcome
            );
        };
        let reference = Observation::of_reference(observation);
        assert_eq!(reference.output, b"ok\n");
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let target_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        let targets: Vec<_> = configurations.iter().map(|c| &c.options.target).collect();
        let tools =
            Toolchain::in_tree_diagnostic(&target_root.join("views-runtime"), &targets).unwrap();
        std::fs::create_dir_all(target_root.join(&name)).unwrap();
        for (configuration, outcome) in differential(
            &validated,
            &entry,
            &reference,
            &configurations,
            &tools,
            &target_root.join(name),
        ) {
            assert_eq!(outcome, Outcome::Agree, "{configuration}");
        }
    });
}
fn native_matrix() -> Vec<Configuration> {
    let mut out = vec![];
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        for opt in [OptLevel::None, OptLevel::Speed] {
            for partition in [Partition::Whole, Partition::Singleton] {
                out.push(Configuration {
                    options: Options {
                        regions: false,
                        ..Options::new(target.clone())
                    },
                    engine: Engine::Native {
                        opt,
                        partition,
                        runtime: Runtime::Diagnostic,
                    },
                });
            }
        }
    }
    out
}
const CONTROL: &str = r#"
flat type Sliced { prefix: String, text: String }
fn output_error[E](take e: E) -> () {}
fn scalar_count(s: String) -> u64 {
  let mut n: u64 = 0
  for x in string.scalars(s) { n += 1 }
  n
}
fn position_count(s: String) -> u64 {
  let mut n: u64 = 0
  for p in string.positions(s) { n += 1 }
  n
}
fn grapheme_count(s: String) -> u64 {
  let mut n: u64 = 0
  for x in std@1/unicode.graphemes(s) { n += 1 }
  n
}
fn early(s: String) -> u64 {
  let mut n: u64 = 0
  for x in std@1/unicode.graphemes(s) { n += string.byte_len(x); break }
  n
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let s: String = "e\u{301}😀x"
  let sliced: Sliced = Sliced(prefix: "padding", text: "e\u{301}😀x")
  let valid = (scalar_count(s) == 4 && position_count(s) == 5 && grapheme_count(s) == 3
    && position_count("") == 1 && scalar_count("") == 0 && grapheme_count("") == 0 && early(s) == 3 && scalar_count(sliced.text) == 4 && grapheme_count(sliced.text) == 3 && early(sliced.text) == 3)
  out.print(io.stdout(rt)? grant_error, if valid { "ok\n" } else { "bad\n" }) ? output_error
  Ok(())
}

@noalloc
fn grant_error(refusal: GrantError) -> () uses {} { () }
"#;

#[test]
#[ignore = "links both native targets; tools/check-native.sh"]
fn views_and_early_exit_agree_on_both_native_targets() {
    agree(
        CONTROL.into(),
        native_matrix(),
        "views-control",
        Limits::default(),
    );
}

#[test]
fn views_and_early_exit_pass_every_heap_failure_audit() {
    agree(
        CONTROL.into(),
        vec![Configuration {
            options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            engine: Engine::Heap { campaign: true },
        }],
        "views-heap",
        Limits::default(),
    );
}

fn escaped(s: &str) -> String {
    format!(
        "\"{}\"",
        s.chars()
            .map(|c| format!("\\u{{{:x}}}", u32::from(c)))
            .collect::<String>()
    )
}

/// All official vectors become String/Array literals passed through compiled
/// Hirsel loops. Small leaf suites bound per-body compiler work.
fn official_vectors() -> String {
    let mut calls = vec![];
    for line in include_str!("../../../../native-tests/library/GraphemeBreakTest.txt").lines() {
        let data = line.split('#').next().unwrap().trim();
        if data.is_empty() {
            continue;
        }
        let mut text = String::new();
        let mut boundaries = vec![];
        for token in data.split_whitespace() {
            match token {
                "÷" => boundaries.push(text.len()),
                "×" => {}
                code => text.push(char::from_u32(u32::from_str_radix(code, 16).unwrap()).unwrap()),
            }
        }
        let expected = boundaries
            .windows(2)
            .map(|b| escaped(&text[b[0]..b[1]]))
            .collect::<Vec<_>>()
            .join(", ");
        calls.push(format!(
            "verify({}, [{}]) && verify_slice({}, [{}])",
            escaped(&text),
            expected,
            escaped(&text),
            expected
        ));
    }
    assert_eq!(calls.len(), 1093);
    let mut source = String::from(
        r#"
fn output_error[E](take e: E) -> () {}
fn verify(s: String, expected: Array[String]) -> bool {
  let mut n: u64 = 0
  let mut valid: bool = true
  for x in std@1/unicode.graphemes(s) {
    valid = valid && x == expected[n]
    n += 1
  }
  valid && n == array.len(expected)
}
fn verify_slice(s: String, expected: Array[String]) -> bool {
  let wrapped = std@1/string.concat("🇦\u{200d}", std@1/string.concat(s, "\u{301}Z"))
  match string.position(wrapped, 7) {
    Some(first) => match string.position(wrapped, 7 + string.byte_len(s)) {
      Some(last) => verify(string.slice(wrapped, first, last), expected),
      None => false,
    },
    None => false,
  }
}
"#,
    );
    let mut groups = vec![];
    for (i, chunk) in calls.chunks(10).enumerate() {
        source.push_str(&format!(
            "fn vector_suite_{i}() -> bool {{ {} }}\n",
            chunk.join(" && ")
        ));
        groups.push(format!("vector_suite_{i}()"));
    }
    let mut roots = vec![];
    for (i, chunk) in groups.chunks(10).enumerate() {
        source.push_str(&format!(
            "fn vector_group_{i}() -> bool {{ {} }}\n",
            chunk.join(" && ")
        ));
        roots.push(format!("vector_group_{i}()"));
    }
    source.push_str(&format!("pub fn main(rt: Io, args: Args) -> Result[(), ()] {{\n let valid = {}\n out.print(io.stdout(rt)? grant_error, if valid {{ \"ok\\n\" }} else {{ \"bad\\n\" }}) ? output_error\n Ok(())\n}}\n\n@noalloc\nfn grant_error(refusal: GrantError) -> () uses {{}} {{ () }}\n", roots.join(" && ")));
    source
}

/// The reference run of `official_vectors` takes 22,735,367 interpreter steps,
/// measured 2026-10-16 on 954da374 and the same on b76b4437 (2026-10-15, the first
/// Buck2-buildable tree after 670f6ab4 moved graphemes into std source and added
/// the slice pass): every vector runs twice through interpreted std@1/unicode
/// source. The budget leaves about 30% headroom.
const UNICODE16_STEPS: u64 = 30_000_000;

#[test]
#[ignore = "compiles all 1093 official vectors on both targets; tools/check-native.sh"]
fn unicode16_break_vectors_agree_through_compiled_hirsel() {
    agree(
        official_vectors(),
        native_matrix(),
        "views-unicode16",
        Limits {
            steps: Some(UNICODE16_STEPS),
            ..Limits::default()
        },
    );
}

#[test]
fn encoding_field_copies_keep_only_the_parameter_owner() {
    let source = r#"
type Row { text: String }
fn emit(row: Row, mut out: List[u8]) {
  std@1/json.write_text_into(row.text, mut out)
}
fn main() -> u64 {
  let mut out: List[u8] = []
  emit(Row(text: "é😀\n"), mut out)
  list.len(out)
}
"#;
    let (_, lowered) = crate::rc_support::lower_valid(source);
    let emit = crate::rc_support::unit_named(&lowered, "emit");
    assert!(
        !emit
            .instructions
            .iter()
            .any(|i| matches!(i, hirsel_lir::ir::Inst::Copy { .. })),
        "{}",
        crate::rc_support::dump(emit)
    );
}

#[test]
fn generated_encoding_tag_projection_keeps_only_the_parameter_owner() {
    let source = r#"
@serializable
type Item { title: String, description: String, tags: List[String] }
@preserves_unique(out)
fn item_json_into(value: Item, mut out: List[u8], depth: u64) -> Result[(), std@1/json.JsonError] uses {} = comptime std@1/derive.json_into(shape(Item))
fn main() -> u64 {
  let mut out: List[u8] = []
  let _ = item_json_into(Item(title: "a", description: "b", tags: ["c"]), mut out, 0)
  list.len(out)
}
"#;
    let (_, lowered) = crate::rc_support::lower_valid(source);
    let unit = crate::rc_support::unit_named(&lowered, "item_json_into");
    assert!(
        !unit
            .instructions
            .iter()
            .any(|i| matches!(i, hirsel_lir::ir::Inst::Copy { .. })),
        "{}",
        crate::rc_support::dump(unit)
    );
}

fn encoding_audit(source: &str, name: &str) {
    let source = source.to_owned();
    let name = name.to_owned();
    with_compiler_stack(move || {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../hirsel-driver/tests/fixtures/std_source/main.hir");
        let mut loaded = hirsel_driver::load_program_source(&path, Some(&source)).unwrap();
        loaded.check_for_execution().unwrap();
        let entry = loaded.entry.clone();
        let run = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let reference = Observation::of_reference(run.observation.as_ref().unwrap());
        if name.ends_with("first-failure") {
            assert_eq!(
                reference.failures.first().map(|f| f.rule.as_str()),
                Some("SEM-3")
            );
            assert!(reference.output.is_empty());
        } else {
            assert_eq!(reference.output, b"ok\n");
            assert!(reference.failures.is_empty());
        }
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        let tools =
            Toolchain::in_tree_diagnostic(&root.join("encoding-runtime"), &[&target]).unwrap();
        let directory = root.join(&name);
        std::fs::create_dir_all(&directory).unwrap();
        let configurations = if name.starts_with("encoding-cranelift") {
            vec![Configuration {
                options: Options::new(target),
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            }]
        } else {
            vec![
                Configuration {
                    options: Options::new(target.clone()),
                    engine: Engine::Heap { campaign: true },
                },
                Configuration {
                    options: Options::new(target),
                    engine: Engine::Native {
                        opt: OptLevel::Speed,
                        partition: Partition::Whole,
                        runtime: Runtime::Diagnostic,
                    },
                },
            ]
        };
        for (configuration, outcome) in differential(
            &validated,
            &entry,
            &reference,
            &configurations,
            &tools,
            &directory,
        ) {
            assert_eq!(outcome, Outcome::Agree, "{name} {configuration}\n{source}");
        }
    });
}

#[test]
fn wordfreq_bulk_copies_and_aliased_box_reads_agree() {
    let source = include_str!("../../../../conformance/MEM/MEM-7-wordfreq-bulk-alias-run.hir");
    for name in ["encoding-wordfreq-bulk", "encoding-cranelift-wordfreq-bulk"] {
        encoding_audit(source, name);
    }
}

#[test]
fn wordfreq_bulk_workload_matches_exact_vectors() {
    let workload = include_str!("../../../../bench/native/programs/wordfreq-owned-bulk.hir");
    let common = include_str!("../../../../bench/native/programs/common.hir");
    let common = common.split("pub fn main(").next().unwrap();
    let source = format!(
        r#"{workload}
{common}
@noalloc
fn grant_error(refusal: GrantError) -> () uses {{}} {{ () }}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {{
  failure.check(workload(b"") == b"", TextInvariant())
  failure.check(workload(b" \t\r\n\x0b\x0c") == b"", TextInvariant())
  failure.check(workload(b"a b a b c\n") == b"a\t2\nb\t2\nc\t1\n", TextInvariant())
  failure.check(workload(b"\xc3\xa9 x\t\xc3\xa9\nlast") == b"\xc3\xa9\t2\nx\t1\nlast\t1\n", TextInvariant())
  out.print(io.stdout(rt) ? grant_error, "ok\n") ? error
  Ok(())
}}
"#
    );
    for name in [
        "encoding-wordfreq-vectors",
        "encoding-cranelift-wordfreq-vectors",
    ] {
        encoding_audit(&source, name);
    }
}

const ENCODING_PREFIX: &str = r#"
fn output_error[E](take e: E) -> () {}
@noalloc
fn grant_error(refusal: GrantError) -> () uses {} { () }
fn emit(text: String, mut out: List[u8]) {
  let data = string.as_bytes(text)
  bytes.append(mut out, bytes.slice(data, 1, bytes.len(data)))
}
"#;

#[test]
fn borrowed_encoding_and_alias_controls_agree_on_native_and_heap_failures() {
    encoding_audit(
        &format!(
            r#"{ENCODING_PREFIX}
fn retain(data: Bytes) -> Bytes in data {{ bytes.slice(data, 1, bytes.len(data)) }}
fn twice(data: Bytes, mut a: List[u8], mut b: List[u8]) {{
  bytes.append(mut a, data)
  bytes.append(mut b, data)
}}
type Nested {{ root: Shared[String] }}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {{
  let mut source: List[u8] = []
  std@1/text.append(mut source, "hé😀<&")
  let text = std@1/text.finish_valid(copy source)
  emit(text, mut source)
  let data = bytes.from_list(copy source)
  let kept = retain(data)
  bytes.append(mut source, bytes.slice(data, 1, bytes.len(data)))
  let mut other: List[u8] = []
  twice(kept, mut source, mut other)
  let built: String = "nested root"
  let frozen = shared.freeze(take built)
  let nested = Nested(root: copy frozen)
  let nested_size = shared.with(nested.root, fn(frozen_text: String) -> u64 {{
    let mut bytes: List[u8] = []
    emit(frozen_text, mut bytes)
    list.len(bytes)
  }})
  let root_kept = match shared.thaw(take frozen) {{ Ok(_) => false, Error(_) => true }}
  out.print(io.stdout(rt) ? grant_error, if kept == string.as_bytes("é😀<&é😀<&") && std@1/text.finish_valid(copy source) == "hé😀<&é😀<&é😀<&é😀<&é😀<&é😀<&" && std@1/text.finish_valid(copy other) == "é😀<&é😀<&" && nested_size == 10 && root_kept {{ "ok\n" }} else {{ "bad\n" }}) ? output_error
  Ok(())
}}
"#
        ),
        "encoding-alias-controls",
    );
}

#[test]
fn borrowed_encoding_bounds_fail_before_the_appends_effect() {
    encoding_audit(
        &format!(
            r#"{ENCODING_PREFIX}
fn fail(text: String, mut out: List[u8]) {{
  let data = string.as_bytes(text)
  bytes.append(mut out, bytes.slice(data, 2, 1))
}}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {{
  let mut out: List[u8] = []
  std@1/text.append(mut out, "live")
  fail(std@1/text.finish_valid(copy out), mut out)
  out.print(io.stdout(rt) ? grant_error, "unreachable\n") ? output_error
  Ok(())
}}
"#
        ),
        "encoding-first-failure",
    );
}

#[test]
fn transient_encoding_subslice_has_no_temporary_finite_credit() {
    use hirsel_lir::ir::Inst;
    let source = format!(
        "{ENCODING_PREFIX}\nfn main() -> u64 {{ let mut out: List[u8] = []\n std@1/text.append(mut out, \"\")\n emit(\"hé😀\", mut out)\n list.len(out) }}"
    );
    let (program, lowered) = crate::rc_support::lower_valid(&source);
    let emit = crate::rc_support::unit_named(&lowered, "emit");
    assert_eq!(
        emit.instructions
            .iter()
            .filter(|i| matches!(i, Inst::SliceView { .. }))
            .count(),
        1,
        "{}",
        crate::rc_support::dump(emit)
    );
    let manifest_source = source.clone();
    let manifest = with_compiler_stack(move || {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../hirsel-driver/tests/fixtures/std_source/main.hir");
        let mut loaded = hirsel_driver::load_program_source(&path, Some(&manifest_source)).unwrap();
        loaded.check_for_execution().unwrap();
        hirsel_driver::native::manifest(&loaded.engine.export_native_input().unwrap()).unwrap()
    });
    for target in TargetSpec::executed() {
        let ready =
            hirsel_driver::native::build_program(&program, manifest.clone(), &Options::new(target))
                .unwrap();
        let emit = ready
            .units
            .iter()
            .map(|u| u.unit())
            .find(|u| u.header.definition.0.name == "emit")
            .unwrap();
        let slice_layout = emit
            .instructions
            .iter()
            .find_map(|i| match i {
                Inst::SliceView { result, .. } => {
                    Some(emit.types[emit.values[result.index()].ty.index()].layout)
                }
                _ => None,
            })
            .unwrap();
        assert!(
            !emit.instructions.iter().any(|i| match i {
                Inst::Copy { result: value, .. }
                | Inst::Dup { value }
                | Inst::Release { value, .. } =>
                    emit.types[emit.values[value.index()].ty.index()].layout == slice_layout,
                _ => false,
            }),
            "{}",
            crate::rc_support::dump(emit)
        );
        assert!(
            !emit
                .tables
                .operations
                .iter()
                .any(|op| op.definition.is_core("bytes.slice"))
        );
    }
}

#[test]
fn encoding_contract_twins_keep_take_mut_and_noalloc_checks() {
    let valid = [
        "fn append(xs: List[u8], mut out: List[u8]) { for b in xs { list.push(mut out, b) } }\nfn main() { let mut xs: List[u8] = [1u8]\n let ys = copy xs\n append(ys, mut xs) }",
        "fn emit(text: String, mut out: List[u8]) { let data = string.as_bytes(text)\n bytes.append(mut out, bytes.slice(data,0,bytes.len(data))) }",
        "fn take_bytes(take data: Bytes) {}\nfn main() { let data = b\"abc\"\n take_bytes(copy data) }",
    ];
    let rejected = [
        "fn append(xs: List[u8], mut out: List[u8]) { for b in xs { list.push(mut out, b) } }\nfn main() { let mut xs: List[u8] = [1u8]\n append(xs, mut xs) }",
        "@noalloc\nfn emit(text: String, mut out: List[u8]) { let data = string.as_bytes(text)\n bytes.append(mut out, bytes.slice(data,0,bytes.len(data))) }",
        "fn take_bytes(take data: Bytes) {}\nfn main() { let data = b\"abc\"\n take_bytes(data) }",
    ];
    for source in valid {
        crate::rc_support::try_compile(source).unwrap();
    }
    for source in rejected {
        assert!(crate::rc_support::try_compile(source).is_err(), "{source}");
    }
}

#[test]
fn transient_encoding_c_rejects_forged_credits_and_expired_loans() {
    use hirsel_lir::ir::{Holding, Inst};
    let source = format!(
        "{ENCODING_PREFIX}\nfn main() {{ let mut out: List[u8] = []\n emit(\"abc\",mut out) }}"
    );
    let (program, lowered) = crate::rc_support::lower_valid(&source);
    let id = crate::rc_support::instances_of(&lowered, "emit")[0];
    let unit = &lowered.units[id.index()];
    let (result, value, failure) = unit
        .instructions
        .iter()
        .find_map(|i| match i {
            Inst::SliceView {
                result,
                value,
                failure,
                ..
            } => Some((*result, *value, *failure)),
            _ => None,
        })
        .unwrap();
    let target = crate::rc_support::target();
    let manifest = crate::rc_support::manifest(&program);
    let operations = lowered.operation_registry(&program);
    let inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &operations,
        target: &target,
        layouts: &lowered.table,
        manifest: &manifest,
    };
    for mutation in 0..3 {
        let mut forged = unit.clone();
        let unit = &mut forged;
        match mutation {
            0 => unit.values[result.index()].holding = Holding::Owned,
            1 => unit.values[value.index()].holding = Holding::Plain,
            _ => unit.tables.failures[failure.index()].rule = Some("MEM-14".into()),
        }
        assert!(
            !hirsel_lir::validate::check(&forged, &inputs).is_empty(),
            "forgery {mutation}"
        );
    }
    let Holding::Borrowed(loan) = unit.values[result.index()].holding else {
        panic!("slice view loan")
    };
    let mut editor = hirsel_rc::insert::Editor::new(unit.clone());
    let (b, i) = editor
        .code
        .iter()
        .enumerate()
        .find_map(|(b, c)| {
            c.iter()
                .position(|i| matches!(i, Inst::SliceView { .. }))
                .map(|i| (b, i))
        })
        .unwrap();
    editor.code[b].insert(
        i,
        Inst::EndLoan {
            loan,
            if_active: false,
        },
    );
    let bad = editor.finish();
    assert!(!hirsel_lir::validate::check(&bad, &inputs).is_empty());
}

#[test]
fn transient_encoding_retained_taken_stored_and_two_mutable_uses_keep_ownership() {
    use hirsel_lir::ir::Inst;
    let source = format!(
        r#"{ENCODING_PREFIX}
fn raw(data: Bytes, mut out: List[u8]) {{ bytes.append(mut out, bytes.slice(data, 0, bytes.len(data))) }}
fn retain(data: Bytes) -> Bytes in data {{ bytes.slice(data, 0, bytes.len(data)) }}
fn consume(take data: Bytes) {{}}
fn taken(take text: String) {{ let data = string.as_bytes(text)
 consume(bytes.slice(data, 0, bytes.len(data))) }}
fn sink(data: Bytes, mut a: List[u8], mut b: List[u8]) {{ bytes.append(mut a, data)
 bytes.append(mut b, data) }}
fn two(text: String, mut a: List[u8], mut b: List[u8]) {{ let data = string.as_bytes(text)
 sink(bytes.slice(data, 0, bytes.len(data)), mut a, mut b) }}
fn stored(take text: String, mut values: List[Bytes]) {{ let data = string.as_bytes(text)
 list.push(mut values, bytes.slice(data, 0, bytes.len(data))) }}
fn main() {{ let mut a: List[u8] = []
 let mut b: List[u8] = []
 let mut values: List[Bytes] = []
 std@1/text.append(mut a, "a")
 raw(b"abc", mut a)
 let kept = retain(b"abc")
 bytes.append(mut b, kept)
 taken("abc")
 two("abc", mut a, mut b)
 stored("abc", mut values)
}}
"#
    );
    let (_, lowered) = crate::rc_support::lower_valid(&source);
    for name in ["raw", "retain", "taken", "two", "stored"] {
        let unit = crate::rc_support::unit_named(&lowered, name);
        assert!(
            !unit
                .instructions
                .iter()
                .any(|i| matches!(i, Inst::SliceView { .. })),
            "{name}\n{}",
            crate::rc_support::dump(unit)
        );
        assert!(
            unit.tables
                .operations
                .iter()
                .any(|op| op.definition.is_core("bytes.slice")),
            "{name}\n{}",
            crate::rc_support::dump(unit)
        );
    }
}

#[test]
fn borrowed_encoding_success_and_first_failure_agree_through_cranelift() {
    encoding_audit(
        &format!(
            r#"{ENCODING_PREFIX}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {{
 let mut out: List[u8] = []
 emit("hé😀<&", mut out)
 let valid = std@1/text.finish_valid(take out) == "é😀<&"
 out.print(io.stdout(rt) ? grant_error, if valid {{ "ok\n" }} else {{ "bad\n" }}) ? output_error
 Ok(())
}}
"#
        ),
        "encoding-cranelift",
    );
    encoding_audit(
        &format!(
            r#"{ENCODING_PREFIX}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {{
 let mut out: List[u8] = []
 emit("", mut out)
 out.print(io.stdout(rt) ? grant_error, "unreachable\n") ? output_error
 Ok(())
}}
"#
        ),
        "encoding-cranelift-first-failure",
    );
}

#[test]
fn generated_encoding_preserves_permanent_arena_sharing_and_charge_order() {
    encoding_audit(
        r#"
@serializable
type Row { tags: List[String] }
@preserves_unique(out)
fn row_json_into(value: Row, mut out: List[u8], depth: u64) -> Result[(), std@1/json.JsonError] uses {} = comptime std@1/derive.json_into(shape(Row))
fn output_error[E](take e: E) -> () {}
@noalloc
fn grant_error(refusal: GrantError) -> () uses {} { () }
fn bounded(cap: u64) -> Result[bool, OutOfSpace] {
 arena.bounded(cap, fn(a: Arena) -> bool {
  let tags: List[String] in a = ["a", "b"]
  let mut row = Row(tags: take tags)
  let mut out: List[u8] = []
  let _ = row_json_into(row, mut out, 0)
  list.push(mut row.tags, "c")
  list.len(row.tags) == 3
 })
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
 let valid = bounded(40) == Ok(true) && bounded(39) == Error(OutOfSpace())
 out.print(io.stdout(rt) ? grant_error, if valid { "ok\n" } else { "bad\n" }) ? output_error
 Ok(())
}
"#,
        "encoding-arena-sharing",
    );
}
