use hirsel_driver::*;
use hirsel_engine::*;

fn keyed_work(kind: &str, count: u64) -> (u64, u64) {
    let dir_scratch = tempfile::Builder::new()
        .prefix("hirsel-interp-keyed-")
        .tempdir()
        .unwrap();
    let dir = dir_scratch.path().to_path_buf();
    let path = dir.join("main.hir");
    let (ty, insert, check) = if kind == "upsert" {
        (
            "Map[u64, u64]",
            "map.upsert_with(mut out, i, fn() -> unique u64 uses {} { 1 }, fn(mut value: unique u64) -> () uses {} { value += 1 })",
            "{ map.update_with(mut out, i, fn(mut value: unique u64) -> () uses {} { value += 1 }); map.get(out, i) == Some(2) }",
        )
    } else if kind == "map" {
        (
            "Map[u64, u64]",
            "map.insert(mut out, i, i + 1)",
            "map.get(out, i) == Some(i + 1)",
        )
    } else {
        ("Set[u64]", "set.insert(mut out, i)", "set.contains(out, i)")
    };
    let constructor = if kind == "upsert" { "map" } else { kind };
    std::fs::write(&path, format!(
        "pub fn main() -> Result[(), String] {{\n  let mut out: {ty} = {constructor}.empty()\n  \
         for i in 0u64..{count}u64 {{\n    {insert}\n  }}\n  \
         for i in 0u64..{count}u64 {{\n    if !({check}) {{ return Error(\"missing key\") }}\n  }}\n  \
         Ok(())\n}}\n"
    )).unwrap();
    let setup_before = hirsel_intern::work::read(hirsel_intern::work::Counter::InterpCopiedValues);
    // Ordinary std precheck (25e2b384) executes unrelated compile-time code.
    // Measure both counters over the same application execution interval.
    let mut program = COMPILER
        .with_verify(Verify::On)
        .load_program(&path)
        .unwrap();
    program.check_for_execution().unwrap();
    program.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
        limits: hirsel_interp::Limits {
            steps: None,
            ..Default::default()
        },
        ..Default::default()
    }));
    let before = hirsel_intern::work::read(hirsel_intern::work::Counter::InterpKeyedRows);
    let copies_before = hirsel_intern::work::read(hirsel_intern::work::Counter::InterpCopiedValues);
    eprintln!(
        "std precheck copied {} values before application execution",
        hirsel_intern::work::read(hirsel_intern::work::Counter::InterpCopiedValues) - setup_before
    );
    let observed = run_observed_snapshot(
        &mut program.engine,
        &hirsel_tir::RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
    )
    .unwrap();
    let outcome = observed.outcome.unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
    );
    std::fs::remove_dir_all(&dir).unwrap();
    (
        hirsel_intern::work::read(hirsel_intern::work::Counter::InterpKeyedRows) - before,
        hirsel_intern::work::read(hirsel_intern::work::Counter::InterpCopiedValues) - copies_before,
    )
}

#[test]
fn map_and_set_insertions_and_reads_inspect_linearly_many_entries() {
    with_compiler_stack(|| {
        for kind in ["map", "set"] {
            for count in [2_000, 8_000] {
                let (rows, _) = keyed_work(kind, count);
                assert!(
                    rows <= 4 * count + 1_000,
                    "{count} {kind} insertions and reads inspected {rows} entries"
                );
            }
        }
    });
}

#[test]
fn map_upserts_and_value_callbacks_preserve_the_index_without_copying_the_map() {
    with_compiler_stack(|| {
        for count in [2_000, 8_000] {
            let (rows, copied) = keyed_work("upsert", count);
            assert!(
                rows <= 4 * count + 1_000,
                "{count} upserts inspected {rows} entries"
            );
            assert!(
                copied <= 16 * count + 1_000,
                "{count} upserts copied {copied} values"
            );
        }
    });
}

// Indexmap's entries + index layout (src/inner.rs:21-38) keeps ordinals stable
// through tombstones, as Map does. A fourfold drain must do linear row work.
#[test]
fn map_and_set_drains_inspect_linearly_many_entries() {
    with_compiler_stack(|| {
        for kind in ["map", "set"] {
            for count in [2_000, 8_000] {
                let dir_scratch = tempfile::Builder::new()
                    .prefix("hirsel-keyed-drain-")
                    .tempdir()
                    .unwrap();
                let dir = dir_scratch.path().to_path_buf();
                std::fs::create_dir_all(&dir).unwrap();
                let path = dir.join("main.hir");
                let ty = if kind == "map" {
                    "unique Map[u64,u64]"
                } else {
                    "unique Set[u64]"
                };
                let insert = if kind == "map" {
                    "map.insert(mut out,i,i)"
                } else {
                    "set.insert(mut out,i)"
                };
                let remove = if kind == "map" {
                    "map.remove(mut out,i) == Some(i)"
                } else {
                    "set.remove(mut out,i)"
                };
                std::fs::write(&path, format!(
                    "pub fn main() -> Result[(),String] {{\nlet mut out: {ty} = {kind}.empty()\nfor i in 0u64..{count}u64 {{ {insert} }}\nfor i in 0u64..{count}u64 {{ if !({remove}) {{ return Error(\"missing\") }} }}\nif {kind}.len(out) != 0 {{ return Error(\"length\") }}\nOk(())\n}}"
                )).unwrap();
                let before =
                    hirsel_intern::work::read(hirsel_intern::work::Counter::InterpKeyedRows);
                let result = run_path_with_step_budget(
                    &path,
                    SimulationConfig::default(),
                    None,
                    hirsel_engine::Verify::On,
                    &mut vec![],
                    &mut vec![],
                )
                .unwrap();
                assert_eq!(
                    result,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                );
                let rows = hirsel_intern::work::read(hirsel_intern::work::Counter::InterpKeyedRows)
                    - before;
                assert!(
                    rows <= 6 * count + 1_000,
                    "{kind} drain of {count} inspected {rows} rows"
                );
                std::fs::remove_dir_all(dir).unwrap();
            }
        }
    });
}
