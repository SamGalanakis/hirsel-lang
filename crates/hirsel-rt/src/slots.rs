//! Per-slot generations that never wrap (r6 S1), shared by the task registry
//! and the channel endpoint table. A stale key can never name a later
//! occupant: a slot whose generation is exhausted is never reused. Kept free
//! of runtime dependencies beyond `fault`, so the loom model of the channel
//! table compiles it from this source.

/// Per-slot generations over one row table. A released slot is reused with
/// its next generation; a slot at `LIMIT` is never reused.
#[derive(Debug, Default)]
pub(crate) struct Slots<const LIMIT: u32> {
    free: Vec<(u32, u32)>,
}

impl<const LIMIT: u32> Slots<LIMIT> {
    /// A slot and its generation; a fresh slot is `len`, the table's length.
    #[inline]
    pub(crate) fn take(&mut self, len: usize) -> (u32, u32) {
        match self.free.pop() {
            Some((slot, generation)) => (slot, generation + 1),
            None => (
                u32::try_from(len).unwrap_or_else(|_| crate::fault("a run's row table is full")),
                1,
            ),
        }
    }

    /// Capacity for `additional` releases that must not allocate.
    pub(crate) fn reserve(&mut self, additional: usize) {
        self.free.reserve(additional);
    }

    /// `slot`'s row, of `generation`, left its table.
    #[inline]
    pub(crate) fn release(&mut self, slot: u32, generation: u32) {
        if generation < LIMIT {
            self.free.push((slot, generation));
        }
    }

    #[cfg(test)]
    pub(crate) fn free_len(&self) -> usize {
        self.free.len()
    }

    /// Bounded exhaustion setup: every vacant slot last held `generation`.
    #[cfg(test)]
    pub(crate) fn age(&mut self, generation: u32) {
        assert!(generation < LIMIT, "an aged slot is still reusable");
        for (_, last) in &mut self.free {
            *last = generation;
        }
    }
}
