//! The run scheduler's choice policies (`inputs.scheduler`, docs/spec/determinism.md): which
//! runnable task runs at each scheduling point. Both engines call these at the same points
//! with their runnable queue in the same order, so a seed selects one schedule in either.
//!
//! - `fifo@1` takes the front of the queue and draws nothing.
//! - `random@1` takes candidate `w mod n` for the next SplitMix64 word `w` of its seed
//!   whenever `n >= 2` tasks are runnable.
//! - `pct@1` is probabilistic concurrency testing (Burckhardt, Kothari, Musuvathi and
//!   Nagarakatte, ASPLOS 2010): every task gets a random priority when it is created, the
//!   highest-priority runnable task runs, and at `d - 1` random scheduling points the task
//!   that just ran drops below every initial priority. Its words, in order, choose the depth
//!   `d = 1 + w mod 3`, the horizon `k = 2^(2 + w mod 11)`, the change points `1 + w mod k`,
//!   then one priority `d + (w >> 1)` per created task, the root first. Points past the
//!   horizon take the front of the queue, so every runnable task keeps running (CONC-1).

/// A scheduler contract (`inputs.scheduler.algorithm`, version 1).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Algorithm {
    #[default]
    Fifo,
    Random,
    Pct,
}

impl Algorithm {
    /// The launch configuration's word for each contract.
    pub fn code(self) -> u64 {
        match self {
            Algorithm::Fifo => 0,
            Algorithm::Random => 1,
            Algorithm::Pct => 2,
        }
    }
    pub fn from_code(code: u64) -> Option<Algorithm> {
        [Algorithm::Fifo, Algorithm::Random, Algorithm::Pct]
            .into_iter()
            .find(|algorithm| algorithm.code() == code)
    }
}

/// The largest `pct@1` depth.
const MAX_DEPTH: u64 = 3;

/// One run's scheduler state. A task's priority lives with the task in its engine.
#[derive(Clone, Debug)]
pub struct Schedule {
    algorithm: Algorithm,
    state: u64,
    /// Scheduling points so far (`pct@1`).
    step: u64,
    horizon: u64,
    depth: u64,
    /// Change point `i + 1` lowers to priority `i + 1`.
    changes: [u64; MAX_DEPTH as usize - 1],
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule::new(Algorithm::Fifo, 0)
    }
}

impl Schedule {
    pub fn new(algorithm: Algorithm, seed: u64) -> Schedule {
        let mut schedule = Schedule {
            algorithm,
            state: seed,
            step: 0,
            horizon: 0,
            depth: 0,
            changes: [0; MAX_DEPTH as usize - 1],
        };
        if algorithm == Algorithm::Pct {
            schedule.depth = 1 + schedule.word() % MAX_DEPTH;
            schedule.horizon = 1 << (2 + schedule.word() % 11);
            for i in 0..schedule.depth as usize - 1 {
                schedule.changes[i] = 1 + schedule.word() % schedule.horizon;
            }
        }
        schedule
    }

    pub fn algorithm(&self) -> Algorithm {
        self.algorithm
    }

    /// The next SplitMix64 word, as `random.splitmix64@1` draws them.
    fn word(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    /// The priority of a task created now. Engines call it for the root task first, then for
    /// each spawned child in spawn order.
    pub fn admit(&mut self) -> u64 {
        match self.algorithm {
            Algorithm::Pct => self.depth + (self.word() >> 1),
            Algorithm::Fifo | Algorithm::Random => 0,
        }
    }

    /// A scheduling point: the task that ran last stopped or yielded and at least one task
    /// is runnable. The new priority of that task when this point is a change point.
    pub fn point(&mut self) -> Option<u64> {
        if self.algorithm != Algorithm::Pct {
            return None;
        }
        self.step += 1;
        (0..self.depth as usize - 1)
            .rev()
            .find(|&i| self.changes[i] == self.step)
            .map(|i| i as u64 + 1)
    }

    /// The index of the task to run among the runnable `priorities` (queue order, nonempty),
    /// after [`point`](Self::point).
    pub fn choose(&mut self, priorities: &[u64]) -> usize {
        match self.algorithm {
            Algorithm::Fifo => 0,
            Algorithm::Random if priorities.len() < 2 => 0,
            Algorithm::Random => (self.word() % priorities.len() as u64) as usize,
            Algorithm::Pct if self.step > self.horizon => 0,
            Algorithm::Pct => {
                let mut best = 0;
                for (index, &priority) in priorities.iter().enumerate() {
                    if priority > priorities[best] {
                        best = index;
                    }
                }
                best
            }
        }
    }

    /// Whether a choice among `n` runnable tasks is a recorded scheduling input: the policy
    /// could have chosen another task.
    pub fn recorded(&self, n: usize) -> bool {
        self.algorithm != Algorithm::Fifo && n >= 2
    }
}
