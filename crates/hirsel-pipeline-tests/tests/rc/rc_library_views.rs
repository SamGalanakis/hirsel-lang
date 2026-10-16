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
            steps: UNICODE16_STEPS,
            ..Limits::default()
        },
    );
}
