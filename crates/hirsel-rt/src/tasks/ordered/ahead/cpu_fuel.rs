//! Owner-local fuel between lifecycle entries. This only accumulates compiled
//! points; the real Stretch and Jobs still decide the first language stop.
#[derive(Clone, Default)]
pub(crate) struct Fuel {
    issued: Option<u64>,
    pending: u64,
    room: u64,
}

impl Fuel {
    pub(crate) fn grant(&mut self, points: u64, room: u64) {
        assert_eq!(self.pending, 0, "settle private points before a new grant");
        self.issued = Some(points);
        self.room = room;
    }

    fn consumed(&self, word: i64) -> Option<u64> {
        Some(
            self.pending.saturating_add(
                self.issued?
                    .saturating_sub(word.max(0) as u64)
                    .saturating_add(u64::from(word < 0)),
            ),
        )
    }

    /// Keep a bounded prefix private. Reaching the snapshot's remaining room
    /// returns to the real head gate, which alone may settle or publish a stop.
    pub(crate) fn refill(&mut self, word: i64, chunk: u64) -> Option<u64> {
        let consumed = self.consumed(word)?;
        if consumed >= self.room {
            return None;
        }
        self.pending = consumed;
        let grant = chunk.min(self.room - consumed);
        self.issued = Some(grant);
        Some(grant)
    }

    /// Transfer once into the originating task's existing Stretch. Updating
    /// issued also makes repeated partial captures charge only new points.
    pub(crate) fn capture(&mut self, word: i64) -> Option<u64> {
        let consumed = self.consumed(word)?;
        self.pending = 0;
        self.issued = Some(word.max(0) as u64);
        self.room = self.room.saturating_sub(consumed);
        Some(consumed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_private_chunks_transfer_exactly_once_at_partial_and_final_saves() {
        for room in [1, 7, 65536, 65537, 131080, u64::MAX] {
            let mut fuel = Fuel::default();
            let first = room.min(65536);
            fuel.grant(first, room);
            let word = -1;
            let mut total = first + 1;
            if let Some(next) = fuel.refill(word, 65536) {
                assert_eq!(fuel.capture((next - 1) as i64), Some(total + 1));
                total += 1;
                assert_eq!(fuel.capture((next - 1) as i64), Some(0));
                assert_eq!(fuel.capture((next - 2) as i64), Some(1));
                total += 1;
            } else {
                assert_eq!(fuel.capture(word), Some(total));
            }
            assert!(total >= first + 1);
            let remaining = fuel.issued.unwrap();
            assert_eq!(fuel.capture(0), Some(remaining));
        }
    }

    #[test]
    fn private_snapshot_exhaustion_returns_to_real_settlement_before_another_grant() {
        let mut fuel = Fuel::default();
        assert_eq!(fuel.refill(-1, 4), None);
        fuel.grant(4, 9);
        assert_eq!(fuel.refill(-1, 4), Some(4));
        assert_eq!(fuel.refill(-1, 4), None);
        assert_eq!(fuel.capture(-1), Some(10));
        fuel.grant(2, 2);
        assert_eq!(fuel.capture(0), Some(2));
    }
}
