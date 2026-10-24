use super::*;

fn node(
    run: MetadataRun,
    origin: Origin,
    logical: u64,
    children: &[Receipt<Record>],
) -> Receipt<Record> {
    snapshot(run, origin, Charge::Bytes(logical), &[], 1, children).unwrap()
}
fn scalar(run: MetadataRun) -> Receipt<Record> {
    node(run, Origin::Inline, 0, &[])
}
fn calls() -> u64 {
    crate::io_real::borrowed_net_tests::allocation_calls()
}
fn fits(charge: Charge, remaining: u64) -> bool {
    matches!(charge, Charge::Bytes(bytes) if bytes <= remaining)
}

#[test]
fn retained_dead_list_and_arena_root_both_charge_six_after_construction() {
    let run = MetadataRun::new();
    let mut inventory = Inventory::new(run);
    let heap = node(run, Origin::Heap, 3, &[]);
    let retained = node(
        run,
        Origin::Arena(inventory.grant()),
        16,
        &[heap.clone(), heap],
    );
    let arena_root = retained.clone();
    inventory.allocate(retained).unwrap();
    let unrelated = scalar(run);
    assert_eq!(inventory.entries().len(), 1);
    for root in [&unrelated, &arena_root] {
        let charge = inventory.seal_charge(root).unwrap();
        assert_eq!(charge, Charge::Bytes(6));
        assert!(!fits(charge, 21 - 16));
        assert!(fits(charge, 22 - 16));
        assert_eq!(Charge::Bytes(16).plus(charge), Charge::Bytes(22));
    }
    // Dropping the final external snapshot cannot remove the retained entry.
    drop(arena_root);
    assert_eq!(inventory.seal_charge(&unrelated), Ok(Charge::Bytes(6)));
}

#[test]
fn equal_distinct_heap_nodes_and_aliases_charge_per_occurrence() {
    let run = MetadataRun::new();
    for size in [1, 3] {
        let heap = node(run, Origin::Heap, size, &[]);
        let equal = node(run, Origin::Heap, size, &[]);
        assert_ne!(heap.generation(), equal.generation());
        for children in [[heap.clone(), heap.clone()], [heap.clone(), equal]] {
            let mut inventory = Inventory::new(run);
            let arena = node(run, Origin::Arena(inventory.grant()), 16, &children);
            inventory.allocate(arena.clone()).unwrap();
            let extra = Charge::Bytes(size * 2);
            assert_eq!(inventory.seal_charge(&arena), Ok(extra));
            assert_eq!(Charge::Bytes(16).plus(extra), Charge::Bytes(16 + size * 2));
        }
    }
}

#[test]
fn unique_growth_replaces_but_cow_retains_both_semantic_allocations() {
    let run = MetadataRun::new();
    let heap = node(run, Origin::Heap, 3, &[]);
    for cow in [false, true] {
        let mut inventory = Inventory::new(run);
        let one = node(
            run,
            Origin::Arena(inventory.grant()),
            8,
            std::slice::from_ref(&heap),
        );
        let old_snapshot = one.clone();
        let old_id = inventory.allocate(one).unwrap();
        let two = node(
            run,
            Origin::Arena(inventory.grant()),
            16,
            &[heap.clone(), heap.clone()],
        );
        if cow {
            let new_id = inventory.allocate(two.clone()).unwrap();
            assert_ne!(old_id, new_id);
            assert_eq!(inventory.entries().len(), 2);
            let charge = inventory.seal_charge(&two).unwrap();
            assert_eq!(charge, Charge::Bytes(9));
            assert!(!fits(charge, 32 - 24));
            assert!(fits(charge, 33 - 24));
        } else {
            let previous = inventory.replace(old_id, two.clone()).unwrap();
            assert_eq!(previous.generation(), old_snapshot.generation());
            assert_eq!(inventory.entries().len(), 1);
            assert_eq!(inventory.entries().next().unwrap().0, old_id);
            let charge = inventory.seal_charge(&two).unwrap();
            assert_eq!(charge, Charge::Bytes(6));
            assert!(!fits(charge, 21 - 16));
            assert!(fits(charge, 22 - 16));
        }
        assert_eq!(old_snapshot.record().logical_charge(), Charge::Bytes(8));
        assert_eq!(
            old_snapshot.record().outgoing_heap_charge(),
            Charge::Bytes(3)
        );
        assert_eq!(old_snapshot.children().len(), 1);
        assert_eq!(two.record().logical_charge(), Charge::Bytes(16));
    }
}

#[test]
fn inline_roots_heap_roots_and_nested_arena_edges_have_separate_obligations() {
    let run = MetadataRun::new();
    let mut inventory = Inventory::new(run);
    let heap = node(run, Origin::Heap, 3, &[]);
    let arena = node(
        run,
        Origin::Arena(inventory.grant()),
        8,
        std::slice::from_ref(&heap),
    );
    inventory.allocate(arena.clone()).unwrap();
    let parent = node(
        run,
        Origin::Arena(inventory.grant()),
        8,
        std::slice::from_ref(&arena),
    );
    inventory.allocate(parent.clone()).unwrap();
    assert_eq!(inventory.seal_charge(&parent), Ok(Charge::Bytes(3)));
    let inline = node(run, Origin::Inline, 0, &[arena, heap.clone()]);
    assert_eq!(inventory.seal_charge(&inline), Ok(Charge::Bytes(6)));
    let heap_root = node(run, Origin::Heap, 8, &[heap]);
    assert_eq!(inventory.seal_charge(&heap_root), Ok(Charge::Bytes(14)));
    let static_root = node(run, Origin::Static, 3, &[]);
    assert_eq!(static_root.record().heap_charge(), Charge::Bytes(3));
    let empty = Inventory::new(run);
    assert_eq!(empty.seal_charge(&scalar(run)), Ok(Charge::Bytes(0)));
}

#[test]
fn overflowing_entries_and_finite_sum_overflow_restore_without_recount() {
    let run = MetadataRun::new();
    let mut inventory = Inventory::new(run);
    let max = node(run, Origin::Heap, u64::MAX, &[]);
    let one = node(run, Origin::Heap, 1, &[]);
    let overflowing = node(
        run,
        Origin::Arena(inventory.grant()),
        8,
        &[max.clone(), one.clone()],
    );
    let id = inventory.allocate(overflowing).unwrap();
    let root = scalar(run);
    assert_eq!(inventory.seal_charge(&root), Ok(Charge::Overflow));
    assert!(!fits(inventory.seal_charge(&root).unwrap(), u64::MAX));
    let finite = node(
        run,
        Origin::Arena(inventory.grant()),
        8,
        std::slice::from_ref(&max),
    );
    inventory.replace(id, finite).unwrap();
    assert_eq!(inventory.seal_charge(&root), Ok(Charge::Bytes(u64::MAX)));
    let second = node(run, Origin::Arena(inventory.grant()), 8, &[one]);
    let second_id = inventory.allocate(second).unwrap();
    assert_eq!(inventory.seal_charge(&root), Ok(Charge::Overflow));
    let zero = node(run, Origin::Arena(inventory.grant()), 0, &[]);
    inventory.replace(second_id, zero).unwrap();
    assert_eq!(inventory.seal_charge(&root), Ok(Charge::Bytes(u64::MAX)));
    assert_eq!(inventory.seal_charge(&max), Ok(Charge::Overflow));
}

#[test]
fn transitive_foreign_grants_refuse_at_seal_and_replacement_clears_summary() {
    let run = MetadataRun::new();
    let mut own = Inventory::new(run);
    let other = Inventory::new(run);
    assert_ne!(own.grant(), other.grant());
    let foreign = node(run, Origin::Arena(other.grant()), 0, &[]);
    let intermediate = node(run, Origin::Heap, 3, &[foreign.clone()]);
    // Intermediate graphs may contain mixed grants; actual seal must refuse.
    let mixed = node(run, Origin::Arena(own.grant()), 8, &[intermediate]);
    let id = own.allocate(mixed).unwrap();
    assert_eq!(own.seal_charge(&scalar(run)), Err(Error::ForeignArena));
    let replacement = node(run, Origin::Arena(own.grant()), 0, &[]);
    own.replace(id, replacement).unwrap();
    assert_eq!(own.seal_charge(&scalar(run)), Ok(Charge::Bytes(0)));
    let inline = node(run, Origin::Inline, 0, &[foreign]);
    assert_eq!(own.seal_charge(&inline), Err(Error::ForeignArena));
}

#[test]
fn invalid_boundaries_runs_and_allocation_ids_refuse_without_mutation() {
    let run = MetadataRun::new();
    let mut inventory = Inventory::new(run);
    let mut other = Inventory::new(run);
    let heap = node(run, Origin::Heap, 3, &[]);
    let foreign_run = MetadataRun::new();
    let foreign = node(foreign_run, Origin::Inline, 0, &[]);
    let before = calls();
    for origin in [Origin::Inline, Origin::Shared] {
        assert!(matches!(
            snapshot(run, origin, Charge::Bytes(1), &[], 1, &[]),
            Err(Error::InvalidBoundary)
        ));
    }
    assert!(matches!(
        snapshot(
            run,
            Origin::Shared,
            Charge::Bytes(0),
            &[],
            1,
            std::slice::from_ref(&heap)
        ),
        Err(Error::InvalidBoundary)
    ));
    assert!(matches!(
        snapshot(
            run,
            Origin::Inline,
            Charge::Bytes(0),
            &[],
            1,
            &[heap.clone(), foreign.clone()]
        ),
        Err(Error::Metadata(super::super::Error::DifferentRun))
    ));
    assert_eq!(calls(), before);
    assert_eq!(inventory.allocate(heap), Err(Error::WrongAllocationOrigin));
    assert_eq!(
        inventory.allocate(foreign.clone()),
        Err(Error::Metadata(super::super::Error::DifferentRun))
    );
    assert_eq!(
        inventory.seal_charge(&foreign),
        Err(Error::Metadata(super::super::Error::DifferentRun))
    );
    let own_node = node(run, Origin::Arena(inventory.grant()), 0, &[]);
    let other_node = node(run, Origin::Arena(other.grant()), 0, &[]);
    assert_eq!(
        inventory.allocate(other_node.clone()),
        Err(Error::WrongAllocationOrigin)
    );
    let own_id = inventory.allocate(own_node.clone()).unwrap();
    let other_id = other.allocate(other_node).unwrap();
    assert_ne!(own_id, other_id);
    assert!(matches!(
        inventory.replace(other_id, own_node.clone()),
        Err(Error::UnknownAllocation)
    ));
    let missing = AllocationId {
        grant: inventory.grant(),
        ordinal: 100,
    };
    assert!(matches!(
        inventory.replace(missing, own_node),
        Err(Error::UnknownAllocation)
    ));
    assert_eq!(inventory.entries().len(), 1);
    assert_eq!(inventory.entries().next().unwrap().0, own_id);
    assert_eq!(inventory.seal_charge(&scalar(run)), Ok(Charge::Bytes(0)));
}

#[test]
fn semantic_metadata_clone_preflight_and_drop_do_not_allocate_or_touch_source_bytes() {
    let run = MetadataRun::new();
    let mut inventory = Inventory::new(run);
    // Descriptive bytes can contain a source address without granting access.
    // Metadata neither dereferences it nor claims language credit ownership.
    let mut source = [7u8; 8];
    let address = (source.as_mut_ptr() as usize).to_ne_bytes();
    let shared = snapshot(run, Origin::Shared, Charge::Bytes(0), &address, 8, &[]).unwrap();
    let arena = node(
        run,
        Origin::Arena(inventory.grant()),
        16,
        &[shared.clone(), shared.clone()],
    );
    inventory.allocate(arena.clone()).unwrap();
    let before = calls();
    let cloned = arena.clone();
    assert_eq!(inventory.seal_charge(&cloned), Ok(Charge::Bytes(0)));
    assert_eq!(shared.record().heap_charge(), Charge::Bytes(0));
    drop(arena);
    drop(cloned);
    drop(inventory);
    drop(shared);
    assert_eq!(calls(), before);
    assert_eq!(source, [7u8; 8]);
}
