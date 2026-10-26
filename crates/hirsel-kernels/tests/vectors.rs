hirsel_guard::install!();

use hirsel_kernels::{
    integer::{self, Format, Width},
    siphash,
};
#[test]
fn conversions_and_masked_shifts_against_mathematical_small_integer_oracles() {
    let widths = [Width::W8, Width::W16, Width::W32, Width::W64, Width::W128];
    for from_width in widths {
        for signed in [false, true] {
            let from = Format {
                width: from_width,
                signed,
            };
            for value in -257i128..=257 {
                if !signed && value < 0 {
                    continue;
                }
                // Input representability is a mathematical comparison, independent of exact().
                let fits_from = if signed {
                    value >= (i128::MIN >> (128 - from_width.bits()))
                        && value <= (i128::MAX >> (128 - from_width.bits()))
                } else {
                    value as u128 <= from_width.mask()
                };
                if !fits_from {
                    continue;
                }
                let bits = value as u128 & from_width.mask();
                for to_width in widths {
                    for to_signed in [false, true] {
                        let to = Format {
                            width: to_width,
                            signed: to_signed,
                        };
                        let fits = if to_signed {
                            value >= (i128::MIN >> (128 - to_width.bits()))
                                && value <= (i128::MAX >> (128 - to_width.bits()))
                        } else {
                            value >= 0 && value as u128 <= to_width.mask()
                        };
                        assert_eq!(
                            integer::exact(bits, from, to),
                            fits.then_some(value as u128 & to_width.mask())
                        );
                        assert_eq!(
                            integer::truncate(bits, from, to),
                            value as u128 & to_width.mask()
                        );
                    }
                }
                for count in [0, 1, 7, 8, 31, 63, 64, 127, 128, u128::MAX] {
                    let n = (count % u128::from(from_width.bits())) as u32;
                    let right = if signed {
                        (value >> n) as u128
                    } else {
                        bits >> n
                    };
                    assert_eq!(
                        integer::shift(bits, from, count, false),
                        right & from_width.mask()
                    );
                    assert_eq!(
                        integer::shift(bits, from, count, true),
                        (bits << n) & from_width.mask()
                    );
                }
            }
        }
    }
    assert_eq!(
        integer::exact(
            u128::MAX,
            Format {
                width: Width::W128,
                signed: false
            },
            Format {
                width: Width::W128,
                signed: true
            }
        ),
        None
    );
}
#[test]
fn siphash13_independent_standard_library_oracle() {
    // DefaultHasher is the independently implemented SipHash-1-3 with zero keys.
    // Hasher::write feeds raw bytes; Hash would add type framing and is intentionally unused.
    use std::hash::Hasher;
    for len in 0..=256 {
        let data: Vec<_> = (0..len).map(|i| i as u8).collect();
        let mut oracle = std::collections::hash_map::DefaultHasher::new();
        oracle.write(&data);
        assert_eq!(siphash::siphash13(&data, 0, 0), oracle.finish());
    }
}

/// Choices from an independent SplitMix64 transcription (Python), not from the kernel.
#[test]
fn scheduler_policies_follow_their_seeded_words() {
    use hirsel_kernels::schedule::{Algorithm, Schedule};
    let mut fifo = Schedule::new(Algorithm::Fifo, 7);
    assert_eq!(
        (fifo.admit(), fifo.point(), fifo.choose(&[0, 0, 0])),
        (0, None, 0)
    );
    // random@1 seed 7: words mod 2, 3, 4, 5, 2, 3; a lone candidate draws nothing.
    let mut random = Schedule::new(Algorithm::Random, 7);
    let mut chosen = Vec::new();
    for n in [2, 3, 1, 4, 5, 2, 1, 3] {
        assert_eq!(random.point(), None);
        chosen.push(random.choose(&vec![0; n]));
    }
    assert_eq!(chosen, [1, 0, 0, 2, 3, 0, 0, 0]);
    // pct@1 seed 5: depth 3, horizon 64, change points 8 (to priority 1) and 6 (to 2).
    let mut pct = Schedule::new(Algorithm::Pct, 5);
    let priorities = [pct.admit(), pct.admit(), pct.admit()];
    assert_eq!(
        priorities,
        [
            1733626130553941733,
            3510497739974877221,
            9090219046513020307
        ]
    );
    let points: Vec<_> = (0..10).map(|_| pct.point()).collect();
    let mut expected = [None; 10];
    expected[5] = Some(2);
    expected[7] = Some(1);
    assert_eq!(points, expected);
    assert_eq!(
        pct.choose(&[priorities[0], priorities[2], priorities[1]]),
        1
    );
    assert_eq!(pct.choose(&[5, 5, 1]), 0, "ties keep queue order");
    for _ in 0..60 {
        pct.point();
    }
    assert_eq!(pct.choose(&[1, 2, 3]), 0, "past the horizon the front runs");
    assert!(!fifo.recorded(3) && !random.recorded(1) && random.recorded(2));
    for algorithm in [Algorithm::Fifo, Algorithm::Random, Algorithm::Pct] {
        assert_eq!(Algorithm::from_code(algorithm.code()), Some(algorithm));
    }
    assert_eq!(Algorithm::from_code(3), None);
}

#[test]
fn siphash13_streaming_matches_independent_oracle_at_every_chunk_boundary() {
    use std::hash::Hasher;
    for len in 0..=256 {
        let data: Vec<_> = (0..len).map(|i| i as u8).collect();
        let mut oracle = std::collections::hash_map::DefaultHasher::new();
        oracle.write(&data);
        let expected = oracle.finish();
        for split in 0..=len {
            let mut stream = siphash::SipHasher13::new(0, 0);
            stream.write(&data[..split]);
            stream.write(&[]);
            stream.write(&data[split..]);
            assert_eq!(stream.finish(), expected, "len={len} split={split}");
        }
        let mut stream = siphash::SipHasher13::new(0, 0);
        for byte in &data {
            stream.write(std::slice::from_ref(byte));
        }
        assert_eq!(stream.finish(), expected);
    }
    // Nonzero seeded states obey the same framing law as one-shot hashing.
    for seed in [1, 7, u64::MAX] {
        let data = [0xa5; 79];
        let expected = siphash::siphash13(&data, seed, 0x6869_7273_656c_2d37);
        for width in 1..=17 {
            let mut stream = siphash::SipHasher13::new(seed, 0x6869_7273_656c_2d37);
            for bytes in data.chunks(width) {
                stream.write(bytes);
            }
            assert_eq!(stream.finish(), expected);
        }
    }
}

#[test]
fn siphash13_partial_finishes_preserve_unaligned_tail_bytes() {
    use std::hash::Hasher;
    let data: Vec<u8> = (0..=255).map(|i: u8| i.wrapping_mul(173) ^ 0xa5).collect();
    for offset in 0..8 {
        for width in 1..=17 {
            let mut stream = siphash::SipHasher13::new(0, 0);
            let mut oracle = std::collections::hash_map::DefaultHasher::new();
            for bytes in data[offset..].chunks(width) {
                stream.write(bytes);
                oracle.write(bytes);
                assert_eq!(
                    stream.finish(),
                    oracle.finish(),
                    "offset={offset} width={width}"
                );
                stream.write(&[]);
                assert_eq!(stream.finish(), oracle.finish());
            }
        }
    }
}
