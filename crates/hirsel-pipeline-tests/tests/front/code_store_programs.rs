//! The code store (hirsel-code-store) over real programs' definition groups:
//! all-or-nothing adds that name exactly what is missing, idempotent re-adds,
//! refusal of bytes that do not hash to their key, signature policies, seeded
//! storage faults and a place's code check on arrival.
use hirsel_code_store::*;
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_object_storage::{DirObjects, MemoryObjects, ObjectStorage};
use hirsel_proof_export::DefinitionGroup;
use hirsel_syntax::ContentHash;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const PROGRAM: &str = "fn leaf(x: u64) -> u64 { x * 2u64 }\nfn other(n: u64) -> u64 { n + 7u64 }\nfn mid(n: u64) -> u64 { leaf(n) + other(1u64) }\nfn even(n: u64) -> bool {\n  if n == 0u64 { true } else { odd(n - 1u64) }\n}\nfn odd(n: u64) -> bool {\n  if n == 0u64 { false } else { even(n - 1u64) }\n}\npub fn main() -> Result[(), ()] {\n  let n = mid(3u64)\n  let e = even(n)\n  Ok(())\n}\n";

fn named(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}

/// The definition groups of `main`'s closure in `source`, with every
/// definition's hash from the hash query.
fn groups(source: &str) -> (Vec<DefinitionGroup>, BTreeMap<DefPath, ContentHash>) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("program.hir", source.into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let groups = engine.definition_groups([named("main")]).unwrap();
        let hashes = engine.definition_hashes([named("main")]).unwrap().hashes;
        (groups, hashes)
    })
}

fn hash_of(groups: &[DefinitionGroup], name: &str) -> ContentHash {
    let path = named(name);
    groups
        .iter()
        .flat_map(|g| g.members.iter().zip(&g.hashes))
        .find(|(p, _)| **p == path)
        .map(|(_, h)| *h)
        .unwrap()
}

fn dir_store(signatures: Signatures) -> (tempfile::TempDir, CodeStore) {
    let dir = tempfile::tempdir().unwrap();
    let objects = DirObjects::open(dir.path().join("code")).unwrap();
    (
        dir,
        CodeStore::over("code.dir@1", Arc::new(objects), signatures),
    )
}

fn files(dir: &tempfile::TempDir) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut pending = vec![dir.path().to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(&path).unwrap() {
            let entry = entry.unwrap().path();
            if entry.is_dir() {
                pending.push(entry);
            } else {
                let name = entry
                    .strip_prefix(dir.path())
                    .unwrap()
                    .display()
                    .to_string();
                out.insert(name, std::fs::read(&entry).unwrap());
            }
        }
    }
    out
}

#[test]
fn objects_recompute_the_hash_query_and_a_recursive_group_is_one_object_per_member() {
    let (groups, hashes) = groups(PROGRAM);
    let mut seen = BTreeMap::new();
    for group in &groups {
        for (member, object) in CodeObject::of_group(group).into_iter().enumerate() {
            assert_eq!(object.hash(), hashes[&group.members[member]]);
            let decoded = CodeObject::decode(object.hash(), object.bytes().to_vec()).unwrap();
            assert_eq!(decoded, object);
            seen.insert(group.members[member].clone(), object);
        }
    }
    assert_eq!(
        seen.keys().collect::<Vec<_>>(),
        hashes.keys().collect::<Vec<_>>()
    );
    // even and odd share one group: each names only what lies outside it.
    let (even, odd) = (&seen[&named("even")], &seen[&named("odd")]);
    assert_ne!(even.hash(), odd.hash());
    assert!(!even.dependencies().contains(&odd.hash()));
    assert!(seen[&named("main")].dependencies().contains(&even.hash()));
    assert!(
        seen[&named("mid")]
            .dependencies()
            .contains(&hash_of(&groups, "leaf"))
    );
}

#[test]
fn an_add_missing_dependencies_adds_nothing_and_names_exactly_the_missing_hashes() {
    let (groups, _) = groups(PROGRAM);
    let (dir, store) = dir_store(Signatures::None);
    let mut bundle = Bundle::of_groups(&groups);
    let (leaf, other) = (hash_of(&groups, "leaf"), hash_of(&groups, "other"));
    bundle.remove(&leaf);
    bundle.remove(&other);
    assert_eq!(
        store.add(&bundle).unwrap(),
        Added::Missing(BTreeSet::from([leaf, other]))
    );
    assert!(files(&dir).is_empty());
    // With them, the same closure is complete.
    let whole = Bundle::of_groups(&groups);
    assert_eq!(store.add(&whole).unwrap(), Added::Ok);
    match store.closure(&hash_of(&groups, "main")).unwrap() {
        Closure::Complete(objects) => assert_eq!(objects.len(), whole.len()),
        missing => panic!("{missing:?}"),
    }
}

#[test]
fn re_adding_a_bundle_is_idempotent() {
    let (groups, _) = groups(PROGRAM);
    let (dir, store) = dir_store(Signatures::None);
    let bundle = Bundle::of_groups(&groups);
    assert_eq!(store.add(&bundle).unwrap(), Added::Ok);
    let stored = files(&dir);
    assert_eq!(stored.len(), bundle.len());
    assert_eq!(store.add(&bundle).unwrap(), Added::Ok);
    assert_eq!(files(&dir), stored);
    for (hash, bytes) in bundle.objects() {
        assert_eq!(store.get(hash).unwrap().unwrap().bytes(), bytes);
    }
}

#[test]
fn bytes_that_do_not_hash_to_their_key_are_refused_on_add_and_on_read() {
    let (groups, _) = groups(PROGRAM);
    let (_dir, store) = dir_store(Signatures::None);
    let leaf = hash_of(&groups, "leaf");
    let bundle = Bundle::of_groups(&groups);
    let original = String::from_utf8(
        bundle
            .objects()
            .find(|(h, _)| **h == leaf)
            .unwrap()
            .1
            .to_vec(),
    )
    .unwrap();
    // A changed constant still decodes but hashes elsewhere.
    let tampered_text = original.replacen("(Integer 2)", "(Integer 3)", 1);
    assert_ne!(tampered_text, original, "{original}");
    let mut tampered = bundle.clone();
    tampered.insert(leaf, tampered_text.into_bytes());
    match store.add(&tampered) {
        Err(StoreError::Refused {
            hash,
            reason: ObjectError::Tampered { actual },
        }) => {
            assert_eq!(hash, leaf);
            assert_ne!(actual, leaf);
        }
        other => panic!("{other:?}"),
    }
    // Bytes claimed for another definition's hash.
    let mut swapped = bundle.clone();
    swapped.insert(hash_of(&groups, "other"), original.clone().into_bytes());
    assert!(matches!(
        store.add(&swapped),
        Err(StoreError::Refused {
            reason: ObjectError::Tampered { .. },
            ..
        })
    ));
    // Non-canonical spacing is refused even though it means the same.
    let mut spaced = bundle.clone();
    spaced.insert(leaf, original.replacen(' ', "  ", 1).into_bytes());
    assert!(matches!(
        store.add(&spaced),
        Err(StoreError::Refused {
            reason: ObjectError::Malformed(_),
            ..
        })
    ));
    assert_eq!(store.get(&leaf).unwrap(), None);
    // A medium that changes stored bytes is caught on every read.
    let objects = Arc::new(MemoryObjects::new());
    let store = CodeStore::over("code.sim@1", objects.clone(), Signatures::None);
    assert_eq!(store.add(&bundle).unwrap(), Added::Ok);
    objects.overwrite(
        &format!("code/{}", hex(&leaf)),
        original
            .replacen("(Integer 2)", "(Integer 3)", 1)
            .into_bytes(),
    );
    assert!(matches!(store.get(&leaf), Err(StoreError::Corrupt { hash, .. }) if hash == leaf));
    assert!(matches!(
        store.closure(&hash_of(&groups, "main")),
        Err(StoreError::Corrupt { hash, .. }) if hash == leaf
    ));
}

#[test]
fn a_signing_store_refuses_unsigned_and_untrusted_code_and_rechecks_on_read() {
    let (groups, _) = groups(PROGRAM);
    let trusted = Signer::from_seed(&[7; 32]);
    let stranger = Signer::from_seed(&[9; 32]);
    let policy = Signatures::Ed25519 {
        keys: vec![trusted.public_key()],
    };
    let (dir, store) = dir_store(policy.clone());
    let bundle = Bundle::of_groups(&groups);
    let first = *bundle.objects().next().unwrap().0;
    assert_eq!(store.add(&bundle), Err(StoreError::Unsigned(first)));
    let mut foreign = bundle.clone();
    foreign.sign(&stranger);
    assert_eq!(store.add(&foreign), Err(StoreError::Unsigned(first)));
    // A trusted key's signature over another hash does not count.
    let mut misdirected = bundle.clone();
    let wrong = trusted.sign(&ContentHash([0; 32]));
    misdirected.insert_signature(first, trusted.public_key(), wrong);
    assert_eq!(store.add(&misdirected), Err(StoreError::Unsigned(first)));
    assert!(files(&dir).is_empty());
    let mut signed = bundle.clone();
    signed.sign(&trusted);
    assert_eq!(store.add(&signed).unwrap(), Added::Ok);
    let main = hash_of(&groups, "main");
    assert!(matches!(
        store.closure(&main).unwrap(),
        Closure::Complete(_)
    ));
    // Code written by an unsigned store into the same objects is refused
    // when read through the signing one.
    let unsigned = CodeStore::over(
        "code.dir@1",
        Arc::new(DirObjects::open(dir.path().join("code")).unwrap()),
        Signatures::None,
    );
    let (extra, _) = self::groups(&PROGRAM.replace("x * 2u64", "x * 5u64"));
    assert_eq!(unsigned.add(&Bundle::of_groups(&extra)).unwrap(), Added::Ok);
    let changed = hash_of(&extra, "leaf");
    assert!(unsigned.get(&changed).unwrap().is_some());
    assert_eq!(store.get(&changed), Err(StoreError::Unsigned(changed)));
}

#[test]
fn seeded_storage_faults_never_leave_an_object_without_its_closure() {
    let (groups, _) = groups(PROGRAM);
    let bundle = Bundle::of_groups(&groups);
    let dependencies: BTreeMap<String, Vec<String>> = groups
        .iter()
        .flat_map(CodeObject::of_group)
        .map(|object| {
            let key = |hash: &ContentHash| format!("code/{}", hex(hash));
            (
                key(&object.hash()),
                object.dependencies().iter().map(key).collect(),
            )
        })
        .collect();
    // Seeds whose fault streams interrupt the first attempts.
    for seed in [1, 3] {
        let objects = Arc::new(MemoryObjects::with_faults(seed, 8));
        let store = CodeStore::over("code.sim@1", objects.clone(), Signatures::None);
        let mut attempts = 0;
        loop {
            attempts += 1;
            let outcome = store.add(&bundle);
            // Every object stored so far has its whole closure stored.
            let stored = objects.contents();
            for key in stored.keys() {
                for dependency in &dependencies[key] {
                    assert!(
                        stored.contains_key(dependency),
                        "seed {seed}: {key} without {dependency}"
                    );
                }
            }
            match outcome {
                Ok(added) => {
                    assert_eq!(added, Added::Ok);
                    break;
                }
                Err(StoreError::Storage(_)) => assert!(attempts < 1_000, "seed {seed}"),
                Err(other) => panic!("{other:?}"),
            }
        }
        assert!(attempts > 1, "seed {seed} never faulted");
        assert_eq!(objects.contents().len(), dependencies.len());
    }
}

#[test]
fn a_place_runs_its_own_code_fetches_a_stored_closure_and_otherwise_mismatches() {
    let (groups, local) = groups(PROGRAM);
    let (newer, newer_hashes) = self::groups(&PROGRAM.replace("n + 7u64", "n + 8u64"));
    let main = named("main");
    let identity = CodeIdentity {
        path: main.clone(),
        hash: newer_hashes[&main],
    };
    assert_ne!(identity.hash, local[&main]);
    let own = CodeIdentity {
        path: main.clone(),
        hash: local[&main],
    };
    assert_eq!(
        resolve(&own, &local, &CodeStore::none()),
        Ok(Arrival::Local)
    );
    // code.none@1 holds only the place's revision.
    assert_eq!(
        resolve(&identity, &local, &CodeStore::none()),
        Err(VersionMismatch {
            missing: BTreeSet::from([identity.hash]),
            cause: None
        })
    );
    let objects = Arc::new(MemoryObjects::new());
    let store = CodeStore::over("code.sim@1", objects.clone(), Signatures::None);
    assert_eq!(store.add(&Bundle::of_groups(&groups)).unwrap(), Added::Ok);
    assert_eq!(
        resolve(&identity, &local, &store),
        Err(VersionMismatch {
            missing: BTreeSet::from([identity.hash]),
            cause: None
        })
    );
    // A medium written around `add` can hold main without its closure: the
    // mismatch names exactly what is missing.
    let newer_main = newer
        .iter()
        .flat_map(CodeObject::of_group)
        .find(|object| object.hash() == identity.hash)
        .unwrap();
    objects
        .put_new(&format!("code/{}", hex(&identity.hash)), newer_main.bytes())
        .unwrap();
    let mid = hash_of(&newer, "mid");
    assert_eq!(
        resolve(&identity, &local, &store),
        Err(VersionMismatch {
            missing: BTreeSet::from([mid]),
            cause: None
        })
    );
    assert_eq!(store.add(&Bundle::of_groups(&newer)).unwrap(), Added::Ok);
    match resolve(&identity, &local, &store) {
        Ok(Arrival::Fetched(objects)) => {
            let expected: BTreeSet<_> = newer_hashes.values().copied().collect();
            assert_eq!(objects.keys().copied().collect::<BTreeSet<_>>(), expected);
        }
        other => panic!("{other:?}"),
    }
    // A failing store is a mismatch with its cause.
    let failing = CodeStore::over(
        "code.sim@1",
        Arc::new(MemoryObjects::with_faults(1, 1)),
        Signatures::None,
    );
    assert!(matches!(
        resolve(&identity, &local, &failing),
        Err(VersionMismatch {
            cause: Some(StoreError::Storage(_)),
            ..
        })
    ));
}
