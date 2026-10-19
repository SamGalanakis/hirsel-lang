//! Scripted signals (`signals.script@1`): the simulation delivers the script's arrivals one
//! per idle point, when no task is runnable and before virtual time advances. From the
//! run's first `io.signals` projection an arrival of an observed kind is pending until a
//! `signal.wait` consumes it; at most one arrival of a kind is pending, and a second
//! Interrupt while one is pending, an arrival before the projection or of a kind the grant
//! does not name takes the default action, which ends the run `Signaled`.
use super::Library;
use super::dispatch::{arg, encode_value, error, ok, variant};
use crate::{BlockedWait, Error, OperationContext, OperationResult, Value, invalid};
use hirsel_run_schema::SignalKind;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub(super) struct Signals {
    /// Parked waits: token to the kinds they wait for, as bits.
    waits: BTreeMap<u64, u8>,
    /// Pending arrivals, earliest first.
    pending: Vec<SignalKind>,
    /// Whether the run projected its Signals, so observed kinds no longer kill it.
    installed: bool,
    /// Scripted arrivals already delivered.
    delivered: usize,
}
impl Signals {
    pub fn contains(&self, token: u64) -> bool {
        self.waits.contains_key(&token)
    }
    /// Consume the earliest pending arrival among `bits`.
    fn take(&mut self, bits: u8) -> Option<SignalKind> {
        let index = self.pending.iter().position(|k| bits & k.bit() != 0)?;
        Some(self.pending.remove(index))
    }
}

/// `SignalKind` as its variant value.
fn kind_value(kind: SignalKind) -> Value {
    variant(kind.ordinal(), vec![])
}

impl Library {
    pub(super) fn signals_granted(&self) -> Option<&hirsel_run_schema::SignalsGrant> {
        self.simulation.config.signals.as_ref()
    }
    /// `io.signals`: the run's Signals, observing its grant's kinds from now on.
    pub(super) fn project_signals(&mut self) -> Value {
        if self.signals_granted().is_none() {
            return error(variant(0, vec![]));
        }
        self.signals.installed = true;
        ok(self
            .simulation
            .handle("Signals", super::simulation::Grant::Root("Signals".into())))
    }
    /// A host adapter's arrival, which the next `signal.wait` among its kind consumes.
    pub fn supply_signal(&mut self, kind: SignalKind) {
        self.signals.pending.push(kind);
    }
    /// `signal.wait`; a host adapter completes a parked wait here after supplying its kind.
    pub fn wait_signal(
        &mut self,
        a: &[Value],
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.simulation.id(arg(a, 0)?, "Signals")?;
        let Value::Sequence { elements, .. } = arg(a, 1)? else {
            return Err(invalid("signal.wait expects List[SignalKind]"));
        };
        let mut bits = 0u8;
        for element in elements {
            match element {
                Some(Value::Variant { variant, .. }) => {
                    bits |= SignalKind::from_ordinal(*variant)
                        .ok_or_else(|| invalid("invalid SignalKind"))?
                        .bit();
                }
                _ => return Err(invalid("invalid SignalKind")),
            }
        }
        self.tasks.yield_now = true;
        let input = vec![bits];
        let granted = self.signals_granted().map_or(0, |g| g.bits());
        let value = if bits == 0 {
            error(variant(2, vec![]))
        } else if bits & !granted != 0 {
            error(variant(1, vec![]))
        } else if let Some(cause) = self.tasks.canceled() {
            error(variant(0, vec![variant(cause, vec![])]))
        } else if let Some(kind) = self.signals.take(bits) {
            ok(kind_value(kind))
        } else {
            let token = self.next_callback;
            self.next_callback += 1;
            self.signals.waits.insert(token, bits);
            self.simulation
                .record("Signals", "signal.wait.suspend", input, vec![], context)?;
            return Ok(OperationResult::Park {
                token,
                wait: BlockedWait {
                    spawn_path: self.tasks.path(),
                    anchor: context.anchor().unwrap().clone(),
                    kind: "signal.wait".into(),
                    targets: vec![],
                },
            });
        };
        self.simulation.record(
            "Signals",
            "signal.wait",
            input,
            encode_value(&value),
            context,
        )?;
        Ok(OperationResult::returned(value))
    }
    /// A cancelled wait, or one an arrival of its kinds is pending for, completes.
    pub(super) fn poll_signal(
        &mut self,
        token: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let bits = self.signals.waits[&token];
        let value = if let Some(cause) = self.tasks.canceled() {
            error(variant(0, vec![variant(cause, vec![])]))
        } else if let Some(kind) = self.signals.take(bits) {
            ok(kind_value(kind))
        } else {
            return Ok(None);
        };
        self.simulation.record(
            "Signals",
            "signal.wait",
            vec![bits],
            encode_value(&value),
            context,
        )?;
        self.signals.waits.remove(&token);
        Ok(Some(OperationResult::returned(value)))
    }
    /// With no runnable task: deliver the script's next arrival. False when none is left;
    /// `Signaled` when it takes the default action.
    pub(super) fn deliver_signal(&mut self) -> Result<bool, Error> {
        let Some(grant) = self.signals_granted() else {
            return Ok(false);
        };
        let Some(&kind) = grant
            .script
            .as_deref()
            .and_then(|script| script.get(self.signals.delivered))
        else {
            return Ok(false);
        };
        let observed = grant.kinds.contains(&kind);
        self.signals.delivered += 1;
        let pending = self.signals.pending.contains(&kind);
        if !self.signals.installed || !observed || (pending && kind == SignalKind::Interrupt) {
            return Err(Error::Signaled(kind));
        }
        if !pending {
            self.signals.pending.push(kind);
        }
        Ok(true)
    }
}
