//! `io.signals` and `signal.wait` under a scripted grant (`signals.script@1`): the
//! deliveries happen at H's idle points, as the reference's do
//! (`hirsel-interp/src/library/signals.rs`).
use super::{Io, IoGrant, io_ok};
use hirsel_heapcheck::{OpOutcome, OpValue, SignalDelivery};
use hirsel_rt::launch::IO_HANDLE;
use hirsel_run_schema::SignalKind;

#[derive(Clone, Default)]
pub(super) struct Signals {
    grant: Option<hirsel_run_schema::SignalsGrant>,
    /// Pending arrivals, earliest first.
    pending: Vec<SignalKind>,
    installed: bool,
    delivered: usize,
}

impl Signals {
    pub(super) fn new(grant: Option<hirsel_run_schema::SignalsGrant>) -> Self {
        Self {
            grant,
            ..Self::default()
        }
    }
    pub(super) fn deliver(&mut self) -> SignalDelivery {
        let Some(grant) = &self.grant else {
            return SignalDelivery::None;
        };
        let Some(&kind) = grant
            .script
            .as_deref()
            .and_then(|script| script.get(self.delivered))
        else {
            return SignalDelivery::None;
        };
        self.delivered += 1;
        let held = self.pending.contains(&kind);
        if !self.installed
            || !grant.kinds.contains(&kind)
            || (held && kind == SignalKind::Interrupt)
        {
            return SignalDelivery::Default(kind.ordinal());
        }
        if !held {
            self.pending.push(kind);
        }
        SignalDelivery::Pending
    }
}

/// `hirsel_rt::io::SignalResult`: tag, then the kind's ordinal or the error's tag, then a
/// cancellation's cause.
fn result(tag: u32, value: u32, cause: u32) -> OpValue {
    let mut bytes = Vec::with_capacity(12);
    for word in [tag, value, cause] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    OpValue::Plain(bytes)
}

/// The kinds a `List[SignalKind]` operand names, as bits.
fn kinds(operand: &OpValue) -> Option<u8> {
    let ordinals: Vec<u32> = match operand {
        OpValue::Sequence(items) => items
            .iter()
            .map(|item| match item {
                OpValue::Variant { tag, .. } => Some(*tag),
                _ => None,
            })
            .collect::<Option<_>>()?,
        OpValue::Elements { bits, .. } => bits
            .iter()
            .map(|b| u32::try_from(*b).ok())
            .collect::<Option<_>>()?,
        _ => return None,
    };
    ordinals
        .into_iter()
        .try_fold(0, |bits, o| Some(bits | SignalKind::from_ordinal(o)?.bit()))
}

impl Io {
    /// `io.signals`, `signal.wait` and their cancellation point; `None` for other operations.
    pub(super) fn signals(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[OpValue],
        cancel: Option<u32>,
    ) -> Option<OpOutcome> {
        match (module, name, args) {
            (Some("io"), "signals", [OpValue::Handle(IO_HANDLE)]) if cancel.is_none() => {
                if self.signals.grant.is_none() {
                    // GrantError.NotGranted.
                    return Some(OpOutcome::Value(OpValue::Variant {
                        tag: 1,
                        value: Box::new(OpValue::Variant {
                            tag: 0,
                            value: Box::new(OpValue::Unit),
                        }),
                    }));
                }
                self.signals.installed = true;
                Some(OpOutcome::Value(io_ok(OpValue::Handle(
                    self.grant(IoGrant::Signals),
                ))))
            }
            (Some("signal"), "wait", [OpValue::Handle(handle), operand]) => {
                if !matches!(self.grants.get(handle), Some(IoGrant::Signals)) {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                let Some(bits) = kinds(operand) else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                let granted = self.signals.grant.as_ref().map_or(0, |g| g.bits());
                let value = if bits == 0 {
                    result(1, 2, 0)
                } else if bits & !granted != 0 {
                    result(1, 1, 0)
                } else if let Some(cause) = cancel {
                    result(1, 0, cause)
                } else if let Some(index) = self
                    .signals
                    .pending
                    .iter()
                    .position(|k| bits & k.bit() != 0)
                {
                    result(0, self.signals.pending.remove(index).ordinal(), 0)
                } else {
                    return Some(OpOutcome::Pending);
                };
                Some(OpOutcome::Value(value))
            }
            _ => None,
        }
    }
}
