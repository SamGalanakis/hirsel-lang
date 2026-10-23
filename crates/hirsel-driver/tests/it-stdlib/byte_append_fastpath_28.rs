//! Rust Vec append-elements structure and fixed extend_from_slice vector.
//! f6c1107f8be1, alloc/src/vec/mod.rs:2995-3031, alloctests/tests/vec.rs:169-180.
//! MIT/Apache-2.0; adapted to borrowed bytes, COW and MEM-6 charging.
use hirsel_driver::*;
use hirsel_engine::*;

fn details(e: &ToolError) -> String {
    match e {
        ToolError::Query {
            error: hirsel_types::QueryError::Diagnostics(diagnostics),
            sources,
            ..
        } => diagnostics
            .iter()
            .map(|d| d.render_human(sources))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => e.to_string(),
    }
}

fn parity(label: &str, body: &str, rule: Option<&str>) {
    with_compiler_stack(|| {
        let source = format!("pub fn main(t: Test) -> Result[(), ()] {{\n{body}\nOk(())\n}}\n");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/std_source/main.hir");
        let mut program = load_program_source(&path, Some(&source))
            .unwrap_or_else(|e| panic!("{label}: {}\n{source}", details(&e)));
        program
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{label}: {}\n{source}", details(&e)));
        let config = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let recorded =
            run_observed_snapshot(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        let observation = recorded.observation.unwrap();
        match rule {
            None => assert!(observation.failures.is_empty(), "{label}: {observation:?}"),
            Some(rule) => assert_eq!(
                observation.failures.first().map(|f| f.rule.clone()),
                Some(hirsel_diag::RuleId::new(rule).unwrap()),
                "{label}: {observation:?}"
            ),
        }
        let checked =
            run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        assert_eq!(
            checked.observation.as_ref(),
            Some(&observation),
            "{label}: checked"
        );
        let events = observation
            .effects
            .iter()
            .filter_map(|e| match &e.effect {
                hirsel_interp::observation::Effect::CapabilityConsumed(e) => Some(e.clone()),
                _ => None,
            })
            .collect();
        let replayed = run_observed_snapshot(
            &mut program.engine,
            &RunConfiguration {
                inputs: events,
                ..config
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert_eq!(
            replayed.observation.as_ref(),
            Some(&observation),
            "{label}: replay"
        );
        let expected = native::Observation::of_reference(&observation);
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Heap { campaign: true },
            },
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::None,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Default,
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
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("byte-append-fastpath-28")
            .join(label);
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
        assert_eq!(outcomes.len(), 4);
        for (name, result) in outcomes {
            eprintln!("{label}: {name}: {result:?}");
            assert_eq!(
                result,
                native::Outcome::Agree,
                "{label}: {name}: {result:?}"
            );
        }
    });
}

#[test]
fn room_exact_full_growth_snapshot_alias_and_callback() {
    let mut body = String::new();
    for reserve in [0, 4, 5, 10, 64] {
        body += &format!(
            r#"{{
let mut b: List[u8] = []
list.reserve(mut b, {reserve})
bytes.append(mut b, b"\x01\x02\x03\x04\x05")
bytes.append(mut b, b"\x06\x07\x08\x09\x00")
test.check(t, bytes.from_list(copy b) == b"\x01\x02\x03\x04\x05\x06\x07\x08\x09\x00")
let old = copy b
bytes.append(mut b, b"")
let source = bytes.from_list(copy b)
bytes.append(mut b, bytes.slice(source, 2, 7))
test.check(t, list.len(b) == 15 && b[10] == 3u8 && b[14] == 7u8 && list.len(old) == 10 && bytes.len(source) == 10)
}}
"#
        );
    }
    body += r#"
let mut immortal: List[u8] = []
bytes.append(mut immortal, b"")
text.append(mut immortal, "abc")
let held: List[u8] = [97u8,98u8,99u8]
let shared = copy held
let install = fn(mut b: List[u8], take source: List[u8]) { b = take source }
install(mut immortal, take shared)
bytes.append(mut immortal, b"d")
test.check(t, held == [97u8,98u8,99u8] && immortal == [97u8,98u8,99u8,100u8])
let mut history: List[u8] = [97u8,98u8]
list.reserve(mut history, 64)
bytes.append_from_history(mut history, 2, 7)
test.check(t, bytes.from_list(take history) == b"ababababa")
"#;
    parity("heap", &body, None);
}

#[test]
fn bounded_exact_budget_exhaustion_and_zero_noop() {
    let mut body = String::new();
    for (cap, shared, count, expected) in [
        (3, false, 0, "Ok(3u64)"),
        (4, false, 1, "Ok(4u64)"),
        (3, false, 1, "Error(OutOfSpace())"),
        (3, true, 0, "Ok(3u64)"),
        (7, true, 1, "Ok(4u64)"),
        (6, true, 1, "Error(OutOfSpace())"),
    ] {
        let share = if shared { "let old = copy b" } else { "" };
        let check = if shared {
            "failure.check(old == [97u8,98u8,99u8], TextInvariant())"
        } else {
            ""
        };
        let data = if count == 0 { "" } else { "x" };
        body += &format!(
            r#"{{
let outcome = arena.bounded({cap}, fn(a: Arena) -> u64 {{
  let mut b: List[u8] in a = [97u8,98u8,99u8]
  list.reserve(mut b, 64)
  {share}
  bytes.append(mut b, b{data:?})
  {check}
  list.len(b)
}})
test.check(t, outcome == {expected})
}}
"#
        );
    }
    parity("arena", &body, None);
}

#[test]
fn failure_before_and_after_publication() {
    parity(
        "source-failure",
        r#"
let mut b: List[u8] = [97u8]
bytes.append(mut b, bytes.slice(b"xy", 0, 3))
"#,
        Some("SEM-3"),
    );
    parity(
        "after-publication",
        r#"
let mut b: List[u8] = []
list.reserve(mut b, 64)
bytes.append(mut b, b"abc")
test.check(t, list.len(b) == 3)
failure.check(false, Arithmetic())
"#,
        Some("SEM-1"),
    );
}
