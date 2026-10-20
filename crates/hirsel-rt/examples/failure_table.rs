//! Counted load-only workload: decoding site text is deliberately never requested.
hirsel_guard::install!();

use hirsel_rt::isolate::FailureTable;
use std::hint::black_box;

fn main() {
    let count: u64 = std::env::args()
        .nth(1)
        .unwrap_or("10000".into())
        .parse()
        .unwrap();
    let rounds: usize = std::env::args()
        .nth(2)
        .unwrap_or("100".into())
        .parse()
        .unwrap();
    let mut bytes = Vec::new();
    bytes.extend(count.to_le_bytes());
    for code in 0..count {
        let definition = format!("benchmark@1/module.function_{code}");
        let file = "benchmark/module.hir";
        for word in [code, 42, 7, definition.len() as u64] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend(definition.as_bytes());
        bytes.extend((file.len() as u64).to_le_bytes());
        bytes.extend(file.as_bytes());
    }
    bytes.extend(0u64.to_le_bytes());
    for _ in 0..rounds {
        black_box(FailureTable::decode(black_box(&bytes)).unwrap());
    }
    println!("{count} {rounds}");
}
