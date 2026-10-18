//! Run budgets (`budgets.limits@1`, docs/spec/determinism.md "Budgets"): fuel, charged
//! at every function entry and at every jump to the same or an earlier block, and
//! memory, the logical bytes of every value the run holds (`Value::logical_bytes`).
//!
//! Memory is measured exactly, by walking the run's values, and only the decision of
//! *when* to measure is estimated: constructions and library results charge their
//! shallow size, and a measurement is due once the charges since the last one pass
//! the remaining headroom (at least a sixteenth of the budget, so a run near its
//! budget is not walked at every charge), or once enough fuel has passed for
//! uncharged growth. Every input is deterministic, so a replay stops where the
//! recorded run stopped.
use crate::BudgetKind;

/// The host's budgets for one run; None leaves a dimension unbounded.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RunBudgets {
    pub fuel: Option<u64>,
    pub memory_bytes: Option<u64>,
}

/// Fuel between measurements is at least this, and at least a sixty-fourth of the
/// last measured bytes, so measuring costs a bounded share of the run.
const MEASURE_FUEL: u64 = 4096;

pub(crate) struct Meter {
    budgets: RunBudgets,
    /// Fuel charged so far.
    fuel: u64,
    /// Logical bytes at the last measurement, and the fuel then.
    live: u64,
    measured_at: u64,
    /// Estimated bytes charged since the last measurement.
    charged: u64,
}
impl Meter {
    pub fn new(budgets: RunBudgets) -> Self {
        Self {
            budgets,
            fuel: 0,
            live: 0,
            measured_at: 0,
            charged: 0,
        }
    }
    /// Charge one unit of fuel, or report that the budget is spent.
    pub fn fuel(&mut self) -> Result<(), BudgetKind> {
        if self.budgets.fuel == Some(self.fuel) {
            return Err(BudgetKind::Fuel);
        }
        self.fuel += 1;
        Ok(())
    }
    pub fn metered(&self) -> bool {
        self.budgets.memory_bytes.is_some()
    }
    /// Charge an estimate of newly held bytes.
    pub fn charge(&mut self, bytes: u64) {
        self.charged = self.charged.saturating_add(bytes);
    }
    /// Whether the next fuel point measures.
    pub fn due(&self) -> bool {
        let Some(limit) = self.budgets.memory_bytes else {
            return false;
        };
        self.charged > limit.saturating_sub(self.live).max(limit / 16)
            || self.fuel - self.measured_at >= MEASURE_FUEL.max(self.live / 64)
    }
    /// Record a measurement of `live` bytes, or report that it exceeds the budget.
    pub fn measured(&mut self, live: u64) -> Result<(), BudgetKind> {
        self.live = live;
        self.charged = 0;
        self.measured_at = self.fuel;
        match self.budgets.memory_bytes {
            Some(limit) if live > limit => Err(BudgetKind::Memory),
            _ => Ok(()),
        }
    }
}
