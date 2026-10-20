//! Neal's complete term tables and CPython's fixed testFsum vectors.
use super::reference_vectors::{self as vectors, Case};
use serde_json::Value;

const XSUM: &str = include_str!("../../../../native-tests/library/vectors/xsum.json");
const CPYTHON: &str = include_str!("../../../../native-tests/library/vectors/cpython-fsum.json");
const F32: &str = include_str!("../../../../native-tests/library/vectors/exact-sums-f32.json");

fn cases(json: &str, family: &str, count: usize, variants: bool) -> Vec<Case> {
    let rows: Vec<Value> = serde_json::from_str(json).unwrap();
    assert_eq!(rows.len(), count);
    let uint = if family == "f64" { "u64" } else { "u32" };
    let sign = if family == "f64" {
        1u64 << 63
    } else {
        1u64 << 31
    };
    let mut cases = Vec::new();
    for row in rows {
        let words: Vec<_> = row["input"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| u64::from_str_radix(w.as_str().unwrap(), 16).unwrap())
            .collect();
        let expected = u64::from_str_radix(row["expected"].as_str().unwrap(), 16).unwrap();
        // xsum-check.c:638,717,824,870 also negate each table's inputs.
        // Every table is exercised forwards and backwards to expose order bugs.
        for negative in [false, true].into_iter().take(if variants { 2 } else { 1 }) {
            for reverse in [false, true] {
                let mut words = words.clone();
                let mut expected = expected;
                if negative {
                    for word in &mut words {
                        *word ^= sign;
                    }
                    // Canonical NaN and cancellation zero have no negative sign.
                    let magnitude = expected & (sign - 1);
                    let inf = if family == "f64" {
                        0x7ff0000000000000
                    } else {
                        0x7f800000
                    };
                    expected = if magnitude > inf {
                        expected
                    } else if magnitude == 0 {
                        if !words.is_empty() && words.iter().all(|&w| w == sign) {
                            sign
                        } else {
                            0
                        }
                    } else {
                        expected ^ sign
                    };
                }
                if reverse {
                    words.reverse();
                }
                let items = words
                    .iter()
                    .map(|word| format!("core@1/{family}.from_bits(0x{word:x}{uint})"))
                    .collect::<Vec<_>>()
                    .join(", ");
                cases.push(Case {
                    id: format!("{} negative={negative} reverse={reverse}", row["id"].as_str().unwrap()),
                    body: format!(
                        "let values: Array[{family}] = [{items}]\ncore@1/test.check(t, core@1/{family}.bits(std@1/{family}.sum_exact(values)) == 0x{expected:x}{uint})"
                    ),
                });
            }
        }
    }
    cases
}

fn xsum(shard: usize) {
    let rows: Vec<Value> = serde_json::from_str(XSUM).unwrap();
    for (table, count) in [
        ("one_term", 41),
        ("two_term", 169),
        ("three_term", 18),
        ("ten_term", 14),
    ] {
        assert_eq!(
            rows.iter()
                .filter(|r| r["id"]
                    .as_str()
                    .unwrap()
                    .starts_with(&format!("xsum/{table}/")))
                .count(),
            count
        );
    }
    vectors::run(
        "xsum term tables",
        &cases(XSUM, "f64", 242, true),
        shard,
        8,
        16,
        "",
    );
}

macro_rules! shards {
    ($run:ident; $($name:ident: $shard:literal),+ $(,)?) => {
        $(#[test] fn $name() { $run($shard); })+
    };
}
shards!(xsum; xsum_0: 0, xsum_1: 1, xsum_2: 2, xsum_3: 3,
    xsum_4: 4, xsum_5: 5, xsum_6: 6, xsum_7: 7);

fn binary32(shard: usize) {
    vectors::run(
        "binary32 exact sums",
        &cases(F32, "f32", 498, true),
        shard,
        8,
        16,
        "",
    );
}
shards!(binary32; binary32_0: 0, binary32_1: 1, binary32_2: 2, binary32_3: 3,
    binary32_4: 4, binary32_5: 5, binary32_6: 6, binary32_7: 7);

#[test]
fn cpython_test_fsum_all_fixed_vectors() {
    // Includes both 1000-term series, the 1024-term partials-resize vector
    // and the appended 1001-term telescoping case. No step budget or truncation.
    vectors::run(
        "CPython testFsum",
        &cases(CPYTHON, "f64", 14, false),
        0,
        1,
        1,
        "",
    );
}

#[test]
fn deferred_carry_boundaries_and_cancellation() {
    let mut cases = Vec::new();
    for count in [2046, 2047, 2048, 4095, 8193] {
        for shift in [0u32, 1, 31, 32, 33, 2044, 2045] {
            // Largest possible high addend and both signs exercise carry slack.
            // A lone least-subnormal term must survive complete cancellation.
            let exact_integer = (((1u128 << 53) - 1) * count << shift.min(33)) + 1;
            let expected = if shift > 33 {
                f64::INFINITY.to_bits()
            } else {
                ((exact_integer as f64) * f64::from_bits(1)).to_bits()
            };
            cases.push(Case {
                id: format!("deferred carry count={count} shift={shift}"),
                body: format!(
                    "let mut a = acc_zero()\nacc_add(mut a, 1u64, 0u64, false)\nlet mut i: u64 = 0\nwhile i < {count}u64 {{ acc_add(mut a, 9007199254740991u64, {shift}u64, false); i += 1 }}\ncore@1/test.check(t, acc_finish(a, 52u64, 2047u64, 9223372036854775808u64, false) == {expected}u64)\ni = 0\nwhile i < {count}u64 {{ acc_add(mut a, 9007199254740991u64, {shift}u64, true); i += 1 }}\ncore@1/test.check(t, acc_finish(a, 52u64, 2047u64, 9223372036854775808u64, false) == 1u64)"
                ),
            });
        }
    }
    vectors::run(
        "xsum deferred carries",
        &cases,
        0,
        1,
        4,
        include_str!("../../../../stdlib/std@1/superacc.hir"),
    );
}
