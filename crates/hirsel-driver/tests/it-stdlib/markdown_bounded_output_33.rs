//! cmark-gfm single-walker port with fallible output admission (BSD-2-Clause).
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::Path;

fn run(engine: &str) {
    with_compiler_stack(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/markdown-bounded-output-33.hir");
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
        eprintln!("MARKDOWN_33_STARTED vectors/{engine}");
        if engine == "reference" || engine == "checked-h" {
            let config = RunConfiguration {
                entry: program.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
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
                "{execution:?}"
            );
            let observation = execution.observation.unwrap();
            assert_eq!(observation.exit_code, Some(0));
            assert!(
                observation.failures.is_empty() && observation.output.is_empty(),
                "{observation:?}"
            );
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
                .join("markdown-bounded-output-33")
                .join(engine);
            std::fs::create_dir_all(&dir).unwrap();
            let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &matrix).unwrap();
            let outcomes = native::differential_in(
                &validated,
                &program.entry,
                &expected,
                &matrix,
                &tools,
                &dir,
                &native::Fixture::default(),
            );
            assert_eq!(outcomes.len(), 1);
            for (name, result) in outcomes {
                assert_eq!(result, native::Outcome::Agree, "{name}: {result:?}");
            }
        }
        eprintln!("MARKDOWN_33_PASS vectors/{engine}");
    });
}

#[test]
fn markdown_bounded_output_33_llvm() {
    run("llvm");
}
#[test]
fn markdown_bounded_output_33_cranelift() {
    run("cranelift");
}

fn run_part(fixture: &str, part: &str, checked: bool) {
    with_compiler_stack(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        let mut program = load_program(&path).unwrap();
        program.check_for_execution().unwrap();
        let config = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let simulation = SimulationConfig {
            args: vec![part.into()],
            ..Default::default()
        };
        let engine = if checked { "checked-h" } else { "reference" };
        eprintln!("MARKDOWN_33_STARTED part/{part}/{engine}");
        let execution = if checked {
            run_observed_checked(&mut program.engine, &config, simulation)
        } else {
            run_observed_snapshot(&mut program.engine, &config, simulation)
        }
        .unwrap();
        assert!(
            matches!(
                execution.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{part}/{engine}: {execution:?}"
        );
        let observation = execution.observation.unwrap();
        assert!(
            observation.failures.is_empty() && observation.output.is_empty(),
            "{part}/{engine}: {observation:?}"
        );
        eprintln!("MARKDOWN_33_PASS part/{part}/{engine}");
    });
}
macro_rules! part {
    ($name:ident, $part:literal, $checked:literal) => {
        #[test]
        fn $name() {
            run_part("markdown-bounded-output-33-parts.hir", $part, $checked);
        }
    };
}
part!(
    markdown_bounded_output_33_part_text_reference,
    "text",
    false
);
part!(markdown_bounded_output_33_part_text_checked_h, "text", true);
part!(
    markdown_bounded_output_33_part_code_reference,
    "code",
    false
);
part!(markdown_bounded_output_33_part_code_checked_h, "code", true);
part!(
    markdown_bounded_output_33_part_heading_reference,
    "heading",
    false
);
part!(
    markdown_bounded_output_33_part_heading_checked_h,
    "heading",
    true
);
part!(
    markdown_bounded_output_33_part_link_reference,
    "link",
    false
);
part!(markdown_bounded_output_33_part_link_checked_h, "link", true);
part!(
    markdown_bounded_output_33_part_image_reference,
    "image",
    false
);
part!(
    markdown_bounded_output_33_part_image_checked_h,
    "image",
    true
);
part!(markdown_bounded_output_33_part_raw_reference, "raw", false);
part!(markdown_bounded_output_33_part_raw_checked_h, "raw", true);
part!(
    markdown_bounded_output_33_part_table_reference,
    "table",
    false
);
part!(
    markdown_bounded_output_33_part_table_checked_h,
    "table",
    true
);
part!(
    markdown_bounded_output_33_part_footnote_reference,
    "footnote",
    false
);
part!(
    markdown_bounded_output_33_part_footnote_checked_h,
    "footnote",
    true
);
part!(
    markdown_bounded_output_33_part_sink_reference,
    "sink",
    false
);
part!(markdown_bounded_output_33_part_sink_checked_h, "sink", true);
part!(
    markdown_bounded_output_33_part_cleanup_reference,
    "cleanup",
    false
);
part!(
    markdown_bounded_output_33_part_cleanup_checked_h,
    "cleanup",
    true
);

macro_rules! parsed_part {
    ($name:ident, $part:literal, $checked:literal) => {
        #[test]
        fn $name() {
            run_part(
                "markdown-bounded-output-33-parsed-parts.hir",
                $part,
                $checked,
            );
        }
    };
}
parsed_part!(
    markdown_bounded_output_33_parsed_text_reference,
    "text",
    false
);
parsed_part!(
    markdown_bounded_output_33_parsed_text_checked_h,
    "text",
    true
);
parsed_part!(
    markdown_bounded_output_33_parsed_link_reference,
    "link",
    false
);
parsed_part!(
    markdown_bounded_output_33_parsed_link_checked_h,
    "link",
    true
);
parsed_part!(
    markdown_bounded_output_33_parsed_image_reference,
    "image",
    false
);
parsed_part!(
    markdown_bounded_output_33_parsed_image_checked_h,
    "image",
    true
);
parsed_part!(
    markdown_bounded_output_33_parsed_html_reference,
    "html",
    false
);
parsed_part!(
    markdown_bounded_output_33_parsed_html_checked_h,
    "html",
    true
);
parsed_part!(
    markdown_bounded_output_33_parsed_blocked_reference,
    "blocked",
    false
);
parsed_part!(
    markdown_bounded_output_33_parsed_blocked_checked_h,
    "blocked",
    true
);
parsed_part!(
    markdown_bounded_output_33_parsed_raw_reference,
    "raw",
    false
);
parsed_part!(markdown_bounded_output_33_parsed_raw_checked_h, "raw", true);
parsed_part!(
    markdown_bounded_output_33_parsed_table_reference,
    "table",
    false
);
parsed_part!(
    markdown_bounded_output_33_parsed_table_checked_h,
    "table",
    true
);
parsed_part!(
    markdown_bounded_output_33_parsed_emphasis_reference,
    "emphasis",
    false
);
parsed_part!(
    markdown_bounded_output_33_parsed_emphasis_checked_h,
    "emphasis",
    true
);
parsed_part!(
    markdown_bounded_output_33_parsed_subnodes_reference,
    "subnodes",
    false
);
parsed_part!(
    markdown_bounded_output_33_parsed_subnodes_checked_h,
    "subnodes",
    true
);
parsed_part!(
    markdown_bounded_output_33_parsed_footer_reference,
    "footer",
    false
);
parsed_part!(
    markdown_bounded_output_33_parsed_footer_checked_h,
    "footer",
    true
);
parsed_part!(
    markdown_bounded_output_33_parsed_tail_reference,
    "tail",
    false
);
parsed_part!(
    markdown_bounded_output_33_parsed_tail_checked_h,
    "tail",
    true
);
parsed_part!(
    markdown_bounded_output_33_parsed_sink_reference,
    "sink",
    false
);
parsed_part!(
    markdown_bounded_output_33_parsed_sink_checked_h,
    "sink",
    true
);

part!(
    markdown_bounded_output_33_part_literal_sink_reference,
    "literal-sink",
    false
);
part!(
    markdown_bounded_output_33_part_literal_sink_checked_h,
    "literal-sink",
    true
);
