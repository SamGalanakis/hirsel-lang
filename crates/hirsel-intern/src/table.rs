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
        let (chunk, offset) = Self::place(index);
        let rows = self.chunks[chunk].get_or_init(|| {
            (0..1usize << (chunk as u32 + BASE_BITS))
                .map(|_| OnceLock::new())
                .collect()
        });
        if rows[offset].set(value).is_err() {
            panic!("append-only table row {index} written twice");
        }
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
