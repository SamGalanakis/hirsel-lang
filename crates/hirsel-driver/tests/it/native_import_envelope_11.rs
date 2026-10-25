//! Retired import-cache wire vectors must miss at the demand-product boundary.
use super::native_import_cutover_19::assert_retired_import_payloads;

// Handwritten: magic, version, section count, ordered (tag,length) headers,
// then raw payloads. The fourth section includes both byte boundaries.
const EMPTY: &[u8] = b"HIRIMP\x02\x05\x00\x00\x01\x00\x02\x00\x03\x00\x04\x00";
const BINARY: &[u8] = b"HIRIMP\x02\x05\x00\x01\x01\x02\x02\x00\x03\x02\x04\x00a[]\x00\xff";

#[test]
fn retired_handcrafted_sections_and_go_varint_prefix_vectors_miss() {
    let mut retired = vec![EMPTY.to_vec(), BINARY.to_vec()];
    for prefix in [
        &b"\x7f"[..],
        &b"\x80\x01"[..],
        &b"\xff\x01"[..],
        &b"\x80\x02"[..],
        &b"\x80\x80\x01"[..],
        &b"\x80\x00"[..],
        &b"\x81\x00"[..],
        &b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\x02"[..],
        &b"\x80\x80\x80\x80\x80\x80\x80\x80\x80\x80\x00"[..],
        &b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\x01"[..],
    ] {
        let mut bytes = b"HIRIMP\x02\x05\x00".to_vec();
        bytes.extend_from_slice(prefix);
        bytes.extend_from_slice(b"\x01\x00\x02\x00\x03\x00\x04\x00");
        retired.push(bytes);
    }
    for (size, prefix) in [
        (127, &b"\x7f"[..]),
        (128, &b"\x80\x01"[..]),
        (255, &b"\xff\x01"[..]),
        (256, &b"\x80\x02"[..]),
        (16384, &b"\x80\x80\x01"[..]),
    ] {
        let mut bytes = b"HIRIMP\x02\x05\x00".to_vec();
        bytes.extend_from_slice(prefix);
        bytes.extend_from_slice(b"\x01\x00\x02\x00\x03\x00\x04\x00");
        bytes.extend(std::iter::repeat_n(255, size));
        retired.push(bytes);
    }
    for bytes in [EMPTY, BINARY] {
        retired.extend((0..bytes.len()).map(|end| bytes[..end].to_vec()));
        let mut trailing = bytes.to_vec();
        trailing.push(0);
        retired.push(trailing);
    }
    for index in [0, 5, 6, 7, 8, 10, 12, 14, 16] {
        let mut corrupt = BINARY.to_vec();
        corrupt[index] ^= 0x80;
        retired.push(corrupt);
    }
    let mut sum = b"HIRIMP\x02\x05\x00".to_vec();
    sum.extend_from_slice(b"\xff\xff\xff\xff\xff\xff\xff\xff\x7f\x01\xff\xff\xff\xff\xff\xff\xff\xff\x7f\x02\x02\x03\x00\x04\x00");
    retired.push(sum);
    let mut swapped = BINARY.to_vec();
    swapped.swap(10, 14);
    swapped.swap(11, 15);
    retired.push(swapped);
    retired.push(br#"["old unit",[],[],[0,255]]"#.to_vec());
    assert_retired_import_payloads(&retired);
}
