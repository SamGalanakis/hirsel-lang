//! Code store configuration entries.

hirsel_guard::install!();
use hirsel_code_store::*;

#[test]
fn code_entries_open_their_provider_and_refuse_what_they_do_not_define() {
    let dir = tempfile::tempdir().unwrap();
    let key = Signer::from_seed(&[7; 32]).public_key().hex();
    let path = dir.path().join("code");
    let opened = |entry: serde_json::Value| {
        config::open(&entry).map(|s| (s.provider(), s.signatures().clone()))
    };
    assert_eq!(
        opened(serde_json::json!({"provider": "code.none@1"})),
        Ok(("code.none@1", Signatures::None))
    );
    assert_eq!(
        opened(serde_json::json!({"provider": "code.dir@1", "path": path,
            "signatures": {"provider": "sign.ed25519@1", "keys": [key]}})),
        Ok((
            "code.dir@1",
            Signatures::Ed25519 {
                keys: vec![PublicKey::parse(&key).unwrap()]
            }
        ))
    );
    assert_eq!(
        opened(serde_json::json!({"provider": "code.object@1",
            "objects": {"provider": "objects.sim@1"}})),
        Ok(("code.object@1", Signatures::None))
    );
    assert_eq!(
        opened(serde_json::json!({"provider": "code.sim@1", "seed": 3, "fail_one_in": 5})),
        Ok(("code.sim@1", Signatures::None))
    );
    assert_eq!(
        CodeStore::none().add(&Bundle::new()),
        Err(StoreError::NoStore)
    );
    for (entry, error) in [
        (
            serde_json::json!({"provider": "code.none@1", "path": "/x"}),
            "code.none@1 has no member \"path\"",
        ),
        (
            serde_json::json!({"provider": "code.dir@1", "path": "x"}),
            "code.dir@1 path is an absolute directory",
        ),
        (
            serde_json::json!({"provider": "code.git@1"}),
            "unsupported code store provider code.git@1",
        ),
        (
            serde_json::json!({"provider": "code.sim@1", "signatures": {"provider": "sign.ed25519@1", "keys": []}}),
            "sign.ed25519@1 keys are sorted, unique and at least one",
        ),
        (
            serde_json::json!({"provider": "code.sim@1", "signatures": {"provider": "sign.ed25519@1", "keys": [key.to_uppercase()]}}),
            &*format!(
                "sign.ed25519@1 key \"{}\" is not 64 lowercase hex digits",
                key.to_uppercase()
            ),
        ),
        (
            serde_json::json!({"provider": "code.object@1", "objects": {"provider": "objects.s3@1"}}),
            "unsupported object storage provider objects.s3@1",
        ),
    ] {
        assert_eq!(opened(entry).err().as_deref(), Some(error));
    }
}
