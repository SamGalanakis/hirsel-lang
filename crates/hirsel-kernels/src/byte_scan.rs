//! Go strings.IndexAny and asciiSet, src/strings/strings.go:184-240,948-967
//! at dc70946c365d0c23dc2fccc2c3ff94c20c77e3b8 (BSD-3-Clause).
//! Byte extents use all 256 entries, including non-ASCII bytes.

/// First byte at or after `start` contained in `needles`, or `source.len()`.
/// An empty set or a start outside the extent has no match.
pub fn find_any(source: &[u8], needles: &[u8], start: u64) -> u64 {
    let Ok(start) = usize::try_from(start) else {
        return source.len() as u64;
    };
    let Some(rest) = source.get(start..) else {
        return source.len() as u64;
    };
    if needles.is_empty() {
        return source.len() as u64;
    }
    if rest.len() <= 8 {
        return rest
            .iter()
            .position(|byte| needles.contains(byte))
            .map_or(source.len(), |at| start + at) as u64;
    }
    if needles.contains(&rest[0]) {
        return start as u64;
    }
    let mut set = [false; 256];
    for &byte in needles {
        set[usize::from(byte)] = true;
    }
    rest.iter()
        .position(|&byte| set[usize::from(byte)])
        .map_or(source.len(), |at| start + at) as u64
}

#[cfg(test)]
mod tests {
    use super::find_any;
    #[test]
    fn all_bytes_offsets_and_extents() {
        let data: Vec<u8> = (0..=255).collect();
        for byte in 0..=255u8 {
            for start in [0, u64::from(byte), u64::from(byte) + 1, 256, u64::MAX] {
                let expected = if start <= u64::from(byte) {
                    u64::from(byte)
                } else {
                    256
                };
                assert_eq!(find_any(&data, &[byte, byte], start), expected);
            }
        }
        assert_eq!(find_any(&data[64..128], &[63, 64, 127, 128], 1), 63);
        assert_eq!(find_any(&data, &[], 0), 256);
        assert_eq!(find_any(&[], &[0], 0), 0);
        assert_eq!(find_any(b"hello\0world", b"\r\n\0", 0), 5);
    }
}
