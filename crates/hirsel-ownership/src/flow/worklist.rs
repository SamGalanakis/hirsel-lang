//! The minimum dirty reverse-postorder rank, as in rustc 19833af7
//! compiler/rustc_mir_dataflow/src/framework/mod.rs:290-377. Insertion
//! revisits an earlier rank immediately; duplicate insertions have no effect.
pub(super) struct Pending {
    words: Vec<u64>,
    next: usize,
}
impl Pending {
    pub(super) fn new(count: usize) -> Self {
        Self {
            words: vec![0; count.div_ceil(64)],
            next: count,
        }
    }
    pub(super) fn insert(&mut self, rank: usize) {
        self.words[rank / 64] |= 1 << (rank % 64);
        self.next = self.next.min(rank);
    }
    pub(super) fn pop_first(&mut self) -> Option<usize> {
        while let Some(word) = self.words.get_mut(self.next / 64) {
            if *word != 0 {
                let rank = self.next / 64 * 64 + word.trailing_zeros() as usize;
                *word &= *word - 1;
                self.next = rank;
                return Some(rank);
            }
            self.next = (self.next / 64 + 1) * 64;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn dirty_ranks_match_an_ordered_set_across_feedback_and_word_boundaries() {
        for count in [0, 1, 63, 64, 65, 129, 257] {
            let mut pending = Pending::new(count);
            let mut reference = BTreeSet::new();
            assert_eq!(pending.pop_first(), None);
            let mut random = 93u64;
            for step in 0..4096 {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                if count != 0 && step % 3 != 0 {
                    let rank = random as usize % count;
                    pending.insert(rank);
                    reference.insert(rank);
                } else {
                    assert_eq!(pending.pop_first(), reference.pop_first());
                }
            }
            while let Some(rank) = reference.pop_first() {
                assert_eq!(pending.pop_first(), Some(rank));
            }
            assert_eq!(pending.pop_first(), None);
        }
    }
}
