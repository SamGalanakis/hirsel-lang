//! Independent version-2 vectors: the table and effects are raw JSON sections,
//! following serde_json src/raw.rs:16-76's verbatim payload contract.
#[path = "../../src/native/import_envelope.rs"]
mod envelope;

#[test]
fn raw_table_and_effect_json_have_independent_boundaries() {
    // Handwritten lengths, never constructed with the production encoder.
    let bytes = b"HIRIMP\x02\x05\x00\x01\x01\x02\x02\x02\x03\x02\x04\x07u[][][]{\"x\":1}";
    let sections = [
        &b"u"[..],
        &b"[]"[..],
        &b"[]"[..],
        &b"[]"[..],
        &br#"{"x":1}"#[..],
    ];
    assert_eq!(envelope::decode(bytes), Some(sections));
    assert_eq!(envelope::encode(sections).unwrap(), bytes);
    for section in &sections[1..] {
        serde_json::from_slice::<serde_json::Value>(section).unwrap();
    }
    // Whitespace, escapes and non-ASCII bytes are transferred verbatim.
    let raw = " {\"text\":\"λ\\n\",\"bytes\":[0,255]} \n".as_bytes();
    let encoded = envelope::encode([b"u", b"[]", b"[]", b"[]", raw]).unwrap();
    assert_eq!(envelope::decode(&encoded).unwrap()[4], raw);
    assert!(encoded.ends_with(raw));
}

#[test]
fn legacy_json_and_version_one_numeric_byte_array_frames_miss() {
    // Previous HIRIMP v1, four sections with (table, Vec<u8>) JSON effects.
    for old in [
        &b"HIRIMP\x01\x04\x00\x00\x01\x00\x02\x00\x03\x00"[..],
        &b"HIRIMP\x01\x04\x00\x01\x01\x02\x02\x02\x03\x0cu[][][[],[91,93]]"[..],
        &br#"["u",[],[],[91,93]]"#[..],
    ] {
        assert!(envelope::decode(old).is_none());
    }
    // A new version must also use the new section count, even for an empty body.
    assert!(envelope::decode(b"HIRIMP\x02\x04\x00\x00\x01\x00\x02\x00\x03\x00").is_none());
}

#[test]
fn effect_boundaries_reject_truncation_overflow_reordering_and_trailing_data() {
    let complete = b"HIRIMP\x02\x05\x00\x01\x01\x02\x02\x02\x03\x02\x04\x02u[][][][]";
    assert!(envelope::decode(complete).is_some());
    for end in 0..complete.len() {
        assert!(envelope::decode(&complete[..end]).is_none(), "prefix {end}");
    }
    let mut trailing = complete.to_vec();
    trailing.push(b' ');
    assert!(envelope::decode(&trailing).is_none());
    let mut swapped = complete.to_vec();
    swapped.swap(14, 16);
    assert!(envelope::decode(&swapped).is_none());
    for length in [
        &b"\x80\x00"[..],
        &b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\x02"[..],
        &b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\x01"[..],
    ] {
        let mut corrupt = complete[..17].to_vec();
        corrupt.extend_from_slice(length);
        corrupt.extend_from_slice(&complete[18..]);
        assert!(envelope::decode(&corrupt).is_none());
    }
}
