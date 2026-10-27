//! Exact cumulative parser admission and unchanged CommonMark AST/rendering.
use crate::source_std::load_program_source;
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::Path;

fn seal_native(dir: &Path, tools: &native::Toolchain, engine: &str, part: &str) {
    use sha2::{Digest, Sha256};
    fn tree(path: &Path, files: &mut std::collections::BTreeMap<String, String>) {
        if path.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                tree(&entry.unwrap().path(), files);
            }
        } else if path.is_file() {
            let bytes = std::fs::read(path).unwrap();
            files.insert(
                path.display().to_string(),
                format!("{:x}", Sha256::digest(bytes)),
            );
        }
    }
    let mut files = std::collections::BTreeMap::new();
    for path in [dir, &tools.cache, &tools.lld, &tools.kit] {
        tree(path, &mut files);
    }
    for path in tools.runtime.values() {
        tree(path, &mut files);
    }
    if let Some(path) = std::env::var_os("HIRSEL_IMAGE") {
        tree(Path::new(&path), &mut files);
    }
    let receipt = serde_json::json!({"timing":"before native launch", "engine":engine, "part":part, "files":files});
    std::fs::write(dir.join("inputs-before-launch.json"), receipt.to_string()).unwrap();
    eprintln!("MARKDOWN_40_SEAL {receipt}");
}

fn run(engine: &str) {
    with_compiler_stack(|| {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/markdown-owned-ast-40.hir");
        if engine == "reference" || engine == "checked-h" {
            let options = "markdown.default_options()";
            let extensions = "{ let mut o = markdown.default_options()\n o.tables = true\n o.footnotes = true\n o }";
            let mut cells = vec![];
            for (index, source) in [
                "",
                "x",
                "a\r\nb\rc",
                "é🐑",
                "> - repeated\n> - repeated",
                "[x][r] [x][r] ![x][r]\n\n[r]: /same \"title\"",
                "```same\nsame\n```\n\n`same`\n\n<div>same</div>\n\nx <i>same</i>",
            ]
            .iter()
            .enumerate()
            {
                cells.push((
                    format!("receipt-{index}"),
                    format!(
                        "receipt({}, {options})",
                        serde_json::to_string(source).unwrap()
                    ),
                ));
            }
            cells.push((
                "receipt-extensions".into(),
                format!(
                    "receipt(\"a|a\\n-|-\\na|a\\n\\n[^a]: same\\n\\n[^a] [^a]\", {extensions})"
                ),
            ));
            for (index, (source, work, nesting, extended)) in [
                ("", 2, 1, false),
                ("x", 7, 3, false),
                ("a\n\nb", 13, 3, false),
                ("\0", 9, 3, false),
                ("*x*", 36, 4, false),
                ("> - x", 19, 6, false),
                ("[x]: /u\n\n[x]", 20, 4, false),
                ("a|b\n-|-\nc|d", 50, 5, true),
                ("[^a]: x\n\n[^a]", 32, 4, true),
            ]
            .iter()
            .enumerate()
            {
                let options = if *extended { extensions } else { options };
                // Hirsel JSON string literals use \x00 for NUL rather than JSON's \u0000.
                let source = serde_json::to_string(source)
                    .unwrap()
                    .replace("\\u0000", "\\x00");
                cells.push((
                    format!("budget-{index}"),
                    format!("exact({source}, {options}, {work}, {nesting})"),
                ));
            }
            cells.push(("repeated-reference".into(), r#"let mut tree = markdown.parse_owned("[x][r] [x][r]\n\n[r]: /same \"title\"", markdown.default_options())
              tree.children[0].children[0].kind = Link("/changed", "new")
              failure.check(tree.children[0].children[2].kind == Link("/same", "title"), TextInvariant())"#.into()));
            let helpers = include_str!("../fixtures/markdown-owned-ast-40.hir")
                .replace("pub fn main()", "fn native_all()");
            for (part, body) in cells {
                let source =
                    format!("{helpers}\npub fn main() -> Result[(), ()] {{\n{body}\nOk(())\n}}\n");
                let mut program = load_program_source(&path, Some(&source)).unwrap();
                program
                    .check_for_execution()
                    .unwrap_or_else(|e| panic!("{part}: {e}"));
                let input = program.engine.export_native_input().unwrap();
                program
                    .engine
                    .import_native_input(input)
                    .expect("independent V import");
                let config = RunConfiguration {
                    entry: program.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                };
                eprintln!("MARKDOWN_40_STARTED {part}/{engine}");
                let execution = if engine == "reference" {
                    run_observed_snapshot(&mut program.engine, &config, SimulationConfig::default())
                } else {
                    run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
                }
                .unwrap();
                assert!(
                    matches!(
                        execution.outcome,
                        Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                    ),
                    "{part}: {execution:?}"
                );
                let observation = execution.observation.unwrap();
                assert_eq!(observation.exit_code, Some(0));
                assert!(
                    observation.failures.is_empty() && observation.output.is_empty(),
                    "{part}: {observation:?}"
                );
                eprintln!(
                    "MARKDOWN_40_PASS {part}/{engine} {:?}",
                    execution.step_usage
                );
            }
        } else {
            let mut program = load_program(&path).unwrap();
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
            eprintln!("MARKDOWN_40_STARTED vectors/{engine}");
            let input = program.engine.export_native_input().unwrap();
            program
                .engine
                .import_native_input(input)
                .expect("independent V import");
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
                .join("markdown-owned-ast-40")
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
            for part in ["ownership"] {
                eprintln!("MARKDOWN_40_STARTED {part}/{engine}");
                let case_dir = dir.join(&part);
                std::fs::create_dir_all(&case_dir).unwrap();
                let fixture = native::Fixture {
                    args: vec![part.to_owned()],
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
                seal_native(&case_dir, &tools, engine, &part);
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
                eprintln!("MARKDOWN_40_PASS {part}/{engine}");
            }
        }
        eprintln!("MARKDOWN_40_PASS vectors/{engine}");
    });
}

#[test]
fn markdown_owned_ast_40_reference() {
    run("reference");
}
#[test]
fn markdown_owned_ast_40_checked_h() {
    run("checked-h");
}
#[test]
fn markdown_owned_ast_40_llvm() {
    run("llvm");
}
#[test]
fn markdown_owned_ast_40_cranelift() {
    run("cranelift");
}

#[test]
fn markdown_owned_ast_40_source_negatives() {
    with_compiler_stack(|| {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/markdown-owned-ast-40.hir");
        let positive = r#"pub fn main() {
          let tree = markdown.parse_owned("x", markdown.default_options())
          let _ = shared.freeze(take tree)
        }"#;
        let mut accepted = load_program_source(&path, Some(positive)).unwrap();
        accepted
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        for statement in [
            r#"let tree = markdown.parse("x", markdown.default_options())"#,
            r#"let original = markdown.parse_owned("x", markdown.default_options())
                let tree = copy original"#,
            r#"let root = shared.freeze(markdown.parse_owned("x", markdown.default_options()))
                let tree = shared.with(root, fn(value: markdown.Node) -> markdown.Node uses {} { copy value })"#,
        ] {
            let source =
                format!("pub fn main() {{ {statement}\n let _ = shared.freeze(take tree)\n }}");
            let mut p = load_program_source(&path, Some(&source)).unwrap();
            let error = p
                .check_for_execution()
                .expect_err("unqualified/copy/shared receipt became unique");
            let ToolError::Query {
                error: hirsel_types::QueryError::Diagnostics(diagnostics),
                sources,
                ..
            } = error
            else {
                panic!("not an ownership diagnostic: {error}")
            };
            for d in &diagnostics {
                eprintln!("{}", d.render_human(&sources));
            }
            assert!(
                diagnostics.iter().any(|d| d.rule.to_string() == "MEM-11"
                    && d.message.contains("unique")
                    && sources
                        .slice(d.primary)
                        .is_some_and(|text| text.contains("tree"))),
                "missing causal MEM-11 at freeze(take tree)"
            );
        }
    });
}

#[test]
fn markdown_owned_ast_40_independent_v() {
    with_compiler_stack(|| {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/markdown-owned-ast-40.hir");
        let mut p = load_program(&path).unwrap();
        p.check_for_execution().unwrap_or_else(|e| panic!("{e}"));
        let input = p.engine.export_native_input().unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let mut seen = 0;
        for d in input.definitions.values().filter(|d| {
            [
                "parse_owned",
                "parse_bounded_owned",
                "parse_shared",
                "literal_document",
                "build_node",
            ]
            .contains(&d.path.path().0.name.as_str())
        }) {
            hirsel_validate::assert_valid(d, &input.types, &signatures);
            seen += 1;
        }
        assert_eq!(seen, 5);
        let make = input
            .definitions
            .values()
            .find(|d| d.path.path().0.name == "make")
            .unwrap();
        hirsel_validate::assert_valid(make, &input.types, &signatures);
        // Inspect the final-return plumbing: construction payload copies remain,
        // but no producer helper copies a completed Node.
        for d in input.definitions.values().filter(|d| {
            [
                "parse_owned",
                "parse_bounded_owned",
                "parse_shared",
                "literal_document",
                "build_node",
            ]
            .contains(&d.path.path().0.name.as_str())
        }) {
            for body in &d.bodies {
                for value in &body.rvalues {
                    if let hirsel_tir::Rvalue::Use(operand) = value {
                        if operand.usage == hirsel_types::UseKind::Copy {
                            let ty = input.types.row(body.places[operand.place.0 as usize].ty);
                            assert!(
                                !matches!(&ty.kind, hirsel_types::TypeKind::Named { definition, .. } if definition.path().0.name == "Node"),
                                "completed Node copy in {}",
                                d.path.path()
                            );
                        }
                    }
                }
            }
            eprintln!("MARKDOWN_40_IR_NO_FINAL_NODE_COPY {}", d.path.path());
        }
        let mut wrong = (**make).clone();
        let mut changed = vec![];
        for (body_index, body) in wrong.bodies.iter_mut().enumerate() {
            for (block_index, block) in body.blocks.iter_mut().enumerate() {
                if let hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Definition { path, .. },
                    arguments,
                    ..
                } = &mut block.terminator
                {
                    if path.0.name == "parse_bounded_owned" {
                        arguments[0].value.usage = hirsel_types::UseKind::Copy;
                        changed.push((
                            hirsel_tir::BodyId(body_index as u32),
                            hirsel_tir::BlockId(block_index as u32),
                        ));
                    }
                }
            }
        }
        assert_eq!(changed.len(), 1);
        let report = hirsel_validate::validate(&wrong, &input.types, &signatures, &mut vec![]);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.body == Some(changed[0].0)
                    && v.block == Some(changed[0].1)
                    && matches!(
                        v.kind,
                        hirsel_validate::ViolationKind::TypeMismatch {
                            entity: "argument use"
                        }
                    )),
            "forged take accepted: {report:?}"
        );
        eprintln!("MARKDOWN_40_V_REJECTED {:?}", report.violations);
    });
}
