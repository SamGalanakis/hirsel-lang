use super::*;

fn base(name: &str, body: u32) -> InstanceKey {
    let types = hirsel_types::TypeInterner::new(None);
    InstanceKey::new(&DefPath::core(name), body, &[], &types, None, None).unwrap()
}
fn decode(key: &InstanceKey) -> Result<InstanceKey, InstanceKeyError> {
    InstanceKey::from_encoding(key.definition(), key.body(), key.encoding().to_vec())
}
#[test]
fn ordinary_and_nested_selected_keys_recompute_their_existing_digests() {
    let original = base("key_codec", 7);
    let profile = original.derived("profile-test", &[b"first", b"second"]);
    let request = profile.derived("demand-test", &[b"contract"]);
    for key in [&original, &profile, &request] {
        let restored = decode(key).unwrap();
        assert_eq!(restored, *key);
        assert_eq!(restored.digest(), key.digest());
        assert_eq!(restored.definition(), key.definition());
        assert_eq!(restored.body(), key.body());
        assert_eq!(restored.encoding(), key.encoding());
    }
}
#[test]
fn another_owner_body_schema_or_malformed_atom_is_refused() {
    let key = base("key_codec", 7).derived("selected-test", &[b"input"]);
    assert!(
        InstanceKey::from_encoding(&DefPath::core("other"), 7, key.encoding().to_vec()).is_err()
    );
    assert!(InstanceKey::from_encoding(key.definition(), 8, key.encoding().to_vec()).is_err());
    let mut truncated = key.encoding().to_vec();
    truncated.pop();
    let mut trailing = key.encoding().to_vec();
    trailing.push(0);
    let mut huge = key.encoding().to_vec();
    huge[..8].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut unknown = key.encoding().to_vec();
    unknown[8] ^= 1;
    for bytes in [vec![], vec![0; 7], truncated, trailing, huge, unknown] {
        assert!(InstanceKey::from_encoding(key.definition(), key.body(), bytes).is_err());
    }
}
#[test]
fn ancestry_retains_the_complete_source_context_through_nested_requests() {
    let original = base("key_codec", 7);
    let profile = original.derived("profile-test", &[b"ordinary"]);
    let other = original.derived("profile-test", &[b"unique"]);
    let requested = profile.derived("demand-test", &[b"first"]);
    let nested = requested.derived("implementation-test", &[b"second"]);
    let restored = decode(&nested).unwrap();
    for ancestor in [&original, &profile, &requested, &nested] {
        assert!(restored.is_same_or_derived_from(ancestor));
    }
    assert!(!restored.is_same_or_derived_from(&other));
    assert!(!restored.is_same_or_derived_from(&base("key_codec", 8)));
    assert!(!restored.is_same_or_derived_from(&base("other", 7)));
    assert!(!original.is_same_or_derived_from(&nested));
}
