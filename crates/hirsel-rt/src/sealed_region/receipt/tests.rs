use super::*;

fn holders<T: Copy>(receipt: &Receipt<T>) -> usize {
    // SAFETY: the supplied holder keeps the header live. These assertions use
    // a single thread or run after every competing thread has joined.
    unsafe { receipt.pointer.as_ref() }
        .holders
        .load(Ordering::Relaxed)
}

#[test]
fn repeated_child_occurrences_and_snapshot_clones_have_distinct_holder_rules() {
    let run = MetadataRun::new();
    let leaf = run.node(3u64, &[]).unwrap();
    let parent = run.node(16, &[leaf.clone(), leaf.clone()]).unwrap();
    assert_eq!(holders(&leaf), 3);
    let before = crate::io_real::borrowed_net_tests::allocation_calls();
    let snapshot = parent.clone();
    assert_eq!(
        before,
        crate::io_real::borrowed_net_tests::allocation_calls()
    );
    assert_eq!(holders(&parent), 2);
    assert_eq!(holders(&leaf), 3);
    let borrowed = snapshot.children().next().unwrap();
    assert_eq!(*borrowed.record(), 3);
    assert_eq!(holders(&leaf), 3);
    let before = crate::io_real::borrowed_net_tests::allocation_calls();
    let retained = borrowed.retain();
    assert_eq!(
        before,
        crate::io_real::borrowed_net_tests::allocation_calls()
    );
    assert_eq!(holders(&leaf), 4);
    assert_eq!(retained.generation(), leaf.generation());
    drop(parent);
    assert_eq!(holders(&leaf), 4);
    drop(snapshot);
    assert_eq!(holders(&leaf), 2);
    drop(retained);
    assert_eq!(holders(&leaf), 1);
}

#[test]
fn immutable_versions_preserve_old_inventory_snapshot() {
    let run = MetadataRun::new();
    let leaf = run.node((1u64, 3u64), &[]).unwrap();
    let old = run.node((2, 8), std::slice::from_ref(&leaf)).unwrap();
    let journal = old.clone();
    let new = run.node((2, 16), &[leaf.clone(), leaf.clone()]).unwrap();
    assert_eq!(*journal.record(), (2, 8));
    assert_eq!(*new.record(), (2, 16));
    assert_ne!(old.generation(), new.generation());
    assert_eq!(journal.children().len(), 1);
    assert_eq!(new.children().len(), 2);
    drop(old);
    assert_eq!(holders(&leaf), 4);
    drop(new);
    assert_eq!(holders(&leaf), 2);
    drop(journal);
    assert_eq!(holders(&leaf), 1);
}

#[test]
fn different_run_child_refuses_before_any_holder_acquisition() {
    let first = MetadataRun::new();
    let second = MetadataRun::new();
    let leaf = first.node(7u64, &[]).unwrap();
    let own = second.node(8, &[]).unwrap();
    let children = [own.clone(), leaf.clone()];
    let before = crate::io_real::borrowed_net_tests::allocation_calls();
    assert!(matches!(
        second.node(0, &children),
        Err(Error::DifferentRun)
    ));
    assert_eq!(
        before,
        crate::io_real::borrowed_net_tests::allocation_calls()
    );
    assert_eq!(holders(&leaf), 2);
    assert_eq!(holders(&own), 2);
    drop(children);
    assert_eq!(holders(&leaf), 1);
    assert_eq!(holders(&own), 1);
    assert_eq!(leaf.run(), first);
}

#[test]
fn diamond_release_allocates_nothing_and_retires_every_retained_edge() {
    let run = MetadataRun::new();
    let leaf = run.node(3u64, &[]).unwrap();
    let left = run.node(1, std::slice::from_ref(&leaf)).unwrap();
    let right = run.node(2, std::slice::from_ref(&leaf)).unwrap();
    let root = run.node(0, &[left, right]).unwrap();
    assert_eq!(holders(&leaf), 3);
    let before = crate::io_real::borrowed_net_tests::allocation_calls();
    drop(root);
    assert_eq!(
        before,
        crate::io_real::borrowed_net_tests::allocation_calls()
    );
    assert_eq!(holders(&leaf), 1);
}

#[test]
fn receiver_retains_a_child_after_source_thread_and_parent_exit() {
    let root = std::thread::spawn(|| {
        let run = MetadataRun::new();
        let leaf = run.node(7u64, &[]).unwrap();
        run.node(1, &[leaf]).unwrap()
    })
    .join()
    .unwrap();
    let child = root.children().next().unwrap().retain();
    drop(root);
    assert_eq!(*child.record(), 7);
    assert_eq!(holders(&child), 1);
    std::thread::spawn(move || {
        let before = crate::io_real::borrowed_net_tests::allocation_calls();
        drop(child);
        assert_eq!(
            before,
            crate::io_real::borrowed_net_tests::allocation_calls()
        );
    })
    .join()
    .unwrap();
}

#[test]
fn concurrent_last_holders_release_each_child_occurrence_once() {
    let run = MetadataRun::new();
    let leaf = run.node(7u64, &[]).unwrap();
    let root = run.node(0, &[leaf.clone(), leaf.clone()]).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
    let mut threads = Vec::new();
    for _ in 0..4 {
        let root = root.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            let before = crate::io_real::borrowed_net_tests::allocation_calls();
            drop(root);
            assert_eq!(
                before,
                crate::io_real::borrowed_net_tests::allocation_calls()
            );
        }));
    }
    drop(root);
    // Workers cannot consume their holders until the source holder is gone.
    // Thus a worker, rather than this thread, performs the final release.
    barrier.wait();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(holders(&leaf), 1);
}

#[test]
fn deep_release_uses_constant_stack_and_zero_scratch_allocations() {
    std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(|| {
            let run = MetadataRun::new();
            let leaf = run.node(7u64, &[]).unwrap();
            let mut root = leaf.clone();
            for ordinal in 0..1536 {
                root = run.node(ordinal, std::slice::from_ref(&root)).unwrap();
            }
            assert_eq!(holders(&leaf), 2);
            let before = crate::io_real::borrowed_net_tests::allocation_calls();
            drop(root);
            assert_eq!(
                before,
                crate::io_real::borrowed_net_tests::allocation_calls()
            );
            assert_eq!(holders(&leaf), 1);
        })
        .unwrap()
        .join()
        .unwrap();
}
