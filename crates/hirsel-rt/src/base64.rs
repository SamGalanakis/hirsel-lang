//! Safe, pinned Base64 acceleration. The source parser remains the error oracle.
//!
//! base64-simd 0.8.0 src/lib.rs:234-277 exposes the safe destination-slice API;
//! src/decode.rs:30-75,148-170 checks lengths, padding and canonical bits.
//! vsimd 0.8.0 src/macros.rs:96-133 resolves the ISA lazily and caches it.

pub(crate) fn engine(url_safe: bool, padded: bool) -> base64_simd::Base64 {
    match (url_safe, padded) {
        (false, true) => base64_simd::STANDARD,
        (false, false) => base64_simd::STANDARD_NO_PAD,
        (true, true) => base64_simd::URL_SAFE,
        (true, false) => base64_simd::URL_SAFE_NO_PAD,
    }
}

#[cfg(test)]
fn encode(data: &[u8], url_safe: bool, padded: bool) -> Vec<u8> {
    engine(url_safe, padded).encode_to_string(data).into_bytes()
}

/// A rejected attempt exposes no partial output. The caller must run the source
/// parser from the beginning to obtain Hirsel's exact error kind and byte offset.
#[cfg(test)]
fn decode(data: &[u8], url_safe: bool, padded: bool) -> Option<Vec<u8>> {
    engine(url_safe, padded).decode_to_vec(data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn oracle(url_safe: bool, padded: bool) -> base64::engine::GeneralPurpose {
        use base64::engine::general_purpose::*;
        match (url_safe, padded) {
            (false, true) => STANDARD,
            (false, false) => STANDARD_NO_PAD,
            (true, true) => URL_SAFE,
            (true, false) => URL_SAFE_NO_PAD,
        }
    }

    // Adapted from base64-simd 0.8.0 tests/it.rs round trips and comparison
    // with base64's independent scalar engine; all four strict modes and
    // adversarial positions/tails are Hirsel-specific additions.
    #[test]
    fn independent_scalar_lengths_and_mutations() {
        let mut seed = 2026102330u32;
        for url_safe in [false, true] {
            for padded in [false, true] {
                let scalar = oracle(url_safe, padded);
                for len in 0..=1024 {
                    let raw: Vec<u8> = (0..len)
                        .map(|_| {
                            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                            (seed >> 24) as u8
                        })
                        .collect();
                    let encoded = encode(&raw, url_safe, padded);
                    assert_eq!(encoded, scalar.encode(&raw).as_bytes());
                    assert_eq!(decode(&encoded, url_safe, padded), Some(raw));
                    for at in [0, len / 2, encoded.len().saturating_sub(1)] {
                        if at >= encoded.len() {
                            continue;
                        }
                        for byte in [0, 10, 13, 32, b'=', b'+', b'/', b'-', b'_', 255] {
                            let mut mutated = encoded.clone();
                            mutated[at] = byte;
                            assert_eq!(
                                decode(&mutated, url_safe, padded),
                                scalar.decode(&mutated).ok(),
                                "url={url_safe} pad={padded} len={len} at={at} byte={byte}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn exhaustive_canonical_tails_and_padding() {
        for url_safe in [false, true] {
            for padded in [false, true] {
                let scalar = oracle(url_safe, padded);
                for a in 0..=255u8 {
                    for b in 0..=255u8 {
                        for input in [
                            &[a, b][..],
                            &[a, b, b'=', b'='][..],
                            &[b'A', a, b][..],
                            &[b'A', a, b, b'='][..],
                        ] {
                            assert_eq!(
                                decode(input, url_safe, padded),
                                scalar.decode(input).ok(),
                                "url={url_safe} pad={padded} input={input:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
