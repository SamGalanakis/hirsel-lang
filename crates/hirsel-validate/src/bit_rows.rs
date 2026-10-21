//! Packed finite rows indexed by the flow model's interned slot paths.
//!
//! Borrowed from rustc_index/src/bit_set.rs:16-30, 70-94: dense u64 words,
//! with the same fixed domain and trailing-bit discipline. Unlike rustc's
//! flat bitset, words live in persistent radix rows: failure-edge snapshots
//! copy only the changed word's path, including in very wide constructors.
//! This container supplies no proofs or lattice operations; V supplies those.
use hirsel_intern::work::Counter;
use hirsel_types::rows::Rows;
use std::ops::Index;

pub(super) trait Value: Copy + Eq + 'static {
    const BITS: usize;
    fn bits(self) -> u8;
    fn value(bits: u8) -> &'static Self;
}
impl Value for bool {
    const BITS: usize = 1;
    fn bits(self) -> u8 {
        u8::from(self)
    }
    fn value(bits: u8) -> &'static Self {
        if bits == 0 { &false } else { &true }
    }
}
const BYTES: [u8; 256] = {
    let mut out = [0; 256];
    let mut i = 0;
    while i < 256 {
        out[i] = i as u8;
        i += 1;
    }
    out
};
impl Value for u8 {
    const BITS: usize = 8;
    fn bits(self) -> u8 {
        self
    }
    fn value(bits: u8) -> &'static Self {
        &BYTES[bits as usize]
    }
}
#[derive(Clone, Eq, PartialEq)]
pub(super) struct BitRows<T> {
    words: Rows<u64>,
    len: usize,
    default: T,
}
impl<T: Value> BitRows<T> {
    pub fn new(default: T, len: usize) -> Self {
        let mut word = 0;
        for shift in (0..64).step_by(T::BITS) {
            word |= (default.bits() as u64) << shift;
        }
        Self {
            words: Rows::new(word, len.div_ceil(64 / T::BITS)),
            len,
            default,
        }
    }
    pub fn with_copy_counter(mut self, counter: Counter) -> Self {
        self.words = self.words.with_copy_counter(counter);
        self
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn entries(&self) -> Vec<(usize, &T)> {
        let n = 64 / T::BITS;
        self.words
            .entries()
            .into_iter()
            .flat_map(|(word, _)| {
                (word * n..((word + 1) * n).min(self.len))
                    .filter(|&i| self[i] != self.default)
                    .map(|i| (i, &self[i]))
            })
            .collect()
    }
    pub fn set(&mut self, index: usize, value: T) {
        assert!(index < self.len);
        let n = 64 / T::BITS;
        let shift = (index % n) * T::BITS;
        let mask = ((1u64 << T::BITS) - 1) << shift;
        let word = (self.words[index / n] & !mask) | ((value.bits() as u64) << shift);
        self.words.set(index / n, word);
    }
    pub fn differences(&self, other: &Self) -> Vec<usize> {
        assert_eq!(self.len, other.len);
        let n = 64 / T::BITS;
        let mut out = Vec::new();
        for word in self.words.differences(&other.words) {
            let mut bits = self.words[word] ^ other.words[word];
            while bits != 0 {
                let row = bits.trailing_zeros() as usize / T::BITS;
                let index = word * n + row;
                if index < self.len {
                    out.push(index);
                }
                bits &= !(((1u64 << T::BITS) - 1) << (row * T::BITS));
            }
        }
        out
    }
    pub fn or_since(&mut self, base: &Self, other: &Self) {
        assert_eq!(self.len, base.len);
        assert_eq!(self.len, other.len);
        self.words
            .join_since(&base.words, &other.words, |a, b| *a |= b);
    }
    pub fn and_since(&mut self, base: &Self, other: &Self) {
        assert_eq!(self.len, base.len);
        assert_eq!(self.len, other.len);
        self.words
            .join_since(&base.words, &other.words, |a, b| *a &= b);
    }
}
impl<T: Value> Index<usize> for BitRows<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        assert!(index < self.len);
        let n = 64 / T::BITS;
        let bits = (self.words[index / n] >> ((index % n) * T::BITS)) & ((1u64 << T::BITS) - 1);
        T::value(bits as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_and_delta_joins_match_scalar_rows_across_word_boundaries() {
        for len in [0, 1, 7, 8, 9, 63, 64, 65, 513, 4097] {
            let base = BitRows::new(1u8, len);
            let mut a = base.clone();
            let mut b = base.clone();
            for i in (0..len).step_by(3) {
                a.set(i, 3);
            }
            for i in (0..len).step_by(5) {
                b.set(i, 5);
            }
            let saved = a.clone();
            a.or_since(&base, &b);
            for i in 0..len {
                assert_eq!(base[i], 1);
                assert_eq!(saved[i], if i % 3 == 0 { 3 } else { 1 });
                assert_eq!(a[i], if i % 5 == 0 { saved[i] | 4 } else { saved[i] });
            }
            let full = BitRows::new(255u8, len);
            let mut intersection = full.clone();
            let mut other = full.clone();
            for i in (0..len).step_by(3) {
                intersection.set(i, 253);
            }
            for i in (0..len).step_by(5) {
                other.set(i, 251);
            }
            intersection.and_since(&full, &other);
            for i in 0..len {
                assert_eq!(
                    intersection[i],
                    (if i % 3 == 0 { 253 } else { 255 }) & (if i % 5 == 0 { 251 } else { 255 })
                );
            }
            let mut bits = BitRows::new(false, len);
            for i in (0..len).step_by(7) {
                bits.set(i, true);
            }
            assert_eq!(
                bits.differences(&BitRows::new(false, len)),
                (0..len).step_by(7).collect::<Vec<_>>()
            );
        }
    }
}
