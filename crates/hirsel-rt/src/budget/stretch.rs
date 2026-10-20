//! Exact budget summaries of non-head turns (design §6 Budgets, ordered mode).
//!
//! A turn that runs while it is not the head cannot check the meter live: earlier turns have
//! not settled. It records its charges as a [`Stretch`] instead, and retirement settles the
//! stretch against the settled level and remaining fuel ([`Settled::settle`]), stopping the
//! run with exactly the first stop and kind the serial (width-1) run has.
//!
//! - Credit events (logical deaths only a commit discovers: a deferred root release reaching
//!   zero, a close dropping leftovers) split a stretch into **pieces**.
//! - A piece keeps its fuel `f`, its net bytes `n`, and its **envelope**: the strict record
//!   highs `(u, h)` of its running level, `h` relative to the piece's start and `u` the piece
//!   fuel charged before that allocation. Entries with the same `u` keep only the largest `h`.
//!   A memory-only budget keeps only the largest `h`, a fuel-only budget only `f`.
//! - A piece keeps at most [`ENVELOPE_MAX`] entries and a stretch at most [`PIECES_MAX`]
//!   pieces; an event that would exceed either is refused with [`Pause`], and the segment
//!   pauses until its turn is head.
//!
//! Why settlement is exact: the serial run's first memory stop in a piece is at its first
//! allocation whose level passes the limit, which is a strict record high, and that
//! allocation is reached only if the `u` fuel charges before it succeed. Its first fuel stop
//! is the charge numbered `φ + 1`. So the piece stops for memory when its first entry with
//! `ℓ + h > limit` has `u ≤ φ`, otherwise for fuel when `f > φ`, and otherwise moves the level
//! by `n` and the fuel by `f`.

use hirsel_run_schema::BudgetKind;

/// The most envelope entries one piece keeps.
pub const ENVELOPE_MAX: usize = 1024;
/// The most pieces one stretch keeps.
pub const PIECES_MAX: usize = 64;

/// Which budgets a run has, so a summary keeps only what settlement reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Budgets {
    pub fuel: bool,
    pub memory: bool,
}

/// An event a stretch cannot summarise within its bounds: the segment pauses until head.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pause {
    Envelope,
    Pieces,
}

/// One piece: the charges between two credit events.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Piece {
    /// Fuel charged.
    pub fuel: u64,
    /// Net bytes: allocations less frees.
    pub net: i64,
    /// Strict record highs `(u, h)` of the running level, `h > 0` increasing.
    pub envelope: Vec<(u64, i64)>,
    /// The running level relative to the piece's start.
    level: i64,
}

/// A non-head turn's charges since its last retired or head point.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stretch {
    budgets: Budgets,
    /// Pieces and, before each but the first, the credit event that opened it.
    pieces: Vec<(u64, Piece)>,
}

impl Stretch {
    pub fn new(budgets: Budgets) -> Stretch {
        Stretch {
            budgets,
            pieces: vec![(0, Piece::default())],
        }
    }
    fn piece(&mut self) -> &mut Piece {
        &mut self.pieces.last_mut().expect("a stretch has a piece").1
    }
    /// One fuel charge.
    pub fn fuel(&mut self) {
        self.piece().fuel += 1;
    }
    /// An allocation of `bytes`.
    pub fn alloc(&mut self, bytes: u64) -> Result<(), Pause> {
        let memory = self.budgets.memory;
        let fuel = self.budgets.fuel;
        let piece = self.piece();
        let level = piece.level.saturating_add(bytes as i64);
        let high = piece.envelope.last().map_or(0, |&(_, h)| h);
        if memory && level > high {
            let u = if fuel { piece.fuel } else { 0 };
            let full = piece.envelope.len() == ENVELOPE_MAX;
            match piece.envelope.last_mut() {
                Some(last) if last.0 == u => *last = (u, level),
                _ if full => return Err(Pause::Envelope),
                _ => piece.envelope.push((u, level)),
            }
        }
        piece.level = level;
        piece.net = level;
        Ok(())
    }
    /// A free of `bytes` within the segment.
    pub fn free(&mut self, bytes: u64) {
        let piece = self.piece();
        piece.level = piece.level.saturating_sub(bytes as i64);
        piece.net = piece.level;
    }
    /// A credit event of `bytes`, computed when its op commits: it opens a new piece.
    pub fn credit(&mut self, bytes: u64) -> Result<(), Pause> {
        if self.pieces.len() == PIECES_MAX {
            return Err(Pause::Pieces);
        }
        self.pieces.push((bytes, Piece::default()));
        Ok(())
    }
    /// The pieces, each with the credit event before it (0 for the first).
    pub fn pieces(&self) -> impl Iterator<Item = (u64, &Piece)> {
        self.pieces.iter().map(|(credit, piece)| (*credit, piece))
    }
}

/// The run's settled budget state at the retirement frontier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settled {
    /// Settled logical bytes.
    pub level: u64,
    /// The memory budget, `u64::MAX` for none.
    pub limit: u64,
    /// Fuel left, `u64::MAX` for none.
    pub fuel: u64,
}

impl Settled {
    /// Settle `stretch` at its turn's retirement: the stop the serial run reaches inside it,
    /// or the state after it.
    pub fn settle(&mut self, stretch: &Stretch) -> Result<(), BudgetKind> {
        for (credit, piece) in stretch.pieces() {
            self.level = self.level.saturating_sub(credit);
            let room = self.limit.saturating_sub(self.level);
            // The first entry whose level passes the limit: h increases along the envelope.
            let first = piece.envelope.partition_point(|&(_, h)| h as u64 <= room);
            if let Some(&(u, _)) = piece.envelope.get(first)
                && u <= self.fuel
            {
                return Err(BudgetKind::Memory);
            }
            if piece.fuel > self.fuel {
                return Err(BudgetKind::Fuel);
            }
            if self.fuel != u64::MAX {
                self.fuel -= piece.fuel;
            }
            self.level = self.level.saturating_add_signed(piece.net);
        }
        Ok(())
    }
}

/// One event of a turn, in program order: the exact charge stream the summaries replace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Fuel,
    Alloc(u64),
    Free(u64),
    Credit(u64),
}

/// The serial run's meter over `events`: the reference settlement must equal.
pub fn reference(state: &mut Settled, events: &[Event]) -> Result<(), BudgetKind> {
    for &event in events {
        match event {
            Event::Fuel => {
                if state.fuel == 0 {
                    return Err(BudgetKind::Fuel);
                }
                if state.fuel != u64::MAX {
                    state.fuel -= 1;
                }
            }
            Event::Alloc(bytes) => {
                if state.level.saturating_add(bytes) > state.limit {
                    return Err(BudgetKind::Memory);
                }
                state.level += bytes;
            }
            Event::Free(bytes) | Event::Credit(bytes) => {
                state.level = state.level.saturating_sub(bytes);
            }
        }
    }
    Ok(())
}

/// The stretch summarising `events`, or the pause one of them causes.
pub fn summarise(budgets: Budgets, events: &[Event]) -> Result<Stretch, Pause> {
    let mut stretch = Stretch::new(budgets);
    for &event in events {
        match event {
            Event::Fuel => stretch.fuel(),
            Event::Alloc(bytes) => stretch.alloc(bytes)?,
            Event::Free(bytes) => stretch.free(bytes),
            Event::Credit(bytes) => stretch.credit(bytes)?,
        }
    }
    Ok(stretch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Event::*;

    const BOTH: Budgets = Budgets {
        fuel: true,
        memory: true,
    };

    fn settle(
        state: Settled,
        budgets: Budgets,
        events: &[Event],
    ) -> (Result<(), BudgetKind>, Settled) {
        let mut settled = state;
        let stretch = summarise(budgets, events).expect("within bounds");
        let result = settled.settle(&stretch);
        (result, settled)
    }

    /// Review-3 finding 3: settled 90, limit 100, one fuel point left.
    #[test]
    fn review_three_examples() {
        let state = Settled {
            level: 90,
            limit: 100,
            fuel: 1,
        };
        let memory_first = [Alloc(20), Fuel, Fuel, Free(20)];
        let stretch = summarise(BOTH, &memory_first).unwrap();
        assert_eq!(stretch.pieces().next().unwrap().1.envelope, [(0, 20)]);
        assert_eq!(
            settle(state, BOTH, &memory_first).0,
            Err(BudgetKind::Memory)
        );
        let fuel_first = [Fuel, Fuel, Alloc(20), Free(20)];
        let stretch = summarise(BOTH, &fuel_first).unwrap();
        assert_eq!(stretch.pieces().next().unwrap().1.envelope, [(2, 20)]);
        assert_eq!(settle(state, BOTH, &fuel_first).0, Err(BudgetKind::Fuel));
        // A close freeing 80 before a 20-byte allocation is a credit event: 90 - 80 + 20 = 30.
        let credited = [Credit(80), Alloc(20)];
        let (result, after) = settle(state, BOTH, &credited);
        assert_eq!(result, Ok(()));
        assert_eq!(after.level, 30);
        for events in [&memory_first[..], &fuel_first, &credited] {
            let mut reference_state = state;
            assert_eq!(
                reference(&mut reference_state, events),
                settle(state, BOTH, events).0
            );
        }
    }

    /// Review-2's counterexample: a younger summary whose record high 20 stops the run at its
    /// turn when 90 + 20 > 100, although its net is 0.
    #[test]
    fn record_high_stops_a_net_zero_turn() {
        let state = Settled {
            level: 90,
            limit: 100,
            fuel: u64::MAX,
        };
        let memory = Budgets {
            fuel: false,
            memory: true,
        };
        assert_eq!(
            settle(state, memory, &[Alloc(20), Free(20)]).0,
            Err(BudgetKind::Memory)
        );
    }

    fn next(seed: &mut u64) -> u64 {
        *seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = *seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Summaries settle to the exact reference over a generated corpus of turns: the same stop
    /// and kind, or the same level and fuel after the turn, for every budget combination.
    #[test]
    fn summaries_equal_the_reference_charge_stream() {
        let mut seed = 20261020;
        for case in 0..20_000 {
            let budgets = match case % 3 {
                0 => BOTH,
                1 => Budgets {
                    fuel: true,
                    memory: false,
                },
                _ => Budgets {
                    fuel: false,
                    memory: true,
                },
            };
            let limit = if budgets.memory {
                64 + next(&mut seed) % 256
            } else {
                u64::MAX
            };
            let level = if budgets.memory {
                next(&mut seed) % (limit + 1)
            } else {
                next(&mut seed) % 512
            };
            let fuel = if budgets.fuel {
                next(&mut seed) % 24
            } else {
                u64::MAX
            };
            let state = Settled { level, limit, fuel };
            let len = next(&mut seed) % 40;
            let mut held = level;
            let events: Vec<Event> = (0..len)
                .map(|_| match next(&mut seed) % 7 {
                    0 | 1 => Fuel,
                    2 | 3 => {
                        let bytes = 1 + next(&mut seed) % 48;
                        held += bytes;
                        Alloc(bytes)
                    }
                    4 | 5 => {
                        let bytes = next(&mut seed) % (held + 1);
                        held -= bytes;
                        Free(bytes)
                    }
                    _ => {
                        let bytes = next(&mut seed) % (held + 1);
                        held -= bytes;
                        Credit(bytes)
                    }
                })
                .collect();
            let mut expected = state;
            let expected_result = reference(&mut expected, &events);
            let (result, settled) = settle(state, budgets, &events);
            assert_eq!(result, expected_result, "case {case}: {state:?} {events:?}");
            if result.is_ok() {
                assert_eq!(settled, expected, "case {case}: {state:?} {events:?}");
            }
        }
    }

    /// A piece past its envelope bound and a stretch past its piece bound pause.
    #[test]
    fn envelope_and_piece_overflow_pause() {
        let mut events = Vec::new();
        for _ in 0..ENVELOPE_MAX {
            events.extend([Fuel, Alloc(1)]);
        }
        assert!(summarise(BOTH, &events).is_ok());
        events.extend([Fuel, Alloc(1)]);
        assert_eq!(summarise(BOTH, &events), Err(Pause::Envelope));
        // Without fuel the envelope keeps one entry, so it never overflows.
        let memory = Budgets {
            fuel: false,
            memory: true,
        };
        let stretch = summarise(memory, &events).unwrap();
        assert_eq!(stretch.pieces().next().unwrap().1.envelope.len(), 1);
        let credits = vec![Credit(0); PIECES_MAX];
        assert_eq!(summarise(BOTH, &credits), Err(Pause::Pieces));
        assert!(summarise(BOTH, &credits[1..]).is_ok());
    }
}
