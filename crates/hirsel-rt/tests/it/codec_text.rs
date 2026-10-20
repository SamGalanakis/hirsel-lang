//! A codec envelope as text: the descriptor's names, every scalar, sequences and maps, and
//! a refusal, never a panic, for malformed, deep or oversized input.
use hirsel_rt::codec_text::render;

fn hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

/// An envelope of descriptor `nodes` (after the version, root and count) and `payload`.
fn envelope(count: u32, nodes: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut descriptor = vec![];
    descriptor.extend(1u32.to_le_bytes());
    descriptor.extend(0u32.to_le_bytes());
    descriptor.extend(count.to_le_bytes());
    descriptor.extend(nodes);
    let mut bytes = b"HSC1".to_vec();
    bytes.extend(1u32.to_le_bytes());
    bytes.extend((descriptor.len() as u64).to_le_bytes());
    bytes.extend(descriptor);
    bytes.extend((payload.len() as u64).to_le_bytes());
    bytes.extend(payload);
    bytes
}
fn text(s: &str) -> Vec<u8> {
    let mut bytes = (s.len() as u64).to_le_bytes().to_vec();
    bytes.extend(s.as_bytes());
    bytes
}

#[test]
fn a_recorded_receipt_names_its_type_and_fields() {
    // A ledger-service transfer's outcome, as its callee's tape recorded it.
    let receipt = hex(
        "48534331010000009600000000000000010000000000000003000000060e000000000000006c65646765725f736572766963650100000000000000310100000006000000000000006c6564676572070000000000000052656365697074010000000204000000040000000000000066726f6d020000000200000000000000746f0200000005000000000000006d6f766564020000000300000000000000666565020000000004200000000000000001000000000000000200000000000000c8000000000000000000000000000000",
    );
    assert_eq!(
        render(&receipt).unwrap(),
        "Receipt(from: 1, to: 2, moved: 200, fee: 0)"
    );
    // Every proper prefix is refused.
    for end in 0..receipt.len() {
        assert!(render(&receipt[..end]).is_err(), "{end}");
    }
    let mut trailing = receipt.clone();
    trailing.push(0);
    assert!(render(&trailing).is_err());
}

#[test]
fn scalars_render_as_source_spells_them() {
    let cases: [(u8, &[u8], &str); 9] = [
        (0, &[1], "true"),
        (4, &u64::MAX.to_le_bytes(), "18446744073709551615"),
        (6, &[0x80], "-128"),
        (9, &(-5i64).to_le_bytes(), "-5"),
        (
            10,
            &i128::MIN.to_le_bytes(),
            "-170141183460469231731687303715884105728",
        ),
        (12, &1.5f64.to_le_bytes(), "1.5"),
        (13, &text("a\"b\n"), "\"a\\\"b\\n\""),
        (14, &text("\0z"), "b\"\\x00z\""),
        (15, &('é' as u32).to_le_bytes(), "'é'"),
    ];
    for (kind, payload, expected) in cases {
        assert_eq!(render(&envelope(1, &[0, kind], payload)).unwrap(), expected);
    }
    // A bool other than 00 or 01, and an invalid char, are refused.
    assert!(render(&envelope(1, &[0, 0], &[2])).is_err());
    assert!(render(&envelope(1, &[0, 15], &0xd800u32.to_le_bytes())).is_err());
}

#[test]
fn variants_lists_and_maps_render_with_their_names() {
    // 0: variant None() | Some(1), labelled `value`; 1: List[2]; 2: u8.
    let mut nodes = vec![3];
    nodes.extend(2u32.to_le_bytes());
    nodes.extend(text("None"));
    nodes.extend(0u32.to_le_bytes());
    nodes.extend(text("Some"));
    nodes.extend(1u32.to_le_bytes());
    nodes.push(1);
    nodes.extend(text("value"));
    nodes.extend(1u32.to_le_bytes());
    nodes.extend([5, 1]);
    nodes.extend(1u32.to_le_bytes());
    nodes.push(0);
    nodes.extend(2u32.to_le_bytes());
    nodes.extend([0, 1]);
    let mut payload = 1u32.to_le_bytes().to_vec();
    payload.extend(2u64.to_le_bytes());
    payload.extend([7, 9]);
    assert_eq!(
        render(&envelope(3, &nodes, &payload)).unwrap(),
        "Some(value: [7, 9])"
    );
    assert_eq!(
        render(&envelope(3, &nodes, &0u32.to_le_bytes())).unwrap(),
        "None()"
    );
    // An arm the descriptor does not have is refused.
    assert!(render(&envelope(3, &nodes, &2u32.to_le_bytes())).is_err());
    // Map[u8, u8]: 0 the map, 1 u8.
    let mut map = vec![5, 5];
    map.extend(2u32.to_le_bytes());
    map.push(0);
    map.extend(1u32.to_le_bytes());
    map.push(0);
    map.extend(1u32.to_le_bytes());
    map.extend([0, 1]);
    let mut payload = 1u64.to_le_bytes().to_vec();
    payload.extend([3, 4]);
    assert_eq!(render(&envelope(2, &map, &payload)).unwrap(), "{3: 4}");
}

#[test]
fn deep_or_endless_values_are_refused() {
    // A recursive list of itself: 0 is List[0], nested past the depth bound.
    let mut nodes = vec![5, 1];
    nodes.extend(1u32.to_le_bytes());
    nodes.push(0);
    nodes.extend(0u32.to_le_bytes());
    let payload: Vec<u8> = (0..200)
        .flat_map(|_| 1u64.to_le_bytes())
        .chain(0u64.to_le_bytes())
        .collect();
    assert!(render(&envelope(1, &nodes, &payload)).is_err());
    // A list of a billion units reads no bytes, so the text's length bounds it.
    let mut units = vec![5, 1];
    units.extend(1u32.to_le_bytes());
    units.push(0);
    units.extend(1u32.to_le_bytes());
    units.push(1);
    units.extend(0u32.to_le_bytes());
    let payload = 1_000_000_000u64.to_le_bytes();
    assert!(render(&envelope(2, &units, &payload)).is_err());
}
