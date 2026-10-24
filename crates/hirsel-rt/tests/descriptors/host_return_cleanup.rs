use super::source;

const ADMIT: &str = "tasks::ordered::ahead::host_return::admit";

#[test]
fn host_return_cleanup_never_emits_an_operation_or_ordinal() {
    let graph = source();
    assert!(graph.directly_calls("tasks::ordered::ahead::native::run", ADMIT));
    assert_eq!(super::physical_entry_violation(graph, ADMIT), None);
}

#[test]
fn host_return_cleanup_never_runs_a_finalizer_or_borrows_the_whole_run() {
    assert_eq!(
        source().reaches(ADMIT, |key| matches!(
            key,
            "finalize::run_finalizer" | "finalize::run_close" | "TaskCtx::run"
        )),
        None
    );
}
