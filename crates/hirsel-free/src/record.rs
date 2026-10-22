//! The compressed records of a free-mode run (design §5 What is recorded) and their bytes.
//!
//! Each task appends records to its own log, in op order; every record names its op ordinal,
//! delta-coded against the task's previous record. A sub-event on a shared object records its
//! **gap** only when the gap is nonzero: a record is written in the section of the phase it
//! belongs to, so a cut that admits a phase holds that phase's records, and an absent gap
//! record of an admitted sub-event is exactly gap 0. Uncontended object ops record nothing.
//!
//! A record is a tag byte (the kind in the low nibble; a small value in the high nibble, 15
//! meaning "15 plus a varint follows"), the op delta as a varint, then the kind's fields.

/// Object identity: (creator spawn path, creator op ordinal), or a `serial` package's gate.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ObjectId {
    Made { path: Vec<u32>, op: u64 },
    Gate(String),
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObjectId::Made { path, op } => write!(f, "{path:?}#{op}"),
            ObjectId::Gate(package) => write!(f, "gate {package}"),
        }
    }
}

/// One compressed record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Record {
    /// Sub-event `sub` of op `op` on a shared object had `gap` (> 0) other tasks' sub-events
    /// on the object since this task's previous one (since creation for its first).
    Gap {
        op: u64,
        sub: u32,
        gap: u64,
    },
    /// An atomic load's value, as its cell's bits.
    Load {
        op: u64,
        bits: u128,
    },
    /// `task.await_any`'s member, as its add-order index.
    AwaitAny {
        op: u64,
        index: u64,
    },
    /// The task's observed cancellation changed to `cause` at an unshielded check in op `op`.
    Cancel {
        op: u64,
        /// Zero-based unshielded check within this op, across all its phases.
        check: u64,
        cause: Option<u32>,
    },
    /// A `scope_until` deadline expired (rather than its scope exiting first) at op `op`.
    Expired {
        op: u64,
    },
    /// `shared.thaw`'s outcome.
    Thaw {
        op: u64,
        ok: bool,
    },
    /// A host request's entered phase: it is `Prepared` as request `id`.
    Prepared {
        op: u64,
        id: u64,
    },
    /// One position on host key `key`: its epoch, as the gap from this task's previous epoch on
    /// the key (from epoch 0), and its mode.
    Key {
        op: u64,
        key: u32,
        shared: bool,
        gap: u64,
    },
    /// A host observation at an ended phase (engine bytes).
    Observation {
        op: u64,
        bytes: Vec<u8>,
    },
    /// A host request's completion at its done phase (engine bytes).
    Completion {
        op: u64,
        bytes: Vec<u8>,
    },
    /// A host wait parked (`blocked`) or became ready, at op `op`.
    Blocked {
        op: u64,
    },
    Ready {
        op: u64,
    },
}

impl Record {
    pub fn op(&self) -> u64 {
        match *self {
            Record::Gap { op, .. }
            | Record::Load { op, .. }
            | Record::AwaitAny { op, .. }
            | Record::Cancel { op, .. }
            | Record::Expired { op }
            | Record::Thaw { op, .. }
            | Record::Prepared { op, .. }
            | Record::Key { op, .. }
            | Record::Observation { op, .. }
            | Record::Completion { op, .. }
            | Record::Blocked { op }
            | Record::Ready { op } => op,
        }
    }
}

const GAP: u8 = 0;
const LOAD: u8 = 1;
const ANY: u8 = 2;
const CANCEL: u8 = 3;
const EXPIRED: u8 = 4;
const THAW: u8 = 5;
const PREPARED: u8 = 6;
const KEY: u8 = 7;
const OBSERVATION: u8 = 8;
const COMPLETION: u8 = 9;
const BLOCKED: u8 = 10;
const READY: u8 = 11;

pub fn put_varint(out: &mut Vec<u8>, mut value: u128) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// A cursor over encoded bytes.
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }
    pub fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
    pub fn byte(&mut self) -> Result<u8, String> {
        let byte = *self.bytes.get(self.at).ok_or("truncated record")?;
        self.at += 1;
        Ok(byte)
    }
    pub fn varint(&mut self) -> Result<u128, String> {
        let mut value = 0u128;
        let mut shift = 0;
        loop {
            let byte = self.byte()?;
            if shift > 126 || (shift == 126 && byte & 0x7c != 0) {
                return Err("varint overflow".into());
            }
            value |= u128::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }
    pub fn u64(&mut self) -> Result<u64, String> {
        u64::try_from(self.varint()?).map_err(|_| "value out of range".into())
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        u32::try_from(self.varint()?).map_err(|_| "value out of range".into())
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&end| end <= self.bytes.len())
            .ok_or("truncated record")?;
        let bytes = &self.bytes[self.at..end];
        self.at = end;
        Ok(bytes)
    }
}

/// A task's record encoder: ops are delta-coded against its previous record.
#[derive(Clone, Default)]
pub struct Encoder {
    op: u64,
}

impl Encoder {
    fn head(&mut self, out: &mut Vec<u8>, kind: u8, small: u64, op: u64) {
        assert!(
            op >= self.op,
            "records out of op order: {op} after {}",
            self.op
        );
        if small < 15 {
            out.push(kind | (small as u8) << 4);
        } else {
            out.push(kind | 15 << 4);
            put_varint(out, u128::from(small - 15));
        }
        put_varint(out, u128::from(op - self.op));
        self.op = op;
    }

    pub fn encode(&mut self, record: &Record, out: &mut Vec<u8>) {
        match *record {
            Record::Gap { op, sub, gap } => {
                assert!(gap > 0, "a zero gap is never recorded");
                self.head(out, GAP, gap, op);
                put_varint(out, u128::from(sub));
            }
            Record::Load { op, bits } => {
                self.head(out, LOAD, 0, op);
                put_varint(out, bits);
            }
            Record::AwaitAny { op, index } => self.head(out, ANY, index, op),
            Record::Cancel { op, check, cause } => {
                self.head(
                    out,
                    CANCEL,
                    cause.map_or(0, |cause| u64::from(cause) + 1),
                    op,
                );
                put_varint(out, u128::from(check));
            }
            Record::Expired { op } => self.head(out, EXPIRED, 0, op),
            Record::Thaw { op, ok } => self.head(out, THAW, u64::from(ok), op),
            Record::Prepared { op, id } => {
                self.head(out, PREPARED, 0, op);
                put_varint(out, u128::from(id));
            }
            Record::Key {
                op,
                key,
                shared,
                gap,
            } => {
                self.head(out, KEY, gap, op);
                put_varint(out, u128::from(key) << 1 | u128::from(shared));
            }
            Record::Observation { op, ref bytes } => {
                self.head(out, OBSERVATION, 0, op);
                put_varint(out, bytes.len() as u128);
                out.extend_from_slice(bytes);
            }
            Record::Completion { op, ref bytes } => {
                self.head(out, COMPLETION, 0, op);
                put_varint(out, bytes.len() as u128);
                out.extend_from_slice(bytes);
            }
            Record::Blocked { op } => self.head(out, BLOCKED, 0, op),
            Record::Ready { op } => self.head(out, READY, 0, op),
        }
    }
}

/// Decode one task's whole log.
pub fn decode(bytes: &[u8]) -> Result<Vec<Record>, String> {
    let mut reader = Reader::new(bytes);
    let mut op = 0u64;
    let mut out = Vec::new();
    while !reader.done() {
        let tag = reader.byte()?;
        let small = match u64::from(tag >> 4) {
            15 => reader.u64()?.checked_add(15).ok_or("value out of range")?,
            small => small,
        };
        op = op.checked_add(reader.u64()?).ok_or("op out of range")?;
        out.push(match tag & 15 {
            GAP if small > 0 => Record::Gap {
                op,
                sub: reader.u32()?,
                gap: small,
            },
            LOAD if small == 0 => Record::Load {
                op,
                bits: reader.varint()?,
            },
            ANY => Record::AwaitAny { op, index: small },
            CANCEL => Record::Cancel {
                op,
                check: reader.u64()?,
                cause: match small {
                    0 => None,
                    n => Some(u32::try_from(n - 1).map_err(|_| "cause out of range")?),
                },
            },
            EXPIRED if small == 0 => Record::Expired { op },
            THAW if small < 2 => Record::Thaw { op, ok: small == 1 },
            PREPARED if small == 0 => Record::Prepared {
                op,
                id: reader.u64()?,
            },
            KEY => {
                let word = reader.u64()?;
                let key = u32::try_from(word >> 1).map_err(|_| "key out of range")?;
                Record::Key {
                    op,
                    key,
                    shared: word & 1 == 1,
                    gap: small,
                }
            }
            kind @ (OBSERVATION | COMPLETION) if small == 0 => {
                let n = usize::try_from(reader.u64()?).map_err(|_| "length out of range")?;
                let bytes = reader.take(n)?.to_vec();
                if kind == OBSERVATION {
                    Record::Observation { op, bytes }
                } else {
                    Record::Completion { op, bytes }
                }
            }
            BLOCKED if small == 0 => Record::Blocked { op },
            READY if small == 0 => Record::Ready { op },
            kind => return Err(format!("bad record kind {kind} (value {small})")),
        });
    }
    Ok(out)
}

/// Owner-local check cursor. A changed cancellation names its check, so a
/// later retry cannot replace an earlier unchanged observation in the same op.
#[derive(Default)]
pub(crate) struct CheckIndex {
    op: u64,
    next: u64,
}
impl CheckIndex {
    pub(crate) fn peek(&self, op: u64) -> u64 {
        if self.op == op { self.next } else { 0 }
    }
    pub(crate) fn take(&mut self, op: u64) -> u64 {
        let check = self.peek(op);
        self.op = op;
        self.next = check.checked_add(1).expect("cancellation check overflow");
        check
    }
}
