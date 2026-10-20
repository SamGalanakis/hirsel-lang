//! The operation descriptors (`hirsel_rt_operations::descriptors`, multicore design §4.1)
//! against the runtime's source: every exported entry and every task, channel, mutex, atomic and
//! `Shared` registry operation is classified, the E set is the one the footprints allow, and no
//! entry marked task-local, immutable or concurrent reaches the scheduler, the Io binding or an
//! entry of another locality.

hirsel_guard::install!();

mod graph;

use hirsel_rt_operations::descriptors::{
    Class, E, ENTRIES, INLINE, Locality, OPS, OUTSIDE_E, REGISTRY, REGISTRY_FAMILIES, Visibility,
    entry_descriptor, op_descriptor,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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

/// The call-graph check (§4.1): an entry marked task-local, immutable or concurrent never
/// reaches `scheduler()`, the Io binding, or an entry of another locality. A stop is the
/// executor's terminal boundary and is not followed.
#[test]
fn local_immutable_and_concurrent_entries_stay_in_their_locality() {
    let graph = source();
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
        if let Some(path) = graph.reaches(start, |key| {
            if graph::FORBIDDEN.contains(&key) {
                return true;
            }
            graph
                .entry_symbol(key)
                .and_then(entry_descriptor)
                .is_some_and(|other| other.symbol != symbol && other.locality != entry.locality)
        }) {
            violations.push(format!(
                "{symbol} ({:?}): {}",
                entry.locality,
                path.join(" -> ")
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "locality violations:\n{}",
        violations.join("\n")
    );
}
