use super::*;
fn descriptor(i: u8, rows: u64, len: u64) -> Descriptor {
    Descriptor {
        id: ContentHash([i; 32]),
        rows,
        len,
    }
}
#[test]
fn admission_checks_each_aggregate_cap_without_allocating_from_counts() {
    assert_eq!(
        admission(&[descriptor(0, MAX_ROWS as u64, 128), descriptor(1, 1, 128)]),
        Some(SkipReason::RowLimit)
    );
    assert_eq!(
        admission(&[descriptor(0, 1, MAX_BYTES as u64), descriptor(1, 1, 128)]),
        Some(SkipReason::ByteLimit)
    );
    let layers: Vec<_> = (0..9).map(|i| descriptor(i, 1, 128)).collect();
    assert_eq!(admission(&layers), Some(SkipReason::LayerLimit));
    assert_eq!(
        admission(&[descriptor(0, u64::MAX, 128), descriptor(1, 1, 128)]),
        Some(SkipReason::RowLimit)
    );
    assert_eq!(
        admission(&[descriptor(0, 1, u64::MAX), descriptor(1, 1, 128)]),
        Some(SkipReason::ByteLimit)
    );
    assert_eq!(
        admission(&[descriptor(0, MAX_ROWS as u64, MAX_BYTES as u64)]),
        None
    );
}

#[test]
fn protocol_epoch_factory_observes_pin_and_compaction_lock_order() {
    use std::{fs, os::fd::AsRawFd, path::Path};
    fn entry(n: u8) -> Entry {
        Entry {
            key: Key {
                kind: KeyKind::Content,
                digest: ContentHash([n; 32]),
            },
            location: Location {
                segment: ContentHash([n; 32]),
                offset: 64,
                len: 128,
            },
        }
    }
    fn write(root: &Path, n: u8) {
        let epoch = crate::pack::index_benchmark_epoch(root).unwrap();
        let delta = build(root, &[entry(n)], crate::Durability::Rebuildable).unwrap();
        assert!(matches!(
            publish_delta(root, delta, &epoch).unwrap(),
            PublishOutcome::Published { .. }
        ));
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, 1);
    let path = root.to_owned();
    observe_pin(move || {
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.join(".pack-lock"))
            .unwrap();
        // SAFETY: a separate valid fd, nonblocking flock has no pointers.
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            -1
        );
    });
    let old = load_pinned(root).unwrap();
    write(root, 2);
    let path = root.to_owned();
    observe_compaction(move || {
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.join(".pack-lock"))
            .unwrap();
        // SAFETY: a separate valid fd; this short lock is released before write.
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        drop(lock);
        write(&path, 3);
    });
    assert!(matches!(
        compact(
            root,
            MaintenanceBudget {
                max_input_bytes: 128 << 20,
                max_rows: 2_000_000,
                max_scratch_bytes: 256 << 20
            }
        )
        .unwrap(),
        CompactOutcome::Published { layers: 2, .. }
    ));
    assert_eq!(find(&old, entry(1).key), Some(entry(1).location));
    assert_eq!(find(&old, entry(3).key), None);
    let current = load_pinned(root).unwrap();
    for n in [1, 2, 3] {
        assert_eq!(find(&current, entry(n).key), Some(entry(n).location));
    }
}

#[test]
fn exact_eight_by_250k_boundary_fits_existing_scratch_grant_without_row_allocation() {
    let layers: Vec<_> = (0..8)
        .map(|i| descriptor(i, 250_000, (128 << 20) / 8))
        .collect();
    assert_eq!(admission(&layers), None);
    assert_eq!(selection(&layers), 8);
    let scratch = compaction_scratch(&layers).unwrap();
    assert_eq!(layers.iter().map(|d| d.rows).sum::<u64>(), 2_000_000);
    assert_eq!(layers.iter().map(|d| d.len).sum::<u64>(), 128 << 20);
    assert!(scratch <= 256 << 20, "scratch={scratch}");
    // Charge the whole physical output cap and worst-case dictionary, plus
    // validation arrays, rather than merely decreasing the old estimate.
    assert!(scratch > (128 << 20) + 32 * 2_000_000);
    assert!(compaction_scratch(&[descriptor(0, u64::MAX, 128)]).is_none());
}
