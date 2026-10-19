use hirsel_driver::*;
use hirsel_engine::*;

fn keyed_work(kind: &str, count: u64) -> u64 {
    let dir = std::env::temp_dir().join(format!(
        "hirsel-interp-keyed-{kind}-{count}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
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
    let before = hirsel_intern::work::read(hirsel_intern::work::Counter::InterpKeyedRows);
    let outcome = run_path_with_step_budget(
        &path,
        SimulationConfig::default(),
        None,
        &mut vec![],
        &mut vec![],
    )
    .unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Returned(hirsel_run_config::Returned::Ok)
    );
    std::fs::remove_dir_all(&dir).unwrap();
    hirsel_intern::work::read(hirsel_intern::work::Counter::InterpKeyedRows) - before
}

#[test]
fn map_and_set_insertions_and_reads_inspect_linearly_many_entries() {
    with_compiler_stack(|| {
        for kind in ["map", "set"] {
            for count in [2_000, 8_000] {
                let rows = keyed_work(kind, count);
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
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::InterpCopiedValues);
            let rows = keyed_work("upsert", count);
            let copied =
                hirsel_intern::work::read(hirsel_intern::work::Counter::InterpCopiedValues)
                    - before;
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
