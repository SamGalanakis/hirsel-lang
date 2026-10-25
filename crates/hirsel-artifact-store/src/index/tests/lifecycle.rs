use crate::{index::*, pack::index_benchmark_epoch};
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
        build(root, entries, crate::Durability::Rebuildable).unwrap(),
        &index_benchmark_epoch(root).unwrap(),
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
    assert!(absent.retained_bytes() > 0);
    assert_eq!(absent.retained_bytes(), absent.clone().retained_bytes());
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
    drop(build(root, &[entry(1)], crate::Durability::Rebuildable).unwrap());
    assert!(!root.join("index/MANIFEST").exists());
    published(publish(root, &[entry(1)]));
    let before = fs::read(root.join("index/MANIFEST")).unwrap();
    assert_eq!(publish(root, &[entry(1)]), PublishOutcome::Empty);
    assert_eq!(before, fs::read(root.join("index/MANIFEST")).unwrap());
    let other = tempfile::tempdir().unwrap();
    assert!(
        publish_delta(
            root,
            build(root, &[entry(2)], crate::Durability::Rebuildable).unwrap(),
            &index_benchmark_epoch(other.path()).unwrap()
        )
        .is_err()
    );
    assert!(build(root, &[entry(2), entry(1)], crate::Durability::Rebuildable).is_err());
    assert!(build(root, &[entry(1), entry(1)], crate::Durability::Rebuildable).is_err());
    let mut bad = entry(3);
    bad.location.offset = 65;
    assert!(build(root, &[bad], crate::Durability::Rebuildable).is_err());
}

#[test]
fn stale_common_pin_writers_preserve_disjoint_updates_and_serialized_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let stale = load_pinned(root).unwrap();
    let a = build(root, &[entry(1)], crate::Durability::Rebuildable).unwrap();
    let b = build(root, &[entry(2)], crate::Durability::Rebuildable).unwrap();
    let epoch = index_benchmark_epoch(root).unwrap();
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
    let epoch = index_benchmark_epoch(root).unwrap();
    let delta = build(root, &[entry(1)], crate::Durability::Rebuildable).unwrap();
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
        publish_delta(root, delta, &epoch).unwrap(),
        PublishOutcome::Skipped(SkipReason::Contended)
    );
    assert_eq!(compact(root, budget()).unwrap(), CompactOutcome::Deferred);
    drop(lock);
    assert!(!stage.exists());
}

#[test]
fn layer_installed_but_manifest_write_failure_never_changes_root_or_recovers_orphan() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    published(publish(root, &[entry(1)]));
    let original = fs::read(root.join("index/MANIFEST")).unwrap();
    let delta = build(root, &[entry(2)], crate::Durability::Rebuildable).unwrap();
    let stage = fs::read_dir(root.join("index/staging"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    // Fail create-new of the staged manifest after the completed layer link.
    fs::write(stage.join("manifest"), b"injected fault").unwrap();
    assert!(publish_delta(root, delta, &index_benchmark_epoch(root).unwrap()).is_err());
    assert_eq!(fs::read(root.join("index/MANIFEST")).unwrap(), original);
    assert!(!stage.exists());
    let pin = load_pinned(root).unwrap();
    assert_eq!(find(&pin, entry(1).key), Some(entry(1).location));
    assert_eq!(find(&pin, entry(2).key), None);
    assert_eq!(
        fs::read_dir(root.join("index"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("L-"))
            .count(),
        2
    );
    fs::remove_file(root.join("index/MANIFEST")).unwrap();
    assert_eq!(find(&load_pinned(root).unwrap(), entry(1).key), None);
}

#[test]
fn explicit_old_new_missing_torn_manifest_and_layer_survivor_permutations() {
    let source = tempfile::tempdir().unwrap();
    let root = source.path();
    published(publish(root, &[entry(1)]));
    let old = fs::read(root.join("index/MANIFEST")).unwrap();
    published(publish(root, &[entry(2)]));
    let new = fs::read(root.join("index/MANIFEST")).unwrap();
    let layers: Vec<_> = fs::read_dir(root.join("index"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("L-"))
        .map(|e| (e.file_name(), fs::read(e.path()).unwrap()))
        .collect();
    assert_eq!(layers.len(), 2);
    for manifest in 0..4 {
        for first in 0..3 {
            for second in 0..3 {
                let case = tempfile::tempdir().unwrap();
                fs::create_dir(case.path().join("index")).unwrap();
                match manifest {
                    0 => {}
                    1 => fs::write(case.path().join("index/MANIFEST"), &old).unwrap(),
                    2 => fs::write(case.path().join("index/MANIFEST"), &new).unwrap(),
                    _ => fs::write(case.path().join("index/MANIFEST"), &new[..new.len() - 1])
                        .unwrap(),
                }
                for ((name, bytes), state) in layers.iter().zip([first, second]) {
                    match state {
                        0 => {}
                        1 => fs::write(case.path().join("index").join(name), bytes).unwrap(),
                        _ => fs::write(
                            case.path().join("index").join(name),
                            &bytes[..bytes.len() - 1],
                        )
                        .unwrap(),
                    }
                }
                let pin = load_pinned(case.path()).unwrap();
                if manifest == 0 || manifest == 3 {
                    assert_eq!(find(&pin, entry(1).key), None);
                    assert_eq!(find(&pin, entry(2).key), None);
                }
                if manifest == 2 && (first != 1 || second != 1) {
                    assert_eq!(find(&pin, entry(1).key), None);
                    assert_eq!(find(&pin, entry(2).key), None);
                }
                if manifest == 2 && first == 1 && second == 1 {
                    for id in [1, 2] {
                        assert_eq!(find(&pin, entry(id).key), Some(entry(id).location));
                    }
                }
                if manifest == 1 {
                    assert_eq!(find(&pin, entry(2).key), None);
                }
            }
        }
    }
}

#[test]
fn diagnostics_distinguish_invalid_absence_and_count_each_bad_block_once_per_pin() {
    use std::os::unix::fs::FileExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert_eq!(
        load_pinned(root).unwrap().diagnostics(),
        IndexDiagnostics::default()
    );
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/MANIFEST"), b"corrupt root").unwrap();
    let invalid = load_pinned(root).unwrap();
    assert!(invalid.diagnostics().invalid_root);
    published(publish(root, &[entry(1)]));
    assert!(invalid.clone().diagnostics().invalid_root);
    assert_eq!(find(&invalid, entry(1).key), None);
    let path = fs::read_dir(root.join("index"))
        .unwrap()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().starts_with("L-"))
        .unwrap()
        .path();
    let good = fs::read(&path).unwrap();
    let mut bad = good.clone();
    let fingerprint = u32::from_le_bytes(bad[40..44].try_into().unwrap()) as usize;
    bad[fingerprint] ^= 1;
    fs::write(&path, bad).unwrap();
    let pin = load_pinned(root).unwrap();
    assert_eq!(pin.diagnostics(), IndexDiagnostics::default());
    for _ in 0..4 {
        assert_eq!(find(&pin, entry(1).key), None);
    }
    assert_eq!(
        pin.diagnostics(),
        IndexDiagnostics {
            invalid_root: false,
            bad_blocks: 1,
            bad_buckets: 0
        }
    );
    assert_eq!(pin.clone().diagnostics(), pin.diagnostics());
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.write_all_at(&good, 0).unwrap();
    assert_eq!(find(&pin, entry(1).key), None);
    assert_eq!(pin.diagnostics().bad_blocks, 1);
    let fresh = load_pinned(root).unwrap();
    assert_eq!(find(&fresh, entry(1).key), Some(entry(1).location));
    assert_eq!(fresh.diagnostics(), IndexDiagnostics::default());
}

#[test]
fn bounded_construction_scales_and_reopens_eight_layer_publication() {
    // Git t5335:60-104 vector intent: nonlexical chronological layer order,
    // objects repeated across layers occur once and the newest locator wins.
    let mut observed = Vec::new();
    for rows_per_layer in [128u64, 256, 512] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (layer, segment) in [3, 2, 0, 1, 4, 2, 1, 0].into_iter().enumerate() {
            // Layers l and l + 4 share every key with distinct locators.
            let entries: Vec<_> = (0..rows_per_layer)
                .map(|n| {
                    let mut e = entry(n * 4 + layer as u64 % 4);
                    // Exercise both key domains, skew and boundary crossings.
                    if n >= rows_per_layer / 2 {
                        e.key.kind = KeyKind::Association;
                    }
                    e.location.segment = ContentHash([segment; 32]);
                    e.location.offset = 64 + 8 * layer as u32;
                    e
                })
                .collect();
            published(publish(root, &entries));
        }
        let mut duplicate = entry(0);
        duplicate.location.offset = 1024;
        let manifest = format::Manifest::read(root).unwrap();
        let pin = load_pinned(root).unwrap();
        let scratch = compaction_scratch(&manifest.layers).unwrap();
        let old_root = fs::read(root.join("index/MANIFEST")).unwrap();
        assert_eq!(
            publish(root, &[duplicate]),
            PublishOutcome::Skipped(SkipReason::LayerLimit)
        );
        let short = MaintenanceBudget {
            max_scratch_bytes: scratch - 1,
            ..budget()
        };
        assert_eq!(compact(root, short).unwrap(), CompactOutcome::Deferred);
        assert_eq!(fs::read(root.join("index/MANIFEST")).unwrap(), old_root);
        let capacity = rows_per_layer as usize * 8;
        let actual = std::rc::Rc::new(std::cell::Cell::new(0));
        let sink = actual.clone();
        CONSTRUCTION_OBSERVER.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move |bytes| sink.set(bytes)))
        });
        let exact = MaintenanceBudget {
            max_scratch_bytes: scratch,
            ..budget()
        };
        assert!(matches!(
            compact(root, exact).unwrap(),
            CompactOutcome::Published { layers: 1, .. }
        ));
        assert!(actual.get() > 0);
        assert!(actual.get() <= format::construction_scratch(capacity).unwrap());
        observed.push(actual.get());
        let merged = load_pinned(root).unwrap();
        merged.0.layers[0].validate_all().unwrap();
        assert_eq!(merged.0.layers[0].rows(), capacity / 2);
        // Every key keeps the newest locator; the old pin stays readable.
        for layer in &pin.0.layers {
            for i in 0..layer.rows() {
                let key = layer.entry(i).unwrap().key;
                let newest = find(&pin, key).unwrap();
                assert_eq!(find(&merged, key), Some(newest));
                let n = u64::from_be_bytes(key.digest.0[..8].try_into().unwrap());
                assert_eq!(u64::from(newest.offset), 64 + 8 * (4 + n % 4));
            }
        }
        published(publish(root, &[duplicate]));
        assert_eq!(
            find(&load_pinned(root).unwrap(), duplicate.key),
            Some(duplicate.location)
        );
        assert_ne!(find(&pin, duplicate.key), Some(duplicate.location));
    }
    // Retained construction bytes scale linearly with merged input rows.
    for pair in observed.windows(2) {
        assert!(pair[1] <= pair[0] * 2 + (16 << 10), "{observed:?}");
        assert!(pair[1] >= pair[0] * 3 / 2, "{observed:?}");
    }
}

#[test]
fn interrupted_plane_construction_preserves_root_pins_and_cleans_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    published(publish(root, &[entry(1)]));
    published(publish(root, &[entry(2)]));
    let before = fs::read(root.join("index/MANIFEST")).unwrap();
    let old = load_pinned(root).unwrap();
    CONSTRUCTION_OBSERVER.with(|slot| *slot.borrow_mut() = Some(Box::new(|_| panic!("cancel construction"))));
    assert!(std::panic::catch_unwind(|| compact(root, budget())).is_err());
    assert_eq!(fs::read(root.join("index/MANIFEST")).unwrap(), before);
    assert_eq!(fs::read_dir(root.join("index/staging")).unwrap().count(), 0);
    for n in [1, 2] { assert_eq!(find(&old, entry(n).key), Some(entry(n).location)); }
    assert!(matches!(compact(root, budget()).unwrap(), CompactOutcome::Published { layers: 1, .. }));
}
