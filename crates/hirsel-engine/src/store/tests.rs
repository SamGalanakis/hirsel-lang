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
fn variant(sig: &str, payload: &str) -> VariantRef {
    let reads = Reads::from_vec(vec![(
        ReadKey::Signature(DefId::intern(&path("dep"))),
        h(sig),
    )])
    .unwrap();
    VariantRef::publish(&MemStore::default(), &key("a"), reads, payload.as_bytes()).unwrap()
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
fn read_binding_digest_keeps_canonical_rows_with_bounded_scratch_allocations() {
    fn original(reads: &Reads) -> ContentHash {
        let mut rows: Vec<Vec<u8>> = reads
            .iter()
            .map(|(key, value)| {
                let mut bytes = Vec::new();
                key.encode(&mut bytes);
                bytes.extend(value.0);
                bytes
            })
            .collect();
        rows.sort();
        ContentHash::of_parts("hirsel.result.reads.v1", rows.iter().map(Vec::as_slice))
    }
    for count in [0, 1, 16, 64] {
        let reads = Reads::from_vec(
            (0..count)
                .rev()
                .map(|at| {
                    (
                        ReadKey::Signature(DefId::intern(&path(&format!(
                            "varied_{at:03}_longer_name"
                        )))),
                        h(&at.to_string()),
                    )
                })
                .collect(),
        )
        .unwrap();
        let expected = original(&reads);
        let (actual, allocations) = crate::test_alloc::measured(|| payload_reads_hash(&reads));
        assert_eq!(actual, expected, "count={count}");
        assert!(
            allocations.allocations <= 20,
            "count={count}: {} scratch allocations",
            allocations.allocations
        );
    }
}

#[test]
fn current_variants_validate_bindings_before_observation_and_keep_first_hit() {
    let store = MemStore::default();
    let key = key("bound");
    let observed = ReadKey::Signature(DefId::intern(&path("observed")));
    let forbidden = ReadKey::Signature(DefId::intern(&path("forbidden")));
    let reads = Reads::from_vec(vec![(observed, h("current"))]).unwrap();
    let good = VariantRef::publish(&store, &key, reads, b"first bound payload").unwrap();
    let mut corrupt = good.clone();
    corrupt.reads = Reads::from_vec(vec![(forbidden, h("current"))]).unwrap();
    let mut stale = good.clone();
    stale.inputs = h("other inputs");
    stale.witness = stale.binding(&key);
    let mut entry = Entry {
        variants: vec![corrupt, stale, good.clone()],
    };
    let mut visits = vec![];
    let hit = entry
        .resolve_current(&store, &key, |read| {
            visits.push(*read);
            assert_eq!(
                *read, observed,
                "unbound or wrong-input reads are never observed"
            );
            Some(h("current"))
        })
        .unwrap();
    assert_eq!(visits, [observed]);
    assert_eq!(&*hit.payload, b"first bound payload");
    assert!(
        entry
            .resolve_current(&store, &key, |_| Some(h("changed")))
            .is_none()
    );
    entry.variants = vec![good.clone(), good];
    entry.variants[0].output.len += 1;
    entry.variants[0].witness = entry.variants[0].binding(&key);
    assert!(
        entry
            .resolve_current(&store, &key, |_| Some(h("current")))
            .is_none(),
        "missing content for the first current candidate does not select a later variant"
    );
}

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
    assert_eq!(
        entry.variants[0].output,
        product_domain(QueryKind::Check)
            .reference(b"p3 again")
            .unwrap()
    );
    let dep = ReadKey::Signature(DefId::intern(&path("dep")));
    let current = |sig: &'static str| move |k: &ReadKey| (*k == dep).then(|| h(sig));
    assert_eq!(
        entry.hit(current("s4")).unwrap().output,
        product_domain(QueryKind::Check).reference(b"p4").unwrap()
    );
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
    // Packed roots and outputs share a segment. Retain exactly a's segment
    // plus the checked index, while the older c segment must be evicted.
    let index_files: Vec<_> = std::fs::read_dir(root.join("index"))
        .unwrap()
        .flatten()
        .filter_map(|file| file.metadata().ok().filter(|m| m.is_file()))
        .collect();
    let retained = std::fs::metadata(
        artifacts.output_path(artifacts.lookup(Address(key("a").0)).unwrap().reference),
    )
    .unwrap()
    .len()
        + index_files.iter().map(|meta| meta.len()).sum::<u64>();
    let report = store.gc(retained);
    assert_eq!(report.entries, index_files.len() as u64 + 2);
    assert_eq!(report.removed, 1);
    assert_eq!(report.bytes - report.removed_bytes, retained);
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
    let v = VariantRef::publish(
        &store,
        &key("a"),
        variant("sig", "unused").reads,
        b"checked output",
    )
    .unwrap();
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
    let loaded = Entry::load(&store, &key("a"));
    assert!(loaded.variants[0].resolve(&store, &key("a")).is_none());
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
    let store = MemStore::default();
    let old = VariantRef::publish(
        &store,
        &first,
        variant("signature1", "unused").reads,
        b"same result",
    )
    .unwrap();
    let new = VariantRef::publish(&store, &changed, old.reads.clone(), b"same result").unwrap();
    assert_ne!(old.action(&first), new.action(&changed));
    assert_eq!(
        old.output, new.output,
        "equal outputs ignore source actions"
    );
    let other_reads = VariantRef::publish(
        &store,
        &first,
        variant("signature2", "unused").reads,
        b"same result",
    )
    .unwrap();
    assert_ne!(old.action(&first), other_reads.action(&first));
    assert_eq!(
        old.output, other_reads.output,
        "equal outputs ignore dependency witnesses"
    );
    let mut entry = Entry::default();
    entry.insert(old.clone());
    entry.insert(new);
    assert_eq!(
        entry.variants.len(),
        2,
        "variants retain prior source actions"
    );
    store.put(&first, &entry.encode_for(&first));
    assert_eq!(Entry::load(&store, &changed), entry);
    assert_eq!(store.len(), 2, "one index and one shared output");
    let mut lost_inputs = old.clone();
    lost_inputs.inputs = changed.1;
    assert!(lost_inputs.resolve(&store, &first).is_none());
    assert_eq!(
        &*old.resolve(&store, &first).unwrap().payload,
        b"same result"
    );
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
    let output =
        VariantRef::publish(&store, &first, variant("dep", "unused").reads, b"body").unwrap();
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
        QueryKind::Check as u32 + 256
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

#[test]
fn action_index_admission_does_not_fetch_content_and_missing_outputs_miss() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Default)]
    struct Counted {
        inner: MemStore,
        content_reads: AtomicUsize,
    }
    impl ResultStore for Counted {
        fn get(&self, key: &EntryKey) -> Option<Bytes> {
            self.inner.get(key)
        }
        fn put(&self, key: &EntryKey, bytes: &[u8]) {
            self.inner.put(key, bytes);
        }
        fn get_content(&self, reference: Reference) -> Option<Bytes> {
            self.content_reads.fetch_add(1, Ordering::Relaxed);
            self.inner.get_content(reference)
        }
        fn name(&self) -> &'static str {
            "counted-content"
        }
    }
    let store = Counted::default();
    let key = key("a");
    let mut entry = Entry::default();
    let variant = VariantRef::publish(
        &store,
        &key,
        variant("before", "unused").reads,
        b"one immutable output",
    )
    .unwrap();
    entry.insert(variant.clone());
    let index = entry.encode_for(&key);
    store.put(&key, &index);
    let loaded = Entry::load(&store, &key);
    assert!(loaded.hit(|_| Some(h("after"))).is_none());
    assert_eq!(store.content_reads.load(Ordering::Relaxed), 0);
    let hit = loaded.hit(|_| Some(h("before"))).unwrap();
    assert_eq!(
        &*hit.resolve(&store, &key).unwrap().payload,
        b"one immutable output"
    );
    assert_eq!(store.content_reads.load(Ordering::Relaxed), 1);
    let mut wrong_domain = hit.clone();
    wrong_domain.output.domain.kind += 1;
    assert!(wrong_domain.resolve(&store, &key).is_none());
    let mut oversized = hit.clone();
    oversized.output.len = hirsel_artifact_store::MAX_RECORD as u64 + 1;
    assert!(oversized.resolve(&store, &key).is_none());
    store.inner.take();
    store.put(&key, &index);
    let loaded = Entry::load(&store, &key);
    assert!(
        loaded
            .hit(|_| Some(h("before")))
            .unwrap()
            .resolve(&store, &key)
            .is_none()
    );
}

#[test]
fn directory_packaged_roots_are_read_only_and_local_corruption_does_not_mask_them() {
    let packaged = scratch("packaged");
    let local = scratch("packaged-local");
    let seed = DirStore::new(packaged.clone());
    let query = key("packaged");
    let content_domain = Domain {
        kind: 123,
        schema: 1,
    };
    let reference = seed.put_content(content_domain, b"shared product").unwrap();
    seed.put(&query, b"ordinary query index");
    fn contents(root: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut pending = vec![root.to_path_buf()];
        let mut files = vec![];
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                pending.extend(
                    std::fs::read_dir(&path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
            } else {
                files.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                ));
            }
        }
        files.sort();
        files
    }
    let before = contents(&packaged);
    let reader = DirStore::new(local.clone()).with_read_only_root(packaged.clone());
    assert_eq!(&*reader.get(&query).unwrap(), b"ordinary query index");
    assert_eq!(&*reader.get_content(reference).unwrap(), b"shared product");
    reader.put(&query, b"local query index");
    assert_eq!(&*reader.get(&query).unwrap(), b"local query index");
    let local_store = hirsel_artifact_store::Store::new(local.clone());
    let association = local_store
        .lookup(hirsel_artifact_store::Address(query.0))
        .unwrap();
    std::fs::write(local_store.output_path(association.reference), b"truncated").unwrap();
    assert_eq!(&*reader.get(&query).unwrap(), b"ordinary query index");
    reader.gc(0);
    assert_eq!(&*reader.get_content(reference).unwrap(), b"shared product");
    assert_eq!(
        contents(&packaged),
        before,
        "lookup and local collection must not mutate installed products"
    );
    let packaged_store = hirsel_artifact_store::Store::new(packaged.clone());
    let output = packaged_store.output_path(reference);
    std::fs::write(&output, b"bad container").unwrap();
    assert!(reader.get_content(reference).is_none());
    assert!(
        output.exists(),
        "corrupt installed content is never deleted by a reader"
    );
    let _ = std::fs::remove_dir_all(packaged);
    let _ = std::fs::remove_dir_all(local);
}

#[test]
fn product_associations_share_content_without_conveying_authority() {
    for store in [&MemStore::default() as &dyn ResultStore, &NoneStore] {
        let address = Address(ContentHash::of_parts(
            "product-address",
            [b"one".as_slice()],
        ));
        let action = Action(ContentHash::of_parts("product-action", [b"one".as_slice()]));
        let domain = Domain {
            kind: 0x4e00_0001,
            schema: 1,
        };
        let reference = domain.reference(b"product").unwrap();
        store.associate_product(address, action, reference);
        assert!(
            store.lookup_product(address).is_none(),
            "absent content cannot publish"
        );
        if store.enabled() {
            assert_eq!(store.put_content(domain, b"product"), Some(reference));
            store.associate_product(address, action, reference);
            let hit = store.lookup_product(address).unwrap();
            assert_eq!(hit.action, action);
            assert_eq!(hit.reference, reference);
            assert_eq!(&*store.get_content(reference).unwrap(), b"product");
            assert!(
                store
                    .lookup_product(Address(ContentHash::default()))
                    .is_none()
            );
        }
    }
}

#[test]
fn product_association_byte_transport_refuses_wrong_address_and_trailing_bytes() {
    let store = MemStore::default();
    let address = Address(ContentHash::of_parts(
        "product-address",
        [b"one".as_slice()],
    ));
    let action = Action(ContentHash::default());
    let reference = store
        .put_content(
            Domain {
                kind: 0x4e00_0001,
                schema: 1,
            },
            b"product",
        )
        .unwrap();
    store.associate_product(address, action, reference);
    let key = product_association_key(address);
    let mut bytes = store.get(&key).unwrap().to_vec();
    bytes.push(b'!');
    store.put(&key, &bytes);
    assert!(store.lookup_product(address).is_none());
    store.put(
        &key,
        &serde_json::to_vec(&(ContentHash::default(), action.0, reference)).unwrap(),
    );
    assert!(store.lookup_product(address).is_none());
}

#[test]
fn explicit_directory_selection_keeps_installed_products_without_forwarding_authority() {
    const CHILD: &str = "HIRSEL_STORE_SELECTED_CHILD";
    let query = key("selected-products");
    let domain = Domain {
        kind: 123,
        schema: 1,
    };
    let reference = domain.reference(b"installed bytes").unwrap();
    if let Some(local) = std::env::var_os(CHILD) {
        let local = PathBuf::from(local);
        let store = store_by_name("dir@1", Some(local.clone())).unwrap();
        assert_eq!(&*store.get(&query).unwrap(), b"installed index");
        assert_eq!(&*store.get_content(reference).unwrap(), b"installed bytes");
        assert_eq!(store.local_product_store().unwrap().root(), local);
        // An explicitly constructed library provider receives no ambient roots.
        assert!(DirStore::new(local.join("library")).get(&query).is_none());
        store.put(&query, b"local index");
        assert_eq!(&*store.get(&query).unwrap(), b"local index");
        return;
    }
    let packaged = scratch("selected-packaged");
    let local = scratch("selected-local");
    let seed = DirStore::new(packaged.clone());
    assert_eq!(
        seed.put_content(domain, b"installed bytes"),
        Some(reference)
    );
    seed.put(&query, b"installed index");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "store::tests::explicit_directory_selection_keeps_installed_products_without_forwarding_authority", "--nocapture"])
        .env(CHILD, &local)
        .env("HIRSEL_PRODUCTS", &packaged)
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    assert_eq!(&*seed.get(&query).unwrap(), b"installed index");
    let _ = std::fs::remove_dir_all(packaged);
    let _ = std::fs::remove_dir_all(local);
}

#[test]
fn disabled_writable_cache_keeps_installed_products_and_never_publishes() {
    const CHILD: &str = "HIRSEL_STORE_READ_ONLY_CHILD";
    let query = key("read-only-products");
    let domain = Domain {
        kind: 123,
        schema: 1,
    };
    let reference = domain.reference(b"installed bytes").unwrap();
    let new_reference = domain.reference(b"new bytes").unwrap();
    let address = Address(h("read-only-address"));
    if std::env::var_os(CHILD).is_some() {
        let store = store_by_name("none@1", None).unwrap();
        assert_eq!(store.name(), "none@1");
        assert!(store.enabled());
        assert!(store.local_product_store().is_none());
        assert_eq!(&*store.get(&query).unwrap(), b"installed index");
        assert_eq!(&*store.get_content(reference).unwrap(), b"installed bytes");
        assert!(store.verifier().is_some());
        let other = store_by_name("none@1", None).unwrap();
        assert!(!Arc::ptr_eq(
            &store.verifier().unwrap(),
            &other.verifier().unwrap()
        ));
        store.put(&query, b"ignored local replacement");
        assert!(store.put_content(domain, b"new bytes").is_none());
        store.associate_product(address, Action(h("ignored action")), reference);
        assert_eq!(&*store.get(&query).unwrap(), b"installed index");
        assert!(store.get_content(new_reference).is_none());
        assert!(store.lookup_product(address).is_none());
        return;
    }
    let packaged = scratch("read-only-packaged");
    let seed = DirStore::new(packaged.clone());
    assert_eq!(
        seed.put_content(domain, b"installed bytes"),
        Some(reference)
    );
    seed.put(&query, b"installed index");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "store::tests::disabled_writable_cache_keeps_installed_products_and_never_publishes",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("HIRSEL_PRODUCTS", &packaged)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    assert_eq!(&*seed.get(&query).unwrap(), b"installed index");
    assert!(seed.get_content(new_reference).is_none());
    assert!(seed.lookup_product(address).is_none());
    let _ = std::fs::remove_dir_all(packaged);
}
