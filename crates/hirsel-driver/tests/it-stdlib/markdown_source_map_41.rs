//! Exact cumulative parser admission and unchanged CommonMark AST/rendering.
use crate::source_std::load_program_source;
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::Path;

fn parts(variable: &str) -> Vec<String> {
    let all: Vec<String> = (0..30)
        .map(|n| n.to_string())
        .chain([
            "budgets".into(),
            "edit".into(),
            "edges".into(),
            "metadata".into(),
            "decoded".into(),
            "whitespace".into(),
        ])
        .collect();
    let selected = std::env::var(variable).unwrap_or_else(|_| all.join(","));
    let parts: Vec<String> = selected.split(',').map(str::to_owned).collect();
    assert!(
        !parts.is_empty() && parts.iter().all(|part| all.contains(part)),
        "invalid cells: {selected}"
    );
    parts
}

fn run(engine: &str) {
    with_compiler_stack(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/markdown-source-map-41/helpers.hir");
        let source = fixture_source();
        let mut program = load_program_source(&path, Some(&source)).unwrap();
        program.check_for_execution().unwrap_or_else(|e| {
            if let ToolError::Query {
                error: hirsel_types::QueryError::Diagnostics(d),
                sources,
                ..
            } = &e
            {
                for diagnostic in d {
                    eprintln!("{}", diagnostic.render_human(sources));
                }
            }
            panic!("{e}")
        });
        eprintln!("MARKDOWN_41_STARTED vectors/{engine}");
        if engine == "reference" || engine == "checked-h" {
            let config = RunConfiguration {
                entry: program.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            for part in parts("MARKDOWN_41_HOST_PARTS") {
                eprintln!("MARKDOWN_41_STARTED {part}/{engine}");
                let execution = if engine == "reference" {
                    run_observed_snapshot(
                        &mut program.engine,
                        &config,
                        SimulationConfig {
                            args: vec![part.clone()],
                            ..Default::default()
                        },
                    )
                } else {
                    run_observed_checked(
                        &mut program.engine,
                        &config,
                        SimulationConfig {
                            args: vec![part.clone()],
                            ..Default::default()
                        },
                    )
                }
                .unwrap();
                assert!(
                    matches!(
                        execution.outcome,
                        Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                    ),
                    "{execution:?}"
                );
                let usage = execution.step_usage.expect("actual interpreter usage");
                assert!(usage.consumed > 0 && usage.consumed <= 1_000_000);
                eprintln!("MARKDOWN_41_BODY {part}/{engine} {usage:?}");
                let observation = execution.observation.unwrap();
                assert_eq!(observation.exit_code, Some(0));
                assert!(
                    observation.failures.is_empty() && observation.output.is_empty(),
                    "{observation:?}"
                );
                eprintln!("MARKDOWN_41_PASS {part}/{engine}");
            }
        } else {
            // The fixture's independent oracle is a normal return after all
            // exact-byte/cap/sink assertions, with no printed output or effects.
            let expected = native::Observation {
                exit_code: Some(0),
                blocked: false,
                budget: None,
                signaled: None,
                output: vec![],
                failures: vec![],
                roots: vec![],
            };
            let input = program.engine.export_native_input().unwrap();
            let validated = program.engine.import_native_input(input).unwrap();
            let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
            let matrix = vec![native::Configuration {
                options,
                engine: if engine == "llvm" {
                    native::Engine::Native {
                        opt: native::OptLevel::Speed,
                        partition: native::Partition::Whole,
                        runtime: native::Runtime::Default,
                    }
                } else {
                    native::Engine::Cranelift {
                        partition: native::Partition::Whole,
                        runtime: native::Runtime::Diagnostic,
                    }
                },
            }];
            let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
                .join("markdown-source-map-41")
                .join(engine);
            std::fs::create_dir_all(&dir).unwrap();
            let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &matrix).unwrap();
            let runtime = if engine == "llvm" {
                native::Runtime::Default
            } else {
                native::Runtime::Diagnostic
            };
            let tools = tools.with_runtime(runtime.build());
            let ready = native::build(&validated, &matrix[0].options).unwrap();
            let objects = native::emit_backend_objects(
                &ready,
                if engine == "llvm" {
                    native::Backend::Llvm
                } else {
                    native::Backend::Cranelift
                },
                if engine == "llvm" {
                    native::OptLevel::Speed
                } else {
                    native::OptLevel::None
                },
                native::Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let sites = native::Sites::new(validated.snapshot());
            for part in parts("MARKDOWN_41_NATIVE_PARTS") {
                eprintln!("MARKDOWN_41_STARTED {part}/{engine}");
                let case_dir = dir.join(&part);
                std::fs::create_dir_all(&case_dir).unwrap();
                let fixture = native::Fixture {
                    args: vec![part.clone()],
                    ..Default::default()
                };
                let runnable = native::link_fixture_runnable(
                    &ready,
                    &program.entry,
                    &objects,
                    &tools,
                    &case_dir,
                    &fixture,
                )
                .unwrap();
                let launched = native::launch_fixture(
                    &runnable,
                    &ready.target,
                    &tools,
                    &case_dir,
                    engine == "cranelift",
                    native::Tape::Live {
                        record: None,
                        revocations: None,
                    },
                    &fixture,
                )
                .unwrap();
                let observed = native::observe(&launched, &ready, &sites).unwrap();
                assert_eq!(observed, expected, "{part}/{engine}");
                eprintln!("MARKDOWN_41_PASS {part}/{engine}");
            }
        }
        eprintln!("MARKDOWN_41_PASS vectors/{engine}");
    });
}

#[test]
fn markdown_source_map_41_reference() {
    run("reference");
}
#[test]
fn markdown_source_map_41_checked_h() {
    run("checked-h");
}
#[test]
fn markdown_source_map_41_llvm() {
    run("llvm");
}
#[test]
fn markdown_source_map_41_cranelift() {
    run("cranelift");
}

fn origin(check: &serde_json::Value) -> String {
    match check["origin"].as_str().unwrap() {
        "Cover" => format!(
            "Cover(markdown.ByteRange(start: {}, end: {}))",
            check["range"][0], check["range"][1]
        ),
        "Synthetic" => format!(
            "Synthetic({}(), Some(markdown.ByteRange(start: {}, end: {})))",
            check["reason"].as_str().unwrap(),
            check["related_range"][0],
            check["related_range"][1]
        ),
        "Unavailable" => format!("Unavailable({}())", check["reason"].as_str().unwrap()),
        other => panic!("{other}"),
    }
}

// Each entry names the kind(s), optional field and multiplicity selected by
// the independent design vector. A range appearing on a wrong kind cannot pass.
fn selectors() -> Vec<Vec<(&'static [u8], u8, u64)>> {
    vec![
        vec![(&[13], 0, 1), (&[2], 0, 1), (&[1], 0, 1)],
        vec![(&[2], 0, 1), (&[14], 0, 1), (&[9], 0, 1)],
        vec![(&[4, 5], 0, 1), (&[4, 5], 0, 1), (&[1, 9], 0, 1)],
        vec![(&[16], 0, 1), (&[16], 0, 1), (&[16], 1, 2), (&[16], 2, 2)],
        vec![(&[16], 0, 1), (&[16], 1, 1)],
        vec![(&[1, 9], 0, 1)],
        vec![(&[9], 0, 1), (&[9], 0, 1), (&[9], 0, 1)],
        vec![(&[0], 0, 1), (&[1], 0, 1), (&[10], 0, 1), (&[10], 0, 1)],
        vec![(&[11], 0, 1), (&[11], 0, 1), (&[9], 0, 1)],
        vec![(&[4, 5], 0, 1), (&[1], 0, 1), (&[9], 0, 1), (&[10], 0, 1)],
        vec![(&[3], 0, 1), (&[1, 9], 0, 1)],
        vec![(&[9], 0, 1), (&[0], 0, 1)],
        vec![(&[14], 0, 1), (&[9], 0, 1), (&[9], 0, 1)],
        vec![
            (&[19], 0, 1),
            (&[21, 9], 0, 1),
            (&[20], 0, 1),
            (&[21], 0, 1),
            (&[21], 0, 1),
            (&[21], 0, 1),
        ],
        vec![(&[21, 9], 0, 1), (&[19], 0, 1)],
        vec![(&[5], 0, 1), (&[5], 4, 1), (&[1, 9], 0, 1)],
        vec![(&[18], 0, 1), (&[9], 0, 1)],
        vec![(&[16, 9], 0, 1), (&[16], 1, 1), (&[9], 0, 2), (&[9], 0, 2)],
        vec![
            (&[23], 0, 1),
            (&[23], 0, 1),
            (&[22], 0, 1),
            (&[9], 0, 1),
            (&[24], 0, 1),
        ],
        vec![(&[1], 0, 1), (&[10], 0, 1), (&[8], 0, 1)],
        vec![(&[2], 0, 1), (&[9], 0, 1)],
        vec![(&[12], 0, 1)],
        vec![(&[6], 0, 1), (&[6], 3, 1)],
        vec![(&[7], 0, 1)],
        vec![(&[16], 0, 1), (&[16], 1, 1), (&[16], 2, 1), (&[16], 4, 1)],
        vec![(&[16], 2, 1)],
        vec![(&[0], 0, 1)],
        vec![(&[0, 1, 9], 0, 1)],
        vec![(&[15], 0, 1), (&[], 0, 0)], // edit detachment is checked separately
        vec![
            (&[0], 0, 1),
            (&[2], 0, 1),
            (&[14], 0, 1),
            (&[], 0, 0),
            (&[16], 0, 1),
            (&[10], 0, 1),
            (&[12], 0, 1),
            (&[16], 0, 1),
            (&[3], 0, 1),
            (&[4], 0, 1),
            (&[5], 0, 1),
            (&[1], 0, 1),
            (&[5], 0, 1),
            (&[17], 0, 1),
            (&[9], 0, 1),
        ],
    ]
}

fn fixture_source() -> String {
    use std::fmt::Write;
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../fixtures/markdown-source-map-41/vectors.json"
    ))
    .unwrap();
    let choices = selectors();
    assert_eq!(vectors["vectors"].as_array().unwrap().len(), choices.len());
    let mut source = include_str!("../fixtures/markdown-source-map-41/helpers.hir").to_owned();
    for (index, vector) in vectors["vectors"].as_array().unwrap().iter().enumerate() {
        writeln!(
            source,
            "fn cell_{index}() {{\nlet source = {}\nlet mut options = markdown.default_options()",
            serde_json::to_string(&vector["source"])
                .unwrap()
                .replace("\\u0000", "\\x00")
        )
        .unwrap();
        for (key, value) in vector["options"].as_object().unwrap() {
            writeln!(source, "options.{key} = {value}").unwrap();
        }
        source.push_str("let Ok(d) = bounded(source, options) else { failure.check(false, TextInvariant())\n return }\n");
        writeln!(
            source,
            "failure.check(markdown.source_length(d) == {}, TextInvariant())",
            vector["source"].as_str().unwrap().len()
        )
        .unwrap();
        assert_eq!(
            vector["checks"].as_array().unwrap().len(),
            choices[index].len()
        );
        for (check, (tags, field, count)) in vector["checks"]
            .as_array()
            .unwrap()
            .iter()
            .zip(&choices[index])
        {
            for tag in *tags {
                writeln!(
                    source,
                    "expect(d, {tag}, {field}, {}, {count})",
                    origin(check)
                )
                .unwrap();
            }
            if check["comparison_only"] == true {
                writeln!(source, "failure.check(text_extent(markdown.root(d), 15, 29) == Some(markdown.ByteRange(start: 15, end: 29)), TextInvariant())").unwrap();
            }
            if let Some(range) = check["range"].as_array() {
                for (endpoint, key) in [(0, "start_point"), (1, "end_point")] {
                    writeln!(source, "failure.check(markdown.point(d, {}) == Some(markdown.SourcePoint(line: {}, byte_column: {})), TextInvariant())", range[endpoint], check[key][0], check[key][1]).unwrap();
                }
            }
        }
        if let Some(points) = vector["points"].as_array() {
            for point in points {
                let expected = if point["point"].is_null() {
                    "None()".into()
                } else {
                    format!(
                        "Some(markdown.SourcePoint(line: {}, byte_column: {}))",
                        point["point"][0], point["point"][1]
                    )
                };
                writeln!(
                    source,
                    "failure.check(markdown.point(d, {}) == {expected}, TextInvariant())",
                    point["offset"]
                )
                .unwrap();
            }
        }
        source.push_str("equivalent(take d, source, options)\n}\n");
    }
    source.push_str("fn decoded() {\n");
    for (index, vector) in vectors["vectors"].as_array().unwrap().iter().enumerate() {
        if !vector["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c.get("decoded").is_some())
        {
            continue;
        }
        writeln!(
            source,
            "let mut options_{index} = markdown.default_options()"
        )
        .unwrap();
        for (key, value) in vector["options"].as_object().unwrap() {
            writeln!(source, "options_{index}.{key} = {value}").unwrap();
        }
        writeln!(source, "let Ok(d_{index}) = bounded({}, options_{index}) else {{ failure.check(false, TextInvariant())\n return }}", serde_json::to_string(&vector["source"]).unwrap().replace("\\u0000", "\\x00")).unwrap();
        for (number, (check, (tags, field, count))) in vector["checks"]
            .as_array()
            .unwrap()
            .iter()
            .zip(&choices[index])
            .enumerate()
        {
            let Some(decoded) = check.get("decoded") else {
                continue;
            };
            if check["comparison_only"] == true {
                writeln!(source, "let mut out_{index}_{number}: List[u8] = []\nappend_text_range(markdown.root(d_{index}), 15, 29, mut out_{index}_{number})\nfailure.check(text.finish_valid(take out_{index}_{number}) == {decoded}, TextInvariant())").unwrap();
            } else {
                for tag in *tags {
                    if *field == 0 && ![6, 9, 12].contains(tag) {
                        continue;
                    }
                    writeln!(source, "failure.check(count_decoded(markdown.root(d_{index}), {tag}, {field}, {}, {decoded}) == {count}, TextInvariant())", origin(check)).unwrap();
                }
            }
        }
    }
    source.push_str("}\n");
    source.push_str(BUDGETS);
    source.push_str("pub fn main(args: Args) -> Result[(), ()] {\nlet Ok(which) = arg.text(args, 0) else { return Error(()) }\nmatch which {\n");
    for index in 0..30 {
        writeln!(source, "\"{index}\" => cell_{index}(),").unwrap();
    }
    source.push_str("\"budgets\" => budgets(),\n\"edit\" => edit(),\n\"edges\" => edges(),\n\"metadata\" => metadata(),\n\"decoded\" => decoded(),\n\"whitespace\" => whitespace(),\n_ => failure.check(false, TextInvariant()),\n}\nOk(())\n}\n");
    source
}

const BUDGETS: &str = r#"
fn budget(work: u64) -> markdown.ParseBudget uses {} { markdown.ParseBudget(source_bytes: 16384, work_items: work, nesting: 128) }
fn budgets() {
  for (text, exact) in [("", 5u64), ("x", 16u64), ("x\r\ny", 28u64), ("\x00", 18u64)] {
    let options = markdown.default_options()
    for work in [exact - 1, exact, exact + 1] {
      let accepted = match markdown.parse_located_bounded(unique text, options, budget(work)) { Ok(_) => true, Error(WorkItems()) => false, _ => { failure.check(false, TextInvariant())
        false } }
      failure.check(accepted == (work >= exact), TextInvariant())
    }
  }
  let options = markdown.default_options()
  failure.check(match markdown.parse_located_bounded("", options, budget(4)) { Error(WorkItems()) => true, _ => false }, TextInvariant())
  failure.check(match markdown.parse_located_bounded("", options, budget(5)) { Ok(_) => true, _ => false }, TextInvariant())
  failure.check(match markdown.parse_bounded("x", options, budget(7)) { Ok(_) => true, _ => false }, TextInvariant())
  failure.check(match markdown.parse_located_bounded("x", options, budget(7)) { Error(WorkItems()) => true, _ => false }, TextInvariant())
  failure.check(match markdown.parse_located_bounded("x", options, markdown.ParseBudget(source_bytes: 0, work_items: 0, nesting: 0)) { Error(SourceBytes()) => true, _ => false }, TextInvariant())
  failure.check(match markdown.parse_located_bounded("", options, markdown.ParseBudget(source_bytes: 0, work_items: 5, nesting: 0)) { Error(Nesting()) => true, _ => false }, TextInvariant())
}
fn edit() {
  let Ok(d) = bounded("**old**\n", markdown.default_options()) else { failure.check(false, TextInvariant())
    return }
  let old_origin = {
    let Some(paragraph) = markdown.first_child(markdown.root(d)) else { return }
    let Some(strong) = markdown.first_child(paragraph) else { return }
    markdown.origin(strong)
  }
  let mut editable = markdown.into_node(take d)
  editable.children[0].children[0].kind = Text("new")
  failure.check(old_origin == Cover(markdown.ByteRange(start: 0, end: 7)), TextInvariant())
  failure.check(editable.children[0].children[0].kind == Text("new"), TextInvariant())
  let Ok(fresh) = bounded("x", markdown.default_options()) else { failure.check(false, TextInvariant())
    return }
  let handle = shared.freeze(take fresh)
  failure.check(shared.with(handle, fn(value: markdown.ParsedDocument) -> bool uses {} { markdown.source_length(value) == 1 }), TextInvariant())
  let Ok(thawed) = shared.thaw(take handle) else { failure.check(false, TextInvariant())
    return }
  let _ = markdown.into_node(take thawed)
}
fn whitespace() {
  let source = "\x0bx &amp;\x0c"
  let Ok(d) = bounded(source, markdown.default_options()) else { failure.check(false, TextInvariant())
    return }
  expect(d, 1, 0, Cover(markdown.ByteRange(start: 0, end: 9)), 1)
  expect(d, 9, 0, Cover(markdown.ByteRange(start: 3, end: 8)), 1)
  equivalent(take d, source, markdown.default_options())
}
fn metadata() {
  for (source, work, extension) in [("&amp;", 11u64, 0u8), ("[x]: /u\n\n[x]", 20u64, 0u8), ("a|b\n-|-\nc|d", 50u64, 1u8), ("[^a]: x\n\n[^a]", 32u64, 2u8)] {
    let mut options = markdown.default_options()
    options.tables = extension != 0
    options.footnotes = extension == 2
    failure.check(match markdown.parse_bounded(unique source, options, budget(work)) { Ok(_) => true, _ => false }, TextInvariant())
    failure.check(match markdown.parse_located_bounded(unique source, options, budget(work)) { Error(WorkItems()) => true, _ => false }, TextInvariant())
    let Ok(located) = bounded(source, options) else { failure.check(false, TextInvariant())
      return }
    equivalent(take located, source, options)
  }
}
fn edges() {
  let mut options = markdown.default_options()
  options.tables = true
  for (source, tag, a, b) in [("```\n\n", 6u8, 0u64, 4u64), ("<script>\n\n</script>\n", 7u8, 0u64, 19u64), ("```\nx\n```  \n", 6u8, 0u64, 9u64), ("[r]: /x\nbody\n---\n", 2u8, 8u64, 16u64), ("a | b\n- | -  \n", 19u8, 0u64, 11u64)] {
    let Ok(d) = bounded(source, options) else { failure.check(false, TextInvariant())
      return }
    expect(d, tag, 0, Cover(markdown.ByteRange(start: a, end: b)), 1)
    equivalent(take d, source, options)
  }
}
"#;

#[test]
fn markdown_source_map_41_lifetimes() {
    with_compiler_stack(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/markdown-source-map-41/helpers.hir");
        let setup = r#"let Ok(d) = markdown.parse_located_bounded("x", markdown.default_options(), markdown.ParseBudget(source_bytes: 1, work_items: 100, nesting: 4)) else { return }"#;
        let positive = format!(
            r#"pub fn main() {{ {setup}
          {{ let v = markdown.root(d)
             failure.check(markdown.origin(v) == Cover(markdown.ByteRange(start: 0, end: 1)), TextInvariant()) }}
          let handle = shared.freeze(take d)
          failure.check(shared.with(handle, fn(value: markdown.ParsedDocument) -> bool uses {{}} {{ markdown.source_length(value) == 1 }}), TextInvariant())
          let Ok(d2) = shared.thaw(take handle) else {{ return }}
          let _ = markdown.into_node(take d2)
        }}"#
        );
        let mut accepted = load_program_source(&path, Some(&positive)).unwrap();
        accepted
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let input = accepted.engine.export_native_input().unwrap();
        accepted
            .engine
            .import_native_input(input)
            .expect("independent V for positive local views/Shared");
        for (label, statement, rules) in [
            (
                "stale",
                "let v = markdown.root(d)\nlet _ = markdown.into_node(take d)\nlet _ = markdown.origin(v)",
                &["MODE-1", "MODE-2", "OWN-3"][..],
            ),
            (
                "owned-kind",
                "let payload = markdown.kind(markdown.root(d))\nlet _ = shared.freeze(take payload)",
                &["MEM-11", "MODE-2"][..],
            ),
            (
                "shared-strip",
                "let h = shared.freeze(take d)\nlet _ = shared.with(h, fn(value: markdown.ParsedDocument) -> unique markdown.Node uses {} { markdown.into_node(take value) })",
                &["OWN-1"][..],
            ),
            (
                "detached",
                "let editable = markdown.into_node(take d)\nlet _ = markdown.origin(editable)",
                &["LANG-7"][..],
            ),
        ] {
            let text = format!("pub fn main() {{ {setup}\n{statement}\n}}");
            let mut program = load_program_source(&path, Some(&text)).unwrap();
            let error = program.check_for_execution().expect_err(label);
            let ToolError::Query {
                error: hirsel_types::QueryError::Diagnostics(diagnostics),
                sources,
                ..
            } = error
            else {
                panic!("{error}")
            };
            for diagnostic in &diagnostics {
                eprintln!("{label}: {}", diagnostic.render_human(&sources));
            }
            assert!(
                diagnostics
                    .iter()
                    .any(|d| rules.contains(&d.rule.to_string().as_str())),
                "{label}: missing ownership/type refusal"
            );
        }
        let forged = format!(
            "pub fn main() {{ {setup}\n let fake = markdown.NodeView(nodes: [], rows: [], slot: 0)\n let _ = markdown.origin(fake)\n }}"
        );
        let mut program = load_program_source(&path, Some(&forged)).unwrap();
        let error = program
            .check_for_execution()
            .expect_err("public node ID constructor escaped opacity");
        let ToolError::Query {
            error: hirsel_types::QueryError::Diagnostics(diagnostics),
            sources,
            ..
        } = error
        else {
            panic!("{error}")
        };
        assert!(
            diagnostics.iter().any(|d| sources
                .slice(d.primary)
                .is_some_and(|text| text.contains("NodeView"))),
            "missing refusal at forged opaque constructor"
        );
        for diagnostic in &diagnostics {
            eprintln!("forged-view: {}", diagnostic.render_human(&sources));
        }
    });
}
