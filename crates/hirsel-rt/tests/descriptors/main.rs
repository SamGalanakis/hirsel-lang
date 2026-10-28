//! The operation descriptors (`hirsel_rt_operations::descriptors`, multicore design §4.1)
//! against the runtime's source: every exported entry and every task, channel, mutex, atomic and
//! `Shared` registry operation is classified, the E set is the one the footprints allow, and no
//! entry marked task-local, immutable or concurrent reaches the scheduler, the Io binding or an
//! entry of another locality.

hirsel_guard::install!();

include!("modules.rs");

use hirsel_rt_operations::descriptors::{
    Class, E, ENTRIES, INLINE, Locality, OPS, OUTSIDE_E, REGISTRY, REGISTRY_FAMILIES, Visibility,
    entry_descriptor, op_descriptor,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[test]
fn every_semantic_entry_acquires_the_scheduler_role_directly() {
    let graph = source();
    // These abort the process unconditionally and touch no scheduler or run
    // object. The foreign-leave entry recovers its context from Interval.
    let terminal = [
        "hirsel_rt_v2_diag_panic",
        "hirsel_rt_v2_invariant_fault",
        "hirsel_rt_v2_foreign_violation",
        "hirsel_rt_v2_callback_failed",
    ];
    let mut checked = 0;
    let mut violations = Vec::new();
    for (symbol, key) in graph.entries() {
        if !matches!(
            entry_descriptor(symbol).unwrap().locality,
            Locality::Op | Locality::Physical
        ) || terminal.contains(&symbol)
        {
            continue;
        }
        if TASK_OWNED_ENTRIES.contains(&symbol) {
            if !graph.directly_calls(key, "tasks::role::task_owned") {
                violations.push(format!("task-owned entry skips the barrier: {symbol}"));
            }
            if let Some(path) = task_owned_violation(graph, key) {
                violations.push(format!(
                    "task-owned entry reaches the scheduler outside a guard: {symbol}: {}",
                    path.join(" -> ")
                ));
            }
        } else if !graph.directly_calls(key, "Guard::new") {
            violations.push(format!("unguarded semantic entry: {symbol} ({key})"));
        }
        if entry_descriptor(symbol).unwrap().locality == Locality::Physical {
            // Physical scheduling services are independently checked below;
            // the entry itself may not submit or commit a semantic operation.
            if let Some(path) = physical_entry_violation(graph, key) {
                violations.push(format!(
                    "physical entry creates an operation or ordinal: {symbol}: {}",
                    path.join(" -> ")
                ));
            }
        }
        checked += 1;
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
    assert!(checked >= 150, "only checked {checked} semantic entries");
}

#[test]
fn parallel_services_never_borrow_the_whole_run_exclusively() {
    let graph = source();
    for (symbol, key) in graph.entries() {
        if !matches!(
            entry_descriptor(symbol).unwrap().locality,
            Locality::Local | Locality::Immutable | Locality::Concurrent
        ) {
            continue;
        }
        assert_eq!(
            graph.reaches(key, |target| target == "TaskCtx::run"),
            None,
            "{symbol} can exclusively borrow RunShared during another worker's shared read"
        );
    }
}

/// Semantic entries whose own work is the running task's storage
/// (`tasks::role::task_owned`): they wait at the ahead barrier without the
/// role, and every step that reaches role-protected state opens its own guard.
const TASK_OWNED_ENTRIES: &[&str] = &["hirsel_rt_v2_finalize"];

/// A path from a task-owned entry to role-protected state that no function on
/// it guards directly.
fn task_owned_violation(graph: &graph::Graph, key: &str) -> Option<Vec<String>> {
    graph.reaches_until(
        key,
        |target| {
            matches!(
                target,
                "tasks::scheduler"
                    | "tasks::role::with_role"
                    | "tasks::role::enter"
                    | "Binding::get"
                    | "TaskCtx::run"
            )
        },
        // The barrier is Guard::new's own wait, checked as a physical service.
        |target| target == PHYSICAL_AHEAD_SERVICES[0] || graph.directly_calls(target, "Guard::new"),
    )
}

/// The runtime's call graph, parsed once on a thread with room for syn's recursion over the
/// largest runtime files.
fn source() -> &'static graph::Graph {
    static GRAPH: std::sync::OnceLock<graph::Graph> = std::sync::OnceLock::new();
    GRAPH.get_or_init(|| {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(|| graph::Graph::parse(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src")))
            .unwrap()
            .join()
            .unwrap()
    })
}

#[test]
fn every_exported_entry_has_exactly_one_descriptor() {
    let graph = source();
    let exported: BTreeSet<&str> = graph.entries().map(|(symbol, _)| symbol).collect();
    assert!(
        exported.len() > 150,
        "found only {} entries",
        exported.len()
    );
    let mut rows = BTreeMap::<&str, usize>::new();
    for entry in ENTRIES {
        *rows.entry(entry.symbol).or_default() += 1;
    }
    let unclassified: Vec<_> = exported.iter().filter(|s| !rows.contains_key(*s)).collect();
    assert!(
        unclassified.is_empty(),
        "unclassified runtime entries: {unclassified:?}"
    );
    let stale: Vec<_> = rows.keys().filter(|s| !exported.contains(*s)).collect();
    assert!(
        stale.is_empty(),
        "descriptors of no exported entry: {stale:?}"
    );
    let duplicated: Vec<_> = rows.iter().filter(|(_, n)| **n > 1).collect();
    assert!(
        duplicated.is_empty(),
        "entries described twice: {duplicated:?}"
    );
}

#[test]
fn every_op_has_one_descriptor_with_a_phases_kind() {
    let mut names = BTreeSet::new();
    for op in OPS {
        assert!(names.insert(op.name), "op {} described twice", op.name);
        assert!(
            !op.transitions.is_empty(),
            "op {} has no transition",
            op.name
        );
    }
    for entry in ENTRIES {
        match entry.locality {
            Locality::Op => {
                assert!(
                    !entry.ops.is_empty(),
                    "{} is an op boundary naming no op",
                    entry.symbol
                );
                for op in entry.ops {
                    assert!(
                        op_descriptor(op).is_some(),
                        "{} performs undescribed op {op}",
                        entry.symbol
                    );
                }
            }
            _ => assert!(
                entry.ops.is_empty(),
                "{} is not an op boundary but names ops",
                entry.symbol
            ),
        }
    }
    for (operation, ops) in REGISTRY {
        for op in *ops {
            assert!(
                op_descriptor(op).is_some(),
                "registry {operation} performs undescribed op {op}"
            );
        }
    }
}

#[test]
fn every_synchronising_registry_operation_is_classified() {
    let classified: BTreeSet<&str> = REGISTRY.iter().map(|(name, _)| *name).collect();
    let unclassified: Vec<_> = hirsel_rt_operations::NATIVE_PATHS
        .iter()
        .filter(|name| REGISTRY_FAMILIES.iter().any(|f| name.starts_with(f)))
        .filter(|name| !classified.contains(*name))
        .collect();
    assert!(
        unclassified.is_empty(),
        "unclassified registry operations: {unclassified:?}"
    );
    let stale: Vec<_> = classified
        .iter()
        .filter(|name| !hirsel_rt_operations::NATIVE_PATHS.contains(name))
        .collect();
    assert!(
        stale.is_empty(),
        "registry rows of no native operation: {stale:?}"
    );
}

/// E is closed: its ops touch objects only, are internal and not head-only, and every other
/// such op is excluded for a stated reason (§4.1).
#[test]
fn the_early_commit_set_is_the_one_the_footprints_allow() {
    for name in E {
        let op = op_descriptor(name).unwrap_or_else(|| panic!("E names undescribed {name}"));
        assert!(op.objects_only(), "{name} in E touches more than objects");
        assert_eq!(
            op.visibility,
            Visibility::Internal,
            "{name} in E is external"
        );
        assert_ne!(op.class(), Class::H, "{name} in E is head-only");
    }
    for op in OPS {
        let candidate = op.objects_only() && op.class() != Class::H;
        let excluded = OUTSIDE_E.iter().any(|(name, _)| *name == op.name);
        assert!(
            !(E.contains(&op.name) && excluded),
            "{} both in and outside E",
            op.name
        );
        if candidate {
            assert!(
                E.contains(&op.name) || excluded,
                "{} could commit early: put it in E or OUTSIDE_E",
                op.name
            );
        } else {
            assert!(
                !excluded,
                "{} is no E candidate, so needs no exclusion",
                op.name
            );
        }
    }
    // §4.1's classes of the design's op table.
    let class = |name| op_descriptor(name).unwrap().class();
    for name in [
        "atomic.create",
        "atomic.store",
        "task.add",
        "task.detach",
        "drop.trivial",
    ] {
        assert_eq!(class(name), Class::D, "{name}");
    }
    for name in [
        "mutex.take",
        "channel.send",
        "channel.receive",
        "task.join",
        "drop.leftovers",
    ] {
        assert_eq!(class(name), Class::R, "{name}");
    }
    for name in [
        "shared.thaw",
        "host.request",
        "extern.call",
        "gate.enter",
        "stop",
    ] {
        assert_eq!(class(name), Class::H, "{name}");
    }
}

/// Every word compiled code touches inline has a descriptor.
#[test]
fn every_inline_effect_is_classified() {
    let abi = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../hirsel-lir/src/runtime_abi.rs"),
    )
    .unwrap();
    let words: BTreeSet<&str> = abi
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| w.starts_with("CTX_") && w.ends_with("_OFFSET"))
        .collect();
    assert!(!words.is_empty(), "found no run-context word");
    let unclassified: Vec<_> = words
        .iter()
        .filter(|w| !INLINE.iter().any(|i| i.word == **w))
        .collect();
    assert!(
        unclassified.is_empty(),
        "unclassified inline words: {unclassified:?}"
    );
    for inline in INLINE {
        assert_eq!(
            inline.locality,
            Locality::Local,
            "{} is not task-local",
            inline.word
        );
    }
}

/// Task-local code can invoke an unobservable concurrent service: copying a value
/// adds a Shared root. Each service is checked independently, so this does not permit
/// a path to the scheduler, Io binding or an observable op. Ordered budget
/// services may physically save a segment until head and retire existing logs;
/// they cannot create an operation or a scheduling point for the allocation or
/// refill itself. Their semantic-entry and whole-Run safety checks stay separate.
const PHYSICAL_BUDGET_SERVICES: &[&str] = &[
    "tasks::ordered::engine::reserve_allocation",
    "tasks::ordered::engine::fuel_refill",
    "tasks::ordered::engine::at_head",
];

// These save/adopt the same segment without publishing an operation. Their
// role acquisition and forbidden semantic paths are checked independently.
const PHYSICAL_AHEAD_SERVICES: &[&str] = &[
    "tasks::ordered::ahead::barrier",
    "tasks::ordered::ahead::refill",
];

// The idle-side counterparts of the budget services: the dispatcher settles a
// claimed private-ahead segment at head before switching into it
// (`tasks::ordered::ahead::native::run`). They retire the claimed task's
// existing logs as `fuel_refill` does, and are checked as it is.
const IDLE_BUDGET_SERVICES: &[&str] = &[
    "tasks::ordered::engine::ahead_refill",
    "tasks::ordered::engine::host_segment_barrier",
    "tasks::ordered::engine::ahead_allocation",
];

fn budget_service_violations(graph: &graph::Graph) -> Vec<String> {
    PHYSICAL_BUDGET_SERVICES
        .iter()
        .chain(PHYSICAL_AHEAD_SERVICES)
        .chain(IDLE_BUDGET_SERVICES)
        .filter_map(|service| {
            graph
                .reaches(service, |key| {
                    matches!(
                        key,
                        "tasks::ordered::engine::accepted"
                            | "tasks::ordered::engine::submit"
                            | "Frontier::offer"
                            | "Frontier::apply_head"
                            | "Frontier::continue_head"
                    )
                })
                .map(|path| path.join(" -> "))
        })
        .collect()
}

#[test]
fn ordered_budget_services_are_guarded_and_do_not_create_semantic_operations() {
    let graph = source();
    for (caller, service) in [
        ("alloc::ordered_reserve", PHYSICAL_BUDGET_SERVICES[0]),
        ("budget::refill", PHYSICAL_BUDGET_SERVICES[1]),
    ] {
        assert!(
            graph.directly_calls(caller, service),
            "missing service: {caller}"
        );
        assert!(
            graph.directly_calls(caller, "Guard::new"),
            "unguarded service: {caller}"
        );
    }
    for caller in [
        "budget::hirsel_rt_v2_fuel_refill",
        "budget::charge_primitive",
    ] {
        assert!(
            graph.directly_calls(caller, "budget::refill"),
            "missing guarded refill path: {caller}"
        );
    }
    let violations = budget_service_violations(graph);
    assert!(
        violations.is_empty(),
        "physical budget boundary creates an operation: {violations:?}"
    );
}

fn locality_violations(graph: &graph::Graph) -> Vec<String> {
    let mut violations = Vec::new();
    for (symbol, start) in graph.entries() {
        let Some(entry) = entry_descriptor(symbol) else {
            continue;
        };
        if !matches!(
            entry.locality,
            Locality::Local | Locality::Immutable | Locality::Concurrent
        ) {
            continue;
        }
        if let Some(path) = graph.reaches_until(
            start,
            |key| {
                if graph::FORBIDDEN.contains(&key) {
                    return true;
                }
                graph
                    .entry_symbol(key)
                    .and_then(entry_descriptor)
                    .is_some_and(|other| {
                        other.symbol != symbol
                            && other.locality != entry.locality
                            && !matches!(
                                (entry.locality, other.locality),
                                (Locality::Local, Locality::Concurrent)
                            )
                    })
            },
            |key| {
                entry.locality == Locality::Local
                    && (PHYSICAL_BUDGET_SERVICES.contains(&key)
                        || PHYSICAL_AHEAD_SERVICES.contains(&key))
            },
        ) {
            violations.push(format!(
                "{symbol} ({:?}): {}",
                entry.locality,
                path.join(" -> ")
            ));
        }
    }
    violations
}

/// The call-graph check (§4.1): every non-op entry stays in compatible localities.
#[test]
fn local_immutable_and_concurrent_entries_stay_in_their_locality() {
    let violations = locality_violations(source());
    assert!(
        violations.is_empty(),
        "locality violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn concurrent_services_remain_checked_when_called_from_local_entries() {
    let root_scratch = tempfile::Builder::new()
        .prefix("hirsel-descriptor-locality-")
        .tempdir()
        .unwrap();
    let root = root_scratch.path().to_path_buf();
    for (body, forbidden) in [
        ("", None),
        ("tasks::scheduler();", Some("tasks::scheduler")),
        ("Binding::get();", Some("Binding::get")),
        ("hirsel_rt_v2_net_read();", Some("hirsel_rt_v2_net_read")),
    ] {
        std::fs::write(
            root.join("lib.rs"),
            format!(
                "#[no_mangle] pub fn hirsel_rt_v2_unique_copy() {{ hirsel_rt_v2_root_dup(); }}
                 #[no_mangle] pub fn hirsel_rt_v2_root_dup() {{ {body} }}
                 #[no_mangle] pub fn hirsel_rt_v2_net_read() {{}}
                 mod tasks {{ pub fn scheduler() {{}} }}
                 struct Binding; impl Binding {{ fn get() {{}} }}"
            ),
        )
        .unwrap();
        let violations = locality_violations(&graph::Graph::parse(&root));
        if let Some(forbidden) = forbidden {
            assert_eq!(violations.len(), 1, "{violations:?}");
            assert!(
                violations[0].starts_with("hirsel_rt_v2_root_dup (Concurrent)"),
                "{violations:?}"
            );
            assert!(violations[0].contains(forbidden), "{violations:?}");
        } else {
            assert!(violations.is_empty(), "{violations:?}");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lexical_bindings_do_not_become_calls_to_same_named_functions() {
    let root_scratch = tempfile::Builder::new()
        .prefix("hirsel-descriptor-bindings-")
        .tempdir()
        .unwrap();
    let root = root_scratch.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    for (body, reaches) in [
        ("let saved = 1; consume(saved);", false),
        ("if let Some(saved) = Some(1) { consume(saved); }", false),
        ("let cb = |saved: u64| consume(saved);", false),
        ("for saved in [1] { consume(saved); }", false),
        ("let saved = saved(); consume(saved);", true),
        ("let saved = saved; consume(saved);", true),
        ("{ let saved = 1; consume(saved); } saved();", true),
        ("let cb = |saved: u64| consume(saved); saved();", true),
        ("for saved in [saved()] { consume(saved); }", true),
        ("if let Some(saved) = Some(1) {} else { saved(); }", true),
        (
            "match Some(1) { Some(saved) => consume(saved), None => saved() }",
            true,
        ),
        ("let saved = 1; worker::saved();", true),
    ] {
        std::fs::write(
            root.join("lib.rs"),
            format!(
                "#[no_mangle] pub fn hirsel_rt_v2_unique_copy() {{ worker::enqueue(); }}
             mod worker {{ pub fn enqueue() {{ {body} }}
               fn consume(_: u64) {{}} pub fn saved() {{ crate::tasks::scheduler(); }} }}
             mod tasks {{ pub fn scheduler() {{}} }}"
            ),
        )
        .unwrap();
        let graph = graph::Graph::parse(&root);
        assert_eq!(
            graph
                .reaches("hirsel_rt_v2_unique_copy", |target| target
                    == "tasks::scheduler")
                .is_some(),
            reaches,
            "{body}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn physical_budget_boundaries_preserve_the_other_audits() {
    let root_scratch = tempfile::Builder::new()
        .prefix("hirsel-descriptor-budgets-")
        .tempdir()
        .unwrap();
    let root = root_scratch.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    for (local, service, locality_red, semantic_red, run_red) in [
        ("", "crate::tasks::scheduler();", false, false, false),
        ("crate::tasks::scheduler();", "", true, false, false),
        ("crate::Binding::get();", "", true, false, false),
        ("", "submit();", false, true, false),
        ("", "crate::Frontier::offer();", false, true, false),
        ("", "crate::TaskCtx::run();", false, false, true),
    ] {
        std::fs::write(root.join("lib.rs"), format!(
            "#[no_mangle] pub fn hirsel_rt_v2_alloc() {{ tasks::ordered::engine::reserve_allocation(); {local} }}
             mod tasks {{ pub fn scheduler() {{}} pub mod ordered {{ pub mod engine {{
               pub fn reserve_allocation() {{ {service} }} fn submit() {{}}
             }} }} }}
             struct Binding; impl Binding {{ fn get() {{}} }}
             struct Frontier; impl Frontier {{ fn offer() {{}} }}
             struct TaskCtx; impl TaskCtx {{ fn run() {{}} }}"
        )).unwrap();
        let graph = graph::Graph::parse(&root);
        assert_eq!(
            !locality_violations(&graph).is_empty(),
            locality_red,
            "{local}/{service}"
        );
        assert_eq!(
            !budget_service_violations(&graph).is_empty(),
            semantic_red,
            "{local}/{service}"
        );
        assert_eq!(
            graph
                .reaches("hirsel_rt_v2_alloc", |key| key == "TaskCtx::run")
                .is_some(),
            run_red,
            "{local}/{service}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn physical_ahead_boundaries_preserve_the_other_audits() {
    let scratch = tempfile::Builder::new()
        .prefix("hirsel-descriptor-ahead-")
        .tempdir()
        .unwrap();
    let root = scratch.path();
    for service in ["barrier", "refill"] {
        for (local, body, locality_red, semantic_red, run_red) in [
            ("", "crate::tasks::scheduler();", false, false, false),
            ("crate::tasks::scheduler();", "", true, false, false),
            ("crate::Binding::get();", "", true, false, false),
            (
                "",
                "crate::tasks::ordered::engine::submit();",
                false,
                true,
                false,
            ),
            ("", "crate::Frontier::offer();", false, true, false),
            ("", "crate::TaskCtx::run();", false, false, true),
        ] {
            std::fs::write(root.join("lib.rs"), format!(
                "#[no_mangle] pub fn hirsel_rt_v2_alloc() {{ tasks::ordered::ahead::{service}(); {local} }}
                 mod tasks {{ pub fn scheduler() {{}} pub mod ordered {{
                   pub mod ahead {{ pub fn {service}() {{ {body} }} }}
                   pub mod engine {{ pub fn submit() {{}} }}
                 }} }}
                 struct Binding; impl Binding {{ fn get() {{}} }}
                 struct Frontier; impl Frontier {{ fn offer() {{}} }}
                 struct TaskCtx; impl TaskCtx {{ fn run() {{}} }}"
            )).unwrap();
            let graph = graph::Graph::parse(root);
            assert_eq!(
                !locality_violations(&graph).is_empty(),
                locality_red,
                "{service}: {local}/{body}"
            );
            assert_eq!(
                !budget_service_violations(&graph).is_empty(),
                semantic_red,
                "{service}: {local}/{body}"
            );
            assert_eq!(
                graph
                    .reaches("hirsel_rt_v2_alloc", |key| key == "TaskCtx::run")
                    .is_some(),
                run_red,
                "{service}: {local}/{body}"
            );
        }
    }
}

#[test]
fn physical_ahead_services_acquire_the_role_without_creating_operations() {
    let graph = source();
    assert!(graph.directly_calls(PHYSICAL_AHEAD_SERVICES[0], "tasks::role::enter"));
    assert!(graph.directly_calls(PHYSICAL_AHEAD_SERVICES[0], "pool::free::suspend_current"));
    assert!(graph.directly_calls(PHYSICAL_AHEAD_SERVICES[1], PHYSICAL_AHEAD_SERVICES[0]));
    let violations = budget_service_violations(graph);
    assert!(
        violations.is_empty(),
        "physical service creates an operation: {violations:?}"
    );
}

#[test]
fn repeated_super_paths_resolve_direct_and_imported_role_calls() {
    let scratch = tempfile::Builder::new()
        .prefix("hirsel-descriptor-super-")
        .tempdir()
        .unwrap();
    for path in ["super::super::role::enter", "crate::tasks::role::enter"] {
        for imported in [false, true] {
            let (import, call) = if imported {
                (format!("use {path} as claim;"), "claim".to_owned())
            } else {
                (String::new(), path.to_owned())
            };
            std::fs::write(scratch.path().join("lib.rs"), format!(
                "mod tasks {{ pub mod role {{ pub fn enter() {{}} }}
                  pub mod ordered {{ pub mod ahead {{ {import} pub fn barrier() {{ {call}(); }} }} }} }}"
            )).unwrap();
            let graph = graph::Graph::parse(scratch.path());
            assert!(
                graph.directly_calls("tasks::ordered::ahead::barrier", "tasks::role::enter"),
                "{path}, imported={imported}"
            );
        }
    }
}

/// Physical bookkeeping cannot emit an operation, phase, or ordinal. The
/// scheduling services retain their separate strict audits; this boundary is
/// the same caller contract used for task-local physical preflight.
fn physical_entry_violation(graph: &graph::Graph, key: &str) -> Option<Vec<String>> {
    graph.reaches_until(
        key,
        |target| {
            matches!(
                target,
                "tasks::ordered::engine::accepted"
                    | "tasks::ordered::engine::submit"
                    | "Frontier::offer"
                    | "Frontier::apply_head"
                    | "Frontier::continue_head"
                    | "tasks::free::record::begin"
                    | "tasks::free::record::publish"
                    | "tasks::free::record::publish_with_replay"
                    | "Log::subevents"
            )
        },
        |target| {
            PHYSICAL_BUDGET_SERVICES.contains(&target)
                || PHYSICAL_AHEAD_SERVICES.contains(&target)
                || IDLE_BUDGET_SERVICES.contains(&target)
        },
    )
}

#[test]
fn physical_classification_rejects_operation_and_ordinal_emission() {
    assert_eq!(
        entry_descriptor("hirsel_rt_v2_host_arm").unwrap().locality,
        Locality::Physical
    );
    let root = tempfile::tempdir().unwrap();
    for (body, forbidden) in [
        ("read_identity();", None),
        (
            "tasks::ordered::engine::submit();",
            Some("tasks::ordered::engine::submit"),
        ),
        (
            "tasks::free::record::begin();",
            Some("tasks::free::record::begin"),
        ),
        ("Log::subevents();", Some("Log::subevents")),
    ] {
        std::fs::write(
            root.path().join("lib.rs"),
            format!(
                r#"
            #[no_mangle] pub fn hirsel_rt_v2_host_arm() {{ Guard::new(); {body} }}
            struct Guard; impl Guard {{ fn new() {{}} }}
            fn read_identity() {{}}
            struct Log; impl Log {{ fn subevents() {{}} }}
            mod tasks {{
                pub mod ordered {{ pub mod engine {{ pub fn submit() {{}} }} }}
                pub mod free {{ pub mod record {{ pub fn begin() {{}} }} }}
            }}
        "#
            ),
        )
        .unwrap();
        let graph = graph::Graph::parse(root.path());
        assert!(graph.directly_calls("hirsel_rt_v2_host_arm", "Guard::new"));
        let violation = physical_entry_violation(&graph, "hirsel_rt_v2_host_arm");
        if let Some(forbidden) = forbidden {
            let path = violation.expect("a guarded Physical entry must reject semantic emission");
            assert_eq!(path.last().unwrap(), forbidden, "{body}: {path:?}");
        } else {
            assert_eq!(violation, None);
        }
    }
    // The real descriptor uses the identical predicate, and the stricter
    // ordinal check still accepts its actual operation-free body.
    assert_eq!(
        physical_entry_violation(
            source(),
            "tasks::ordered::ahead::host_return::hirsel_rt_v2_host_arm"
        ),
        None
    );
}

#[test]
fn conditional_exports_keep_their_descriptor_and_role_obligations() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("lib.rs"),
        r#"
        #[cfg_attr(not(feature="operation-latency"), unsafe(no_mangle))]
        pub fn hirsel_rt_v2_mutex_take() { Guard::new(); }
        #[cfg_attr(feature="ordinary-attribute", inline)]
        pub fn helper() {}
        struct Guard; impl Guard { fn new() {} }
    "#,
    )
    .unwrap();
    let graph = graph::Graph::parse(root.path());
    let entries: Vec<_> = graph.entries().collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "hirsel_rt_v2_mutex_take");
    assert!(graph.directly_calls(entries[0].1, "Guard::new"));
}
