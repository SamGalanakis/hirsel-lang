use super::*;
fn descriptor(i: u8, rows: u64, len: u64) -> Descriptor {
    Descriptor {
        id: ContentHash([i; 32]),
        rows,
        len,
    }
}
#[test]
fn admission_checks_each_aggregate_cap_without_allocating_from_counts() {
    assert_eq!(
        admission(&[descriptor(0, MAX_ROWS as u64, 128), descriptor(1, 1, 128)]),
        Some(SkipReason::RowLimit)
    );
    assert_eq!(
        admission(&[descriptor(0, 1, MAX_BYTES as u64), descriptor(1, 1, 128)]),
        Some(SkipReason::ByteLimit)
    );
    let layers: Vec<_> = (0..9).map(|i| descriptor(i, 1, 128)).collect();
    assert_eq!(admission(&layers), Some(SkipReason::LayerLimit));
    assert_eq!(
        admission(&[descriptor(0, u64::MAX, 128), descriptor(1, 1, 128)]),
        Some(SkipReason::RowLimit)
    );
    assert_eq!(
        admission(&[descriptor(0, 1, u64::MAX), descriptor(1, 1, 128)]),
        Some(SkipReason::ByteLimit)
    );
    assert_eq!(
        admission(&[descriptor(0, MAX_ROWS as u64, MAX_BYTES as u64)]),
        None
    );
}
#[test]
fn selection_is_inclusive_accumulated_adjacent_suffix_and_forced_pair_at_cap() {
    let runs = |rows: &[u64]| {
        rows.iter()
            .enumerate()
            .map(|(i, n)| descriptor(i as u8, *n, 128))
            .collect::<Vec<_>>()
    };
    assert_eq!(selection(&runs(&[1, 4, 20])), 3);
    assert_eq!(selection(&runs(&[1, 5, 20])), 1);
    assert_eq!(
        selection(&runs(&[1, 5, 25, 125, 625, 3125, 15625, 78125])),
        2
    );
    assert_eq!(selection(&[]), 0);
}
