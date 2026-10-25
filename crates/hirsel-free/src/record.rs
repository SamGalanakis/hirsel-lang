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

/// Where an encoder writes: a byte vector, or a section's reserved log room
/// ([`crate::section::LogOut`]).
pub trait Out {
    fn byte(&mut self, byte: u8);
    fn bytes(&mut self, bytes: &[u8]);
}

impl Out for Vec<u8> {
    fn byte(&mut self, byte: u8) {
        self.push(byte);
    }
    fn bytes(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }
}

impl Out for crate::section::LogOut<'_> {
    fn byte(&mut self, byte: u8) {
        self.push(byte);
    }
    fn bytes(&mut self, bytes: &[u8]) {
        self.extend(bytes);
    }
}

/// The most bytes one record without a payload takes: the tag, a varint small value, the op
/// delta and a 128-bit field.
pub const MAX_FIXED: usize = 1 + 10 + 10 + 19;
/// A payload record's bytes besides the payload: the tag, the op delta and the length.
pub const PAYLOAD_HEAD: usize = 1 + 10 + 10;

/// The most label bytes (an outcome such as `"close receiver"` or `"canceled 7"`) one writer
/// call names. Key and sink names and output bytes are bounded explicitly.
pub const LABEL: usize = 32;

/// Everything one phase may write, computed before its section opens: record bytes for the
/// tape and, for an optional observer, diagnostic notes and their label bytes. A phase's
/// writer enforces it, so a bound that is short fails at once instead of allocating.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecordBound {
    pub bytes: usize,
    pub notes: usize,
    pub text: usize,
}

impl RecordBound {
    /// Room for `n` writer calls without a payload: each writes at most one fixed record and
    /// one note with a label.
    pub const fn records(n: usize) -> RecordBound {
        RecordBound {
            bytes: n * MAX_FIXED,
            notes: n,
            text: n * LABEL,
        }
    }
    /// Add room for one observation or completion of `len` payload bytes.
    pub const fn payload(self, len: usize) -> RecordBound {
        RecordBound {
            bytes: self.bytes + PAYLOAD_HEAD + len,
            ..self
        }
    }
    /// Add room for `n` more writer calls without a payload.
    pub const fn and(self, n: usize) -> RecordBound {
        let more = RecordBound::records(n);
        RecordBound {
            bytes: self.bytes + more.bytes,
            notes: self.notes + more.notes,
            text: self.text + more.text,
        }
    }
    /// Add room for one key position on a key named with `name` bytes.
    pub const fn key(self, name: usize) -> RecordBound {
        RecordBound {
            text: self.text + name,
            ..self.and(1)
        }
    }
    /// Add room for one output note of `len` bytes to a sink named with `sink` bytes.
    pub const fn output(self, sink: usize, len: usize) -> RecordBound {
        RecordBound {
            notes: self.notes + 1,
            text: self.text + sink + len,
            ..self
        }
    }
}

pub fn put_varint(out: &mut impl Out, mut value: u128) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.byte(byte);
            return;
        }
        out.byte(byte | 0x80);
    }
}

/// A cursor over encoded bytes.
#[derive(Clone)]
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
    #[inline]
    pub fn byte(&mut self) -> Result<u8, String> {
        let byte = *self.bytes.get(self.at).ok_or("truncated record")?;
        self.at += 1;
        Ok(byte)
    }
    #[inline]
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
    #[inline]
    pub fn u64(&mut self) -> Result<u64, String> {
        u64::try_from(self.varint()?).map_err(|_| "value out of range".into())
    }
    #[inline]
    pub fn u32(&mut self) -> Result<u32, String> {
        u32::try_from(self.varint()?).map_err(|_| "value out of range".into())
    }
    #[inline]
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
    fn head(&mut self, out: &mut impl Out, kind: u8, small: u64, op: u64) {
        assert!(
            op >= self.op,
            "records out of op order: {op} after {}",
            self.op
        );
        if small < 15 {
            out.byte(kind | (small as u8) << 4);
        } else {
            out.byte(kind | 15 << 4);
            put_varint(out, u128::from(small - 15));
        }
        put_varint(out, u128::from(op - self.op));
        self.op = op;
    }

    /// An observation (`completion` false) or a completion of borrowed payload bytes: the
    /// same bytes as encoding the [`Record`], without building one.
    pub fn payload(&mut self, completion: bool, op: u64, bytes: &[u8], out: &mut impl Out) {
        let kind = if completion { COMPLETION } else { OBSERVATION };
        self.head(out, kind, 0, op);
        put_varint(out, bytes.len() as u128);
        out.bytes(bytes);
    }

    pub fn encode(&mut self, record: &Record, out: &mut impl Out) {
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
            Record::Observation { op, ref bytes } => self.payload(false, op, bytes, out),
            Record::Completion { op, ref bytes } => self.payload(true, op, bytes, out),
            Record::Blocked { op } => self.head(out, BLOCKED, 0, op),
            Record::Ready { op } => self.head(out, READY, 0, op),
        }
    }
}

/// Decode one task's whole log.
pub fn decode(bytes: &[u8]) -> Result<Vec<Record>, String> {
    Decoder::new(bytes).collect()
}

/// One task's log, decoded a record at a time from borrowed bytes. A decoder resumes at any
/// record boundary it reported ([`Decoder::at`]), so a reader can retain the bytes and decode
/// only the records it needs; after an error it yields nothing more.
#[derive(Clone)]
pub struct Decoder<'a> {
    reader: Reader<'a>,
    op: u64,
}

impl<'a> Decoder<'a> {
    pub fn new(bytes: &'a [u8]) -> Decoder<'a> {
        Decoder::at(bytes, Resume::default())
    }

    /// Resume at a boundary an earlier decoder of the same bytes reported.
    pub fn at(bytes: &'a [u8], resume: Resume) -> Decoder<'a> {
        Decoder {
            reader: Reader {
                bytes,
                at: resume.at,
            },
            op: resume.op,
        }
    }

    /// Where the next record starts: its offset, and the op its delta is coded against.
    pub fn resume(&self) -> Resume {
        Resume {
            at: self.reader.at,
            op: self.op,
        }
    }

    #[inline]
    fn record(&mut self) -> Result<Record, String> {
        let reader = &mut self.reader;
        let tag = reader.byte()?;
        let small = match u64::from(tag >> 4) {
            15 => reader.u64()?.checked_add(15).ok_or("value out of range")?,
            small => small,
        };
        let op = self
            .op
            .checked_add(reader.u64()?)
            .ok_or("op out of range")?;
        self.op = op;
        Ok(match tag & 15 {
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
        })
    }
}

/// A record boundary in a task's log.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Resume {
    at: usize,
    op: u64,
}

impl Resume {
    /// The byte offset of the boundary.
    pub fn offset(&self) -> usize {
        self.at
    }
}

impl Iterator for Decoder<'_> {
    type Item = Result<Record, String>;

    #[inline]
    fn next(&mut self) -> Option<Result<Record, String>> {
        if self.reader.done() {
            return None;
        }
        let record = self.record();
        if record.is_err() {
            self.reader.at = self.reader.bytes.len();
        }
        Some(record)
    }
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
