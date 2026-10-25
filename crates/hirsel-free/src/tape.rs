//! The free-mode tape: a header, then one chunk per cut.
//!
//! Each flush appends a **flush chunk** (the scanned tasks' progress at the flush's cut and
//! the log bytes they appended since the previous cut) and the run's end appends a **stop
//! chunk** (the same for the terminal cut with every task's progress, the run's outcome and
//! the stopping task). Chunks are framed (length, CRC-32C, kind), so a tape torn by a crash
//! reads back as its last complete cut: a **truncated tape**, admitted up to that flush's cut.
//! Damage before the final frame is corruption.

use crate::phase::Progress;
use crate::record::{Reader, put_varint};
use crate::section::Closed;
use std::collections::BTreeMap;

pub const MAGIC: &[u8] = b"hirsel-free-tape 2\n";
const HEADER: u8 = 1;
const FLUSH: u8 = 2;
const STOP: u8 = 3;

/// One scanned task in a chunk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskChunk {
    pub path: Vec<u32>,
    pub progress: Progress,
    pub bytes: Vec<u8>,
}

/// How a run ended, as its terminal cut records it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct End {
    /// The engine's outcome text (a stop's cause, `blocked`, or the entry's outcome).
    pub outcome: String,
    /// The task whose stop won, if a task stopped the run.
    pub task: Option<Vec<u32>>,
}

/// The CRC-32C (Castagnoli) of `bytes`.
pub fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                crc >> 1 ^ 0x82f6_3b78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// One chunk, encoded in place behind room for its frame header. The header (length, CRC) is
/// written only once the payload is complete, and the caller appends the whole frame at once
/// (rr `syscallbuf.c` `commit_raw_syscall`: a record's size is committed only after the record).
struct Chunk(Vec<u8>);

impl Chunk {
    fn new(kind: u8, capacity: usize) -> Chunk {
        let mut out = Vec::with_capacity(8 + 1 + capacity);
        out.extend_from_slice(&[0; 8]);
        out.push(kind);
        Chunk(out)
    }
    fn frame(mut self) -> Vec<u8> {
        let len = u32::try_from(self.0.len() - 8).expect("a chunk fits a frame");
        let crc = crc32c(&self.0[8..]);
        self.0[0..4].copy_from_slice(&len.to_le_bytes());
        self.0[4..8].copy_from_slice(&crc.to_le_bytes());
        self.0
    }
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_varint(out, bytes.len() as u128);
    out.extend_from_slice(bytes);
}

fn get_bytes<'a>(reader: &mut Reader<'a>) -> Result<&'a [u8], String> {
    let n = usize::try_from(reader.u64()?).map_err(|_| "length out of range")?;
    reader.take(n)
}

fn put_path(out: &mut Vec<u8>, path: &[u32]) {
    put_varint(out, path.len() as u128);
    for &step in path {
        put_varint(out, u128::from(step));
    }
}

fn get_path(reader: &mut Reader<'_>) -> Result<Vec<u32>, String> {
    let n = reader.u64()?;
    (0..n).map(|_| reader.u32()).collect()
}

/// The header: the magic line and the engine's run binding.
pub fn header(binding: &[u8]) -> Vec<u8> {
    let mut chunk = Chunk::new(HEADER, binding.len());
    chunk.0.extend_from_slice(binding);
    let mut out = MAGIC.to_vec();
    out.extend(chunk.frame());
    out
}

/// A task as a chunk lists it: owned ([`TaskChunk`]) or read in place from a closed cut
/// ([`Closed`]).
pub trait ChunkTask {
    fn path(&self) -> &[u32];
    fn progress(&self) -> Progress;
    /// The number of new log bytes.
    fn byte_len(&self) -> usize;
    /// Append exactly [`byte_len`](Self::byte_len) new log bytes.
    fn bytes(&self, out: &mut Vec<u8>);
}

impl ChunkTask for TaskChunk {
    fn path(&self) -> &[u32] {
        &self.path
    }
    fn progress(&self) -> Progress {
        self.progress
    }
    fn byte_len(&self) -> usize {
        self.bytes.len()
    }
    fn bytes(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.bytes);
    }
}

impl ChunkTask for Closed<'_> {
    fn path(&self) -> &[u32] {
        Closed::path(self)
    }
    fn progress(&self) -> Progress {
        self.progress
    }
    fn byte_len(&self) -> usize {
        self.len()
    }
    fn bytes(&self, out: &mut Vec<u8>) {
        self.slices(|bytes| out.extend_from_slice(bytes));
    }
}

/// Room for a varint of up to `bits` significant bits.
const fn varint(bits: usize) -> usize {
    bits.div_ceil(7)
}

fn tasks_chunk<T: ChunkTask>(kind: u8, keys: &[String], tasks: &[T], extra: usize) -> Chunk {
    let room = varint(64) * 2
        + extra
        + keys.iter().map(|key| varint(64) + key.len()).sum::<usize>()
        + tasks
            .iter()
            .map(|task| varint(64) * 3 + varint(32) * task.path().len() + task.byte_len())
            .sum::<usize>();
    let mut chunk = Chunk::new(kind, room);
    let out = &mut chunk.0;
    put_varint(out, keys.len() as u128);
    for key in keys {
        put_bytes(out, key.as_bytes());
    }
    put_varint(out, tasks.len() as u128);
    for task in tasks {
        put_path(out, task.path());
        put_varint(out, u128::from(task.progress().pack()));
        put_varint(out, task.byte_len() as u128);
        let at = out.len();
        task.bytes(out);
        assert_eq!(
            out.len() - at,
            task.byte_len(),
            "a task lists its declared bytes"
        );
    }
    chunk
}

/// A flush chunk: `keys` are the host keys first named since the previous chunk.
pub fn flush<T: ChunkTask>(keys: &[String], tasks: &[T]) -> Vec<u8> {
    tasks_chunk(FLUSH, keys, tasks, 0).frame()
}

/// The stop chunk: `tasks` lists every task, with its new bytes.
pub fn stop<T: ChunkTask>(keys: &[String], tasks: &[T], end: &End) -> Vec<u8> {
    let extra = varint(64) * 2
        + end.outcome.len()
        + 1
        + end.task.as_ref().map_or(0, |path| varint(32) * path.len());
    let mut chunk = tasks_chunk(STOP, keys, tasks, extra);
    let out = &mut chunk.0;
    put_bytes(out, end.outcome.as_bytes());
    match &end.task {
        None => out.push(0),
        Some(path) => {
            out.push(1);
            put_path(out, path);
        }
    }
    chunk.frame()
}

/// A tape read back.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Tape {
    pub binding: Vec<u8>,
    /// Host keys by index.
    pub keys: Vec<String>,
    /// Each task's whole log.
    pub logs: BTreeMap<Vec<u32>, Vec<u8>>,
    /// The cut admission follows: the terminal cut, or the last flush's.
    pub cut: BTreeMap<Vec<u32>, Progress>,
    /// `None` for a truncated tape.
    pub end: Option<End>,
    /// A torn final frame was cut.
    pub torn: bool,
}

/// Read a tape; a torn final frame is cut, earlier damage refuses.
pub fn parse(bytes: &[u8]) -> Result<Tape, String> {
    let rest = bytes.strip_prefix(MAGIC).ok_or("not a free-mode tape")?;
    let mut frames = Vec::new();
    let mut at = 0;
    let mut torn = false;
    while at < rest.len() {
        let tail = &rest[at..];
        let len =
            (tail.len() >= 8).then(|| u32::from_le_bytes(tail[0..4].try_into().unwrap()) as usize);
        if let Some(len) = len.filter(|&len| len >= 1 && tail.len() - 8 >= len) {
            let body = &tail[8..8 + len];
            if crc32c(body) == u32::from_le_bytes(tail[4..8].try_into().unwrap()) {
                frames.push(body);
                at += 8 + len;
                continue;
            }
            // Only the final frame may be torn: a damaged frame with bytes after it is not.
            if tail.len() - 8 > len {
                return Err("a damaged frame before the end of the tape".into());
            }
        }
        torn = true;
        break;
    }
    let mut tape = Tape {
        torn,
        ..Tape::default()
    };
    let mut frames = frames.into_iter();
    match frames.next() {
        Some([HEADER, binding @ ..]) => tape.binding = binding.to_vec(),
        _ => return Err("the tape has no header".into()),
    }
    for frame in frames {
        if tape.end.is_some() {
            return Err("a chunk after the stop chunk".into());
        }
        let (&kind, payload) = frame.split_first().unwrap();
        if kind != FLUSH && kind != STOP {
            return Err(format!("bad chunk kind {kind}"));
        }
        let mut reader = Reader::new(payload);
        for _ in 0..reader.u64()? {
            let key = String::from_utf8(get_bytes(&mut reader)?.to_vec()).map_err(|_| "bad key")?;
            tape.keys.push(key);
        }
        for _ in 0..reader.u64()? {
            let path = get_path(&mut reader)?;
            let progress = Progress::unpack(reader.u64()?).ok_or("bad progress")?;
            let bytes = get_bytes(&mut reader)?;
            tape.logs
                .entry(path.clone())
                .or_default()
                .extend_from_slice(bytes);
            tape.cut.insert(path, progress);
        }
        if kind == STOP {
            let outcome =
                String::from_utf8(get_bytes(&mut reader)?.to_vec()).map_err(|_| "bad outcome")?;
            let task = match reader.byte()? {
                0 => None,
                1 => Some(get_path(&mut reader)?),
                _ => return Err("bad stopping task".into()),
            };
            tape.end = Some(End { outcome, task });
        }
        if !reader.done() {
            return Err("trailing bytes in a chunk".into());
        }
    }
    Ok(tape)
}
