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
