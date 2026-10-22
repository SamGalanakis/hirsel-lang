//! Pinned Go x/net lexical vectors and explicit bounded-input controls.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

#[test]
fn html_tokenizer_15_vectors_on_h_llvm_and_cranelift() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/html-tokenizer-15/vectors.hir");
        let mut loaded = load_program(&path).unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(100_000_000),
                call_depth: 256,
            },
            ..Default::default()
        }));
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                reference.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            reference.outcome
        );
        eprintln!("html-tokenizer-15: H completed 369 lexical cases and cap controls");
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let configs = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
            native::Configuration {
                options,
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
        ];
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("html-tokenizer-15");
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        eprintln!("html-tokenizer-15: LLVM and Cranelift execution");
        for (config, outcome) in
            native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
        {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{config}: {outcome:?}"
            );
            eprintln!("html-tokenizer-15: {config} agreed");
        }
        eprintln!(
            "html-tokenizer-15: H, LLVM and Cranelift completed all 369 lexical cases and cap controls"
        );
    });
}

// Each bounded source gets a fresh audited machine; exact input/value/span
// lines are preserved. Whole case 251 remains an independent acceptance test.
fn full_entity_case(source: &str) -> &str {
    let lines: Vec<_> = source.lines().collect();
    let matches: Vec<_> = lines
        .windows(2)
        .filter(|pair| pair[0] == "  // 251: all 2231 named references")
        .collect();
    assert_eq!(matches.len(), 1, "expected the unique pinned case 251");
    let line = matches[0][1];
    let cases: Vec<_> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("  check("))
        .collect();
    assert_eq!(cases.len(), 369);
    assert_eq!(cases[251], line, "full entity fixture identity changed");
    line
}

fn memory(label: &str) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let metrics: Vec<_> = status
        .lines()
        .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
        .collect();
    eprintln!("html-tokenizer-15: {label}: {}", metrics.join("; "));
}

fn heap_source(source: &str, label: &str) -> native::Observation {
    memory(&format!("before {label}"));
    let observation = with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/html-tokenizer-15/vectors.hir");
        let mut loaded = load_program_source(&path, Some(source)).unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let ready = native::build(&validated, &options).unwrap();
        let heap_result = native::run_heap_in(
            &ready,
            &loaded.entry,
            &native::Sites::new(validated.snapshot()),
            &native::HeapWorld {
                steps: Some(100_000_000),
                ..Default::default()
            },
        );
        let heap = heap_result.unwrap();
        assert_eq!(heap.counters.live_heap_bytes, 0, "{label}");
        assert!(
            heap.observation.failures.is_empty(),
            "{label}: {:?}",
            heap.observation
        );
        assert!(
            !heap.observation.blocked
                && heap.observation.budget.is_none()
                && heap.observation.signaled.is_none(),
            "{label}: {:?}",
            heap.observation
        );
        assert!(heap.observation.output.is_empty(), "{label}");
        heap.observation
    });
    memory(&format!("after {label}"));
    observation
}

#[test]
fn html_tokenizer_15_heap_remaining_batches() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/html-tokenizer-15/vectors.hir");
    let source = std::fs::read_to_string(&path).unwrap();
    let prefix = source.split("pub fn main(").next().unwrap();
    let cases: Vec<_> = source
        .lines()
        .filter(|line| line.starts_with("  check("))
        .collect();
    assert_eq!(cases.len(), 369);
    let entity_line = full_entity_case(&source);
    let whole_entity = cases.iter().position(|line| *line == entity_line).unwrap();
    // Preserve the full 2231-reference input, in its own ledger. Never replace
    // it with smaller inputs or infer its result from individual references.
    let main = |checks: &str| {
        format!(
            "{prefix}pub fn main(rt: Io, args: Args) -> Result[(), ()] {{\n{checks}\n  Ok(())\n}}\n"
        )
    };
    eprintln!(
        "html-tokenizer-15: scope is 368 remaining lexical cases; full case {whole_entity} is independently required and currently resource-red"
    );
    let mut executed = 0;
    for start in (0..cases.len()).step_by(16) {
        let end = (start + 16).min(cases.len());
        let checks: Vec<_> = (start..end)
            .filter(|i| *i != whole_entity)
            .map(|i| cases[i])
            .collect();
        if checks.is_empty() {
            continue;
        }
        let result = heap_source(
            &main(&checks.join("\n")),
            &format!("cases {start}..{end} (entity case excluded if present)"),
        );
        assert_eq!(result.exit_code, Some(0));
        executed += checks.len();
    }
    assert_eq!(executed, 368);
    let caps = source.split("fn caps() -> () uses {} {").nth(1).unwrap();
    let (basic, remaining) = caps.split_once("  // Every work cutoff").unwrap();
    let (sweep, rest) = remaining.split_once("  // Switch mode").unwrap();
    let basic_source = format!(
        "{prefix}fn caps() -> () uses {{}} {{{basic}\n  // Switch mode{rest}\npub fn main(rt: Io, args: Args) -> Result[(), ()] {{ caps(); Ok(()) }}\n"
    );
    assert_eq!(
        heap_source(
            &basic_source,
            "all fixed cap boundaries, CDATA switch and encoding control"
        )
        .exit_code,
        Some(0)
    );
    // Keep the exact same sweep body as H/native, selecting one source and a
    // bounded work interval per fresh ledger. Stop at its first success just
    // as the original sweep does, so every executed cutoff is accounted for.
    let sweep = &sweep[sweep.find("  let sources:").unwrap()..];
    let sweep = sweep
        .replace(
            "  for source in sources {",
            "  {\n    let source = sources[index]",
        )
        .replace("let mut work: u64 = 0", "let mut work: u64 = first")
        .replace("while work < 2000", "while work < last")
        .replace(
            "    core@1/failure.check(succeeded, TextInvariant())",
            "    return succeeded",
        );
    let mut cutoffs = 0;
    for index in 0..5 {
        let mut complete = false;
        for first in (0..2000).step_by(16) {
            let last = (first + 16).min(2000);
            let small = format!(
                "{prefix}fn sweep(index: u64, first: u64, last: u64) -> bool uses {{}} {{\n{sweep}\n}}\npub fn main(rt: Io, args: Args) -> Result[(), ()] {{ if sweep({index}, {first}, {last}) {{ Ok(()) }} else {{ Error(()) }} }}\n"
            );
            let result = heap_source(&small, &format!("cap source {index}, work {first}..{last}"));
            assert!(matches!(result.exit_code, Some(0 | 1)));
            cutoffs += last - first;
            if result.exit_code == Some(0) {
                complete = true;
                break;
            }
        }
        assert!(
            complete,
            "cap source {index}: no successful cutoff within original 2000 bound"
        );
    }
    eprintln!(
        "html-tokenizer-15: heap completed {executed} remaining exact lexical cases (case251 separate), fixed cap boundaries and all cutoffs through first success for each of 5 sources ({cutoffs} selected work slots; final batches stop at success); every ledger has zero live bytes"
    );
}

#[test]
fn html_tokenizer_15_heap_full_entity_input() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/html-tokenizer-15/vectors.hir");
    let source = std::fs::read_to_string(&path).unwrap();
    let prefix = source.split("pub fn main(").next().unwrap();
    let entity_line = full_entity_case(&source);
    let small = format!(
        "{prefix}pub fn main(rt: Io, args: Args) -> Result[(), ()] {{\n{entity_line}\n Ok(())\n}}\n"
    );
    assert_eq!(
        heap_source(&small, "case251 unchanged whole2231-reference input").exit_code,
        Some(0)
    );
}
