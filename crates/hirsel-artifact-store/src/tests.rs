use super::*;

const DOMAIN: Domain = Domain { kind: 1, schema: 1 };

#[test]
fn independent_operation_handles_never_share_finish_abort_or_pins() {
    let directory = tempfile::tempdir().unwrap();
    let physical = Store::rebuildable(directory.path().to_owned());
    let first = physical.for_operation();
    let second = physical.for_operation();
    let first_request = first.request();
    let second_request = second.request();
    let missing = DOMAIN.reference(b"completed").unwrap();
    assert!(first.get(missing).is_none());
    assert!(second.get(missing).is_none());
    let rejected = first.put(DOMAIN, b"cancelled").unwrap();
    let completed = second.put(DOMAIN, b"completed").unwrap();
    let joined = second.clone().request();
    joined.finish().unwrap();
    assert!(physical.get(completed).is_none());
    drop(first_request);
    second_request.finish().unwrap();
    assert!(physical.get(rejected).is_none());
    assert_eq!(&*physical.get(completed).unwrap(), b"completed");
    let next = first.request();
    assert_eq!(&*first.get(completed).unwrap(), b"completed");
    next.finish().unwrap();
}

#[test]
fn checked_views_bind_the_mapping_range_and_complete_reference() {
    let directory = tempfile::tempdir().unwrap();
    let writer = Store::rebuildable(directory.path().to_owned());
    let reference = writer.put(DOMAIN, &vec![7; 5000]).unwrap();
    let reader = Store::rebuildable(directory.path().to_owned());
    let statistics = reader.enable_statistics();
    let operation = reader.request();
    let held = reader.get(reference).unwrap();
    for _ in 0..8 {
        assert_eq!(reader.get(reference).unwrap().as_ptr(), held.as_ptr());
    }
    assert_eq!(statistics.snapshot().counters["ContentChecks"], 1);
    assert_eq!(statistics.snapshot().counters["ContentReuses"], 8);
    for fault in 0..3 {
        let mut forged = reference;
        match fault {
            0 => forged.domain.kind += 1,
            1 => forged.domain.schema += 1,
            _ => forged.len -= 1,
        }
        assert!(reader.get(forged).is_none());
    }
    let container = reader.raw_container(reference).unwrap();
    let shifted = container.range(8, container.len() as u64 - 8, 1).unwrap();
    assert!(checked_container(shifted, reference).is_none());
    operation.finish().unwrap();
    // A held old view cannot authorize a replacement inode in a new operation.
    let path = writer.output_path(reference);
    fs::remove_file(&path).unwrap();
    fs::write(&path, vec![0; HEADER + 5000]).unwrap();
    let operation = reader.request();
    assert!(reader.get(reference).is_none());
    assert_eq!(&*held, &vec![7; 5000]);
    operation.finish().unwrap();
}

#[test]
fn checked_slots_never_accept_a_colliding_reference_or_a_different_pack_range() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::rebuildable(directory.path().to_owned());
    let batch = store.batch();
    let references: Vec<_> = (0u32..80)
        .map(|number| store.put(DOMAIN, &number.to_le_bytes()).unwrap())
        .collect();
    batch.finish().unwrap();
    let operation = store.request();
    for reference in &references {
        assert_eq!(store.get(*reference).unwrap().len(), 4);
    }
    for pair in references.windows(2) {
        let other = store.raw_container(pair[0]).unwrap();
        assert!(checked_container(other, pair[1]).is_none());
    }
    operation.finish().unwrap();
}

#[test]
fn configured_disk_caps_include_index_growth_and_preserve_held_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::rebuildable(dir.path().to_owned());
    store.set_cache_cap(4096);
    let statistics = store.enable_statistics();
    for n in 0u8..12 {
        let request = store.request();
        let reference = store.put(DOMAIN, &vec![n; 2048]).unwrap();
        store
            .associate(Address(hash(&[n])), Action(hash(&[n])), reference)
            .unwrap();
        let held = store.get(reference).unwrap();
        request.finish().unwrap();
        let report = store.collect(u64::MAX);
        assert!(report.bytes <= 4096, "{report:?}");
        assert_eq!(&*held, &vec![n; 2048]);
    }
    assert!(statistics.snapshot().counters["PublicationAdmissionBytes"] > 12 * 2048);
    assert_eq!(statistics.snapshot().counters["CollectionCommits"], 0);
}

#[test]
fn legacy_large_batch_payloads_have_real_gc_ownership_until_finish() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::rebuildable(dir.path().to_owned());
    let reader = Store::rebuildable(dir.path().to_owned());
    let batch = store.batch();
    let reference = store.put(DOMAIN, &vec![8; 5000]).unwrap();
    let path = store.output_path(reference);
    assert!(path.starts_with(dir.path().join("staging")));
    assert_eq!(reader.collect(0).removed, 0);
    assert!(path.exists());
    assert!(reader.get(reference).is_none());
    batch.finish().unwrap();
    assert!(!path.exists());
    assert_eq!(&*reader.get(reference).unwrap(), &vec![8; 5000]);
}

#[test]
fn scheduled_collection_runs_after_finish_without_a_hidden_publication() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::rebuildable(dir.path().to_owned());
    let statistics = store.enable_statistics();
    let request = store.request();
    let reference = store.put(DOMAIN, b"reclaim after finish").unwrap();
    let held = store.get(reference).unwrap();
    store.schedule_collection(0);
    assert_eq!(statistics.snapshot().counters["GcScans"], 0);
    assert_eq!(statistics.snapshot().counters["IndexPublications"], 0);
    request.finish().unwrap();
    let counters = statistics.snapshot().counters;
    assert_eq!(counters["IndexPublications"], 1);
    assert_eq!(counters["CollectionCommits"], 0);
    assert!(counters["GcScans"] > 0);
    assert!(counters["GcUnlinks"] > 0);
    assert!(Store::new(dir.path().to_owned()).get(reference).is_none());
    assert_eq!(&*held, b"reclaim after finish");
}

#[test]
fn request_spills_stay_private_until_explicit_finish_publishes_one_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::rebuildable(dir.path().to_owned());
    let reader = Store::new(dir.path().to_owned());
    let statistics = store.enable_statistics();
    let request = store.request();
    let mut references = Vec::new();
    for n in 0u32..600 {
        let bytes = n.to_le_bytes();
        let reference = store.put(DOMAIN, &bytes).unwrap();
        store
            .associate(Address(hash(&bytes)), Action(hash(&bytes)), reference)
            .unwrap();
        references.push(reference);
    }
    assert!(statistics.snapshot().counters["PrivateSpills"] > 0);
    assert_eq!(statistics.snapshot().counters["IndexPublications"], 0);
    assert!(!dir.path().join("index/MANIFEST").exists());
    for (n, &reference) in references.iter().enumerate() {
        assert_eq!(&*store.get(reference).unwrap(), &(n as u32).to_le_bytes());
        assert!(reader.get(reference).is_none());
    }
    request.finish().unwrap();
    assert_eq!(statistics.snapshot().counters["IndexPublications"], 1);
    assert_eq!(statistics.snapshot().counters["DropCommits"], 0);
    let scope = reader.batch();
    for (n, &reference) in references.iter().enumerate() {
        assert_eq!(&*reader.get(reference).unwrap(), &(n as u32).to_le_bytes());
    }
    scope.finish().unwrap();
    assert_eq!(fs::read_dir(dir.path().join("staging")).unwrap().count(), 0);
}

#[test]
fn request_drop_aborts_tail_and_gc_neither_commits_nor_steals_live_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::rebuildable(dir.path().to_owned());
    let other = Store::new(dir.path().to_owned());
    let old = store.put(DOMAIN, b"old").unwrap();
    let old_index = fs::read(dir.path().join("index/MANIFEST")).unwrap();
    let statistics = store.enable_statistics();
    let request = store.request();
    let references: Vec<_> = (0u32..1030)
        .map(|n| store.put(DOMAIN, &n.to_le_bytes()).unwrap())
        .collect();
    let large = store.put(DOMAIN, &vec![7; 5000]).unwrap();
    let held = store.get(references[0]).unwrap().range(0, 1, 1).unwrap();
    let private_large = store.output_path(large);
    assert!(private_large.starts_with(dir.path().join("staging")));
    assert!(other.get(large).is_none());
    let before = statistics.snapshot();
    assert_eq!(store.collect(0).removed, 0);
    assert_eq!(other.collect(0).removed, 0);
    assert_eq!(
        statistics.snapshot().counters["IndexPublications"],
        before.counters["IndexPublications"]
    );
    assert!(private_large.exists());
    assert_eq!(
        fs::read(dir.path().join("index/MANIFEST")).unwrap(),
        old_index
    );
    drop(request);
    assert!(!private_large.exists());
    assert_eq!(&*held, &[0]);
    assert_eq!(&*other.get(old).unwrap(), b"old");
    assert!(other.get(references[0]).is_none());
    assert!(other.get(large).is_none());
    assert_eq!(
        fs::read(dir.path().join("index/MANIFEST")).unwrap(),
        old_index
    );
    assert_eq!(statistics.snapshot().counters["IndexPublications"], 0);
    assert_eq!(statistics.snapshot().counters["Aborts"], 1);
    drop(store);
    assert!(statistics.snapshot().counters["LiveBackingBytes"] > held.len() as u64);
    drop(held);
    assert_eq!(statistics.snapshot().counters["LiveBackingBytes"], 0);
}

#[test]
fn failed_request_finish_leaves_no_association_or_orphan_authority() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::rebuildable(dir.path().to_owned());
    let address = Address(hash(b"request crash"));
    let old = store.put(DOMAIN, b"old").unwrap();
    store.associate(address, Action(hash(b"old")), old).unwrap();
    let before = fs::read(dir.path().join("index/MANIFEST")).unwrap();
    let request = store.request();
    let new = store.put(DOMAIN, b"new").unwrap();
    store.associate(address, Action(hash(b"new")), new).unwrap();
    store.crash_before_next_index();
    assert!(request.finish().is_err());
    let reader = Store::new(dir.path().to_owned());
    assert_eq!(fs::read(dir.path().join("index/MANIFEST")).unwrap(), before);
    assert_eq!(reader.lookup(address).unwrap().reference, old);
    assert!(reader.get(new).is_none());
    assert_eq!(fs::read_dir(dir.path().join("staging")).unwrap().count(), 0);
}

#[test]
fn private_byte_and_metadata_limits_publish_only_bounded_prefixes_and_abort_the_tail() {
    for (byte_limit, metadata_limit) in [(40 << 10, 64 << 10), (64 << 10, 8 << 10)] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::rebuildable(dir.path().to_owned());
        store.set_private_limits(byte_limit, metadata_limit);
        let statistics = store.enable_statistics();
        let request = store.request();
        let references: Vec<_> = (0u8..100)
            .map(|n| store.put(DOMAIN, &vec![n; 1024]).unwrap())
            .collect();
        let snapshot = statistics.snapshot();
        assert!(snapshot.counters["AdmissionCommits"] > 0);
        assert_eq!(
            snapshot.counters["IndexPublications"],
            snapshot.counters["AdmissionCommits"]
        );
        assert!(snapshot.counters["PrivatePeakBytes"] <= byte_limit as u64);
        assert!(snapshot.counters["EpochMetadataPeakBytes"] <= metadata_limit as u64);
        let reader = Store::new(dir.path().to_owned());
        let published = references
            .iter()
            .filter(|&&r| reader.get(r).is_some())
            .count();
        assert!(published > 0 && published < references.len());
        drop(request);
        assert_eq!(
            statistics.snapshot().counters["IndexPublications"],
            snapshot.counters["IndexPublications"]
        );
        assert_eq!(
            references
                .iter()
                .filter(|&&r| reader.get(r).is_some())
                .count(),
            published
        );
        assert_eq!(fs::read_dir(dir.path().join("staging")).unwrap().count(), 0);
    }
}

#[test]
fn tiny_unique_payloads_force_metadata_admission_before_byte_pressure() {
    for count in [15, 32] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::rebuildable(dir.path().to_owned());
        store.set_private_limits(1 << 20, 2048);
        let statistics = store.enable_statistics();
        let request = store.request();
        let references: Vec<_> = (0u8..count)
            .map(|n| store.put(DOMAIN, &[n]).unwrap())
            .collect();
        let snapshot = statistics.snapshot();
        assert!(snapshot.counters["AdmissionCommits"] > 0);
        assert!(
            snapshot.counters["EpochMetadataPeakBytes"] <= 2048,
            "{snapshot:?}"
        );
        // All one-byte inputs, including their pack framing, fit far below 1 MiB.
        assert!(snapshot.counters["PrivatePeakBytes"] < 4096);
        let reader = Store::new(dir.path().to_owned());
        let published: Vec<_> = references.iter().map(|&r| reader.get(r)).collect();
        let count = published.iter().filter(|v| v.is_some()).count();
        assert!(count > 0 && count < references.len());
        for (n, bytes) in published.iter().enumerate().take(count) {
            assert_eq!(bytes.as_deref(), Some(&[n as u8][..]));
        }
        assert!(published[count..].iter().all(Option::is_none));
        drop(request);
        assert_eq!(
            statistics.snapshot().counters["IndexPublications"],
            snapshot.counters["IndexPublications"]
        );
        assert!(references[count..].iter().all(|&r| reader.get(r).is_none()));
        assert_eq!(fs::read_dir(dir.path().join("staging")).unwrap().count(), 0);
    }
}

#[test]
fn request_finish_requires_joined_batches_and_abandoned_staging_is_reclaimed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::rebuildable(dir.path().to_owned());
    let request = store.request();
    let batch = store.batch();
    let rejected = store.put(DOMAIN, b"not joined").unwrap();
    assert!(request.finish().is_err());
    drop(batch);
    assert!(Store::new(dir.path().to_owned()).get(rejected).is_none());
    let request = store.request();
    let accepted = store.put(DOMAIN, b"joined").unwrap();
    request.finish().unwrap();
    assert_eq!(
        &*Store::new(dir.path().to_owned()).get(accepted).unwrap(),
        b"joined"
    );
    for namespace in ["staging", "index/staging"] {
        let abandoned = dir.path().join(namespace).join("abandoned");
        fs::create_dir_all(&abandoned).unwrap();
        fs::write(abandoned.join("owner.lock"), b"").unwrap();
        fs::write(abandoned.join("incomplete"), b"not authoritative").unwrap();
        store.collect(u64::MAX);
        assert!(!abandoned.exists());
    }
}

#[test]
fn request_writers_merge_against_latest_committed_index_not_their_old_pins() {
    let dir = tempfile::tempdir().unwrap();
    let first = Store::rebuildable(dir.path().to_owned());
    let second = Store::rebuildable(dir.path().to_owned());
    let a = first.request();
    let b = second.request();
    let missing = DOMAIN.reference(b"absent").unwrap();
    assert!(first.get(missing).is_none());
    assert!(second.get(missing).is_none());
    let x = first.put(DOMAIN, b"first writer").unwrap();
    let y = second.put(DOMAIN, b"second writer").unwrap();
    a.finish().unwrap();
    assert!(second.get(x).is_none(), "request pin is still absent");
    b.finish().unwrap();
    let reader = Store::new(dir.path().to_owned());
    assert_eq!(&*reader.get(x).unwrap(), b"first writer");
    assert_eq!(&*reader.get(y).unwrap(), b"second writer");
}

#[test]
fn identical_resident_updates_compare_complete_content_and_associations_without_io() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::rebuildable(dir.path().to_owned());
    let statistics = store.enable_statistics();
    let scope = store.batch();
    let address = Address(hash(b"identical"));
    let old_action = Action(hash(b"old"));
    let new_action = Action(hash(b"new"));
    let first = store.put(DOMAIN, b"first").unwrap();
    store.associate(address, old_action, first).unwrap();
    let before = statistics.snapshot();
    assert_eq!(store.put(DOMAIN, b"first").unwrap(), first);
    store.associate(address, old_action, first).unwrap();
    assert_eq!(statistics.snapshot().counters["IdenticalUpdates"], 2);
    store.associate(address, new_action, first).unwrap();
    let second = store.put(DOMAIN, b"second").unwrap();
    store.associate(address, new_action, second).unwrap();
    assert_eq!(statistics.snapshot().counters["IdenticalUpdates"], 2);
    assert_eq!(
        store.lookup(address).unwrap(),
        Association {
            action: new_action,
            reference: second
        }
    );
    let large = vec![9; 5000];
    let reference = store.put(DOMAIN, &large).unwrap();
    store.get(reference).unwrap();
    let opened = statistics.snapshot();
    assert_eq!(store.put(DOMAIN, &large).unwrap(), reference);
    let after = statistics.snapshot();
    assert_eq!(after.counters["IdenticalUpdates"], 3);
    assert_eq!(
        after.counters["PublicationAttempts"],
        opened.counters["PublicationAttempts"]
    );
    for counter in [
        "IndexLoadAttempts",
        "IndexStamps",
        "SegmentOpens",
        "StandaloneOpens",
    ] {
        assert_eq!(
            after.counters[counter], opened.counters[counter],
            "{counter}"
        );
    }
    assert_eq!(
        opened.counters["StandaloneOpens"] - before.counters["StandaloneOpens"],
        1
    );
    let foreign = store
        .put(
            Domain {
                schema: 2,
                ..DOMAIN
            },
            &large,
        )
        .unwrap();
    assert_ne!(reference, foreign);
    assert_eq!(statistics.snapshot().counters["IdenticalUpdates"], 3);
    scope.finish().unwrap();
    assert_eq!(
        Store::new(dir.path().to_owned()).lookup(address).unwrap(),
        Association {
            action: new_action,
            reference: second
        }
    );
}

#[test]
fn read_scopes_pin_absence_and_refresh_only_at_the_next_scope() {
    let dir = tempfile::tempdir().unwrap();
    let reader = Store::rebuildable(dir.path().to_owned());
    let writer = Store::rebuildable(dir.path().to_owned());
    let statistics = reader.enable_statistics();
    let reference = DOMAIN.reference(b"later").unwrap();
    let scope = reader.batch();
    for _ in 0..8 {
        assert!(reader.get(reference).is_none());
    }
    assert_eq!(statistics.snapshot().counters["IndexLoadAttempts"], 1);
    assert_eq!(statistics.snapshot().counters["IndexStamps"], 0);
    writer.put(DOMAIN, b"later").unwrap();
    assert!(reader.get(reference).is_none(), "absence is pinned too");
    scope.finish().unwrap();
    let scope = reader.batch();
    assert_eq!(&*reader.get(reference).unwrap(), b"later");
    let before = statistics.snapshot();
    for _ in 0..8 {
        assert_eq!(&*reader.get(reference).unwrap(), b"later");
        assert!(reader.get(DOMAIN.reference(b"missing").unwrap()).is_none());
    }
    let after = statistics.snapshot();
    for counter in [
        "IndexLoadAttempts",
        "IndexStamps",
        "IndexChecks",
        "SegmentOpens",
    ] {
        assert_eq!(
            after.counters[counter], before.counters[counter],
            "{counter}"
        );
    }
    assert_eq!(after.counters["RegionCacheHits"], 8);
    scope.finish().unwrap();
}

#[test]
fn retained_regions_obey_byte_and_count_bounds_and_charge_held_unlinked_backings() {
    let dir = tempfile::tempdir().unwrap();
    let writer = Store::rebuildable(dir.path().to_owned());
    let large: Vec<_> = (0..3)
        .map(|n| writer.put(DOMAIN, &vec![n; 12 << 20]).unwrap())
        .collect();
    let small: Vec<_> = (0..70)
        .map(|n| writer.put(DOMAIN, &vec![n; 5000]).unwrap())
        .collect();
    let reader = Store::rebuildable(dir.path().to_owned());
    let statistics = reader.enable_statistics();
    let scope = reader.batch();
    let held = reader.get(large[0]).unwrap().range(0, 1, 1).unwrap();
    for reference in large.iter().chain(&small) {
        assert_eq!(reader.get(*reference).unwrap().len() as u64, reference.len);
        let cache = reader.regions.lock().unwrap();
        assert!(cache.bytes <= REGION_BYTES);
        assert!(cache.entries.len() <= REGION_COUNT);
        assert_eq!(
            cache.bytes,
            cache.entries.values().map(|r| r.bytes().len()).sum()
        );
    }
    fs::remove_file(writer.output_path(large[0])).unwrap();
    assert_eq!(&*held, &[0]);
    scope.finish().unwrap();
    assert_eq!(
        statistics.snapshot().counters["LiveBackingBytes"],
        (12 << 20) + HEADER as u64
    );
    assert!(reader.get(large[0]).is_none());
    drop(reader);
    assert_eq!(
        statistics.snapshot().counters["LiveBackingBytes"],
        (12 << 20) + HEADER as u64
    );
    drop(held);
    assert_eq!(statistics.snapshot().counters["LiveBackingBytes"], 0);
}

#[test]
fn opened_association_carries_the_same_output_handle_through_consumption() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::rebuildable(dir.path().to_owned());
    let reference = store.put(DOMAIN, &vec![7; 5000]).unwrap();
    let address = Address(hash(b"opened"));
    let action = Action(hash(b"action"));
    store.associate(address, action, reference).unwrap();
    let reader = Store::rebuildable(dir.path().to_owned());
    let statistics = reader.enable_statistics();
    let opened = reader.lookup_open(address).unwrap();
    assert_eq!(opened.association, Association { action, reference });
    assert_eq!(statistics.snapshot().counters["StandaloneOpens"], 1);
    fs::remove_file(store.output_path(reference)).unwrap();
    assert_eq!(&*opened.get().unwrap(), &vec![7; 5000]);
    assert_eq!(statistics.snapshot().counters["StandaloneOpens"], 1);
    assert!(reader.lookup_open(address).is_none());
}

#[test]
fn rebuildable_publication_has_no_syncs_and_lost_records_are_checked_misses() {
    for rebuildable in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = if rebuildable {
            Store::rebuildable(dir.path().to_owned())
        } else {
            Store::new(dir.path().to_owned())
        };
        let statistics = store.enable_statistics();
        let batch = store.batch();
        let small = store.put(DOMAIN, b"small").unwrap();
        let large = store.put(DOMAIN, &vec![1; pack::SMALL + 1]).unwrap();
        let address = Address(hash(b"rebuildable"));
        let action = Action(hash(b"action"));
        store.associate(address, action, large).unwrap();
        batch.finish().unwrap();
        assert_eq!(&*store.get(small).unwrap(), b"small");
        assert_eq!(store.lookup(address).unwrap().reference, large);
        let counters = statistics.snapshot().counters;
        assert_eq!(counters["Publications"], 4);
        let expected = if rebuildable { 0 } else { 4 };
        assert_eq!(counters["FileSyncs"], expected);
        assert_eq!(counters["DirectorySyncs"], if rebuildable { 0 } else { 6 });
        let reader = Store::new(dir.path().to_owned());
        assert_eq!(reader.lookup(address).unwrap().action, action);
        fs::remove_file(store.output_path(large)).unwrap();
        assert!(reader.get(large).is_none());
        assert!(reader.lookup(address).is_none());
        fs::remove_file(dir.path().join("index/MANIFEST")).unwrap();
        let reader = Store::new(dir.path().to_owned());
        assert!(reader.get(small).is_none());
        assert!(reader.lookup(address).is_none());
    }
}

#[test]
fn physical_statistics_share_owner_classify_commits_and_charge_held_backings() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_owned());
    let clone = store.clone();
    assert!(store.statistics().is_none());
    let statistics = clone.enable_statistics();
    assert!(Arc::ptr_eq(&statistics, &store.enable_statistics()));
    assert_ne!(
        statistics.snapshot().owner,
        Store::new(dir.path().to_owned())
            .enable_statistics()
            .snapshot()
            .owner
    );
    let batch = store.batch();
    let references: Vec<_> = (0u32..1030)
        .map(|n| clone.put(DOMAIN, &n.to_le_bytes()).unwrap())
        .collect();
    assert_eq!(
        &*store.get(references[1029]).unwrap(),
        &1029u32.to_le_bytes()
    );
    let prefix = statistics.snapshot();
    assert_eq!(prefix.counters["Epochs"], 1);
    assert_eq!(prefix.counters["RecordLimitCommits"], 1);
    assert_eq!(prefix.counters["IndexPublications"], 1);
    batch.finish().unwrap();
    let held = store.get(references[0]).unwrap();
    let backing_bytes = fs::metadata(store.output_path(references[0]))
        .unwrap()
        .len();
    assert!(backing_bytes > held.len() as u64);
    let snapshot = statistics.snapshot();
    assert_eq!(snapshot.counters["Commits"], 2);
    assert_eq!(snapshot.counters["FinishCommits"], 1);
    assert_eq!(snapshot.counters["IndexPublications"], 2);
    assert_eq!(snapshot.counters["Publications"], 6);
    assert_eq!(snapshot.counters["FileSyncs"], 7);
    assert_eq!(snapshot.counters["DirectorySyncs"], 8);
    assert_eq!(snapshot.counters["PendingPeakRecords"], 1024);
    assert!(snapshot.counters["PendingPeakBytes"] <= (1 << 20) - 64);
    assert!(snapshot.counters["IndexBackingBytesLoaded"] > 0);
    assert_eq!(snapshot.counters["IndexMaintenancePublications"], 0);
    assert!(snapshot.counters["SegmentOpens"] > 0);
    drop(store);
    drop(clone);
    assert_eq!(
        statistics.snapshot().counters["LiveBackingBytes"],
        backing_bytes
    );
    drop(held);
    assert_eq!(statistics.snapshot().counters["LiveBackingBytes"], 0);
}

fn hash(bytes: &[u8]) -> ContentHash {
    ContentHash::of_bytes(bytes)
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
    let path = store.output_path(reference);
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
    let path = store.root.join("index/MANIFEST");
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
    // Start the missing-output control from a fresh valid pin, independent of
    // the deliberately damaged manifest snapshots above.
    let store = Store::new(store.root.clone());
    assert_eq!(store.lookup(address).unwrap().reference, reference);
    fs::remove_file(store.output_path(reference)).unwrap();
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
    // Two payload packs, two original layers, their compacted layer and root.
    assert_eq!(report.entries, 6);
    assert_eq!(report.removed, 6);
    assert_eq!(report.removed_bytes, report.bytes);
    assert_eq!(&*mapped, b"mapped across collection");
    assert!(store.get(reference).is_none());
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        let sentinel = outside.path().join("sentinel");
        fs::write(&sentinel, b"another namespace").unwrap();
        std::os::unix::fs::symlink(outside.path(), store.root.join("packs/foreign")).unwrap();
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
    fs::remove_file(reopened.output_path(reference)).unwrap();
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

#[test]
fn embedded_products_use_the_mapped_store_integrity_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_owned());
    let reference = store
        .put(
            Domain {
                kind: 31,
                schema: 1,
            },
            b"ordinary product",
        )
        .unwrap();
    let bytes = store.container(reference).unwrap().to_vec();
    let embedded = |bytes: Vec<u8>| {
        Embedded(Box::leak(
            vec![(reference, &*Box::leak(bytes.into_boxed_slice()))].into_boxed_slice(),
        ))
    };
    assert_eq!(
        &*embedded(bytes.clone()).get_content(reference).unwrap(),
        b"ordinary product"
    );
    for len in 0..bytes.len() {
        assert!(
            embedded(bytes[..len].to_vec())
                .get_content(reference)
                .is_none()
        );
    }
    let mut corrupt = bytes;
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(embedded(corrupt).get_content(reference).is_none());
}

#[test]
fn retained_owner_survives_parent_drop_clone_and_child_ranges() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Owner(Arc<AtomicBool>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let owner = Arc::new(Owner(dropped.clone()));
    let bytes = Bytes::from(b"immutable transport".as_slice()).retain_owner(owner.clone());
    let clone = bytes.clone();
    let child = bytes.range(10, 9, 1).unwrap();
    let nested = child.range(0, 4, 1).unwrap();
    assert_eq!(&*bytes, b"immutable transport");
    assert_eq!(&*child, b"transport");
    drop(owner);
    drop(bytes);
    drop(clone);
    drop(child);
    assert!(!dropped.load(Ordering::SeqCst));
    assert_eq!(&*nested, b"tran");
    drop(nested);
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn small_records_share_bounded_atomic_batches_and_survive_relocation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    let store = Store::new(root.clone());
    let reader = Store::new(root.clone());
    let batch = store.batch();
    let mut references = Vec::new();
    for n in 0u32..600 {
        let bytes = n.to_le_bytes();
        let reference = store.put(DOMAIN, &bytes).unwrap();
        store
            .associate(Address(hash(&bytes)), Action(hash(&bytes)), reference)
            .unwrap();
        assert_eq!(&*store.get(reference).unwrap(), &bytes);
        references.push(reference);
    }
    // The bounded transaction already committed a complete earlier prefix.
    assert!(
        reader
            .lookup(Address(hash(&599u32.to_le_bytes())))
            .is_none()
    );
    batch.finish().unwrap();
    let packs: Vec<_> = fs::read_dir(root.join("packs"))
        .unwrap()
        .flat_map(|shard| fs::read_dir(shard.unwrap().path()).unwrap())
        .map(|file| file.unwrap().path())
        .collect();
    assert_eq!(packs.len(), 2, "1200 tiny records share two segments");
    assert!(
        packs
            .iter()
            .all(|path| fs::metadata(path).unwrap().len() <= 1 << 20)
    );
    for (n, reference) in references.iter().enumerate() {
        let bytes = (n as u32).to_le_bytes();
        assert_eq!(
            reader.lookup(Address(hash(&bytes))).unwrap().reference,
            *reference
        );
        assert_eq!(&*reader.get(*reference).unwrap(), &bytes);
    }
    let held = reader
        .get(references[599])
        .unwrap()
        .retain_owner(Arc::new(()));
    let moved = dir.path().join("moved");
    fs::rename(&root, &moved).unwrap();
    assert_eq!(
        &*Store::new(moved).get(references[599]).unwrap(),
        &599u32.to_le_bytes()
    );
    drop(store);
    drop(reader);
    assert_eq!(&*held, &599u32.to_le_bytes());
}

#[test]
fn a_crash_after_segment_publication_keeps_only_the_old_committed_index() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    let store = Store::new(root.clone());
    let old = store.put(DOMAIN, b"old").unwrap();
    store
        .associate(Address(hash(b"root")), Action(hash(b"old")), old)
        .unwrap();
    let index = fs::read(root.join("index/MANIFEST")).unwrap();
    let batch = store.batch();
    let new = store.put(DOMAIN, b"new").unwrap();
    store
        .associate(Address(hash(b"root")), Action(hash(b"new")), new)
        .unwrap();
    store.crash_before_next_index();
    assert!(batch.finish().is_err());
    drop(store);
    assert_eq!(fs::read(root.join("index/MANIFEST")).unwrap(), index);
    let reopened = Store::new(root);
    assert_eq!(
        reopened.lookup(Address(hash(b"root"))).unwrap().reference,
        old
    );
    assert_eq!(&*reopened.get(old).unwrap(), b"old");
    assert!(
        reopened.get(new).is_none(),
        "an orphan segment supplies no committed output"
    );
}

#[test]
fn packed_offsets_and_index_truncation_are_checked_before_record_access() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    let store = Store::rebuildable(root.clone());
    let batch = store.batch();
    let a = store.put(DOMAIN, b"first").unwrap();
    let b = store.put(DOMAIN, b"second").unwrap();
    batch.finish().unwrap();
    let layer = current_layer_path(&root);
    let original = fs::read(&layer).unwrap();
    for end in 0..original.len() {
        fs::write(&layer, &original[..end]).unwrap();
        assert!(
            Store::new(root.clone()).get(a).is_none(),
            "layer truncation {end}"
        );
    }
    // Every plane needed by this demand is checked, without requiring an
    // unrelated untouched block to be read merely to reject the whole file.
    let row = usize::from(b.output.0 > a.output.0);
    let mut demanded = vec![0];
    for plane in 0..5 {
        let start = word(&original, 32 + 8 * plane).unwrap() as usize;
        let at = start
            + match plane {
                1 => row,
                2 => row * 33,
                3 => row * 12,
                _ => 0,
            };
        demanded.push(at);
    }
    for at in demanded {
        let mut bad = original.clone();
        bad[at] ^= 1;
        fs::write(&layer, bad).unwrap();
        assert!(
            Store::new(root.clone()).get(b).is_none(),
            "demanded layer byte {at}"
        );
    }
    fs::write(&layer, original).unwrap();
    assert_eq!(&*Store::new(root).get(b).unwrap(), b"second");
}

#[test]
fn strict_batch_finish_waits_for_publication_lock() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    let store = Store::new(root.clone());
    let batch = store.batch();
    let reference = store.put(DOMAIN, b"committed after contention").unwrap();
    let address = Address(hash(b"contended address"));
    store
        .associate(address, Action(hash(b"action")), reference)
        .unwrap();
    let publication = pack::Lock::try_root(&root, true).unwrap();
    std::thread::scope(|scope| {
        let (done, result) = std::sync::mpsc::channel();
        scope.spawn(move || done.send(batch.finish()).unwrap());
        // A strict writer waits for the short publication critical section;
        // contention cannot become an error or a successful skipped write.
        assert!(matches!(
            result.recv_timeout(std::time::Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        drop(publication);
        result
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
    });
    let reopened = Store::new(root);
    assert_eq!(reopened.lookup(address).unwrap().reference, reference);
    assert_eq!(
        &*reopened.get(reference).unwrap(),
        b"committed after contention"
    );
}

#[test]
fn independent_batch_writers_preserve_each_others_committed_addresses() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    std::thread::scope(|scope| {
        for writer in 0u32..2 {
            let root = root.clone();
            scope.spawn(move || {
                let store = Store::new(root);
                let batch = store.batch();
                for n in 0u32..64 {
                    let bytes = (writer * 64 + n).to_le_bytes();
                    let reference = store.put(DOMAIN, &bytes).unwrap();
                    store
                        .associate(Address(hash(&bytes)), Action(hash(&bytes)), reference)
                        .unwrap();
                }
                batch.finish().unwrap();
            });
        }
    });
    let reader = Store::new(root);
    for n in 0u32..128 {
        let bytes = n.to_le_bytes();
        let association = reader.lookup(Address(hash(&bytes))).unwrap();
        assert_eq!(association.action, Action(hash(&bytes)));
        assert_eq!(&*reader.get(association.reference).unwrap(), &bytes);
    }
}

#[test]
fn bounded_layer_maintenance_keeps_later_publication_and_checked_access() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_owned());
    let batch = store.batch();
    let mut last = None;
    for n in 0u32..3000 {
        last = Some(store.put(DOMAIN, &n.to_le_bytes()).unwrap());
    }
    batch.finish().unwrap();
    assert_eq!(&*store.get(last.unwrap()).unwrap(), &2999u32.to_le_bytes());
    assert!(
        fs::metadata(dir.path().join("index/MANIFEST"))
            .unwrap()
            .len()
            <= 420
    );
    assert!(
        index::load_pinned(dir.path()).unwrap().retained_bytes()
            <= index::MAX_BYTES as u64 + (4 << 20)
    );
    let next = store.put(DOMAIN, b"after bounded eviction").unwrap();
    assert_eq!(&*store.get(next).unwrap(), b"after bounded eviction");
}

fn current_layer_path(root: &Path) -> PathBuf {
    let _lock = pack::Lock::try_root(root, false).unwrap();
    let id = index::current_layer_ids_locked(root).unwrap()[0];
    root.join("index").join(format!("L-{}", hex(id)))
}

#[test]
fn coherently_forged_routing_cannot_accept_another_content_domain_or_association() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = Store::rebuildable(root.to_owned());
    let batch = store.batch();
    let a = store.put(DOMAIN, b"first!").unwrap();
    let b = store.put(DOMAIN, b"second").unwrap();
    let address_a = Address(hash(b"first address"));
    let address_b = Address(hash(b"second address"));
    store.associate(address_a, Action(hash(b"old")), a).unwrap();
    store
        .associate(address_b, Action(hash(b"other")), b)
        .unwrap();
    batch.finish().unwrap();
    let pin = index::load_pinned(root).unwrap();
    let content = |r: Reference| index::Key {
        kind: index::KeyKind::Content,
        digest: r.output.0,
    };
    let association = |a: Address| index::Key {
        kind: index::KeyKind::Association,
        digest: a.0,
    };
    let mut entries = vec![
        index::Entry {
            key: content(a),
            location: index::find(&pin, content(b)).unwrap(),
        },
        index::Entry {
            key: association(address_a),
            location: index::find(&pin, association(address_b)).unwrap(),
        },
    ];
    entries.sort_by_key(|e| e.key);
    store.publish_test_routes(&entries).unwrap();
    let reader = Store::rebuildable(root.to_owned());
    assert!(reader.get(a).is_none(), "valid B bytes cannot supply A");
    assert!(
        reader.lookup(address_a).is_none(),
        "valid B association cannot supply A address"
    );
    assert_eq!(&*reader.get(b).unwrap(), b"second");
    let wrong_domain = Reference {
        domain: Domain {
            schema: 2,
            ..DOMAIN
        },
        ..a
    };
    assert!(reader.get(wrong_domain).is_none());
    entries[0].location.offset = u32::MAX;
    assert!(
        index::build(root, &entries, crate::Durability::Rebuildable).is_err(),
        "overflowing routing is refused at the build seam"
    );
}

#[test]
fn index_corruption_diagnostics_are_recorded_once_per_shared_pin() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    let store = Store::rebuildable(root.clone());
    let batch = store.batch();
    let reference = store.put(DOMAIN, b"diagnostic demand").unwrap();
    batch.finish().unwrap();
    let layer = current_layer_path(&root);
    let original = fs::read(&layer).unwrap();
    let mut bad = original.clone();
    let key_plane = word(&bad, 32 + 8 * 2).unwrap() as usize;
    bad[key_plane] ^= 1;
    fs::write(&layer, bad).unwrap();
    let reader = Store::rebuildable(root.clone());
    let statistics = reader.enable_statistics();
    let request = reader.request();
    assert!(reader.get(reference).is_none());
    assert!(reader.clone().get(reference).is_none());
    request.finish().unwrap();
    let counters = statistics.snapshot().counters;
    assert_eq!(counters["InvalidIndexRoots"], 0);
    assert_eq!(counters["BadIndexBlocks"], 1);
    assert_eq!(counters["BadIndexBuckets"], 1);
    drop(reader);
    assert_eq!(statistics.snapshot().counters["BadIndexBlocks"], 1);
    fs::write(&layer, original).unwrap();
    fs::write(root.join("index/MANIFEST"), b"malformed").unwrap();
    let reader = Store::rebuildable(root);
    let statistics = reader.enable_statistics();
    let request = reader.request();
    assert!(reader.get(reference).is_none());
    assert!(reader.get(reference).is_none());
    request.finish().unwrap();
    assert_eq!(statistics.snapshot().counters["InvalidIndexRoots"], 1);
    drop(reader);
    assert_eq!(statistics.snapshot().counters["InvalidIndexRoots"], 1);
}

#[test]
fn large_only_admission_publication_preserves_the_first_index_read() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::rebuildable(dir.path().to_owned());
    let address = Address(hash(b"previous product"));
    let action = Action(hash(b"previous action"));
    let reference = store.put(DOMAIN, b"previous payload").unwrap();
    store.associate(address, action, reference).unwrap();
    drop(store);

    let reopened = Store::rebuildable(dir.path().to_owned());
    reopened.set_private_limits(8192, 16 << 20);
    let statistics = reopened.enable_statistics();
    let request = reopened.request();
    reopened.put(DOMAIN, &vec![1; 5000]).unwrap();
    reopened.put(DOMAIN, &vec![2; 5000]).unwrap();
    assert_eq!(statistics.snapshot().counters["AdmissionCommits"], 1);
    assert_eq!(reopened.lookup(address).unwrap().reference, reference);
    assert_eq!(reopened.lookup(address).unwrap().action, action);
    assert_eq!(&*reopened.get(reference).unwrap(), b"previous payload");
    request.finish().unwrap();
}

#[test]
fn owned_buffer_conversion_keeps_its_payload_and_shared_child_lifetime() {
    let input = (0..128u8).collect::<Vec<_>>();
    assert_eq!(input.len(), input.capacity());
    let pointer = input.as_ptr();
    let bytes = Bytes::from(input);
    assert_eq!(
        bytes.as_ptr(),
        pointer,
        "conversion must move the existing buffer"
    );
    let clone = bytes.clone();
    let child = bytes.range(16, 32, 1).unwrap();
    assert_eq!(clone.as_ptr(), pointer);
    assert_eq!(child.as_ptr(), pointer.wrapping_add(16));
    drop(bytes);
    drop(clone);
    assert_eq!(&*child, &(16..48u8).collect::<Vec<_>>());
}

#[test]
fn small_ranges_charge_the_complete_shared_backing_once() {
    for (bytes, expected) in [
        (Bytes::from(vec![7; 8192]), 7),
        (Bytes::from(Arc::<[u8]>::from(vec![8; 8192])), 8),
    ] {
        let child = bytes.range(4000, 1, 1).unwrap();
        let allocations = |bytes: &Bytes| {
            let mut allocations = std::collections::BTreeMap::new();
            bytes.visit_retained_allocations(|identity, amount| {
                assert!(allocations.insert(identity, amount).is_none());
            });
            allocations
        };
        let parent = allocations(&bytes);
        assert_eq!(parent, allocations(&child));
        assert!(parent.values().sum::<usize>() >= 8192);
        let mut shared = parent.clone();
        child.visit_retained_allocations(|identity, amount| {
            assert_eq!(shared.entry(identity).or_insert(amount), &amount);
        });
        assert_eq!(shared, parent);
        drop(bytes);
        assert_eq!(child[0], expected);
    }
}

#[test]
fn payload_integrity_reuse_is_bound_to_the_exact_reference_and_range() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf());
    let reference = store.put(DOMAIN, b"checked payload").unwrap();
    let bytes = store.get(reference).unwrap();
    assert!(bytes.matches_reference(reference));
    let retained = bytes.clone().retain_owner(Arc::new(()));
    drop(bytes);
    drop(store);
    assert!(retained.matches_reference(reference));
    let complete = retained.range(0, reference.len, 1).unwrap();
    assert!(complete.matches_reference(reference));
    for n in 0..4 {
        let mut foreign = reference;
        match n {
            0 => foreign.domain.kind += 1,
            1 => foreign.domain.schema += 1,
            2 => foreign.len -= 1,
            _ => foreign.output.0.0[0] ^= 1,
        }
        assert!(!retained.matches_reference(foreign));
    }
    let child = retained.range(1, reference.len - 1, 1).unwrap();
    assert!(!child.matches_reference(reference));
    assert!(child.matches_reference(DOMAIN.reference(&child).unwrap()));
    let raw = Bytes::from(b"checked payload".as_slice());
    assert!(raw.matches_reference(reference));
    let changed = Bytes::from(b"checked payloae".as_slice());
    assert!(!changed.matches_reference(reference));
}

#[test]
fn checked_payloads_refuse_substitution_and_retain_the_full_parent_owner() {
    let payload = b"a complete immutable payload";
    let reference = DOMAIN.reference(payload).unwrap();
    assert!(
        Bytes::from(b"a complete immutable payloae".as_slice())
            .checked(reference)
            .is_none()
    );
    let mut foreign = reference;
    foreign.domain.schema += 1;
    assert!(Bytes::from(payload.as_slice()).checked(foreign).is_none());
    let owner = Arc::new(());
    let weak = Arc::downgrade(&owner);
    let checked = Bytes::from(payload.as_slice())
        .retain_owner(owner.clone())
        .checked(reference)
        .unwrap();
    drop(owner);
    let allocations = |bytes: &Bytes| {
        let mut seen = std::collections::BTreeMap::new();
        bytes.visit_retained_allocations(|id, size| {
            seen.insert(id, size);
        });
        seen
    };
    let before = allocations(&checked);
    let clone = checked.clone().checked(reference).unwrap();
    assert_eq!(
        allocations(&clone),
        before,
        "a repeated check retains no new owner"
    );
    let child = checked.range(11, 9, 1).unwrap();
    assert!(!child.matches_reference(reference));
    assert_eq!(
        allocations(&child),
        before,
        "a child retains the complete backing"
    );
    let child_reference = DOMAIN.reference(&child).unwrap();
    let child = child.checked(child_reference).unwrap();
    drop(clone);
    drop(checked);
    assert!(weak.upgrade().is_some());
    assert_eq!(&*child, b"immutable");
    assert!(child.matches_reference(child_reference));
    drop(child);
    assert!(weak.upgrade().is_none());
}
