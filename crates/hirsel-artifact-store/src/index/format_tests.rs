use super::*;

fn fixture_entries(n: usize) -> Vec<Entry> {
    (0..n)
        .map(|i| {
            let mut digest = [0; 32];
            digest[24..].copy_from_slice(&(i as u64).to_be_bytes());
            Entry {
                key: Key {
                    kind: KeyKind::Content,
                    digest: ContentHash(digest),
                },
                location: Location {
                    segment: ContentHash([(i % 7) as u8; 32]),
                    offset: 64 + 8 * (i % 16) as u32,
                    len: 128,
                },
            }
        })
        .collect()
}
fn descriptor(bytes: &[u8], rows: usize) -> Descriptor {
    Descriptor {
        id: ContentHash::of_parts("hirsel.artifact.index-layer.v36", [bytes]),
        rows: rows as u64,
        len: bytes.len() as u64,
    }
}
fn open(bytes: &[u8], n: usize) -> (tempfile::TempDir, io::Result<Layer>) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("index")).unwrap();
    let d = descriptor(bytes, n);
    fs::write(layer_path(dir.path(), d), bytes).unwrap();
    let layer = Layer::open(dir.path(), d);
    (dir, layer)
}
fn repair_header(bytes: &mut [u8]) {
    let c = mask(crc32c::crc32c(&bytes[..124]));
    put32(bytes, 124, c);
}
fn repair_plane(bytes: &mut [u8], j: usize) {
    let start = u32_at(bytes, 32 + j * 8).unwrap() as usize;
    let len = u32_at(bytes, 36 + j * 8).unwrap() as usize;
    for k in 0..len.div_ceil(BLOCK) {
        let lo = start + k * STRIDE;
        let hi = lo + (len - k * BLOCK).min(BLOCK);
        bytes[hi] = (j + 1) as u8;
        let c = crc(&bytes[lo..hi], j, k);
        put32(bytes, hi + 1, c);
    }
}
fn change_logical(bytes: &mut [u8], j: usize, offset: usize, data: &[u8]) {
    let plane = Plane {
        start: u32_at(bytes, 32 + j * 8).unwrap() as usize,
        len: u32_at(bytes, 36 + j * 8).unwrap() as usize,
        status: 0,
    };
    plane.write(bytes, offset, data);
}

#[test]
fn crc32c_leveldb_vectors_masks_and_extend() {
    let vectors = [
        (vec![0; 32], 0x8a9136aa),
        (vec![255; 32], 0x62a8ab43),
        ((0u8..32).collect(), 0x46dd794e),
        ((0u8..32).rev().collect(), 0x113fdb5c),
        (b"123456789".to_vec(), 0xe3069283),
    ];
    for (bytes, expected) in vectors {
        assert_eq!(crc32c::crc32c(&bytes), expected);
        assert_eq!(
            mask(expected).wrapping_sub(0xa282ead8).rotate_left(15),
            expected
        );
        for split in 0..=bytes.len() {
            assert_eq!(
                crc32c::crc32c_append(crc32c::crc32c(&bytes[..split]), &bytes[split..]),
                expected
            );
        }
    }
    assert_ne!(crc(b"a", 0, 0), crc(b"a", 1, 0));
    assert_ne!(crc(b"a", 0, 0), crc(b"a", 0, 1));
    assert_ne!(crc(b"a", 0, 0), crc(b"a\0", 0, 0));
}

#[test]
fn sse_and_forced_generic_all_values_borrows_unaligned_and_partial_masks() {
    for tag in 0..=255u8 {
        for shift in 0..16 {
            let mut backing = [0; 32];
            for lane in 0..16 {
                backing[shift + lane] = match lane % 4 {
                    0 => tag,
                    1 => tag ^ 1,
                    2 => tag.wrapping_add(128),
                    _ => lane as u8,
                };
            }
            let group: &[u8; 16] = backing[shift..shift + 16].try_into().unwrap();
            let scalar = group
                .iter()
                .enumerate()
                .fold(0, |mask, (i, b)| mask | ((u16::from(*b == tag)) << i));
            assert_eq!(crate::group::matches(group, tag), scalar);
            assert_eq!(crate::group::generic(group, tag), scalar);
            for n in 1..=16 {
                let valid = (u16::MAX as u32 >> (16 - n)) as u16;
                assert_eq!(crate::group::generic(group, tag) & valid, scalar & valid);
            }
        }
    }
}

#[test]
fn canonical_roundtrip_padding_cross_block_rows_domains_and_skew() {
    for n in [1, 15, 16, 17, 125, 342, 4095, 4096, 4097] {
        let entries = fixture_entries(n);
        let bytes = encode_layer(&entries).unwrap();
        let (_dir, layer) = open(&bytes, n);
        let layer = layer.unwrap();
        assert_eq!(layer.rows(), n);
        layer.validate_all().unwrap();
        for entry in entries {
            assert_eq!(layer.find(entry.key), Some(entry.location), "N={n}");
        }
    }
    let mut entries = fixture_entries(17);
    let mut association = entries[0];
    association.key.kind = KeyKind::Association;
    entries.push(association);
    let bytes = encode_layer(&entries).unwrap();
    let (_dir, layer) = open(&bytes, entries.len());
    let layer = layer.unwrap();
    assert_eq!(layer.find(entries[0].key), Some(entries[0].location));
    assert_eq!(layer.find(association.key), Some(association.location));
    assert!(encode_layer(&[]).is_err());
}

#[test]
fn every_byte_truncation_and_header_field_rejection() {
    let entries = fixture_entries(2);
    let valid = encode_layer(&entries).unwrap();
    for end in 0..valid.len() {
        let (_dir, result) = open(&valid[..end], 2);
        assert!(result.is_err(), "cut {end}");
    }
    for offset in [
        0, 8, 12, 16, 20, 24, 25, 26, 27, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80,
        123, 124,
    ] {
        let mut bytes = valid.clone();
        bytes[offset] ^= 255;
        if offset < 124 {
            repair_header(&mut bytes);
        }
        let (_dir, result) = open(&bytes, 2);
        assert!(result.is_err(), "header {offset}");
    }
    for (offset, value) in [
        (16, u32::MAX),
        (20, u32::MAX),
        (32, u32::MAX),
        (36, u32::MAX),
        (72, u32::MAX),
        (76, u32::MAX),
    ] {
        let mut bytes = valid.clone();
        put32(&mut bytes, offset, value);
        repair_header(&mut bytes);
        assert!(open(&bytes, 2).1.is_err());
    }
    let mut bytes = valid;
    bytes.push(0);
    assert!(open(&bytes, 2).1.is_err());
}

#[test]
fn manifest_truncations_limits_duplicates_magic_reserved_and_crc() {
    let bytes = encode_layer(&fixture_entries(2)).unwrap();
    let d = descriptor(&bytes, 2);
    let m = Manifest {
        number: 1,
        layers: vec![d],
    };
    let good = m.encode().unwrap();
    assert!(Manifest::decode(&good).is_some());
    for end in 0..good.len() {
        assert!(Manifest::decode(&good[..end]).is_none());
    }
    for at in 0..good.len() {
        let mut bad = good.clone();
        bad[at] ^= 1;
        assert!(Manifest::decode(&bad).is_none(), "byte {at}");
    }
    for (offset, value) in [(8, 2), (12, 31), (24, 9), (24, u32::MAX), (28, 1)] {
        let mut bad = good.clone();
        put32(&mut bad, offset, value);
        let end = bad.len() - 4;
        let c = mask(crc32c::crc32c(&bad[..end]));
        put32(&mut bad, end, c);
        assert!(Manifest::decode(&bad).is_none());
    }
    for (offset, value) in [(16, 0), (64, 0), (64, u64::MAX), (72, u64::MAX)] {
        let mut bad = good.clone();
        put64(&mut bad, offset, value);
        let end = bad.len() - 4;
        let c = mask(crc32c::crc32c(&bad[..end]));
        put32(&mut bad, end, c);
        assert!(Manifest::decode(&bad).is_none());
    }
    let duplicate = Manifest {
        number: 1,
        layers: vec![d, d],
    }
    .encode()
    .unwrap();
    assert!(Manifest::decode(&duplicate).is_none());
    assert!(
        Manifest::decode(
            &Manifest {
                number: 1,
                layers: vec![]
            }
            .encode()
            .unwrap()
        )
        .is_some()
    );
}

#[test]
fn corruption_each_plane_and_trailer_is_lazy_and_bad_is_cached() {
    let entries = fixture_entries(2);
    let valid = encode_layer(&entries).unwrap();
    for j in 0..5 {
        let start = u32_at(&valid, 32 + j * 8).unwrap() as usize;
        let len = u32_at(&valid, 36 + j * 8).unwrap() as usize;
        for at in [start, start + len, start + len + 1] {
            let mut bytes = valid.clone();
            bytes[at] ^= 1;
            let (_dir, result) = open(&bytes, 2);
            if j == 0 {
                assert!(result.is_err());
            } else {
                let layer = result.unwrap();
                assert_eq!(layer.find(entries[0].key), None, "plane {j} byte {at}");
                assert!(layer.validate_all().is_err());
                assert_eq!(layer.find(entries[0].key), None);
            }
        }
    }
    // Last key/locator/dictionary blocks are initially untouched by this hit.
    let entries = fixture_entries(4097);
    let valid = encode_layer(&entries).unwrap();
    for j in [3, 4] {
        let mut bytes = valid.clone();
        let start = u32_at(&bytes, 32 + j * 8).unwrap() as usize;
        let len = u32_at(&bytes, 36 + j * 8).unwrap() as usize;
        bytes[start + (len.div_ceil(BLOCK) - 1) * STRIDE] ^= 1;
        let (_dir, result) = open(&bytes, entries.len());
        let layer = result.unwrap();
        if j == 3 {
            assert_eq!(layer.find(entries[0].key), Some(entries[0].location));
        }
        assert!(layer.validate_all().is_err());
    }
}

#[test]
fn block_position_type_and_length_binding_and_padding() {
    let entries = fixture_entries(4097);
    let valid = encode_layer(&entries).unwrap();
    let mut bytes = valid.clone();
    let start = u32_at(&bytes, 48).unwrap() as usize;
    let a = bytes[start..start + STRIDE].to_vec();
    let b = bytes[start + STRIDE..start + 2 * STRIDE].to_vec();
    bytes[start..start + STRIDE].copy_from_slice(&b);
    bytes[start + STRIDE..start + 2 * STRIDE].copy_from_slice(&a);
    let (_dir, result) = open(&bytes, entries.len());
    assert!(result.unwrap().validate_all().is_err());
    for j in 1..5 {
        let mut bytes = valid.clone();
        let start = u32_at(&bytes, 32 + j * 8).unwrap() as usize;
        let length = u32_at(&bytes, 36 + j * 8).unwrap() as usize;
        bytes[start + length.min(BLOCK)] = (j as u8) ^ 0xff;
        let (_dir, result) = open(&bytes, entries.len());
        assert!(result.unwrap().validate_all().is_err());
    }
    let mut bytes = encode_layer(&entries[..17]).unwrap();
    change_logical(&mut bytes, 1, 17, &[1]);
    repair_plane(&mut bytes, 1);
    let (_dir, result) = open(&bytes, 17);
    assert_eq!(result.unwrap().find(entries[0].key), None);
}

#[test]
fn repaired_crc_cannot_make_invalid_routing_or_locations_valid() {
    let entries = fixture_entries(17);
    let valid = encode_layer(&entries).unwrap();
    for (offset, value) in [(0, 1u32), (4, 18), (8, 0)] {
        let mut bytes = valid.clone();
        change_logical(&mut bytes, 0, offset, &value.to_le_bytes());
        repair_plane(&mut bytes, 0);
        // offset 8=0 may be a valid endpoint; force a decreasing interior pair.
        if offset == 8 {
            change_logical(&mut bytes, 0, 4, &17u32.to_le_bytes());
            repair_plane(&mut bytes, 0);
        }
        assert!(open(&bytes, 17).1.is_err());
    }
    for mutation in [0, 1, 2] {
        let mut bytes = valid.clone();
        let key = match mutation {
            0 => key_bytes(entries[2].key),
            1 => key_bytes(entries[0].key),
            _ => [255; 33],
        };
        change_logical(&mut bytes, 2, 33, &key);
        repair_plane(&mut bytes, 2);
        let (_dir, result) = open(&bytes, 17);
        assert!(result.unwrap().validate_all().is_err());
    }
    for (field, value) in [
        (0, 7u32),
        (4, u32::MAX),
        (4, 0),
        (8, 0),
        (8, 4161),
        (4, 131071),
    ] {
        let mut bytes = valid.clone();
        change_logical(&mut bytes, 3, field, &value.to_le_bytes());
        repair_plane(&mut bytes, 3);
        let (_dir, result) = open(&bytes, 17);
        let layer = result.unwrap();
        assert_eq!(layer.find(entries[0].key), None, "locator {field}={value}");
    }
    let mut bytes = valid;
    change_logical(&mut bytes, 1, 0, &[42]);
    repair_plane(&mut bytes, 1);
    let (_dir, result) = open(&bytes, 17);
    let layer = result.unwrap();
    assert_eq!(layer.find(entries[0].key), None);
    assert!(layer.validate_all().is_err());
}

#[test]
fn untouched_key_and_fingerprint_blocks_are_lazy_and_cached_bad_never_repairs() {
    use std::os::unix::fs::FileExt;
    let mut records = fixture_entries(4097);
    for (i, entry) in records.iter_mut().enumerate() {
        entry.key.digest = ContentHash::of_bytes(&(i as u64).to_le_bytes());
    }
    records.sort_unstable_by_key(|e| e.key);
    let good = encode_layer(&records).unwrap();
    for plane in [1, 2, 3] {
        let mut bad = good.clone();
        let start = u32_at(&bad, 32 + plane * 8).unwrap() as usize;
        let len = u32_at(&bad, 36 + plane * 8).unwrap() as usize;
        let last = (len.div_ceil(BLOCK) - 1) * STRIDE;
        bad[start + last] ^= 1;
        let (dir, result) = open(&bad, records.len());
        let layer = result.unwrap();
        assert_eq!(layer.find(records[0].key), Some(records[0].location));
        assert!(layer.validate_all().is_err());
        // Controlled same-length repair after readers have stopped: no truncate,
        // so no SIGBUS. A pinned Bad status must not become Good after repair.
        let path = layer_path(dir.path(), descriptor(&bad, records.len()));
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.write_all_at(&good, 0).unwrap();
        assert!(layer.validate_all().is_err());
    }
}

#[test]
fn concurrent_first_touches_share_race_safe_crc_and_bucket_status() {
    let records = fixture_entries(4097);
    let bytes = encode_layer(&records).unwrap();
    let (_dir, result) = open(&bytes, records.len());
    let layer = result.unwrap();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for record in records.iter().step_by(127) {
                    assert_eq!(layer.find(record.key), Some(record.location));
                }
            });
        }
    });
    layer.validate_all().unwrap();
    let mut bad = bytes;
    let locator = u32_at(&bad, 56).unwrap() as usize;
    bad[locator] ^= 1;
    let (_dir, result) = open(&bad, records.len());
    let layer = result.unwrap();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| assert_eq!(layer.find(records[0].key), None));
        }
    });
    assert!(layer.validate_all().is_err());
}

#[test]
fn repaired_crc_rejects_misplaced_keys_and_noncanonical_dictionaries_in_full_validation() {
    let records = fixture_entries(342);
    let good = encode_layer(&records).unwrap();
    let mut bad = good.clone();
    let mut misplaced = records[0].key;
    misplaced.digest.0[0] = 0xff;
    change_logical(&mut bad, 2, 0, &key_bytes(misplaced));
    repair_plane(&mut bad, 2);
    let (_dir, result) = open(&bad, records.len());
    let layer = result.unwrap();
    assert!(layer.validate_all().is_err());
    assert_eq!(layer.find(records[0].key), None);
    for duplicate in [false, true] {
        let mut bad = good.clone();
        let first = records[0].location.segment.0;
        change_logical(&mut bad, 4, 32, &if duplicate { first } else { [0xff; 32] });
        repair_plane(&mut bad, 4);
        let (_dir, result) = open(&bad, records.len());
        assert!(result.unwrap().validate_all().is_err());
    }
    // Both blocks have full payloads. Cross-plane swaps must fail even when
    // their byte lengths agree: type and plane ordinal are part of the CRC.
    let mut bad = good;
    let key = u32_at(&bad, 48).unwrap() as usize;
    let locator = u32_at(&bad, 56).unwrap() as usize;
    let a = bad[key..key + STRIDE].to_vec();
    let b = bad[locator..locator + STRIDE].to_vec();
    bad[key..key + STRIDE].copy_from_slice(&b);
    bad[locator..locator + STRIDE].copy_from_slice(&a);
    let (_dir, result) = open(&bad, records.len());
    assert!(result.unwrap().validate_all().is_err());
}
