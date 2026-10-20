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
    assert!(
        std::fs::read_dir(root.join("tmp"))
            .unwrap()
            .next()
            .is_none(),
        "temp files are renamed away"
    );
    // Corrupt b on disk: a miss that deletes it.
    let hex = key("b").hex();
    let file = root.join(&hex[..2]).join(&hex);
    let mut bytes = std::fs::read(&file).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&file, bytes).unwrap();
    assert!(store.get(&key("b")).is_none());
    assert!(!file.exists());
    // LRU: c is written, then a is read, so c is the oldest.
    store.put(&key("c"), &[3; 1000]);
    let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    let hex = key("c").hex();
    std::fs::File::options()
        .append(true)
        .open(root.join(&hex[..2]).join(&hex))
        .unwrap()
        .set_modified(old)
        .unwrap();
    let report = store.gc(1100);
    assert_eq!(report.entries, 2);
    assert_eq!(report.removed, 1);
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
