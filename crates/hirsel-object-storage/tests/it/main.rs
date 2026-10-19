//! The object storage contract, run against every implementation.
use hirsel_object_storage::*;
use std::sync::Arc;

fn contract(objects: &dyn ObjectStorage) {
    assert_eq!(objects.get("code/a1").unwrap(), None);
    assert_eq!(objects.put_new("code/a1", b"first").unwrap(), Put::Created);
    assert_eq!(
        objects.get("code/a1").unwrap().as_deref(),
        Some(&b"first"[..])
    );
    // Create-if-absent never replaces, whatever the new bytes.
    assert_eq!(objects.put_new("code/a1", b"second").unwrap(), Put::Exists);
    assert_eq!(objects.put_new("code/a1", b"first").unwrap(), Put::Exists);
    assert_eq!(
        objects.get("code/a1").unwrap().as_deref(),
        Some(&b"first"[..])
    );
    assert_eq!(objects.put_new("sig/a1/k-2", b"").unwrap(), Put::Created);
    assert_eq!(
        objects.get("sig/a1/k-2").unwrap().as_deref(),
        Some(&b""[..])
    );
    for bad in [
        "",
        "/x",
        "x/",
        "../x",
        "code/../x",
        "code/.tmp-1",
        "Code/x",
        "code//x",
        "a b",
    ] {
        assert_eq!(
            objects.get(bad),
            Err(StorageError::Key(bad.into())),
            "{bad:?}"
        );
        assert_eq!(
            objects.put_new(bad, b"x"),
            Err(StorageError::Key(bad.into())),
            "{bad:?}"
        );
    }
}

#[test]
fn a_directory_creates_each_object_once_and_keeps_it_across_reopening() {
    let dir = tempfile::tempdir().unwrap();
    contract(&DirObjects::open(dir.path().join("store")).unwrap());
    let reopened = DirObjects::open(dir.path().join("store")).unwrap();
    assert_eq!(
        reopened.get("code/a1").unwrap().as_deref(),
        Some(&b"first"[..])
    );
    // No temporary file outlives a put.
    let names: Vec<_> = std::fs::read_dir(dir.path().join("store/code"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["a1"]);
}

#[test]
fn memory_objects_follow_the_same_contract() {
    contract(&MemoryObjects::new());
}

#[test]
fn simulated_faults_are_seeded_and_a_lost_put_reply_may_have_created_the_object() {
    let run = |seed| {
        let objects = MemoryObjects::with_faults(seed, 3);
        let outcomes: Vec<_> = (0..24)
            .map(|i| objects.put_new(&format!("k{}", i % 6), b"v").is_ok())
            .collect();
        (outcomes, objects.contents())
    };
    assert_eq!(run(7), run(7));
    assert_ne!(run(7).0, run(8).0);
    // Some failed put still created its object: the failure was the reply.
    let objects = MemoryObjects::with_faults(7, 2);
    let mut created_by_failed_put = false;
    for i in 0..64 {
        let key = format!("k{i}");
        if objects.put_new(&key, b"v").is_err() && objects.contents().contains_key(&key) {
            created_by_failed_put = true;
        }
    }
    assert!(created_by_failed_put);
}

#[test]
fn entries_open_their_provider_and_refuse_unknown_members() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("objects");
    let entry = serde_json::json!({"provider": "objects.dir@1", "path": path});
    let objects: Arc<dyn ObjectStorage> = open(&entry).unwrap();
    objects.put_new("x", b"1").unwrap();
    assert!(path.join("x").exists());
    assert!(open(&serde_json::json!({"provider": "objects.sim@1", "seed": 3})).is_ok());
    for (entry, error) in [
        (
            serde_json::json!({"provider": "objects.dir@1", "path": "relative"}),
            "objects.dir@1 path is an absolute directory",
        ),
        (
            serde_json::json!({"provider": "objects.sim@1", "faults": 1}),
            "objects.sim@1 has no member \"faults\"",
        ),
        (
            serde_json::json!({"provider": "objects.s3@1"}),
            "unsupported object storage provider objects.s3@1",
        ),
    ] {
        assert_eq!(open(&entry).err().as_deref(), Some(error));
    }
}
