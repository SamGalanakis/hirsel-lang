use super::{
    IDLE_BUDGET_SERVICES, budget_service_violations, graph, physical_entry_violation, source,
};

#[test]
fn idle_cpu_allocation_keeps_budget_settlement_and_heap_work_outside_the_task_stack() {
    let graph = source();
    let serve = "tasks::ordered::ahead::cpu_allocation::serve";
    let allocation = "tasks::ordered::engine::ahead_allocation";
    assert!(IDLE_BUDGET_SERVICES.contains(&allocation));
    assert!(graph.directly_calls(serve, "Request::admit"));
    assert!(graph.directly_calls("Request::admit", allocation));
    for callee in ["alloc::cpu_preflight", "alloc::alloc_prepared"] {
        assert!(graph.directly_calls(serve, callee), "{callee}");
    }
    assert_eq!(
        graph.reaches(serve, |key| matches!(
            key,
            "TaskCtx::run" | "finalize::run_finalizer" | "finalize::run_close"
        )),
        None
    );
    assert!(budget_service_violations(graph).is_empty());
}

#[test]
fn idle_cpu_allocation_still_rejects_operations_and_ordinals_outside_charge_settlement() {
    let root = tempfile::tempdir().unwrap();
    for (body, before, budget_red, physical_red) in [
        ("", "", false, false),
        ("crate::Log::subevents();", "", false, false),
        ("submit();", "", true, false),
        ("crate::Frontier::offer();", "", true, false),
        ("", "crate::Log::subevents();", false, true),
        ("", "crate::tasks::ordered::engine::submit();", false, true),
    ] {
        std::fs::write(
            root.path().join("lib.rs"),
            format!(
                "#[no_mangle] pub fn hirsel_rt_v2_resume_run() {{ Guard::new(); {before}
                tasks::ordered::engine::ahead_allocation(); }}
             struct Guard; impl Guard {{ fn new() {{}} }}
             mod tasks {{ pub mod ordered {{ pub mod engine {{
                 pub fn ahead_allocation() {{ {body} }} pub fn submit() {{}}
             }} }} }}
             struct Frontier; impl Frontier {{ fn offer() {{}} }}
             struct Log; impl Log {{ fn subevents() {{}} }}"
            ),
        )
        .unwrap();
        let graph = graph::Graph::parse(root.path());
        assert_eq!(
            !budget_service_violations(&graph).is_empty(),
            budget_red,
            "{body}/{before}"
        );
        assert_eq!(
            physical_entry_violation(&graph, "hirsel_rt_v2_resume_run").is_some(),
            physical_red,
            "{body}/{before}"
        );
    }
}
