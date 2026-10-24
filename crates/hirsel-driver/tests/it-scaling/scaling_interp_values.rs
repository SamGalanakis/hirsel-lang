use hirsel_driver::*;
use hirsel_engine::*;

/// Value nodes the interpreter copies while a loop pushes onto a List that it
/// also reads through a borrowed parameter and a borrowed core operation each
/// iteration. Every push and read is O(1), so copies stay linear in the count.
fn copied_values(count: u64) -> u64 {
    let dir_scratch = tempfile::Builder::new()
        .prefix("hirsel-interp-values-")
        .tempdir()
        .unwrap();
    let dir = dir_scratch.path().to_path_buf();
    let path = dir.join("main.hir");
    std::fs::write(
        &path,
        format!(
            "fn size(xs: List[u64]) -> u64 {{\n  list.len(xs)\n}}\n\n\
             pub fn main() -> Result[(), String] {{\n  let mut out: List[u64] = []\n  \
             while size(out) < {count}u64 {{\n    list.push(mut out, list.len(out))\n  }}\n  \
             Ok(())\n}}\n"
        ),
    )
    .unwrap();
    let setup_before = hirsel_intern::work::read(hirsel_intern::work::Counter::InterpCopiedValues);
    // 25e2b384 loads ordinary std source; its compile-time execution is setup,
    // not work performed by the list loop under test.
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
    let before = hirsel_intern::work::read(hirsel_intern::work::Counter::InterpCopiedValues);
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
    hirsel_intern::work::read(hirsel_intern::work::Counter::InterpCopiedValues) - before
}

#[test]
fn list_pushes_and_borrowed_reads_copy_linearly_many_values() {
    with_compiler_stack(|| {
        for count in [2_000, 8_000] {
            let copied = copied_values(count);
            assert!(
                copied <= 4 * count + 1_000,
                "{count} pushes copied {copied} values"
            );
        }
    });
}
