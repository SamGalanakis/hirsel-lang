//! Stop gate for the isolated row-text experiment: ordinary indexed carriers
//! must contain independent MEM-12 owners without an importer extension.
use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::{
    QueryEngine, RunConfiguration, SimulationConfig, check_progress, default_context,
    default_environment, entry_path, run_observed_snapshot, with_compiler_stack,
};

// The historical helper fixture stays byte-for-byte intact. Only direct field
// projections covered by 063cbde7 change in this current-main counterpart;
// sequential tag views still use the ordinary String helper.
fn projected_source() -> String {
    include_str!("../fixtures/row-text.hir")
        .replace("own_text(row.text.title)", "unique row.text.title")
        .replace(
            "own_text(row.text.description)",
            "unique row.text.description",
        )
        .replace(
            "pub fn main(",
            r#"fn removals_to_empty() -> bool uses {} {
  let mut rows = direct()
  if !remove(mut rows, 1) || rows.rows[0].id != 2 { return false }
  if remove(mut rows, 1) { return false }
  if !remove(mut rows, 2) || !remove(mut rows, 3) { return false }
  list.len(rows.rows) == 0 && !remove(mut rows, 0)
}
pub fn main("#,
        )
        .replace("retained(false)", "retained(false) && removals_to_empty()")
}

const SOURCE: &str = include_str!("../fixtures/row-text-shape.hir");

#[test]
fn ordinary_indexed_rows_with_nested_text_owners() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("row-text.hir", SOURCE.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&default_context());
        let observed = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let reference = native::Observation::of_reference(
            observed
                .observation
                .as_ref()
                .unwrap_or_else(|| panic!("{:?}", observed.outcome)),
        );
        assert_eq!(reference.output, b"ok\n");
        let input = engine.export_native_input().unwrap();
        let sites = native::Sites::new(&input);
        let validated = engine.import_native_input(input.clone()).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let heap = native::run_heap(&ready, &entry, &sites).unwrap();
        assert_eq!(heap.observation, reference);
        // An ordinary list stores fixed carriers, with a distinct counted
        // FlatBytes owner in each Row. This is not a sequential flat list.
        use hirsel_lir::layout_types::{CopyRecipe, LayoutKind, SizeSpec};
        let carrier = ready
            .table
            .layouts
            .iter()
            .find(|layout| {
                layout.kind == LayoutKind::Aggregate
                    && layout.fields.len() == 2
                    && layout.size == SizeSpec::Fixed(16)
                    && ready
                        .table
                        .layout(layout.fields[1].layout)
                        .is_some_and(|text| {
                            text.copy == CopyRecipe::FlatBytes
                                && matches!(text.kind, LayoutKind::Block { .. })
                        })
            })
            .expect("a fixed Row containing a flat text owner");
        assert_eq!(carrier.fields[1].offset, 8);
        let mut configs = native::smoke_matrix();
        configs.push(native::Configuration {
            options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            engine: native::Engine::Cranelift {
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
        });
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("row-text-shape");
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        for (config, outcome) in
            native::differential(&validated, &entry, &reference, &configs, &tools, &dir)
        {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{config}: {outcome:?}"
            );
        }
    });
}

#[test]
fn adapter_matches_ordinary_items_and_encoded_snapshots() {
    with_compiler_stack(|| {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/row-text.hir");
        let mut loaded =
            hirsel_driver::load_program_source(&path, Some(&projected_source())).unwrap();
        loaded.check_for_execution().unwrap_or_else(|error| {
            if let hirsel_engine::ToolError::Query {
                error: hirsel_types::QueryError::Diagnostics(diagnostics),
                ..
            } = &error
            {
                panic!("{diagnostics:?}");
            }
            panic!("{error}");
        });
        let entry = loaded.entry.clone();
        // The product oracle is the fixture's byte-for-byte comparison with
        // the existing model and store encoder, including retained versions.
        // Its stress case exceeds the interpreter's fixed million-step limit;
        // run it from Rust on H and the two native backends instead.
        let reference = native::Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: b"ok\n".to_vec(),
            failures: vec![],
            roots: vec![],
        };
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let mut configs = native::smoke_matrix();
        // H runs the bounded ownership twin below, without full JSON stress.
        configs.remove(0);
        configs.push(native::Configuration {
            options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            engine: native::Engine::Cranelift {
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
        });
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("row-text-adapter");
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        for (config, outcome) in
            native::differential(&validated, &entry, &reference, &configs, &tools, &dir)
        {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{config}: {outcome:?}"
            );
        }
    });
}

#[test]
fn detached_title_minimal_probe_and_ordinary_twin() {
    with_compiler_stack(|| {
        let source = include_str!("../fixtures/row-text-materialize.hir");
        for kind in ["direct", "helper", "ordinary"] {
            let source = if kind == "helper" {
                source
                    .replace(
                        "fn detached(row:",
                        "fn own(value: String) -> unique String { unique value }\nfn detached(row:",
                    )
                    .replace("unique row.text.title", "own(row.text.title)")
            } else if kind == "direct" {
                source.to_owned()
            } else {
                source.replace("flat type", "type")
            };
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("title.hir", source).unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let entry = entry_path(&default_context());
            let observed = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let reference =
                native::Observation::of_reference(observed.observation.as_ref().unwrap());
            assert_eq!(reference.output, b"abc");
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let ready = native::build(
                &validated,
                &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            )
            .unwrap_or_else(|error| {
                panic!("{kind} must compile after the projection fix: {error}")
            });
            let sites = native::Sites::new(validated.snapshot());
            assert_eq!(
                native::heap_campaign(&ready, &entry, &sites)
                    .unwrap()
                    .observation,
                reference
            );
            let mut configs = native::smoke_matrix();
            configs.push(native::Configuration {
                options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            });
            let dir =
                std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("row-title-{kind}"));
            let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
            for (config, outcome) in
                native::differential(&validated, &entry, &reference, &configs, &tools, &dir)
            {
                assert!(
                    matches!(outcome, native::Outcome::Agree),
                    "{config}: {outcome:?}"
                );
            }
        }
    });
}

#[test]
fn a_live_title_relation_prevents_overlapping_replacement() {
    with_compiler_stack(|| {
        let header = "flat type TextFields { title: String }\ntype Row { text: TextFields }\nfn title(row: Row) -> String in row { copy row.text.title }\n";
        for live in [true, false] {
            let body = if live {
                "fn probe(mut row: unique Row) -> u64 { let saved = title(row); row.text = TextFields(title: \"new\"); string.byte_len(saved) }"
            } else {
                "fn probe(mut row: unique Row) -> u64 { let n = string.byte_len(title(row)); row.text = TextFields(title: \"new\"); n }"
            };
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("loan.hir", format!("{header}{body}"))
                .unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            if live {
                assert!(
                    checked
                        .diagnostics
                        .iter()
                        .any(|d| d.rule.as_str() == "MODE-1"),
                    "a live relation must reject the overlapping write"
                );
                eprintln!("live title rejection: {:?}", checked.diagnostics);
            } else {
                assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            }
        }
    });
}

#[test]
fn heap_checks_owned_snapshots_updates_and_both_release_orders() {
    with_compiler_stack(|| {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/row-text.hir");
        let source = projected_source()
            .replace(&"d".repeat(4000), "long description")
            .replace(
                "store.encode(snapshot(rows)) == store.encode(board)",
                "snapshot(rows) == board",
            );
        let mut loaded = hirsel_driver::load_program_source(&path, Some(&source)).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let entry = loaded.entry.clone();
        let input = loaded.engine.export_native_input().unwrap();
        let sites = native::Sites::new(&input);
        let validated = loaded.engine.import_native_input(input.clone()).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        use hirsel_lir::layout_types::{CopyRecipe, LayoutKind, SizeSpec};
        let carrier = ready
            .table
            .layouts
            .iter()
            .find(|layout| {
                layout.kind == LayoutKind::Aggregate
                    && layout.fields.len() == 5
                    && ready
                        .table
                        .layout(layout.fields[4].layout)
                        .is_some_and(|text| text.copy == CopyRecipe::FlatBytes)
            })
            .expect("the full Row schema keeps one flat owner beside ordinary metadata");
        assert_eq!(carrier.size, SizeSpec::Fixed(40));
        assert_eq!(carrier.align, 8);
        assert_eq!(carrier.fields[4].offset, 32);
        eprintln!(
            "full Row C layout: size {:?}, align {}, text offset {}",
            carrier.size, carrier.align, carrier.fields[4].offset
        );
        let heap = native::run_heap(&ready, &entry, &sites).unwrap();
        assert_eq!(
            heap.observation.output,
            b"ok\n",
            "{}",
            heap.observation.render()
        );
        assert_eq!(heap.observation.exit_code, Some(0));
        assert_eq!(heap.counters.allocations, heap.counters.frees);
        assert_eq!(heap.counters.live_heap_bytes, 0);
        eprintln!("row ownership H counters: {:?}", heap.counters);
    });
}

#[test]
fn exact10k_owned_and_direct_builders_keep_the_fixture_bytes() {
    use sha2::{Digest, Sha256};
    let mut expected = String::from("{\"next_id\":10001,\"revision\":1,\"tasks\":[");
    for id in 1..=10000 {
        if id > 1 {
            expected.push(',');
        }
        expected.push_str(&format!("{{\"id\":{id},\"title\":\"Task {id}\",\"description\":\"Benchmark description\",\"status\":\"Todo\",\"priority\":\"Medium\",\"due\":null,\"tags\":[\"bench\",\"group-{}\"]}}", id % 10));
    }
    expected.push_str("]}");
    assert_eq!(expected.len(), 1_437_828);
    assert_eq!(
        format!("{:x}", Sha256::digest(expected.as_bytes())),
        "de7fb9101257206be69df18c46cfaf60ed02b95d986ea642837bcb63d6e2e4a8"
    );
    with_compiler_stack(move || {
        for builder in ["build(seed10k())", "direct10k()"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/row-text.hir");
            let original = projected_source();
            let source = format!(
                "{}\n{}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{ let rows = {builder}; let _ = out.write(io.stdout(rt)?, store.encode(snapshot(rows))); Ok(()) }}\n",
                original.split("pub fn main(").next().unwrap(),
                include_str!("../fixtures/row-text-tenk.hir")
            );
            let mut loaded = hirsel_driver::load_program_source(&path, Some(&source)).unwrap();
            loaded
                .check_for_execution()
                .unwrap_or_else(|e| panic!("{e}"));
            let entry = loaded.entry.clone();
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input).unwrap();
            let reference = native::Observation {
                exit_code: Some(0),
                blocked: false,
                budget: None,
                signaled: None,
                output: expected.as_bytes().to_vec(),
                failures: vec![],
                roots: vec![],
            };
            let mut configs = native::smoke_matrix();
            configs.remove(0);
            configs.push(native::Configuration {
                options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            });
            let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(
                if builder.starts_with("build") {
                    "row-tenk-owned"
                } else {
                    "row-tenk-direct"
                },
            );
            let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
            for (config, outcome) in
                native::differential(&validated, &entry, &reference, &configs, &tools, &dir)
            {
                match outcome {
                    native::Outcome::Agree => {}
                    native::Outcome::Unavailable(e) => panic!("{builder}: {config}: {e}"),
                    native::Outcome::Mismatch(actual) => panic!(
                        "{builder}: {config}: output length {}, sha256 {:x}, exit {:?}",
                        actual.output.len(),
                        Sha256::digest(&actual.output),
                        actual.exit_code
                    ),
                }
            }
        }
    });
}

#[test]
fn unique_sparse_mutations_and_reads_do_not_rebuild_other_text_owners() {
    with_compiler_stack(|| {
        let mut deltas = vec![];
        for count in [3u64, 17] {
            let mut base = None;
            for (name, action) in [
                ("baseline", ""),
                (
                    "read",
                    "failure.check(scan(rows) == expected, TextInvariant())",
                ),
                ("metadata", "rows[0].status = 9"),
                (
                    "replace",
                    "rows[0].text = unique TextFields(title: \"Updated\", description: \"benchmark\", tags: [\"bench\"])",
                ),
                (
                    "remove",
                    "let removed = list.remove(mut rows, 0); failure.check(removed.id == 1 && rows[0].id == 2, TextInvariant())",
                ),
            ] {
                let expected = count * (count + 1) / 2 + 7 * count;
                let remaining = match name {
                    "replace" => expected + 14,
                    "remove" => expected - 8,
                    _ => expected,
                };
                let source = format!(
                    r#"flat type TextFields {{ title: String, description: String, tags: List[String] }}
type Row {{ id: u64, status: u64, text: TextFields }}
fn scan(rows: List[Row]) -> u64 {{
  let mut total = 0u64
  for row in rows {{
    total += row.id + string.byte_len(row.text.title) + string.byte_len(row.text.description)
    for tag in row.text.tags {{ total += string.byte_len(tag) }}
  }}
  total
}}
pub fn main() -> Result[(), ()] {{
  let mut rows: List[Row] = []
  list.reserve(mut rows, {count})
  for id in 1u64..{end}u64 {{ list.push(mut rows, Row(id: id, status: 0, text: unique TextFields(title: "a", description: "bbb", tags: ["x", "yz"]))) }}
  let expected = {expected}u64
  failure.check(scan(rows) == expected, TextInvariant())
  {action}
  failure.check(scan(rows) == {remaining}u64, TextInvariant())
  Ok(())
}}
"#,
                    end = count + 1
                );
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine.sources.add("row-cost.hir", source).unwrap();
                let checked = check_progress(&mut engine, file, &default_context()).unwrap();
                assert!(
                    checked.diagnostics.is_empty(),
                    "{name}: {:?}",
                    checked.diagnostics
                );
                let input = engine.export_native_input().unwrap();
                let validated = engine.import_native_input(input).unwrap();
                let ready = native::build(
                    &validated,
                    &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
                )
                .unwrap();
                let program = hirsel_heapcheck::Program {
                    target: ready.target.clone(),
                    layouts: ready.table.clone(),
                    units: ready.raw_units(),
                    statics: ready.statics.clone(),
                };
                let run = hirsel_heapcheck::run_ready(
                    &program,
                    &entry_path(&default_context()),
                    &[],
                    &mut hirsel_heapcheck::NoOperations,
                    &hirsel_heapcheck::Config::default(),
                );
                assert!(run.result.is_ok(), "{name}: {:?}", run.result);
                assert_eq!(run.counters.live_heap_bytes, 0);
                assert_eq!(run.counters.allocations, run.counters.frees);
                if count == 3 && matches!(name, "baseline" | "replace" | "remove") {
                    eprintln!("row cleanup H trace {name}: {:#?}", run.trace);
                }
                let allocations: Vec<_> = run
                    .trace
                    .iter()
                    .filter_map(|e| match e {
                        hirsel_heapcheck::Event::Alloc { capacity, .. } => Some(*capacity),
                        _ => None,
                    })
                    .collect();
                eprintln!(
                    "row cost n={count} {name}: {:?}; allocation capacities {allocations:?}",
                    run.counters
                );
                if name == "baseline" {
                    base = Some(run.counters.allocations);
                } else if name == "replace" {
                    deltas.push(run.counters.allocations - base.unwrap());
                } else {
                    assert_eq!(
                        Some(run.counters.allocations),
                        base,
                        "{name} allocates or reconstructs text"
                    );
                }
            }
        }
        assert_eq!(deltas.len(), 2);
        assert_eq!(
            deltas[0], deltas[1],
            "replacement work must not grow with unrelated owners"
        );
        assert!(
            deltas[0] <= 3,
            "one new text owner plus construction temporaries: {deltas:?}"
        );
    });
}
