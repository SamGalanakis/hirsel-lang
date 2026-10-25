use crate::{index::*, pack::index_benchmark_epoch};
use std::{collections::BTreeMap, fs};

fn entries(layer: u8) -> Vec<Entry> {
    // Both kinds share digests; overlaps and long-lived heads span the streams.
    [KeyKind::Content, KeyKind::Association]
        .into_iter()
        .flat_map(|kind| {
            (0..80u32).map(move |n| {
                let mut digest = [0; 32];
                digest[..4].copy_from_slice(&(n * 3 + u32::from(layer % 3)).to_be_bytes());
                Entry {
                    key: Key {
                        kind,
                        digest: ContentHash(digest),
                    },
                    location: Location {
                        segment: ContentHash([layer; 32]),
                        offset: 64 + 8 * n,
                        len: 72,
                    },
                }
            })
        })
        .collect()
}

fn budget() -> MaintenanceBudget {
    MaintenanceBudget {
        max_input_bytes: MAX_BYTES as u64,
        max_rows: MAX_ROWS as u32,
        max_scratch_bytes: MAX_SCRATCH as u64,
    }
}

#[test]
fn nonlexical_layers_merge_uniquely_to_canonical_bytes_with_held_epoch_and_pin() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let epoch = index_benchmark_epoch(root).unwrap();
    let mut deltas: Vec<_> = (0..5)
        .map(|i| {
            let entries = entries(i);
            let delta = build(root, &entries, Durability::Rebuildable).unwrap();
            (delta.descriptor.unwrap().id, delta, entries)
        })
        .collect();
    deltas.sort_unstable_by_key(|(id, _, _)| *id);
    // Git t5335-compact-multi-pack-index.sh:60-104: D C A B E publication
    // order must not be replaced by lexical layer-name order.
    let mut deltas: Vec<_> = deltas.into_iter().map(Some).collect();
    let mut expected = BTreeMap::new();
    for i in [3, 2, 0, 1, 4] {
        let (_, delta, entries) = deltas[i].take().unwrap();
        for entry in entries {
            expected.insert(entry.key, entry);
        }
        assert!(matches!(
            publish_delta(root, delta, &epoch).unwrap(),
            PublishOutcome::Published { .. }
        ));
    }
    let original = Manifest::read(root).unwrap();
    assert!(original.layers.windows(2).any(|w| w[0].id < w[1].id));
    assert!(original.layers.windows(2).any(|w| w[0].id > w[1].id));
    let held = load_pinned(root).unwrap();
    let expected: Vec<_> = expected.into_values().collect();
    assert_eq!(expected.len(), 480);
    assert!(matches!(
        compact(root, budget()).unwrap(),
        CompactOutcome::Published { layers: 1, .. }
    ));
    let current = load_pinned(root).unwrap();
    assert_eq!(current.0.layers[0].rows(), expected.len());
    let output = Manifest::read(root).unwrap().layers[0];
    assert_eq!(
        fs::read(storage::layer_path(root, output)).unwrap(),
        format::encode_layer(&expected).unwrap()
    );
    for descriptor in original.layers {
        fs::remove_file(storage::layer_path(root, descriptor)).unwrap();
    }
    for entry in expected {
        assert_eq!(find(&held, entry.key), Some(entry.location));
        assert_eq!(find(&current, entry.key), Some(entry.location));
    }
    assert_eq!(held.diagnostics(), IndexDiagnostics::default());
    assert_eq!(epoch.root(), root);
}

#[test]
fn shadowed_corrupt_layer_is_lazy_on_lookup_but_refuses_compaction() {
    for plane in 1..5 {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let epoch = index_benchmark_epoch(root).unwrap();
        for i in [0, 3] {
            let delta = build(root, &entries(i), Durability::Rebuildable).unwrap();
            assert!(matches!(
                publish_delta(root, delta, &epoch).unwrap(),
                PublishOutcome::Published { .. }
            ));
        }
        let manifest = Manifest::read(root).unwrap();
        let path = storage::layer_path(root, manifest.layers[1]);
        let mut bytes = fs::read(&path).unwrap();
        let start = u32::from_le_bytes(bytes[32 + plane * 8..36 + plane * 8].try_into().unwrap());
        bytes[start as usize] ^= 1;
        fs::write(path, bytes).unwrap();
        let pin = load_pinned(root).unwrap();
        assert_eq!(pin.diagnostics(), IndexDiagnostics::default());
        for entry in entries(3) {
            assert_eq!(find(&pin, entry.key), Some(entry.location));
        }
        assert_eq!(pin.diagnostics(), IndexDiagnostics::default());
        let before = fs::read(root.join("index/MANIFEST")).unwrap();
        assert_eq!(compact(root, budget()).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(root.join("index/MANIFEST")).unwrap(), before);
    }
}

#[test]
fn merge_orders_late_digest_bytes_and_kinds_with_newest_tie_precedence() {
    // Keys differ only past the first compared word, so every word and the
    // trailing byte of the cursor order must agree with Key's derived order.
    let key = |kind, byte: usize, value: u8| {
        let mut digest = [7; 32];
        digest[byte] = value;
        Key {
            kind,
            digest: ContentHash(digest),
        }
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let epoch = index_benchmark_epoch(root).unwrap();
    let mut expected = BTreeMap::new();
    for layer in 0..3u8 {
        let mut entries: Vec<_> = [KeyKind::Content, KeyKind::Association]
            .into_iter()
            .flat_map(|kind| [7, 15, 23, 31].map(|byte| (kind, byte)))
            .flat_map(|(kind, byte)| [layer, layer + 1].map(|v| key(kind, byte, v)))
            .map(|key| Entry {
                key,
                location: Location {
                    segment: ContentHash([layer; 32]),
                    offset: 64,
                    len: 8,
                },
            })
            .collect();
        entries.sort_unstable_by_key(|e| e.key);
        entries.dedup_by_key(|e| e.key);
        for entry in &entries {
            expected.insert(entry.key, *entry);
        }
        let delta = build(root, &entries, Durability::Rebuildable).unwrap();
        assert!(matches!(
            publish_delta(root, delta, &epoch).unwrap(),
            PublishOutcome::Published { .. }
        ));
    }
    let expected: Vec<_> = expected.into_values().collect();
    assert!(matches!(
        compact(root, budget()).unwrap(),
        CompactOutcome::Published { layers: 1, .. }
    ));
    let output = Manifest::read(root).unwrap().layers[0];
    assert_eq!(
        fs::read(storage::layer_path(root, output)).unwrap(),
        format::encode_layer(&expected).unwrap()
    );
    let pin = load_pinned(root).unwrap();
    for entry in expected {
        assert_eq!(find(&pin, entry.key), Some(entry.location));
    }
}
