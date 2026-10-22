use super::*;
use crate::memo::ReadKey;
use hirsel_syntax::{DefId, ItemPath, PackageId};

fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "s".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: name.into(),
    })
}
fn h(s: &str) -> ContentHash {
    ContentHash::of_bytes(s.as_bytes())
}
fn key(name: &str) -> EntryKey {
    EntryKey::new(
        h("env"),
        &path(name),
        h(name),
        QueryKind::Check,
        LoweringPolicy::Comptime,
    )
}
fn variant(sig: &str, payload: &str) -> Variant {
    Variant {
        inputs: h("source"),
        reads: Reads::from_vec(vec![(
            ReadKey::Signature(DefId::intern(&path("dep"))),
            h(sig),
        )])
        .unwrap(),
        payload: payload.as_bytes().into(),
    }
}
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "hirsel-store-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}
use std::path::PathBuf;

#[test]
fn entry_keys_separate_every_component() {
    let base = key("a");
    assert_eq!(base, key("a"));
    let others = [
        EntryKey::new(
            h("env2"),
            &path("a"),
            h("a"),
            QueryKind::Check,
            LoweringPolicy::Comptime,
        ),
        EntryKey::new(
            h("env"),
            &path("b"),
            h("a"),
            QueryKind::Check,
            LoweringPolicy::Comptime,
        ),
        EntryKey::new(
            h("env"),
            &path("a"),
            h("x"),
            QueryKind::Check,
            LoweringPolicy::Comptime,
        ),
        EntryKey::new(
            h("env"),
            &path("a"),
            h("a"),
            QueryKind::Lower,
            LoweringPolicy::Comptime,
        ),
        EntryKey::new(
            h("env"),
            &path("a"),
            h("a"),
            QueryKind::Check,
            LoweringPolicy::Reachable,
        ),
    ];
    assert!(others.iter().all(|k| *k != base));
}

#[test]
fn entries_hold_four_variants_and_hit_on_matching_reads() {
    let mut entry = Entry::default();
    for i in 0..6 {
        entry.insert(variant(&format!("s{i}"), &format!("p{i}")));
    }
    assert_eq!(entry.variants.len(), VARIANTS);
    entry.insert(variant("s3", "p3 again"));
    assert_eq!(entry.variants.len(), VARIANTS, "same reads replace");
    assert_eq!(&*entry.variants[0].payload, b"p3 again");
    let dep = ReadKey::Signature(DefId::intern(&path("dep")));
    let current = |sig: &'static str| move |k: &ReadKey| (*k == dep).then(|| h(sig));
    assert_eq!(&*entry.hit(current("s4")).unwrap().payload, b"p4");
    assert!(entry.hit(current("s0")).is_none(), "evicted");
    assert!(entry.hit(current("other")).is_none());
    let decoded = Entry::decode(&entry.encode()).unwrap();
    assert_eq!(decoded, entry);
    let mut bytes = entry.encode();
    bytes.push(0);
    assert!(Entry::decode(&bytes).is_none(), "trailing bytes");
    assert!(Entry::decode(&bytes[..10]).is_none(), "truncated");
}

#[test]
fn none_and_mem_stores() {
    let none = NoneStore;
    none.put(&key("a"), b"x");
    assert!(none.get(&key("a")).is_none());
    let mem = MemStore::default();
    mem.put(&key("a"), b"x");
    assert_eq!(&*mem.get(&key("a")).unwrap(), b"x");
    assert!(Arc::ptr_eq(&MemStore::shared(), &MemStore::shared()));
}

#[test]
fn sim_store_detects_corruption_and_loses_puts_on_request() {
    let sim = SimStore::with_faults(SimFaults {
        lose_puts: vec![1],
        corrupt_gets: vec![0],
    });
    sim.put(&key("a"), b"one");
    sim.put(&key("b"), b"two");
    assert!(sim.get(&key("a")).is_none(), "corrupt get is a miss");
    assert!(sim.get(&key("a")).is_none(), "and deletes the entry");
    assert!(sim.get(&key("b")).is_none(), "lost put");
    sim.put(&key("a"), b"three");
    assert_eq!(&*sim.get(&key("a")).unwrap(), b"three");
    assert_eq!(
        sim.log(),
        vec![
            SimOp::Put(key("a")),
            SimOp::Lost(key("b")),
            SimOp::Corrupt(key("a")),
            SimOp::Miss(key("a")),
            SimOp::Miss(key("b")),
            SimOp::Put(key("a")),
            SimOp::Hit(key("a")),
        ]
    );
}

#[test]
fn dir_store_round_trips_rejects_corruption_and_collects_lru() {
    let root = scratch("dir");
    let store = DirStore::new(root.clone());
    assert!(store.get(&key("a")).is_none());
    store.put(&key("a"), &[1; 1000]);
    store.put(&key("b"), &[2; 1000]);
    assert_eq!(&*store.get(&key("a")).unwrap(), &[1; 1000][..]);
    // Corrupt b on disk: a miss that deletes it.
    let artifacts = hirsel_artifact_store::Store::new(root.clone());
    let file = artifacts.output_path(
        artifacts
            .lookup(hirsel_artifact_store::Address(key("b").0))
            .unwrap()
            .reference,
    );
    let mut bytes = std::fs::read(&file).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&file, bytes).unwrap();
    assert!(store.get(&key("b")).is_none());
    assert!(!file.exists());
    // LRU: c is written, then a is read, so c is the oldest.
    store.put(&key("c"), &[3; 1000]);
    let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    let reference = artifacts
        .lookup(hirsel_artifact_store::Address(key("c").0))
        .unwrap()
        .reference;
    std::fs::File::options()
        .append(true)
        .open(artifacts.output_path(reference))
        .unwrap()
        .set_modified(old)
        .unwrap();
    // Two roots plus two live outputs and the dangling corrupt root.
    // The new format accounts for its roots as well as output bytes.
    let report = store.gc(1400);
    assert_eq!(report.entries, 5);
    assert!(report.removed >= 2);
    assert!(report.bytes - report.removed_bytes <= 1400);
    assert!(store.get(&key("c")).is_none());
    assert!(store.get(&key("a")).is_some());
    assert_eq!(store.gc(u64::MAX).removed, 0);
    std::fs::remove_dir_all(&root).unwrap();
}
use std::time::SystemTime;

#[test]
fn stores_are_selected_by_name() {
    for name in ["none@1", "mem@1", "sim@1"] {
        assert_eq!(store_by_name(name, None).unwrap().name(), name);
    }
    let root = scratch("named");
    assert_eq!(store_by_name("dir@1", Some(root)).unwrap().name(), "dir@1");
    assert!(store_by_name("disk@9", None).is_err());
}

#[test]
fn dir_short_lived_writers_enforce_the_cap() {
    let root = scratch("short-writers");
    for n in 0..12 {
        let store = DirStore::new(root.clone()).with_cap(4096);
        store.put(&key(&format!("entry-{n}")), &[n as u8; 2048]);
        let report = store.gc(u64::MAX);
        assert!(
            report.bytes <= 4096,
            "fresh writers must collect: {report:?}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn dir_small_cap_collects_repeated_writes_through_one_handle() {
    let root = scratch("small-cap");
    let store = DirStore::new(root.clone()).with_cap(4096);
    for n in 0..12 {
        store.put(&key(&format!("entry-{n}")), &[n as u8; 2048]);
        let report = store.gc(u64::MAX);
        assert!(report.bytes <= 4096 + 2048 + 216, "{report:?}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn dir_concurrent_handles_publish_complete_records_during_eviction() {
    let root = scratch("publish-evict");
    std::thread::scope(|scope| {
        for n in 0..8 {
            let root = &root;
            scope.spawn(move || {
                // Independent handles must not reuse temporary files, even for
                // simultaneous publication of different bytes at the same key.
                let store = DirStore::new(root.clone()).with_cap(512 * 1024);
                let payload = vec![n as u8; 128 * 1024];
                for _ in 0..16 {
                    store.put(&key("shared"), &payload);
                    if let Some(bytes) = store.get(&key("shared")) {
                        assert_eq!(bytes.len(), payload.len());
                        assert!(bytes.iter().all(|b| *b == bytes[0]));
                        assert!(bytes[0] < 8);
                    }
                    store.gc(512 * 1024);
                }
            });
        }
    });
    let store = DirStore::new(root.clone());
    store.put(&key("shared"), b"complete");
    assert_eq!(&*store.get(&key("shared")).unwrap(), b"complete");
    let artifacts = hirsel_artifact_store::Store::new(root.clone());
    let path = artifacts.output_path(
        artifacts
            .lookup(hirsel_artifact_store::Address(key("shared").0))
            .unwrap()
            .reference,
    );
    let bytes = std::fs::read(&path).unwrap();
    // Every truncated header/payload and a damaged checksum is a miss.
    for cut in 0..bytes.len() {
        std::fs::write(&path, &bytes[..cut]).unwrap();
        assert!(store.get(&key("shared")).is_none(), "cut {cut}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bound_entries_reject_wrong_keys_schema_truncation_and_damaged_records() {
    let store = MemStore::default();
    let mut entry = Entry::default();
    let mut v = variant("sig", "checked output");
    v.bind_payload(&key("a"));
    entry.insert(v);
    let bytes = entry.encode_for(&key("a"));
    store.put(&key("a"), &bytes);
    assert_eq!(Entry::load(&store, &key("a")), entry);
    store.put(&key("b"), &bytes); // completely valid bytes, another owner/phase
    assert!(Entry::load(&store, &key("b")).variants.is_empty());
    for end in 0..bytes.len() {
        store.put(&key("a"), &bytes[..end]);
        assert!(
            Entry::load(&store, &key("a")).variants.is_empty(),
            "cut {end}"
        );
    }
    for index in 0..bytes.len() {
        let mut damaged = bytes.clone();
        damaged[index] ^= 1;
        store.put(&key("a"), &damaged);
        assert!(
            Entry::load(&store, &key("a")).variants.is_empty(),
            "byte {index}"
        );
    }
    let mut obsolete = unframe(&bytes).unwrap().to_vec();
    obsolete[3] = b'9';
    store.put(&key("a"), &frame(&obsolete));
    assert!(Entry::load(&store, &key("a")).variants.is_empty());
    // An otherwise valid frame whose read metadata was lost cannot make its
    // payload unconditional. The immutable per-payload witness still rejects.
    entry.variants[0].reads = Reads::empty();
    store.put(&key("a"), &entry.encode_for(&key("a")));
    let mut loaded = Entry::load(&store, &key("a"));
    assert!(loaded.variants[0].unbind_payload(&key("a")).is_none());
}

#[test]
fn logical_addresses_survive_edits_and_actions_bind_complete_inputs() {
    let first = key("a");
    let changed = EntryKey::new(
        h("env"),
        &path("a"),
        h("changed body"),
        QueryKind::Check,
        LoweringPolicy::Comptime,
    );
    assert_eq!(
        first.0, changed.0,
        "logical address excludes source content"
    );
    let mut old = variant("signature1", "same result");
    old.inputs = first.1;
    let mut new = old.clone();
    new.inputs = changed.1;
    assert_ne!(old.action(&first), new.action(&changed));
    new.inputs = first.1;
    new.reads = variant("signature2", "unused").reads;
    assert_ne!(old.action(&first), new.action(&first));
    old.bind_payload(&first);
    let mut entry = Entry::default();
    entry.insert(old.clone());
    new.inputs = changed.1;
    new.bind_payload(&changed);
    entry.insert(new);
    assert_eq!(
        entry.variants.len(),
        2,
        "variants retain prior source actions"
    );
    let store = MemStore::default();
    store.put(&first, &entry.encode_for(&first));
    assert_eq!(Entry::load(&store, &changed), entry);
    assert_eq!(store.len(), 1);
    let mut lost_inputs = old.clone();
    lost_inputs.inputs = changed.1;
    assert!(lost_inputs.unbind_payload(&first).is_none());
    assert!(old.unbind_payload(&first).is_some());
    assert_eq!(&*old.payload, b"same result");
}

#[test]
fn directory_roots_keep_address_action_and_product_domain_distinct() {
    let root = scratch("action-domains");
    let store = DirStore::new(root.clone());
    let first = key("owner");
    let changed = EntryKey::new(
        h("env"),
        &path("owner"),
        h("edited"),
        QueryKind::Check,
        LoweringPolicy::Comptime,
    );
    let mut entry = Entry::default();
    let mut output = variant("dep", "body");
    output.inputs = first.1;
    output.bind_payload(&first);
    entry.insert(output);
    let action = entry.action(&first);
    store.put_query(first.2, &first, action, &entry.encode_for(&first));
    let artifact_store = hirsel_artifact_store::Store::new(root.clone());
    let association = artifact_store
        .lookup(hirsel_artifact_store::Address(first.0))
        .unwrap();
    assert_eq!(association.action, action);
    assert_ne!(association.action.0, first.0);
    assert_eq!(
        association.reference.domain.kind,
        QueryKind::Check as u32 + 16
    );
    assert_eq!(
        Entry::load(&store, &changed),
        entry,
        "a new source action still locates the previous record"
    );
    let mut wrong_domain = first;
    wrong_domain.2 = QueryKind::Summary;
    assert!(
        store.get(&wrong_domain).is_none(),
        "a forged key cannot cross product domains"
    );
    std::fs::remove_dir_all(root).unwrap();
}
