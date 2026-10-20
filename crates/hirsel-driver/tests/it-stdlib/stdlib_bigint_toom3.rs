//! Exercise the large-input mac3 branches against an independent base-2^64
//! schoolbook oracle. num-bigint tests/biguint.rs:845-869 supplies the product
//! table already covered by SEM-1-bigint-tables-sum-run.
use hirsel_engine::with_compiler_stack;

fn product(a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut out = vec![0; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        let mut carry = 0u128;
        for (j, &y) in b.iter().enumerate() {
            let word = u128::from(x) * u128::from(y) + u128::from(out[i + j]) + carry;
            out[i + j] = word as u64;
            carry = word >> 64;
        }
        out[i + b.len()] = carry as u64;
    }
    out
}

fn hex(words: &[u64]) -> String {
    words
        .iter()
        .rev()
        .map(|word| format!("{word:016x}"))
        .collect()
}

fn cases(sizes: &[(usize, usize)]) {
    with_compiler_stack(|| {
        let mut seed = 0x746f6f6d335f6d75u64;
        for &(na, nb) in sizes {
            // Dense random words, every carry, a zero middle third (including
            // negative evaluation products), and isolated high/low terms.
            for pattern in 0..4 {
                let mut words = |len: usize| -> Vec<u64> {
                    (0..len)
                        .map(|i| {
                            seed ^= seed << 13;
                            seed ^= seed >> 7;
                            seed ^= seed << 17;
                            match pattern {
                                0 => seed | (u64::from(i == len - 1) << 63),
                                1 => u64::MAX,
                                2 if i >= len / 3 && i < 2 * len / 3 => 0,
                                2 => seed | 1,
                                _ => u64::from(i == 0 || i == len - 1),
                            }
                        })
                        .collect()
                };
                let a = words(na);
                let mut b = words(nb);
                if pattern == 2 {
                    // Make w(-1) negative for one factor and positive for the
                    // other: zero outer thirds, dense middle, nonzero top word.
                    b.fill(0);
                    b[nb / 3..2 * nb / 3].fill(u64::MAX);
                    b[nb - 1] = 1;
                }
                let args = [hex(&a), hex(&b), hex(&product(&a, &b))];
                super::numeric_native::check(&args, &format!("{na} x {nb}, pattern {pattern}"));
            }
        }
    });
}

#[test]
fn toom3_crossover_and_unequal_splits() {
    cases(&[
        (127, 127),
        (128, 128),
        (129, 129),
        (130, 131),
        (129, 192),
        (129, 257),
        (255, 255),
        (256, 256),
        (257, 257),
        (258, 259),
        (257, 384),
        (257, 513),
    ]);
}
#[test]
fn toom3_chunking_and_recursive_products() {
    cases(&[
        (129, 258),
        (129, 391),
        (257, 514),
        (257, 779),
        (512, 512),
        (1024, 1025),
    ]);
}
