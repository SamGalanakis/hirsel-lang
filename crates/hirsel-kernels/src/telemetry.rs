//! Telemetry capture (`telemetry.capture@1` and `telemetry.null@1`): the payload bounds,
//! the parent encoding and the span identities every engine derives alike.
//!
//! A parent is the 25-byte prefix `trace_id (16) span_id (8) flags (1)` followed by an
//! opaque tail (std's encoded tracestate). All-zero IDs name no parent: the span is a new
//! root. A span's context has the parent's layout, its own IDs and the parent's flags and
//! tail, so a child's context is a valid parent for the next span.
use std::collections::BTreeSet;

/// The largest payload one telemetry argument may carry, in bytes.
pub const PAYLOAD_LIMIT: usize = 65536;
/// The parent's fixed prefix: trace ID, span ID and flags.
pub const PARENT_PREFIX: usize = 25;

/// A deterministic refusal, checked before any request; ordinals follow core
/// `TelemetryError`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    TooLarge,
    InvalidParent,
}
impl Refusal {
    pub const fn ordinal(self) -> u32 {
        match self {
            Self::TooLarge => 0,
            Self::InvalidParent => 1,
        }
    }
}

/// Every payload of one call fits the bound.
pub fn bounded(payloads: &[&[u8]]) -> Result<(), Refusal> {
    if payloads.iter().any(|p| p.len() > PAYLOAD_LIMIT) {
        Err(Refusal::TooLarge)
    } else {
        Ok(())
    }
}

/// The parent's trace ID, or None for a root. Mixed zero and nonzero IDs, or a short
/// prefix, are `InvalidParent`.
pub fn parent(bytes: &[u8]) -> Result<Option<[u8; 16]>, Refusal> {
    if bytes.len() > PAYLOAD_LIMIT {
        return Err(Refusal::TooLarge);
    }
    if bytes.len() < PARENT_PREFIX {
        return Err(Refusal::InvalidParent);
    }
    let trace: [u8; 16] = bytes[..16].try_into().unwrap();
    let span = &bytes[16..24];
    match (trace == [0; 16], span == [0; 8]) {
        (true, true) => Ok(None),
        (false, false) => Ok(Some(trace)),
        _ => Err(Refusal::InvalidParent),
    }
}

/// A begun span's context: its IDs, then the parent's flags and tail.
pub fn context(trace: &[u8; 16], span: &[u8; 8], parent: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(parent.len().max(PARENT_PREFIX));
    bytes.extend_from_slice(trace);
    bytes.extend_from_slice(span);
    bytes.extend_from_slice(&parent[24..]);
    bytes
}

/// The identities a run has delivered. A new root's trace ID and every span ID are
/// unique within the run; a provider retries a collision before delivering.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Issued {
    traces: BTreeSet<[u8; 16]>,
    spans: BTreeSet<[u8; 8]>,
}
impl Issued {
    /// Whether a provider may deliver `trace` and `span` for a span under `parent`: both
    /// nonzero, a child keeps its parent's trace, a root's trace and every span are new.
    pub fn admits(&self, parent: Option<[u8; 16]>, trace: &[u8; 16], span: &[u8; 8]) -> bool {
        *trace != [0; 16]
            && *span != [0; 8]
            && !self.spans.contains(span)
            && match parent {
                Some(parent) => parent == *trace,
                None => !self.traces.contains(trace),
            }
    }
    /// Record identities [`admits`](Self::admits) accepted.
    pub fn accept(&mut self, parent: Option<[u8; 16]>, trace: [u8; 16], span: [u8; 8]) {
        if parent.is_none() {
            self.traces.insert(trace);
        }
        self.spans.insert(span);
    }
    /// The first identities `draw` supplies that [`admits`](Self::admits) accepts: a root
    /// takes 24 drawn bytes (trace, then span), a child 8 (span).
    pub fn choose(
        &self,
        parent: Option<[u8; 16]>,
        mut draw: impl FnMut(usize) -> Vec<u8>,
    ) -> ([u8; 16], [u8; 8]) {
        loop {
            let (trace, span) = match parent {
                Some(trace) => (trace, draw(8).try_into().unwrap()),
                None => {
                    let bytes = draw(24);
                    (
                        bytes[..16].try_into().unwrap(),
                        bytes[16..].try_into().unwrap(),
                    )
                }
            };
            if self.admits(parent, &trace, &span) {
                return (trace, span);
            }
        }
    }
}

/// `telemetry.null@1`'s identity stream: successive SplitMix64 words of the grant's seed,
/// each little-endian, one word per started eight bytes, exactly as `random.splitmix64@1`
/// draws (a separate stream: it never advances the run's Random seed).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NullIds(u64);
impl NullIds {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn draw(&mut self, count: usize) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(count + 7);
        while bytes.len() < count {
            self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            bytes.extend_from_slice(&(z ^ (z >> 31)).to_le_bytes());
        }
        bytes.truncate(count);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> Vec<u8> {
        vec![0; PARENT_PREFIX]
    }

    #[test]
    fn parents_are_roots_children_or_refused() {
        assert_eq!(parent(&root()), Ok(None));
        let mut child = vec![1; PARENT_PREFIX];
        child.extend_from_slice(b"state");
        assert_eq!(parent(&child), Ok(Some([1; 16])));
        assert_eq!(parent(&[0; 24]), Err(Refusal::InvalidParent));
        let mut mixed = root();
        mixed[16] = 1;
        assert_eq!(parent(&mixed), Err(Refusal::InvalidParent));
        let mut mixed = root();
        mixed[0] = 1;
        assert_eq!(parent(&mixed), Err(Refusal::InvalidParent));
        assert_eq!(parent(&vec![1; PAYLOAD_LIMIT + 1]), Err(Refusal::TooLarge));
        assert_eq!(bounded(&[&[0; PAYLOAD_LIMIT]]), Ok(()));
        assert_eq!(
            bounded(&[&[], &vec![0; PAYLOAD_LIMIT + 1]]),
            Err(Refusal::TooLarge)
        );
    }

    /// The null stream is `random.splitmix64@1`'s: seed 0 starts with the words
    /// 0xe220a8397b1dcdaf, 0x6e789e6aa1b965f4 (the CAP-1 random vectors).
    #[test]
    fn null_ids_follow_splitmix64() {
        let mut ids = NullIds::new(0);
        let issued = Issued::default();
        let (trace, span) = issued.choose(None, |n| ids.draw(n));
        assert_eq!(trace[..8], 0xe220a8397b1dcdafu64.to_le_bytes());
        assert_eq!(trace[8..], 0x6e789e6aa1b965f4u64.to_le_bytes());
        assert_eq!(span, 0x06c45d188009454fu64.to_le_bytes());
        let mut flags = root();
        flags[24] = 1;
        flags.extend_from_slice(b"tail");
        let ctx = context(&trace, &span, &flags);
        assert_eq!(&ctx[..16], &trace);
        assert_eq!(&ctx[16..24], &span);
        assert_eq!(&ctx[24..], b"\x01tail");
        assert_eq!(parent(&ctx), Ok(Some(trace)));
    }

    #[test]
    fn collisions_and_zero_ids_are_redrawn() {
        let mut issued = Issued::default();
        issued.accept(None, [1; 16], [2; 8]);
        // A child keeps its parent's trace; zero and reused spans are redrawn.
        let mut draws = vec![vec![0; 8], vec![2; 8], vec![3; 8]].into_iter();
        let (trace, span) = issued.choose(Some([1; 16]), |_| draws.next().unwrap());
        assert_eq!((trace, span), ([1; 16], [3; 8]));
        assert!(!issued.admits(Some([1; 16]), &[4; 16], &[5; 8]));
        // A root's trace must be new.
        assert!(!issued.admits(None, &[1; 16], &[6; 8]));
        assert!(issued.admits(None, &[7; 16], &[6; 8]));
    }
}
