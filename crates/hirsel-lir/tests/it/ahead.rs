use crate::support::{hash, l, table};
use hirsel_lir::ahead::{ScalarBody, ScalarGraph, scalar_image, stack_bound};
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn scalar_images_exclude_owners_resources_and_raw_pointers() {
    let table = table();
    for layout in [l::UNIT, l::U64, l::F32, l::F64, l::PAY, l::OPT] {
        assert!(scalar_image(&table, layout));
    }
    for layout in [l::PTR, l::BLK, l::PAIR, l::RES] {
        assert!(!scalar_image(&table, layout));
    }
}

fn graph() -> ScalarGraph {
    let root = hash("root");
    let leaf = hash("leaf");
    ScalarGraph {
        root,
        identity: hash("identity"),
        bodies: BTreeMap::from([
            (
                root,
                ScalarBody {
                    body: root,
                    calls: BTreeSet::from([leaf]),
                    fuel_points: vec![true],
                },
            ),
            (
                leaf,
                ScalarBody {
                    body: leaf,
                    calls: BTreeSet::new(),
                    fuel_points: vec![true],
                },
            ),
        ]),
    }
}

#[test]
fn exact_stack_cap_includes_startup_and_handoff() {
    let graph = graph();
    let mut frames = BTreeMap::from([(graph.root, 64), (hash("leaf"), 65392)]);
    assert_eq!(stack_bound(&graph, &frames, 16, 8, 64), Some(65536));
    *frames.get_mut(&hash("leaf")).unwrap() += 1;
    assert_eq!(stack_bound(&graph, &frames, 16, 8, 64), None);
}

#[test]
fn stack_proof_refuses_missing_callee_cycle_and_overflow() {
    let mut graph = graph();
    assert_eq!(
        stack_bound(&graph, &BTreeMap::from([(graph.root, 8)]), 16, 8, 64),
        None
    );
    let frames = BTreeMap::from([(graph.root, u64::MAX), (hash("leaf"), 16)]);
    assert_eq!(stack_bound(&graph, &frames, 16, 8, 64), None);
    graph
        .bodies
        .get_mut(&hash("leaf"))
        .unwrap()
        .calls
        .insert(graph.root);
    let frames = BTreeMap::from([(graph.root, 8), (hash("leaf"), 16)]);
    assert_eq!(stack_bound(&graph, &frames, 16, 8, 64), None);
}

#[test]
fn final_bound_includes_the_exact_depth_leaf_call_extent() {
    use hirsel_lir::ahead::stack_bound_with_depth;
    let graph = graph();
    let frames = BTreeMap::from([(graph.root, 64), (hash("leaf"), 65392)]);
    assert_eq!(stack_bound(&graph, &frames, 16, 8, 32), Some(65504));
    assert_eq!(
        stack_bound_with_depth(&graph, &frames, 16, 8, 32, 64),
        Some(65536)
    );
    assert_eq!(stack_bound_with_depth(&graph, &frames, 16, 8, 32, 65), None);
}
