//! The old table/effects frames cannot authorize demand-native products.
use super::native_import_cutover_19::assert_retired_import_payloads;

#[test]
fn retired_raw_sections_and_previous_versions_miss() {
    let mut payloads = vec![
        b"HIRIMP\x02\x05\x00\x01\x01\x02\x02\x02\x03\x02\x04\x07u[][][]{\"x\":1}".to_vec(),
        b"HIRIMP\x01\x04\x00\x00\x01\x00\x02\x00\x03\x00".to_vec(),
        b"HIRIMP\x01\x04\x00\x01\x01\x02\x02\x02\x03\x0cu[][][[],[91,93]]".to_vec(),
        br#"["u",[],[],[91,93]]"#.to_vec(),
        b"HIRIMP\x02\x04\x00\x00\x01\x00\x02\x00\x03\x00".to_vec(),
        " {\"text\":\"λ\\n\",\"bytes\":[0,255]} \n"
            .as_bytes()
            .to_vec(),
    ];
    let raw = " {\"text\":\"λ\\n\",\"bytes\":[0,255]} \n".as_bytes();
    assert!(raw.len() < 128);
    let mut frame = b"HIRIMP\x02\x05\x00\x01\x01\x02\x02\x02\x03\x02\x04".to_vec();
    frame.push(raw.len() as u8);
    frame.extend_from_slice(b"u[][][]");
    frame.extend_from_slice(raw);
    payloads.push(frame);
    assert_retired_import_payloads(&payloads);
}
