//! Append-only tables that readers index without locks: rows live in chunks of
//! doubling size, so a published row never moves and growth never copies.
use std::sync::OnceLock;

/// The first chunk holds `1 << BASE_BITS` rows; chunk k holds `1 << (k + BASE_BITS)`.
const BASE_BITS: u32 = 6;
/// Enough chunks to address every u32 index.
const CHUNKS: usize = (u32::BITS - BASE_BITS + 1) as usize;

/// Rows indexed by dense u32 positions, each written once and then read freely.
pub struct AppendTable<T> {
    chunks: [OnceLock<Box<[OnceLock<T>]>>; CHUNKS],
}

impl<T> Default for AppendTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> AppendTable<T> {
    pub const fn new() -> Self {
        Self {
            chunks: [const { OnceLock::new() }; CHUNKS],
        }
    }

    /// The chunk and offset of `index`.
    fn place(index: u32) -> (usize, usize) {
        let shifted = u64::from(index) + (1 << BASE_BITS);
        let chunk = 63 - shifted.leading_zeros() - BASE_BITS;
        let offset = shifted - (1 << (chunk + BASE_BITS));
        (chunk as usize, offset as usize)
    }

    /// Publish `value` at `index`. Each index is written once, by its assigner.
    pub fn set(&self, index: u32, value: T) {
        let rows = self.chunk(index);
        let (_, offset) = Self::place(index);
        if rows[offset].set(value).is_err() {
            panic!("append-only table row {index} written twice");
        }
    }

    fn chunk(&self, index: u32) -> &[OnceLock<T>] {
        let (chunk, _) = Self::place(index);
        self.chunks[chunk].get_or_init(|| {
            (0..1usize << (chunk as u32 + BASE_BITS))
                .map(|_| OnceLock::new())
                .collect()
        })
    }

    /// Initialize a row on its first read. Concurrent readers share one value.
    pub fn get_or_init(&self, index: u32, init: impl FnOnce() -> T) -> &T {
        let (_, offset) = Self::place(index);
        self.chunk(index)[offset].get_or_init(init)
    }

    /// Bytes reserved for row slots, including slots not initialized yet.
    pub fn allocated_bytes(&self) -> usize {
        self.chunks
            .iter()
            .filter_map(OnceLock::get)
            .map(|chunk| std::mem::size_of_val(&**chunk))
            .sum()
    }

    /// The row at `index`, once it is published.
    pub fn get(&self, index: u32) -> Option<&T> {
        let (chunk, offset) = Self::place(index);
        self.chunks[chunk].get()?[offset].get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_cover_each_index_once_across_chunk_boundaries() {
        let mut expected = (0, 0);
        for index in 0..10_000u32 {
            assert_eq!(AppendTable::<()>::place(index), expected, "index {index}");
            expected.1 += 1;
            if expected.1 == 1 << (expected.0 as u32 + BASE_BITS) {
                expected = (expected.0 + 1, 0);
            }
        }
        assert_eq!(AppendTable::<()>::place(u32::MAX).0, CHUNKS - 1);
    }

    #[test]
    fn lazy_rows_initialize_once_under_concurrent_reads() {
        let table = AppendTable::new();
        assert_eq!(table.allocated_bytes(), 0);
        let calls = std::sync::atomic::AtomicU32::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    assert_eq!(
                        *table.get_or_init(192, || {
                            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            42
                        }),
                        42
                    );
                });
            }
        });
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(table.get(191), None);
        assert_eq!(table.get(193), None);
        assert_eq!(
            table.allocated_bytes(),
            256 * std::mem::size_of::<OnceLock<i32>>()
        );
    }

    #[test]
    fn rows_read_back_and_unwritten_rows_are_absent() {
        let table = AppendTable::new();
        for index in [0, 63, 64, 191, 192, 5000] {
            table.set(index, index * 2);
        }
        assert_eq!(table.get(64), Some(&128));
        assert_eq!(table.get(5000), Some(&10_000));
        assert_eq!(table.get(65), None);
        assert_eq!(table.get(1 << 20), None);
    }
}
