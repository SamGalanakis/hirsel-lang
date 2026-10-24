//! Exact cumulative parser admission and unchanged CommonMark AST/rendering.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::Path;

const PARTS: &[&str] = &[
    "literal",
    "syntax",
    "table",
    "footnote",
    "malformed",
    "flat",
    "emphasis",
    "quotes",
];

fn parts(variable: &str) -> Vec<String> {
    let chosen = std::env::var(variable).unwrap_or_else(|_| PARTS.join(","));
    let parts: Vec<_> = chosen.split(',').map(str::to_owned).collect();
    assert!(
        !parts.is_empty() && parts.iter().all(|part| PARTS.contains(&part.as_str())),
        "invalid part selector: {chosen}"
    );
    parts
}

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
    eprintln!("MARKDOWN_39_SEAL {receipt}");
}

fn run(engine: &str) {
    with_compiler_stack(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/markdown-parse-budget-39.hir");
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
        eprintln!("MARKDOWN_39_STARTED vectors/{engine}");
        if engine == "reference" || engine == "checked-h" {
            let config = RunConfiguration {
                entry: program.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            for part in parts("MARKDOWN_39_HOST_PARTS") {
                eprintln!("MARKDOWN_39_STARTED {part}/{engine}");
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
                let observation = execution.observation.unwrap();
                assert_eq!(observation.exit_code, Some(0));
                assert!(
                    observation.failures.is_empty() && observation.output.is_empty(),
                    "{observation:?}"
                );
                eprintln!("MARKDOWN_39_PASS {part}/{engine}");
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
                .join("markdown-parse-budget-39")
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
            for part in parts("MARKDOWN_39_NATIVE_PARTS") {
                eprintln!("MARKDOWN_39_STARTED {part}/{engine}");
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
                eprintln!("MARKDOWN_39_PASS {part}/{engine}");
            }
        }
        eprintln!("MARKDOWN_39_PASS vectors/{engine}");
    });
}

#[test]
fn markdown_parse_budget_39_reference() {
    run("reference");
}
#[test]
fn markdown_parse_budget_39_checked_h() {
    run("checked-h");
}
#[test]
fn markdown_parse_budget_39_llvm() {
    run("llvm");
}
#[test]
fn markdown_parse_budget_39_cranelift() {
    run("cranelift");
}
