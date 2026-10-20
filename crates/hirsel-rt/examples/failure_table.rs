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
    // The encoding FailureTable::new reads: counts, 32-byte site records, then the pool.
    let file = "benchmark/module.hir";
    let mut pool = file.as_bytes().to_vec();
    let mut bytes = Vec::new();
    bytes.extend((count as u32).to_le_bytes());
    bytes.extend(0u32.to_le_bytes());
    for code in 0..count {
        let definition = format!("benchmark@1/module.function_{code}");
        bytes.extend(code.to_le_bytes());
        let offset = pool.len() as u32;
        for word in [42, 7, offset, definition.len() as u32, 0, file.len() as u32] {
            bytes.extend(word.to_le_bytes());
        }
        pool.extend(definition.as_bytes());
    }
    bytes.extend(pool);
    let bytes: &'static [u8] = Vec::leak(bytes);
    for _ in 0..rounds {
        black_box(FailureTable::new(black_box(bytes)).unwrap());
    }
    println!("{count} {rounds}");
}
