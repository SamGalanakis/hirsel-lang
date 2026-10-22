use super::*;

const DOMAIN: Domain = Domain { kind: 1, schema: 1 };
fn hash(bytes: &[u8]) -> ContentHash {
    ContentHash::of_bytes(bytes)
}

#[test]
fn actions_change_independently_of_equal_output_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_owned());
    let address = Address(hash(b"definition"));
    let old = store.put(DOMAIN, b"same summary").unwrap();
    store
        .associate(address, Action(hash(b"old body")), old)
        .unwrap();
    let new = store.put(DOMAIN, b"same summary").unwrap();
    assert_eq!(old, new);
    store
        .associate(address, Action(hash(b"new body")), new)
        .unwrap();
    let previous = store.lookup(address).unwrap();
    assert_eq!(previous.action, Action(hash(b"new body")));
    assert_eq!(previous.reference, old);
    assert_eq!(&*store.get(previous.reference).unwrap(), b"same summary");
}

#[test]
fn references_check_overflow_alignment_bounds_and_domain() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_owned());
    let reference = store.put(DOMAIN, b"0123456789abcdef").unwrap();
    let bytes = store.get(reference).unwrap();
    assert_eq!(&*bytes.range(8, 8, 8).unwrap(), b"89abcdef");
    let unaligned = bytes.range(1, 15, 1).unwrap();
    assert!(unaligned.range(0, 8, 8).is_none());
    assert_eq!(&*unaligned.range(7, 8, 8).unwrap(), b"89abcdef");
    for (offset, len, alignment) in [
        (u64::MAX, 1, 1),
        (8, u64::MAX, 8),
        (1, 8, 8),
        (0, 1, 0),
        (0, 1, 3),
        (16, 1, 1),
    ] {
        assert!(bytes.range(offset, len, alignment).is_none());
    }
    assert!(
        store
            .get(Reference {
                domain: Domain { kind: 2, ..DOMAIN },
                ..reference
            })
            .is_none()
    );
    assert!(
        store
            .get(Reference {
                domain: Domain {
                    schema: 2,
                    ..DOMAIN
                },
                ..reference
            })
            .is_none()
    );
}

#[test]
fn every_truncation_and_changed_content_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_owned());
    let reference = store.put(DOMAIN, b"payload").unwrap();
    let path = store.path(reference.output.0, "outputs");
    let bytes = fs::read(&path).unwrap();
    for end in 0..bytes.len() {
        fs::write(&path, &bytes[..end]).unwrap();
        assert!(store.get(reference).is_none(), "truncation at {end}");
    }
    for at in 0..bytes.len() {
        let mut bad = bytes.clone();
        bad[at] ^= 1;
        fs::write(&path, bad).unwrap();
        assert!(store.get(reference).is_none(), "corruption at {at}");
    }
    let mut trailing = bytes;
    trailing.push(0);
    fs::write(path, trailing).unwrap();
    assert!(store.get(reference).is_none());
}

#[test]
fn incomplete_publication_missing_output_and_old_schema_are_misses() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_owned());
    let address = Address(hash(b"root"));
    let reference = store.put(DOMAIN, b"content").unwrap();
    let action = Action(hash(b"complete inputs"));
    assert!(store.lookup(address).is_none());
    store.associate(address, action, reference).unwrap();
    let path = store.path(address.0, "actions");
    let bytes = fs::read(&path).unwrap();
    for end in 0..bytes.len() {
        fs::write(&path, &bytes[..end]).unwrap();
        assert!(store.lookup(address).is_none());
    }
    for at in 0..bytes.len() {
        let mut bad = bytes.clone();
        bad[at] ^= 1;
        fs::write(&path, bad).unwrap();
        assert!(store.lookup(address).is_none(), "root byte {at}");
    }
    let mut old = bytes.clone();
    old[7] = b'0';
    fs::write(&path, old).unwrap();
    assert!(store.lookup(address).is_none());
    fs::write(&path, bytes).unwrap();
    fs::remove_file(store.path(reference.output.0, "outputs")).unwrap();
    assert!(store.lookup(address).is_none());
    assert!(store.associate(address, action, reference).is_err());
}

#[test]
fn collection_bounds_files_and_preserves_live_mappings() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_owned());
    let reference = store.put(DOMAIN, b"mapped across collection").unwrap();
    let mapped = store.get(reference).unwrap();
    store
        .associate(Address(hash(b"root")), Action(hash(b"action")), reference)
        .unwrap();
    let report = store.collect(0);
    assert_eq!(report.entries, 2);
    assert_eq!(report.removed, 2);
    assert_eq!(report.removed_bytes, report.bytes);
    assert_eq!(&*mapped, b"mapped across collection");
    assert!(store.get(reference).is_none());
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        let sentinel = outside.path().join("sentinel");
        fs::write(&sentinel, b"another namespace").unwrap();
        std::os::unix::fs::symlink(outside.path(), store.root.join("outputs/foreign")).unwrap();
        store.collect(0);
        assert_eq!(fs::read(sentinel).unwrap(), b"another namespace");
    }
}

#[test]
fn mapped_views_survive_replacement_unlink_and_source_relocation() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original");
    let moved = dir.path().join("moved");
    let store = Store::new(original.clone());
    let reference = store.put(DOMAIN, b"held by mapping").unwrap();
    let bytes = store.get(reference).unwrap();
    let child = bytes.range(8, 7, 8).unwrap();
    store.put(DOMAIN, b"held by mapping").unwrap();
    fs::rename(original, &moved).unwrap();
    let reopened = Store::new(moved);
    assert_eq!(&*reopened.get(reference).unwrap(), b"held by mapping");
    fs::remove_file(reopened.path(reference.output.0, "outputs")).unwrap();
    drop(bytes);
    assert_eq!(&*child, b"mapping");
}

#[test]
fn concurrent_readers_see_only_complete_associations() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_owned());
    let address = Address(hash(b"root"));
    let a = store.put(DOMAIN, b"first").unwrap();
    let b = store.put(DOMAIN, b"second").unwrap();
    let action_a = Action(hash(b"a"));
    let action_b = Action(hash(b"b"));
    store.associate(address, action_a, a).unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..20 {
                store.associate(address, action_a, a).unwrap();
                store.associate(address, action_b, b).unwrap();
            }
        });
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..100 {
                    let entry = store.lookup(address).unwrap();
                    assert!(
                        entry
                            == Association {
                                action: action_a,
                                reference: a
                            }
                            || entry
                                == Association {
                                    action: action_b,
                                    reference: b
                                }
                    );
                    assert!(store.get(entry.reference).is_some());
                }
            });
        }
    });
}
