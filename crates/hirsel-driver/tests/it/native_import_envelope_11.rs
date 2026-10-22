//! Independent byte vectors for the private import envelope. Include the private
//! codec here so no production API is exposed solely for malformed-input tests.
#[path = "../../src/native/import_envelope.rs"]
mod envelope;

// Handwritten: magic, version, section count, ordered (tag,length) headers,
// then raw payloads. The fourth section includes both byte boundaries.
const EMPTY: &[u8] = b"HIRIMP\x02\x05\x00\x00\x01\x00\x02\x00\x03\x00\x04\x00";
const BINARY: &[u8] = b"HIRIMP\x02\x05\x00\x01\x01\x02\x02\x00\x03\x02\x04\x00a[]\x00\xff";

#[test]
fn handcrafted_sections_and_go_varint_prefix_vectors() {
    for (bytes, sections) in [
        (EMPTY, [&b""[..], &b""[..], &b""[..], &b""[..], &b""[..]]),
        (
            BINARY,
            [&b"a"[..], &b"[]"[..], &b""[..], &b"\x00\xff"[..], &b""[..]],
        ),
    ] {
        assert_eq!(envelope::decode(bytes), Some(sections));
        assert_eq!(envelope::encode(sections).unwrap(), bytes);
    }
    // Go varint_test.go:57-84's independently expected prefix and roundtrip
    // structure, with boundaries for Hirsel's unsigned section lengths.
    for (size, prefix) in [
        (127, &b"\x7f"[..]),
        (128, &b"\x80\x01"[..]),
        (255, &b"\xff\x01"[..]),
        (256, &b"\x80\x02"[..]),
        (16384, &b"\x80\x80\x01"[..]),
    ] {
        let payload = vec![255; size];
        let mut expected = b"HIRIMP\x02\x05\x00".to_vec();
        expected.extend_from_slice(prefix);
        expected.extend_from_slice(b"\x01\x00\x02\x00\x03\x00\x04\x00");
        expected.extend_from_slice(&payload);
        let sections = [&payload[..], &b""[..], &b""[..], &b""[..], &b""[..]];
        assert_eq!(envelope::encode(sections).unwrap(), expected);
        assert_eq!(envelope::decode(&expected), Some(sections));
    }
}

#[test]
fn every_truncation_and_trailing_byte_is_rejected() {
    for bytes in [EMPTY, BINARY] {
        for end in 0..bytes.len() {
            assert!(envelope::decode(&bytes[..end]).is_none(), "prefix {end}");
        }
        let mut trailing = bytes.to_vec();
        trailing.push(0);
        assert!(envelope::decode(&trailing).is_none());
    }
}

#[test]
fn malformed_headers_versions_lengths_and_swapped_sections_are_rejected() {
    for index in [0, 5, 6, 7, 8, 10, 12, 14, 16] {
        let mut corrupt = BINARY.to_vec();
        corrupt[index] ^= 0x80;
        assert!(envelope::decode(&corrupt).is_none(), "byte {index}");
    }
    for length in [
        &b"\x80\x00"[..],                                     // overlong zero
        &b"\x81\x00"[..],                                     // overlong one
        &b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\x02"[..],     // u64 overflow
        &b"\x80\x80\x80\x80\x80\x80\x80\x80\x80\x80\x00"[..], // too many bytes
        &b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\x01"[..],     // u64::MAX, short body
    ] {
        let mut corrupt = b"HIRIMP\x02\x05\x00".to_vec();
        corrupt.extend_from_slice(length);
        corrupt.extend_from_slice(b"\x01\x00\x02\x00\x03\x00\x04\x00");
        assert!(envelope::decode(&corrupt).is_none());
    }
    // Two individually representable lengths whose sum overflows usize on 64-bit.
    let mut sum = b"HIRIMP\x02\x05\x00".to_vec();
    sum.extend_from_slice(b"\xff\xff\xff\xff\xff\xff\xff\xff\x7f\x01\xff\xff\xff\xff\xff\xff\xff\xff\x7f\x02\x02\x03\x00\x04\x00");
    assert!(envelope::decode(&sum).is_none());
    let mut swapped = BINARY.to_vec();
    swapped.swap(10, 14);
    swapped.swap(11, 15);
    assert!(envelope::decode(&swapped).is_none());
    assert!(envelope::decode(br#"["old unit",[],[],[0,255]]"#).is_none());
}
