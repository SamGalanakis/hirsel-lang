//! The `io.signals` grant (CAP-1): the host signals a run observes, and the scripted
//! deliveries of `signals.script@1`. Validation is hirsel-run-config's.
use serde::{Deserialize, Serialize};

/// A host signal a program can observe, in its docs/spec/core-library.md `SignalKind`
/// variant order. Closed: other host signals are never observed.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    Interrupt,
    Terminate,
    Hangup,
}
impl SignalKind {
    pub const ALL: [SignalKind; 3] = [Self::Interrupt, Self::Terminate, Self::Hangup];
    /// The record spelling, which also orders a grant's kinds.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "interrupt",
            Self::Terminate => "terminate",
            Self::Hangup => "hangup",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
    /// The variant ordinal, which is also the kind's bit in a set of kinds.
    pub const fn ordinal(self) -> u32 {
        self as u32
    }
    pub fn from_ordinal(ordinal: u32) -> Option<Self> {
        Self::ALL.get(ordinal as usize).copied()
    }
    pub const fn bit(self) -> u8 {
        1 << self as u8
    }
    /// The Linux signal number.
    pub const fn number(self) -> i32 {
        match self {
            Self::Interrupt => 2,
            Self::Terminate => 15,
            Self::Hangup => 1,
        }
    }
}

/// The kinds of a set given as bits (`SignalKind::bit`), in variant order.
pub fn signal_kinds(bits: u8) -> impl Iterator<Item = SignalKind> {
    SignalKind::ALL
        .into_iter()
        .filter(move |kind| bits & kind.bit() != 0)
}

/// `io.signals`: the provider, the kinds the run observes (sorted by name, unique,
/// nonempty) and, for `signals.script@1` only, the scripted arrivals in delivery order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalsGrant {
    pub provider: String,
    pub kinds: Vec<SignalKind>,
    /// Required for signals.script@1, forbidden for every other provider.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub script: Option<Vec<SignalKind>>,
}
impl SignalsGrant {
    /// The observed kinds as bits (`SignalKind::bit`).
    pub fn bits(&self) -> u8 {
        self.kinds.iter().fold(0, |bits, kind| bits | kind.bit())
    }
}

// Missing is None; an explicit null is not a script, under any provider.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    de: D,
) -> Result<Option<Vec<T>>, D::Error> {
    Vec::deserialize(de).map(Some)
}
