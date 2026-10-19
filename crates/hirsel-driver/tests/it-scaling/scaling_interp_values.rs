use hirsel_driver::*;
use hirsel_engine::*;

/// Value nodes the interpreter copies while a loop pushes onto a List that it
/// also reads through a borrowed parameter and a borrowed core operation each
/// iteration. Every push and read is O(1), so copies stay linear in the count.
fn copied_values(count: u64) -> u64 {
    let dir = std::env::temp_dir().join(format!(
        "hirsel-interp-values-{count}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
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
    let before = hirsel_intern::work::read(hirsel_intern::work::Counter::InterpCopiedValues);
    let outcome = run_path_with_step_budget(
        &path,
        SimulationConfig::default(),
        None,
        &mut vec![],
        &mut vec![],
    )
    .unwrap();
    assert_eq!(outcome, RunOutcome::Exit(0));
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
