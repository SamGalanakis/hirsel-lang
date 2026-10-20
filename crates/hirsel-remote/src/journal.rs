//! The host's call journal seam (RUN-11 reconciliation, RUN-13). A journal that is durable
//! records each call key before its callee may run and each decision before its outcome is
//! sent, so a restarted host still knows every call it ever decided: its status answers
//! prove a call never ran, and a call admitted but never decided is `InDoubt(Abandoned())`.
//!
//! - `journal.none@1` (the default): nothing survives the process; a restarted host is a
//!   new incarnation that callers learn of when they reconnect.
//! - `journal.dir@1` (`{"path"}`): an append-only file of length-framed, checksummed
//!   records, synced before each record is acknowledged; a torn tail is dropped on recovery.
//! - `journal.sim@1`: the same records in memory shared with the test, which loses every
//!   record not yet synced when the test crashes the host.
use hirsel_run_schema::CallOutcome;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A stream's identity at a host: the caller's name and the stream bytes it opened.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StreamKey {
    pub caller: String,
    pub stream: [u8; 32],
}

/// One journal record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Entry {
    /// A stream's first session: the caller run instance it belongs to.
    Stream {
        stream: StreamKey,
        instance: [u8; 16],
    },
    /// A request at `position` passed every check and may run.
    Admitted {
        stream: StreamKey,
        position: u64,
        key: [u8; 32],
        digest: [u8; 32],
    },
    /// The position's decision.
    Decided {
        stream: StreamKey,
        position: u64,
        key: [u8; 32],
        digest: [u8; 32],
        end: CallOutcome,
        callee: Option<[u8; 16]>,
    },
    /// The caller holds every outcome up to `position`.
    Acknowledged { stream: StreamKey, position: u64 },
}

/// A host's call journal.
pub trait Journal: Send {
    /// Whether records survive a crash of the host process.
    fn durable(&self) -> bool;
    /// The host incarnation: fixed for a durable journal's whole life, fresh otherwise.
    fn incarnation(&self) -> [u8; 16];
    /// Append `entry`; when this returns, a durable journal keeps it through a crash.
    fn append(&mut self, entry: &Entry) -> io::Result<()>;
    /// Every record a previous incarnation of this journal kept, in order.
    fn recover(&mut self) -> io::Result<Vec<Entry>>;
}

/// `journal.none@1`.
pub struct NoJournal {
    incarnation: [u8; 16],
}
impl NoJournal {
    pub fn new(incarnation: [u8; 16]) -> Self {
        Self { incarnation }
    }
}
impl Journal for NoJournal {
    fn durable(&self) -> bool {
        false
    }
    fn incarnation(&self) -> [u8; 16] {
        self.incarnation
    }
    fn append(&mut self, _: &Entry) -> io::Result<()> {
        Ok(())
    }
    fn recover(&mut self) -> io::Result<Vec<Entry>> {
        Ok(vec![])
    }
}

const MAGIC: &[u8; 8] = b"HRJRNL01";

/// `journal.dir@1`: `<path>/calls.journal`, created with a random incarnation in its header.
pub struct DirJournal {
    file: File,
    path: PathBuf,
    incarnation: [u8; 16],
}
impl DirJournal {
    /// Open or create the journal in `dir`, whose incarnation is `fresh` when it is created.
    pub fn open(dir: &Path, fresh: [u8; 16]) -> io::Result<Self> {
        let path = dir.join("calls.journal");
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(&path)?;
        let mut header = [0u8; 24];
        let read = file.read(&mut header)?;
        let incarnation = if read == 0 {
            file.write_all(MAGIC)?;
            file.write_all(&fresh)?;
            file.sync_all()?;
            File::open(dir)?.sync_all()?;
            fresh
        } else if read == 24 && &header[..8] == MAGIC {
            header[8..].try_into().unwrap()
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a hirsel call journal",
            ));
        };
        Ok(Self {
            file,
            path,
            incarnation,
        })
    }
}
impl Journal for DirJournal {
    fn durable(&self) -> bool {
        true
    }
    fn incarnation(&self) -> [u8; 16] {
        self.incarnation
    }
    fn append(&mut self, entry: &Entry) -> io::Result<()> {
        self.file.write_all(&frame(entry))?;
        self.file.sync_data()
    }
    fn recover(&mut self) -> io::Result<Vec<Entry>> {
        let mut bytes = vec![];
        File::open(&self.path)?.read_to_end(&mut bytes)?;
        let (entries, intact) = unframe(&bytes[24.min(bytes.len())..]);
        // A torn or corrupt tail was never acknowledged: drop it so appends follow the
        // last intact record.
        let keep = 24 + intact;
        if keep < bytes.len() {
            self.file.set_len(keep as u64)?;
            self.file.sync_all()?;
        }
        Ok(entries)
    }
}

/// `journal.sim@1`: records in memory, shared with the test that owns [`SimDisk`].
pub struct SimJournal {
    disk: SimDisk,
    incarnation: [u8; 16],
}
/// The simulated medium: what survived, and what a crash will lose.
#[derive(Clone, Default)]
pub struct SimDisk(Arc<Mutex<SimMedium>>);
#[derive(Default)]
struct SimMedium {
    synced: Vec<Entry>,
    incarnation: Option<[u8; 16]>,
}
impl SimDisk {
    /// A journal over this medium; the first opener fixes its incarnation.
    pub fn journal(&self, fresh: [u8; 16]) -> SimJournal {
        let incarnation = *self.0.lock().unwrap().incarnation.get_or_insert(fresh);
        SimJournal {
            disk: self.clone(),
            incarnation,
        }
    }
    pub fn records(&self) -> Vec<Entry> {
        self.0.lock().unwrap().synced.clone()
    }
}
impl Journal for SimJournal {
    fn durable(&self) -> bool {
        true
    }
    fn incarnation(&self) -> [u8; 16] {
        self.incarnation
    }
    // Every append is synced before it returns, as journal.dir@1's is, so a crash loses
    // nothing an append acknowledged.
    fn append(&mut self, entry: &Entry) -> io::Result<()> {
        self.disk.0.lock().unwrap().synced.push(entry.clone());
        Ok(())
    }
    fn recover(&mut self) -> io::Result<Vec<Entry>> {
        Ok(self.disk.records())
    }
}

// Record layout: u32 length, 32-byte SHA-256 of the body, body. The body is a tag byte and
// fixed-width or length-framed fields.
fn frame(entry: &Entry) -> Vec<u8> {
    let body = encode(entry);
    let mut out = Vec::with_capacity(body.len() + 36);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&Sha256::digest(&body));
    out.extend_from_slice(&body);
    out
}
fn unframe(mut bytes: &[u8]) -> (Vec<Entry>, usize) {
    let mut entries = vec![];
    let mut intact = 0;
    while bytes.len() >= 36 {
        let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let Some(body) = bytes.get(36..36 + len) else {
            break;
        };
        if Sha256::digest(body)[..] != bytes[4..36] {
            break;
        }
        let Some(entry) = decode(body) else {
            break;
        };
        entries.push(entry);
        intact += 36 + len;
        bytes = &bytes[36 + len..];
    }
    (entries, intact)
}

fn put_stream(out: &mut Vec<u8>, stream: &StreamKey) {
    put_bytes(out, stream.caller.as_bytes());
    out.extend_from_slice(&stream.stream);
}
fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}
fn put_end(out: &mut Vec<u8>, end: &CallOutcome) {
    let (tag, payload): (u8, Vec<u8>) = match end {
        CallOutcome::Ok(b) => (0, b.clone()),
        CallOutcome::Failed(b) => (1, b.clone()),
        CallOutcome::Faulted(b) => (2, b.clone()),
        CallOutcome::NotRun(r) => (3, r.to_le_bytes().to_vec()),
        CallOutcome::InDoubt(r) => (4, r.to_le_bytes().to_vec()),
        CallOutcome::Canceled(c) => (5, c.to_le_bytes().to_vec()),
    };
    out.push(tag);
    put_bytes(out, &payload);
}
fn encode(entry: &Entry) -> Vec<u8> {
    let mut out = vec![];
    match entry {
        Entry::Stream { stream, instance } => {
            out.push(0);
            put_stream(&mut out, stream);
            out.extend_from_slice(instance);
        }
        Entry::Admitted {
            stream,
            position,
            key,
            digest,
        } => {
            out.push(1);
            put_stream(&mut out, stream);
            out.extend_from_slice(&position.to_le_bytes());
            out.extend_from_slice(key);
            out.extend_from_slice(digest);
        }
        Entry::Decided {
            stream,
            position,
            key,
            digest,
            end,
            callee,
        } => {
            out.push(2);
            put_stream(&mut out, stream);
            out.extend_from_slice(&position.to_le_bytes());
            out.extend_from_slice(key);
            out.extend_from_slice(digest);
            put_end(&mut out, end);
            match callee {
                None => out.push(0),
                Some(c) => {
                    out.push(1);
                    out.extend_from_slice(c);
                }
            }
        }
        Entry::Acknowledged { stream, position } => {
            out.push(3);
            put_stream(&mut out, stream);
            out.extend_from_slice(&position.to_le_bytes());
        }
    }
    out
}

struct Cursor<'a>(&'a [u8]);
impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }
    fn fixed<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.fixed()?))
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let len = usize::try_from(self.u64()?).ok()?;
        Some(self.take(len)?.to_vec())
    }
    fn stream(&mut self) -> Option<StreamKey> {
        Some(StreamKey {
            caller: String::from_utf8(self.bytes()?).ok()?,
            stream: self.fixed()?,
        })
    }
    fn end(&mut self) -> Option<CallOutcome> {
        let tag = self.fixed::<1>()?[0];
        let payload = self.bytes()?;
        let small = || Some(u32::from_le_bytes(payload.as_slice().try_into().ok()?));
        Some(match tag {
            0 => CallOutcome::Ok(payload),
            1 => CallOutcome::Failed(payload),
            2 => CallOutcome::Faulted(payload),
            3 => CallOutcome::NotRun(small()?),
            4 => CallOutcome::InDoubt(small()?),
            5 => CallOutcome::Canceled(small()?),
            _ => return None,
        })
    }
}
fn decode(body: &[u8]) -> Option<Entry> {
    let mut c = Cursor(body);
    let tag = c.fixed::<1>()?[0];
    let entry = match tag {
        0 => Entry::Stream {
            stream: c.stream()?,
            instance: c.fixed()?,
        },
        1 => Entry::Admitted {
            stream: c.stream()?,
            position: c.u64()?,
            key: c.fixed()?,
            digest: c.fixed()?,
        },
        2 => Entry::Decided {
            stream: c.stream()?,
            position: c.u64()?,
            key: c.fixed()?,
            digest: c.fixed()?,
            end: c.end()?,
            callee: match c.fixed::<1>()?[0] {
                0 => None,
                1 => Some(c.fixed()?),
                _ => return None,
            },
        },
        3 => Entry::Acknowledged {
            stream: c.stream()?,
            position: c.u64()?,
        },
        _ => return None,
    };
    c.0.is_empty().then_some(entry)
}
