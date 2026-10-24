//! Timer waits under virtual time: a simulated sleep never waits for the host. The
//! monotonic reading advances only when no task is runnable, to the earliest pending
//! deadline, and the waits due then resume in the order they suspended. Cancelling a
//! suspended wait removes its timer (Go runtime/time.go deletes a stopped timer the same way).
use super::Library;
use super::dispatch::{arg, encode_value, error, ok, variant};
use super::simulation::Fault;
use crate::{BlockedWait, Error, OperationCall, OperationContext, OperationResult, Value, invalid};
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub(super) struct Timers {
    /// Wait token to (deadline ticks, recorded request).
    pending: BTreeMap<u64, (u64, Vec<u8>)>,
}
impl Timers {
    pub fn deadline(&self, token: u64) -> Option<u64> {
        self.pending.get(&token).map(|(deadline, _)| *deadline)
    }
    pub fn contains(&self, token: u64) -> bool {
        self.pending.contains_key(&token)
    }
}
fn instant(value: &Value) -> Result<u64, Error> {
    match value {
        Value::Handle { kind, id } if kind == "Instant" => Ok(*id),
        _ => Err(invalid("invalid Instant representation")),
    }
}
impl Library {
    /// Pure: the clock operand is only the brand's owner, so a host adapter's clock
    /// handle serves as well as a simulated one.
    pub(super) fn instant_checked_add(&self, a: &[Value]) -> Result<Value, Error> {
        if !matches!(arg(a, 0)?, Value::Handle { kind, .. } if kind == "MonotonicClock") {
            return Err(invalid("expected MonotonicClock"));
        }
        let start = instant(arg(a, 1)?)?;
        let Value::Integer(n) = arg(a, 2)? else {
            return Err(invalid("instant.checked_add nanoseconds"));
        };
        let n = u64::try_from(n.bits()).map_err(|_| invalid("instant.checked_add nanoseconds"))?;
        Ok(match start.checked_add(n) {
            Some(ticks) => super::some(Value::Handle {
                kind: "Instant".into(),
                id: ticks,
            }),
            None => super::none(),
        })
    }
    pub(super) fn sleep_until(
        &mut self,
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let a = &*call.arguments;
        self.simulation.id(arg(a, 0)?, "MonotonicClock")?;
        let deadline = instant(arg(a, 1)?)?;
        self.tasks.yield_now = true;
        let input = deadline.to_le_bytes().to_vec();
        let value = if let Some(cause) = self.tasks.canceled() {
            error(variant(1, vec![variant(cause, vec![])]))
        } else if let Some(fault) = self.simulation.fault("clock.sleep_until") {
            match fault {
                Fault::Fault { code, partial: 0 } if code <= 1 => {
                    error(variant(0, vec![variant(code, vec![])]))
                }
                _ => return Err(invalid("invalid clock wait error")),
            }
        } else if deadline <= self.simulation.monotonic_now() {
            ok(Value::Unit)
        } else {
            let token = self.next_callback;
            self.next_callback += 1;
            self.timers.pending.insert(token, (deadline, input.clone()));
            self.simulation.record(
                "MonotonicClock",
                "clock.sleep_until.suspend",
                input,
                vec![],
                context,
            )?;
            return Ok(OperationResult::Park {
                token,
                wait: BlockedWait {
                    spawn_path: self.tasks.path(),
                    anchor: context.anchor().unwrap().clone(),
                    kind: "clock.sleep_until".into(),
                    targets: vec![],
                },
            });
        };
        self.simulation.record(
            "MonotonicClock",
            "clock.sleep_until",
            input,
            encode_value(&value),
            context,
        )?;
        Ok(OperationResult::returned(value))
    }
    /// A due or cancelled wait completes; any other stays suspended.
    pub(super) fn poll_timer(
        &mut self,
        token: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let (deadline, input) = self.timers.pending[&token].clone();
        let value = if let Some(cause) = self.tasks.canceled() {
            error(variant(1, vec![variant(cause, vec![])]))
        } else if deadline <= self.simulation.monotonic_now() {
            ok(Value::Unit)
        } else {
            return Ok(None);
        };
        self.simulation.record(
            "MonotonicClock",
            "clock.sleep_until",
            input,
            encode_value(&value),
            context,
        )?;
        self.timers.pending.remove(&token);
        Ok(Some(OperationResult::returned(value)))
    }
    /// With no runnable task, advance virtual time to the earliest pending timer wait or
    /// scope deadline (CONC-3).
    pub(super) fn advance_virtual_time(&mut self) -> bool {
        match self
            .timers
            .pending
            .values()
            .map(|(deadline, _)| *deadline)
            .chain(self.next_scope_deadline())
            .chain(
                self.simulation
                    .next_child_gate()
                    .filter(|t| *t > self.simulation.monotonic_now()),
            )
            .chain(
                self.network_waits
                    .values()
                    .filter(|w| matches!(w.name.as_str(), "child.wait" | "proc.wait_group"))
                    .filter_map(|w| {
                        super::process::deadline(w.arguments.get(2)?)
                            .ok()
                            .flatten()
                            .map(|(_, t)| t)
                    })
                    .filter(|t| *t > self.simulation.monotonic_now()),
            )
            .min()
        {
            Some(deadline) => {
                let from = self.simulation.monotonic_now();
                if self
                    .simulation
                    .config
                    .process
                    .as_ref()
                    .is_some_and(|g| g.timed.is_some())
                    && deadline > from
                {
                    self.process_advance = Some((from, deadline));
                }
                self.simulation.advance_monotonic(deadline);
                true
            }
            None => false,
        }
    }
}
