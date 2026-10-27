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
    pub(super) level: i64,
}

/// A non-head turn's charges since its last retired or head point.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stretch {
    budgets: Budgets,
    /// Pieces and, before each but the first, the credit event that opened it.
    pub(super) pieces: Vec<(u64, Piece)>,
    // Filled only by an explicitly enabled runtime witness (or protocol tests).
    pub(crate) exact: Vec<super::stretch_trace::Charge>,
}

impl Stretch {
    pub fn new(budgets: Budgets) -> Stretch {
        Stretch {
            budgets,
            pieces: vec![(0, Piece::default())],
            exact: Vec::new(),
        }
    }
    pub(super) fn piece(&mut self) -> &mut Piece {
        &mut self.pieces.last_mut().expect("a stretch has a piece").1
    }
    /// One fuel charge.
    pub fn fuel(&mut self) {
        self.fuel_points(1);
    }
    /// Owner-local compiled chunks report their consumed points in one batch.
    pub fn fuel_points(&mut self, points: u64) {
        #[cfg(test)]
        if points != 0 {
            self.exact.push(super::stretch_trace::Charge::Fuel(points));
        }
        let piece = self.piece();
        piece.fuel = piece.fuel.saturating_add(points);
    }
    pub fn fuel_charges(&self) -> u64 {
        self.pieces
            .iter()
            .fold(0, |total, (_, piece)| total.saturating_add(piece.fuel))
    }
    /// Whether appending this allocation needs an envelope pause. Inspecting
    /// the last piece avoids cloning a growing summary at every allocation.
    pub fn allocation_pauses(&self, bytes: u64) -> bool {
        let piece = &self.pieces.last().expect("a stretch has a piece").1;
        let level = piece.level.saturating_add(bytes as i64);
        let high = piece.envelope.last().map_or(0, |&(_, high)| high);
        let fuel = if self.budgets.fuel { piece.fuel } else { 0 };
        self.budgets.memory
            && level > high
            && piece.envelope.len() == ENVELOPE_MAX
            && piece.envelope.last().is_none_or(|&(u, _)| u != fuel)
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
        #[cfg(test)]
        self.exact.push(super::stretch_trace::Charge::Alloc(bytes));
        Ok(())
    }
    /// A free of `bytes` within the segment.
    pub fn free(&mut self, bytes: u64) {
        let piece = self.piece();
        piece.level = piece.level.saturating_sub(bytes as i64);
        piece.net = piece.level;
        #[cfg(test)]
        self.exact.push(super::stretch_trace::Charge::Free(bytes));
    }
    /// A credit event of `bytes`, computed when its op commits: it opens a new piece.
    pub fn credit(&mut self, bytes: u64) -> Result<(), Pause> {
        if self.pieces.len() == PIECES_MAX {
            return Err(Pause::Pieces);
        }
        self.pieces.push((bytes, Piece::default()));
        #[cfg(test)]
        self.exact.push(super::stretch_trace::Charge::Credit(bytes));
        Ok(())
    }
    /// The pieces, each with the credit event before it (0 for the first).
    pub fn pieces(&self) -> impl Iterator<Item = (u64, &Piece)> {
        self.pieces.iter().map(|(credit, piece)| (*credit, piece))
    }

    /// Join two consecutive charge prefixes without introducing a credit at
    /// their physical settlement boundary. Failure leaves the receiver intact.
    pub(crate) fn append(&mut self, next: &Self) -> Result<(), Pause> {
        assert_eq!(self.budgets, next.budgets);
        if self.pieces.len() + next.pieces.len() - 1 > PIECES_MAX {
            return Err(Pause::Pieces);
        }
        let mut joined = self.clone();
        let left = joined.piece();
        let right = &next.pieces[0].1;
        for &(fuel, high) in &right.envelope {
            let fuel = if self.budgets.fuel {
                left.fuel.saturating_add(fuel)
            } else {
                0
            };
            let high = left.level.saturating_add(high);
            if high > left.envelope.last().map_or(0, |&(_, h)| h) {
                let full = left.envelope.len() == ENVELOPE_MAX;
                match left.envelope.last_mut() {
                    Some(last) if last.0 == fuel => *last = (fuel, high),
                    _ if full => return Err(Pause::Envelope),
                    _ => left.envelope.push((fuel, high)),
                }
            }
        }
        left.fuel = left.fuel.saturating_add(right.fuel);
        left.level = left.level.saturating_add(right.net);
        left.net = left.level;
        joined.pieces.extend_from_slice(&next.pieces[1..]);
        #[cfg(test)]
        joined.exact.extend_from_slice(&next.exact);
        *self = joined;
        Ok(())
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

/// The first stopped job, indexed in the batch's serial order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JobStop {
    pub job: usize,
    pub kind: BudgetKind,
}

/// Completed job summaries awaiting serial retirement. Publication order is
/// physical only; neither publication nor a missing older job settles charges.
/// Replay publishes the recorded summaries through this same interface.
#[derive(Debug)]
pub struct Jobs {
    pending: Vec<Option<(Stretch, bool)>>,
    next: usize,
    settled: Settled,
    stopped: Option<JobStop>,
}

impl Jobs {
    /// One bounded CPU batch (at most twice its admitted width).
    pub fn new(width: u32, count: usize, settled: Settled) -> Self {
        assert!(matches!(width, 1 | 4 | 8 | 16), "admitted CPU width");
        assert!(count <= width as usize * 2, "bounded CPU batch");
        Self {
            pending: vec![None; count],
            next: 0,
            settled,
            stopped: None,
        }
    }

    /// CPU settlement also enforces the batch grant. A stricter run memory
    /// limit remains stricter; physical admission is checked independently.
    pub fn cpu(
        grant: &hirsel_run_schema::CpuGrant,
        width: u32,
        count: usize,
        mut settled: Settled,
    ) -> Self {
        assert!(width <= grant.max_workers, "granted CPU width");
        settled.limit = settled.limit.min(grant.max_batch_bytes);
        Self::new(width, count, settled)
    }

    /// Publish one completed job, including its cleanup.
    /// The caller retains a recording copy before transferring the summary.
    pub fn publish(&mut self, job: usize, charges: Stretch) {
        self.submit(job, charges, true);
    }

    /// Publish a prefix paused at a physical or summary bound. Settlement
    /// checks the prefix but cannot advance to the next job until this job
    /// publishes its continuation and cleanup. A denied allocation may be
    /// included only when it is the next serial charge; resumption must not
    /// charge it again after successful settlement.
    pub fn pause(&mut self, job: usize, charges: Stretch) {
        self.submit(job, charges, false);
    }

    fn submit(&mut self, job: usize, charges: Stretch, complete: bool) {
        assert!(job >= self.next, "a job retires once");
        assert!(
            self.settled.limit == u64::MAX || charges.budgets.memory,
            "memory-limited jobs retain allocation envelopes"
        );
        assert!(
            self.settled.fuel == u64::MAX || charges.budgets.fuel,
            "fuel-limited jobs retain allocation fuel offsets"
        );
        let slot = self.pending.get_mut(job).expect("job belongs to batch");
        assert!(slot.is_none(), "a job publishes once");
        *slot = Some((charges, complete));
    }

    /// Settle the available serial prefix. A stop is sticky: discarded younger
    /// work cannot replace its job or kind, even when published afterward.
    pub fn retire(&mut self) -> Result<usize, JobStop> {
        if let Some(stop) = self.stopped {
            return Err(stop);
        }
        while let Some(Some((_, complete))) = self.pending.get(self.next) {
            let complete = *complete;
            self.retire_one(self.next)?;
            if !complete {
                break;
            }
        }
        Ok(self.next)
    }

    fn retire_one(&mut self, job: usize) -> Result<(), JobStop> {
        assert_eq!(job, self.next, "CPU prefixes retire in job order");
        let (charges, complete) = self.pending[job].as_ref().expect("a published prefix");
        if let Err(kind) = self.settled.settle(charges) {
            let stop = JobStop { job, kind };
            self.stopped = Some(stop);
            return Err(stop);
        }
        let complete = *complete;
        self.pending[job] = None;
        if complete {
            self.next += 1;
        }
        Ok(())
    }

    /// Withdraw only unpublished retirement work. The owning mapper receives
    /// each prefix back and decides which of its retained blocks can be freed.
    /// Neither the serial cut nor settled bytes/fuel move on withdrawal.
    pub fn withdraw_younger(&mut self, job: usize) -> Vec<(usize, Stretch, bool)> {
        assert!(
            job >= self.next && job < self.pending.len(),
            "live CPU ordinal"
        );
        self.pending
            .iter_mut()
            .enumerate()
            .skip(job + 1)
            .filter_map(|(ordinal, slot)| {
                slot.take()
                    .map(|(charges, complete)| (ordinal, charges, complete))
            })
            .collect()
    }

    /// Retire exactly this already-published native prefix, never a younger
    /// available job. The global frontier may contain orchestration or cleanup
    /// charges between the two job prefixes; import those before settlement.
    pub(crate) fn retire_published_at(
        &mut self,
        job: usize,
        frontier: &mut Settled,
    ) -> (Stretch, Result<(), JobStop>) {
        assert_eq!(job, self.next, "native CPU prefixes retire in job order");
        let charges = self.pending[job]
            .as_ref()
            .expect("a published native prefix")
            .0
            .clone();
        if let Some(stop) = self.stopped {
            return (charges, Err(stop));
        }
        self.settled = Settled {
            limit: frontier.limit.min(self.settled.limit),
            ..*frontier
        };
        let result = self.retire_one(job);
        frontier.level = self.settled.level;
        frontier.fuel = self.settled.fuel;
        (charges, result)
    }

    pub fn settled(&self) -> Settled {
        self.settled
    }

    /// Retire a prefix at the native frontier. Between job prefixes the
    /// caller may have retired orchestration work or result cleanup. Import
    /// that state without losing this batch's stricter memory limit.
    pub(crate) fn retire_prefix_at(
        &mut self,
        job: usize,
        charges: Stretch,
        complete: bool,
        frontier: &mut Settled,
    ) -> Result<(), JobStop> {
        if let Some(stop) = self.stopped {
            return Err(stop);
        }
        assert_eq!(job, self.next, "native CPU prefixes retire in job order");
        assert!(
            self.pending[job].is_none(),
            "published CPU prefix retires first"
        );
        self.settled = Settled {
            limit: frontier.limit.min(self.settled.limit),
            ..*frontier
        };
        self.submit(job, charges, complete);
        let result = self.retire_one(job);
        frontier.level = self.settled.level;
        frontier.fuel = self.settled.fuel;
        result
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

    #[test]
    fn joining_physically_settled_prefixes_matches_the_unsplit_charge_stream() {
        let vectors = [
            vec![Fuel, Alloc(16), Fuel, Alloc(32), Free(24), Fuel, Alloc(64)],
            vec![Free(32), Fuel, Alloc(16), Alloc(32), Credit(8), Alloc(24)],
            vec![Alloc(8), Credit(8), Fuel, Credit(0), Fuel, Alloc(16)],
            vec![Alloc(16), Alloc(32), Free(48), Alloc(64), Alloc(16)],
        ];
        for fuel in [false, true] {
            for memory in [false, true] {
                let budgets = Budgets { fuel, memory };
                for events in &vectors {
                    let expected = summarise(budgets, events).unwrap();
                    for first in 0..=events.len() {
                        for second in first..=events.len() {
                            let mut joined = summarise(budgets, &events[..first]).unwrap();
                            joined
                                .append(&summarise(budgets, &events[first..second]).unwrap())
                                .unwrap();
                            joined
                                .append(&summarise(budgets, &events[second..]).unwrap())
                                .unwrap();
                            assert_eq!(joined, expected, "{events:?}, {first}, {second}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn joined_prefixes_keep_the_original_envelope_and_piece_caps() {
        let events: Vec<_> = (0..ENVELOPE_MAX).flat_map(|_| [Fuel, Alloc(1)]).collect();
        let mut full = summarise(BOTH, &events).unwrap();
        let before = full.clone();
        assert_eq!(
            full.append(&summarise(BOTH, &[Fuel, Alloc(1)]).unwrap()),
            Err(Pause::Envelope)
        );
        assert_eq!(full, before);
        // Allocations at the same fuel offset replace a high even at the cap.
        full.append(&summarise(BOTH, &[Alloc(1)]).unwrap()).unwrap();
        assert_eq!(full.pieces[0].1.envelope.len(), ENVELOPE_MAX);
        let mut full = summarise(BOTH, &vec![Credit(1); PIECES_MAX - 1]).unwrap();
        let before = full.clone();
        assert_eq!(
            full.append(&summarise(BOTH, &[Credit(1)]).unwrap()),
            Err(Pause::Pieces)
        );
        assert_eq!(full, before);
    }

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

    #[test]
    fn cpu_jobs_keep_cleanup_highs_and_fuel_memory_first_stop_order() {
        for width in [4, 8, 16] {
            for (cleanup, fuel, kind) in [
                (vec![Alloc(20), Free(20), Fuel, Fuel], 1, BudgetKind::Memory),
                (vec![Fuel, Fuel, Alloc(20), Free(20)], 1, BudgetKind::Fuel),
                (vec![Fuel, Alloc(20), Free(20)], 1, BudgetKind::Memory),
                // Root death opens a piece before the cleanup allocation.
                (
                    vec![Credit(30), Fuel, Alloc(50), Free(50)],
                    1,
                    BudgetKind::Memory,
                ),
            ] {
                let state = Settled {
                    level: 70,
                    limit: 100,
                    fuel,
                };
                let first = summarise(BOTH, &[Alloc(20)]).unwrap();
                let second = summarise(BOTH, &cleanup).unwrap();
                let younger = summarise(BOTH, &[Alloc(200)]).unwrap();
                let mut expected = state;
                assert_eq!(reference(&mut expected, &[Alloc(20)]), Ok(()));
                assert_eq!(reference(&mut expected, &cleanup), Err(kind));
                let mut live = Jobs::new(width, 3, state);
                live.publish(2, younger.clone());
                live.publish(1, second.clone());
                assert_eq!(live.retire(), Ok(0));
                live.publish(0, first.clone());
                let stop = Err(JobStop { job: 1, kind });
                assert_eq!(live.retire(), stop);
                assert_eq!(live.retire(), stop);
                let mut replay = Jobs::new(width, 3, state);
                replay.publish(0, first);
                assert_eq!(replay.retire(), Ok(1));
                replay.publish(1, second);
                assert_eq!(replay.retire(), stop);
                replay.publish(2, younger);
                assert_eq!(replay.retire(), stop);
            }
        }
    }

    #[test]
    fn cpu_job_credits_and_a_stricter_run_limit_are_preserved() {
        let grant = hirsel_run_schema::CpuGrant {
            provider: "cpu.scoped@1".into(),
            max_workers: 4,
            max_domains: 1,
            max_physical_bytes: 1 << 30,
            max_batch_bytes: 100,
        };
        for (limit, expected) in [
            (u64::MAX, Ok(2)),
            (
                80,
                Err(JobStop {
                    job: 1,
                    kind: BudgetKind::Memory,
                }),
            ),
        ] {
            let mut jobs = Jobs::cpu(
                &grant,
                4,
                2,
                Settled {
                    level: 60,
                    limit,
                    fuel: 7,
                },
            );
            jobs.publish(1, summarise(BOTH, &[Credit(40), Alloc(70), Fuel]).unwrap());
            assert_eq!(jobs.retire(), Ok(0));
            jobs.publish(0, summarise(BOTH, &[Fuel]).unwrap());
            assert_eq!(jobs.retire(), expected);
            if expected.is_ok() {
                assert_eq!(
                    jobs.settled(),
                    Settled {
                        level: 90,
                        limit: 100,
                        fuel: 5
                    }
                );
            }
        }
    }

    #[test]
    fn cpu_job_paused_prefix_resumes_before_younger_cleanup() {
        let mut jobs = Jobs::new(
            4,
            2,
            Settled {
                level: 0,
                limit: 100,
                fuel: 8,
            },
        );
        jobs.publish(1, summarise(BOTH, &[Alloc(100)]).unwrap());
        jobs.pause(0, summarise(BOTH, &[Fuel, Alloc(80)]).unwrap());
        assert_eq!(jobs.retire(), Ok(0));
        assert_eq!(
            jobs.settled(),
            Settled {
                level: 80,
                limit: 100,
                fuel: 7
            }
        );
        assert_eq!(jobs.retire(), Ok(0));
        jobs.publish(0, summarise(BOTH, &[Credit(80), Fuel]).unwrap());
        assert_eq!(jobs.retire(), Ok(2));
        assert_eq!(
            jobs.settled(),
            Settled {
                level: 100,
                limit: 100,
                fuel: 6
            }
        );
    }

    #[test]
    fn native_cpu_publications_wait_for_each_global_retirement_without_duplicate_charge() {
        for width in [1, 4, 8, 16] {
            let mut state = Settled {
                level: 0,
                limit: 100,
                fuel: 20,
            };
            let mut jobs = Jobs::new(width, 2, state);
            let younger = summarise(BOTH, &[Fuel, Alloc(60)]).unwrap();
            jobs.pause(1, younger.clone());
            jobs.retire_prefix_at(
                0,
                summarise(BOTH, &[Fuel, Alloc(30)]).unwrap(),
                false,
                &mut state,
            )
            .unwrap();
            assert_eq!(
                state,
                Settled {
                    level: 30,
                    limit: 100,
                    fuel: 19
                }
            );
            jobs.retire_prefix_at(0, summarise(BOTH, &[Free(30)]).unwrap(), true, &mut state)
                .unwrap();
            assert_eq!(
                state,
                Settled {
                    level: 0,
                    limit: 100,
                    fuel: 19
                }
            );
            assert!(jobs.pending[1].is_some(), "job zero never retires job one");
            // Orchestration between mapper prefixes belongs to the run frontier.
            state.level = 10;
            state.fuel -= 2;
            let (recorded, result) = jobs.retire_published_at(1, &mut state);
            assert_eq!(recorded, younger);
            assert_eq!(result, Ok(()));
            assert_eq!(
                state,
                Settled {
                    level: 70,
                    limit: 100,
                    fuel: 16
                }
            );
            jobs.retire_prefix_at(1, summarise(BOTH, &[Free(60)]).unwrap(), true, &mut state)
                .unwrap();
            assert_eq!(
                state,
                Settled {
                    level: 10,
                    limit: 100,
                    fuel: 16
                }
            );
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    jobs.retire_published_at(1, &mut state)
                }))
                .is_err(),
                "a summary cannot retire twice"
            );
        }
    }

    #[test]
    fn withdrawing_younger_summaries_returns_owner_charges_without_settling_them() {
        for width in [1, 4, 8, 16] {
            let mut state = Settled {
                level: 0,
                limit: 100,
                fuel: 20,
            };
            let mut jobs = Jobs::new(width, 2, state);
            let younger = summarise(BOTH, &[Fuel, Alloc(60), Free(20)]).unwrap();
            jobs.publish(1, younger.clone());
            let withdrawn = jobs.withdraw_younger(0);
            assert_eq!(withdrawn, vec![(1, younger.clone(), true)]);
            assert!(
                jobs.withdraw_younger(0).is_empty(),
                "one withdrawal per prefix"
            );
            assert_eq!(jobs.settled(), state);
            jobs.retire_prefix_at(
                0,
                summarise(BOTH, &[Fuel, Alloc(30)]).unwrap(),
                true,
                &mut state,
            )
            .unwrap();
            jobs.publish(1, withdrawn.into_iter().next().unwrap().1);
            let (_, result) = jobs.retire_published_at(1, &mut state);
            assert_eq!(result, Ok(()));
            assert_eq!(
                state,
                Settled {
                    level: 70,
                    limit: 100,
                    fuel: 18
                }
            );
        }
    }
}
