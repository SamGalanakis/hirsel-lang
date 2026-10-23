use crate::{index::*, pack::EpochLease};
use hirsel_syntax::ContentHash;
use std::{fs, path::Path};

fn entry(n: u64) -> Entry {
    let mut digest = [0; 32];
    digest[..8].copy_from_slice(&n.to_be_bytes());
    Entry {
        key: Key {
            kind: KeyKind::Content,
            digest: ContentHash(digest),
        },
        location: Location {
            segment: ContentHash([n as u8; 32]),
            offset: 64,
            len: 128,
        },
    }
}
fn publish(root: &Path, entries: &[Entry]) -> PublishOutcome {
    publish_delta(
        root,
        build(root, entries).unwrap(),
        &EpochLease(root.to_owned()),
    )
    .unwrap()
}
fn budget() -> MaintenanceBudget {
    MaintenanceBudget {
        max_input_bytes: 128 << 20,
        max_rows: 2_000_000,
        max_scratch_bytes: 256 << 20,
    }
}
fn published(outcome: PublishOutcome) {
    assert!(
        matches!(outcome, PublishOutcome::Published { .. }),
        "{outcome:?}"
    );
}

#[test]
fn pins_cache_absence_clone_and_hold_unlinked_backings() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let absent = load_pinned(root).unwrap();
    published(publish(root, &[entry(1)]));
    assert_eq!(find(&absent, entry(1).key), None);
    let old = load_pinned(root).unwrap();
    let cloned = old.clone();
    assert!(old.retained_bytes() > 128);
    published(publish(root, &[entry(2)]));
    let current = load_pinned(root).unwrap();
    assert_eq!(find(&old, entry(2).key), None);
    assert_eq!(find(&current, entry(2).key), Some(entry(2).location));
    for path in fs::read_dir(root.join("index"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
    {
        if path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("L-")
        {
            fs::remove_file(path).unwrap();
        }
    }
    assert_eq!(find(&cloned, entry(1).key), Some(entry(1).location));
    assert_eq!(find(&load_pinned(root).unwrap(), entry(1).key), None);
}

#[test]
fn empty_drop_duplicate_and_root_mismatch_never_publish() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert_eq!(publish(root, &[]), PublishOutcome::Empty);
    drop(build(root, &[entry(1)]).unwrap());
    assert!(!root.join("index/MANIFEST").exists());
    published(publish(root, &[entry(1)]));
    let before = fs::read(root.join("index/MANIFEST")).unwrap();
    assert_eq!(publish(root, &[entry(1)]), PublishOutcome::Empty);
    assert_eq!(before, fs::read(root.join("index/MANIFEST")).unwrap());
    let other = tempfile::tempdir().unwrap();
    assert!(
        publish_delta(
            root,
            build(root, &[entry(2)]).unwrap(),
            &EpochLease(other.path().to_owned())
        )
        .is_err()
    );
    assert!(build(root, &[entry(2), entry(1)]).is_err());
    assert!(build(root, &[entry(1), entry(1)]).is_err());
    let mut bad = entry(3);
    bad.location.offset = 65;
    assert!(build(root, &[bad]).is_err());
}

#[test]
fn stale_common_pin_writers_preserve_disjoint_updates_and_serialized_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let stale = load_pinned(root).unwrap();
    let a = build(root, &[entry(1)]).unwrap();
    let b = build(root, &[entry(2)]).unwrap();
    let epoch = EpochLease(root.to_owned());
    published(publish_delta(root, a, &epoch).unwrap());
    let old_manifest = fs::read(root.join("index/MANIFEST")).unwrap();
    published(publish_delta(root, b, &epoch).unwrap());
    assert_eq!(find(&stale, entry(1).key), None);
    let pin = load_pinned(root).unwrap();
    for n in [1, 2] {
        assert_eq!(find(&pin, entry(n).key), Some(entry(n).location));
    }
    let mut newer = entry(1);
    newer.location.offset = 128;
    published(publish(root, &[newer]));
    assert_eq!(
        find(&load_pinned(root).unwrap(), newer.key),
        Some(newer.location)
    );
    fs::write(root.join("index/MANIFEST"), old_manifest).unwrap();
    assert_eq!(
        find(&load_pinned(root).unwrap(), newer.key),
        Some(entry(1).location)
    );
    assert_eq!(find(&pin, entry(2).key), Some(entry(2).location));
}

#[test]
fn ratio_equality_accumulated_suffix_and_newest_duplicate_compact() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let old: Vec<_> = (0..4).map(entry).collect();
    published(publish(root, &old));
    let mut newest = entry(0);
    newest.location.offset = 128;
    assert!(matches!(
        publish(root, &[newest]),
        PublishOutcome::Published {
            maintenance_due: true,
            ..
        }
    ));
    assert_eq!(
        compact(
            root,
            MaintenanceBudget {
                max_rows: 4,
                ..budget()
            }
        )
        .unwrap(),
        CompactOutcome::Deferred
    );
    assert!(matches!(
        compact(root, budget()).unwrap(),
        CompactOutcome::Published { layers: 1, .. }
    ));
    let pin = load_pinned(root).unwrap();
    assert_eq!(find(&pin, newest.key), Some(newest.location));
    for e in &old[1..] {
        assert_eq!(find(&pin, e.key), Some(e.location));
    }
    assert_eq!(compact(root, budget()).unwrap(), CompactOutcome::NotNeeded);
    // Oldest 20 does not fit 4*youngest(1), but fits 4*(1+4).
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    published(publish(root, &(0..20).map(entry).collect::<Vec<_>>()));
    published(publish(root, &(100..104).map(entry).collect::<Vec<_>>()));
    published(publish(root, &[entry(200)]));
    assert!(matches!(
        compact(root, budget()).unwrap(),
        CompactOutcome::Published { layers: 1, .. }
    ));
}

#[test]
fn eight_layer_admission_budget_and_forced_youngest_pair() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Geometrically decreasing sizes prevent ratio-triggered maintenance.
    let mut next = 0;
    for n in [78125, 15625, 3125, 625, 125, 25, 5, 1] {
        let entries: Vec<_> = (next..next + n).map(entry).collect();
        next += n;
        published(publish(root, &entries));
    }
    assert_eq!(
        publish(root, &[entry(next)]),
        PublishOutcome::Skipped(SkipReason::LayerLimit)
    );
    assert_eq!(
        compact(
            root,
            MaintenanceBudget {
                max_scratch_bytes: 0,
                ..budget()
            }
        )
        .unwrap(),
        CompactOutcome::Deferred
    );
    assert!(matches!(
        compact(root, budget()).unwrap(),
        CompactOutcome::Published { layers: 7, .. }
    ));
    published(publish(root, &[entry(next)]));
}

#[test]
fn lock_contention_is_bounded_and_live_staging_has_owner() {
    use std::os::fd::AsRawFd;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let delta = build(root, &[entry(1)]).unwrap();
    let stage = fs::read_dir(root.join("index/staging"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let owner = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(stage.join("owner.lock"))
        .unwrap();
    // SAFETY: valid local fd, no pointers.
    assert_eq!(
        unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        -1
    );
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".pack-lock"))
        .unwrap();
    // SAFETY: valid local fd, no pointers.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    assert_eq!(find(&load_pinned(root).unwrap(), entry(1).key), None);
    assert_eq!(
        publish_delta(root, delta, &EpochLease(root.to_owned())).unwrap(),
        PublishOutcome::Skipped(SkipReason::Contended)
    );
    assert_eq!(compact(root, budget()).unwrap(), CompactOutcome::Deferred);
    drop(lock);
    assert!(!stage.exists());
}

#[test]
fn crash_survivors_missing_layers_and_old_format_are_misses() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    published(publish(root, &[entry(1)]));
    let manifest = fs::read(root.join("index/MANIFEST")).unwrap();
    for end in 0..manifest.len() {
        fs::write(root.join("index/MANIFEST"), &manifest[..end]).unwrap();
        assert_eq!(
            find(&load_pinned(root).unwrap(), entry(1).key),
            None,
            "manifest cut {end}"
        );
    }
    fs::write(root.join("index/MANIFEST"), b"old index format").unwrap();
    assert_eq!(find(&load_pinned(root).unwrap(), entry(1).key), None);
    // Complete orphan layers are never recovered by scanning.
    published(publish(root, &[entry(2)]));
    assert_eq!(find(&load_pinned(root).unwrap(), entry(1).key), None);
    assert_eq!(current_layer_ids_locked(root).unwrap().len(), 1);
}
