//! The dispatcher hands a claimed frame to another activation; the physical
//! audits stop there only when that activation takes the role itself.
//!
//! The vectors follow `tokio/src/sync/tests/loom_notify.rs:12-48`: a waiter
//! registered before the notify is woken (`notify_one`), a notify before the
//! waiter polls is the notifier's own effect, and each of several waiters is
//! accounted separately (`notify_waiters`). Here the waiter is the started
//! fiber, registered by acquiring the role, and the notifier is the
//! dispatcher that starts it.

use super::{
    IDLE_BUDGET_SERVICES, budget_service_violations, physical_entry_violation, source,
    task_owned_violation,
};

#[test]
fn dispatch_hands_claimed_frames_to_their_own_activations() {
    let graph = source();
    let resume = "pool::free::resume";
    assert!(graph.directly_calls(resume, "tasks::compact::activate"));
    assert!(
        graph
            .starts("tasks::compact::activate")
            .eq(["tasks::compact::entry"])
    );
    assert!(graph.own_activation("tasks::compact::entry"));
    assert!(graph.directly_calls("tasks::compact::entry", "Guard::physical"));
    // A private-ahead frame is driven on the idle side, under its own role.
    let native = "tasks::ordered::ahead::native::run";
    assert!(graph.directly_calls(resume, native));
    assert!(graph.directly_calls(native, "tasks::role::enter"));
    for service in IDLE_BUDGET_SERVICES {
        let caller = if *service == "tasks::ordered::engine::ahead_allocation" {
            let allocation = "tasks::ordered::ahead::cpu_allocation::serve";
            assert!(graph.directly_calls(native, allocation));
            allocation
        } else {
            native
        };
        assert!(graph.directly_calls(caller, service), "{service}");
    }
    // The idle loop takes no role: switching to it is still traversed.
    assert!(
        graph
            .starts("pool::idle_of")
            .any(|f| f == "pool::home_idle")
    );
    assert!(!graph.own_activation("pool::home_idle"));
}

#[test]
fn a_started_fiber_is_its_own_activation_only_with_its_own_role() {
    let scratch = tempfile::tempdir().unwrap();
    let (role, new) = ("crate::Guard::physical();", "crate::Guard::new();");
    let op = "crate::tasks::ordered::engine::accepted();";
    let run = "crate::TaskCtx::run();";
    let one = "stack.prepare(entry);";
    let both = "stack.prepare(entry); stack.prepare(other);";
    // (before dispatch, stack type, starts, entry's role, semantic, ordinal, whole run)
    for (before, stack, start, taken, semantic_red, ordinal_red, run_red) in [
        ("", "Stack", one, role, false, false, false),
        ("", "Stack", one, new, false, false, false),
        ("", "Stack", one, "", true, true, true),
        ("", "Stack", one, "take();", true, true, true),
        (op, "Stack", one, role, true, true, false),
        (run, "Stack", one, role, false, false, true),
        ("", "Journal", one, role, true, true, true),
        ("", "Stack", both, role, false, true, false),
    ] {
        std::fs::write(scratch.path().join("lib.rs"), format!(
            "#[no_mangle] pub fn hirsel_rt_v2_resume_run() {{ Guard::new(); pool::resume(); }}
             mod pool {{ pub fn resume() {{ {before} crate::tasks::compact::activate(); }} }}
             mod tasks {{
               pub mod compact {{
                 pub fn activate(stack: crate::{stack}) {{ {start} }}
                 pub fn entry() {{ {taken} crate::tasks::ordered::engine::accepted(); crate::TaskCtx::run(); }}
                 pub fn other() {{ crate::tasks::free::record::begin(); }}
                 fn take() {{ crate::Guard::physical(); }}
               }}
               pub mod ordered {{
                 pub mod ahead {{ pub fn barrier() {{ crate::pool::resume(); }} }}
                 pub mod engine {{ pub fn accepted() {{}} }}
               }}
               pub mod free {{ pub mod record {{ pub fn begin() {{}} }} }}
             }}
             struct Guard; impl Guard {{ fn new() {{}} fn physical() {{}} }}
             struct Stack; impl Stack {{ fn prepare(&self, _: fn()) {{}} }}
             struct Journal; impl Journal {{ fn prepare(&self, _: fn()) {{}} }}
             struct TaskCtx; impl TaskCtx {{ fn run() {{}} }}"
        )).unwrap();
        let graph = super::graph::Graph::parse(scratch.path());
        let case = format!("{before}/{stack}/{start}/{taken}");
        assert_eq!(
            graph.starts("tasks::compact::activate").next().is_some(),
            stack == "Stack",
            "{case}"
        );
        assert_eq!(
            !budget_service_violations(&graph).is_empty(),
            semantic_red,
            "{case}"
        );
        let ordinal = physical_entry_violation(&graph, "hirsel_rt_v2_resume_run");
        assert_eq!(ordinal.is_some(), ordinal_red, "{case}: {ordinal:?}");
        if start == both {
            let path = ordinal.unwrap();
            assert!(
                path.contains(&"tasks::compact::other".to_owned()),
                "{path:?}"
            );
        }
        assert_eq!(
            graph
                .reaches("tasks::ordered::ahead::barrier", |key| key
                    == "TaskCtx::run")
                .is_some(),
            run_red,
            "{case}"
        );
    }
}

#[test]
fn a_task_owned_entry_reaches_role_state_only_through_a_guard() {
    let scratch = tempfile::tempdir().unwrap();
    for (step, red) in [
        ("", None),
        ("shielded();", None),
        ("tasks::scheduler();", Some("tasks::scheduler")),
        ("tasks::role::with_role();", Some("tasks::role::with_role")),
        ("unshielded();", Some("tasks::scheduler")),
    ] {
        std::fs::write(
            scratch.path().join("lib.rs"),
            format!(
                "#[no_mangle] pub fn hirsel_rt_v2_finalize() {{ tasks::role::task_owned(); drain(); }}
                 fn drain() {{ {step} }}
                 fn shielded() {{ Guard::new(); tasks::scheduler(); }}
                 fn unshielded() {{ tasks::scheduler(); Guard::physical(); }}
                 mod tasks {{
                   pub fn scheduler() {{}}
                   pub mod role {{
                     pub fn task_owned() {{ crate::tasks::ordered::ahead::barrier(); }}
                     pub fn with_role() {{}} pub fn enter() {{}}
                   }}
                   pub mod ordered {{ pub mod ahead {{ pub fn barrier() {{ super::super::role::enter(); }} }} }}
                 }}
                 struct Guard; impl Guard {{ fn new() {{}} fn physical() {{}} }}"
            ),
        )
        .unwrap();
        let graph = super::graph::Graph::parse(scratch.path());
        assert!(graph.directly_calls("hirsel_rt_v2_finalize", "tasks::role::task_owned"));
        let path = task_owned_violation(&graph, "hirsel_rt_v2_finalize");
        assert_eq!(
            path.as_ref().and_then(|p| p.last()).map(String::as_str),
            red,
            "{step}: {path:?}"
        );
    }
}
