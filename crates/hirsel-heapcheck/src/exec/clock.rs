//! Time (CONC-3, the core clock library): timer waits, scope deadlines and a `proc.run`
//! deadline, over the model's one virtual monotonic reading.
//!
//! The model grants clocks and owns the reading ([`Operations::monotonic`]); H owns what
//! waits for it and when it moves. A run follows the deterministic simulation's virtual
//! time: the reading advances only when no task is runnable, to the earliest pending timer
//! wait or scope deadline, and each poll expires the due scope deadlines first. That is one
//! legal schedule of many: on a host, any pending deadline may pass while any operation
//! runs. So every operation a task starts while a deadline is pending, and every `proc.run`
//! whose deadline is not yet reached, is a timer choice point, and `Config::fire` names one
//! at which the earliest pending deadline fires early: the reading jumps to it before the
//! operation proceeds, so a scope deadline cancels the operation, a sleeper wakes before it
//! completes, or the run times out. One early firing per run bounds the exploration, as one
//! injected failure per run bounds fault injection.
use super::*;
use hirsel_core_contract::OperationId;

/// `CancelCause::DeadlineExceeded` (core library).
pub(super) const CANCEL_DEADLINE: u32 = 3;

/// A scope's deadline: an instant of monotonic clock `clock`, until it fires or the scope exits.
#[derive(Clone, Copy, Debug)]
pub(super) struct ScopeDeadline {
    pub(super) clock: u64,
    pub(super) at: u64,
    pub(super) expired: bool,
}

/// A task's suspended timer wait: the operation to perform again once it is due, and its
/// completion once a poll found it due or the task canceled.
#[derive(Clone)]
pub(super) struct Timer {
    clock: u64,
    pub(super) until: u64,
    op: OperationId,
    args: Vec<OpValue>,
    completion: Option<OpOutcome>,
}

impl<S: Stage> Machine<'_, S> {
    /// The reading of `clock`, which the model must have granted.
    fn reading(&self, clock: u64) -> Checked<u64> {
        self.ops.monotonic(clock).ok_or_else(|| {
            err(
                V::Malformed,
                "a deadline on a clock the model did not grant",
            )
        })
    }

    /// A borrowed `Deadline` argument's clock handle and its Instant's ticks.
    pub(super) fn deadline_words(
        &mut self,
        f: &Frame,
        argument: &CallArgument,
    ) -> Checked<(u64, u64)> {
        let (image, layout) = match argument.value {
            ArgumentValue::Value(v) => match self.read_use(f, v)? {
                Val::Image(image) => (image, self.value_type(f.unit, v)?.layout),
                _ => return fail(V::TypeConfusion, "a Deadline that is not a record"),
            },
            ArgumentValue::Address { address, .. } => {
                let r = self.resolve_use(f, address)?;
                let size = self.size(r.layout)?;
                let image = self
                    .ledger
                    .check(r.ptr)?
                    .extent
                    .copy(r.ptr.offset, size, None);
                (image, r.layout)
            }
        };
        let word = |v: &OpValue| match v {
            OpValue::Handle(word) => Some(*word),
            OpValue::Bits { bits, .. } => u64::try_from(*bits).ok(),
            _ => None,
        };
        match self.operand_at(&image, 0, layout)? {
            OpValue::Aggregate(fields) => match fields.as_slice() {
                [clock, at] => word(clock).zip(word(at)),
                _ => None,
            },
            _ => None,
        }
        .ok_or_else(|| {
            err(
                V::TypeConfusion,
                "a Deadline that is not a clock and an Instant",
            )
        })
    }

    /// The earliest pending deadline, with its clock: a live scope's deadline that has not
    /// fired, or a timer wait not yet completed.
    fn earliest(&self) -> Option<(u64, u64)> {
        let s = self.tasks.as_deref()?;
        let scope = s.armed().next().map(|(_, d)| (d.at, d.clock));
        let timer = s
            .timers
            .values()
            .filter(|t| t.completion.is_none())
            .map(|t| (t.until, t.clock))
            .min();
        scope.into_iter().chain(timer).min()
    }

    /// Expire every scope deadline the reading reached, by deadline with ties in entry order.
    pub(super) fn expire_due(&mut self) -> Checked<()> {
        let Some(s) = self.tasks.as_deref() else {
            return Ok(());
        };
        let armed: Vec<(u64, u32, u64)> = s.armed().map(|(i, d)| (d.at, i, d.clock)).collect();
        for (at, index, clock) in armed {
            if self.reading(clock)? >= at {
                self.scheduler().expire(index)?;
            }
        }
        Ok(())
    }

    /// With no task runnable: advance the reading to the earliest pending deadline, so a
    /// pending wait never makes the run Blocked (CONC-8). False when nothing is pending.
    pub(super) fn advance(&mut self) -> Checked<bool> {
        match self.earliest() {
            Some((at, clock)) => {
                if self.reading(clock)? < at {
                    self.ops.advance_monotonic(at);
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// A task starts an operation: a timer choice point while a deadline is pending. At the
    /// chosen one the earliest pending deadline fires now, and the scope deadlines it reaches
    /// expire before the operation checks its cancellation.
    pub(super) fn timer_choice(&mut self) -> Checked<()> {
        let Some((at, clock)) = self.earliest() else {
            return Ok(());
        };
        if self.reading(clock)? >= at {
            return Ok(());
        }
        let index = self.choices;
        self.choices += 1;
        if self.config.fire == Some(index) {
            self.ops.advance_monotonic(at);
            self.expire_due()?;
        }
        Ok(())
    }

    /// An operation racing its own deadline `at`: a timer choice point unless another pending
    /// deadline comes first (its firing at the operation's start already cancels it). True
    /// when the deadline fires at it: the reading jumps there and the operation times out.
    pub(super) fn deadline_choice(&mut self, clock: u64, at: u64) -> Checked<bool> {
        if self.reading(clock)? >= at {
            return fail(
                V::Malformed,
                "a deadline race whose deadline already passed",
            );
        }
        if self.earliest().is_some_and(|(earlier, _)| earlier <= at) {
            return Ok(false);
        }
        let index = self.choices;
        self.choices += 1;
        if self.config.fire != Some(index) {
            return Ok(false);
        }
        self.ops.advance_monotonic(at);
        Ok(true)
    }

    /// Suspend the running task in a timer wait until `until`: the operation yields first,
    /// and its call runs again once a poll completed the wait.
    pub(super) fn timer_wait(
        &mut self,
        clock: u64,
        until: u64,
        op: OperationId,
        args: Vec<OpValue>,
    ) -> Checked<()> {
        if self.reading(clock)? >= until {
            return fail(V::Malformed, "a timer wait whose deadline already passed");
        }
        let s = self.scheduler();
        let current = s.current;
        s.task(current)?.wait = Some(tasks::Wait::Timer);
        s.timers.insert(
            current,
            Timer {
                clock,
                until,
                op,
                args,
                completion: None,
            },
        );
        self.park_until(tasks::Park::Timer { until }, true)
    }

    /// The completion of the running task's timer wait, when its call runs again after the
    /// poll that ended it. The wait yielded when it parked, so the operation's yield is spent.
    pub(super) fn timer_resumed(&mut self) -> Checked<Option<OpOutcome>> {
        let Some(s) = self.tasks.as_deref_mut() else {
            return Ok(None);
        };
        let current = s.current;
        if s.task(current)?.wait != Some(tasks::Wait::Timer) {
            return Ok(None);
        }
        s.task(current)?.wait = None;
        match s.timers.remove(&current).and_then(|t| t.completion) {
            Some(OpOutcome::Wait { .. } | OpOutcome::Deadline { .. }) | None => {
                fail(V::Malformed, "a timer wait resumed without its completion")
            }
            Some(outcome) => Ok(Some(outcome)),
        }
    }

    /// Whether parked `task`'s timer wait is over: its task canceled (the operation completes
    /// `Canceled`), or the reading reached its deadline (the operation is performed again).
    pub(super) fn poll_timer(&mut self, task: u32) -> Checked<bool> {
        let timer = self
            .scheduler()
            .timers
            .get(&task)
            .cloned()
            .ok_or_else(|| err(V::Malformed, "a timer park without its wait"))?;
        let shielded = self.task_shielded(task)?;
        let completion = match self.scheduler().canceled_task(task, shielded)? {
            Some(cause) => self
                .ops
                .canceled(&timer.op, &timer.args, cause)
                .ok_or_else(|| err(V::Malformed, "a timer wait that is no cancellation point"))?,
            None if self.reading(timer.clock)? >= timer.until => {
                self.ops.call(&timer.op, &timer.args, &mut self.output)
            }
            None => return Ok(false),
        };
        // The wait yielded when it parked.
        self.ops.yielded();
        if let Some(t) = self.scheduler().timers.get_mut(&task) {
            t.completion = Some(completion);
        }
        Ok(true)
    }
}
