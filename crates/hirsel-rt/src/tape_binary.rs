//! Ordinary v8 tapes: a header, event frames and a terminal event count.
//! The same payload codec is used by ordinary and durable recording, in both engines.
use crate::io::Event;
use crate::launch::{RecordedRun, Recording};
use crate::tape_wire::{Decode, Wire, decode};
use hirsel_run_schema::tape::{self, MAGIC};
use std::{io, io::Write};

impl Wire for RecordedRun {
    fn encode(&self, out: &mut Vec<u8>) {
        self.canonical.encode(out);
        // Placement is diagnostic provenance, retained in its own versioned JSON encoding.
        self.placement.as_ref().map(|p| p.to_json()).encode(out);
        self.admitted_at.encode(out);
        self.revocation
            .as_ref()
            .map(|p| (&p.reference, &p.revision, p.revoked))
            .map(|(a, b, c)| (a.clone(), b.clone(), c))
            .encode(out);
        self.instance.encode(out);
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        Ok(Self {
            canonical: Wire::decode(input)?,
            placement: Option::<Vec<u8>>::decode(input)?
                .map(|p| hirsel_run_config::Placement::from_json(&p).map_err(|e| e.to_string()))
                .transpose()?,
            admitted_at: Wire::decode(input)?,
            revocation: Option::<(String, String, bool)>::decode(input)?.map(
                |(reference, revision, revoked)| hirsel_run_config::RevocationEvidence {
                    reference,
                    revision,
                    revoked,
                },
            ),
            instance: Wire::decode(input)?,
        })
    }
}
impl Wire for Recording {
    fn encode(&self, out: &mut Vec<u8>) {
        self.arguments.encode(out);
        self.run_identity.encode(out);
        self.hash_seed.encode(out);
        self.random_seed.encode(out);
        self.test_seed.encode(out);
        self.binding.encode(out);
        self.tree.encode(out);
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        Ok(Self {
            arguments: Wire::decode(input)?,
            run_identity: Wire::decode(input)?,
            hash_seed: Wire::decode(input)?,
            random_seed: Wire::decode(input)?,
            test_seed: Wire::decode(input)?,
            binding: Wire::decode(input)?,
            tree: Wire::decode(input)?,
            events: vec![],
        })
    }
}

/// Validate all frame envelopes without copying payloads. Event ranges remain in
/// one immutable tape buffer; replay may decode just the event it is consuming.
#[derive(Clone, Debug)]
pub struct Tape {
    bytes: std::sync::Arc<Vec<u8>>,
    events: Vec<std::ops::Range<usize>>,
    pub complete: bool,
    /// Confirmed terminal outcome, absent on legacy count-only trailers.
    pub outcome: Option<hirsel_run_schema::RunOutcome>,
    pub valid: usize,
}
impl Tape {
    pub fn read(bytes: std::sync::Arc<Vec<u8>>) -> Result<(Recording, Self), String> {
        if !bytes.starts_with(MAGIC) {
            return Err("not a v8 binary tape".into());
        }
        let header = tape::read(&bytes, MAGIC.len(), 0)?;
        if header.kind != 1 {
            return Err("binary tape without header".into());
        }
        let record: Recording = decode(header.payload)?;
        validate_header(&record)?;
        let mut at = header.range.end;
        let mut events = vec![];
        let mut complete = false;
        let mut outcome = None;
        while at < bytes.len() {
            let frame = tape::read(&bytes, at, events.len() as u64 + 1)?;
            match frame.kind {
                3 if !complete => events.push(at + tape::FRAME..frame.range.end),
                6 if !complete => {
                    let (count, text): (u64, String) = decode(frame.payload)?;
                    if count != events.len() as u64 {
                        return Err("the trailer's event count disagrees".into());
                    }
                    outcome = Some(crate::journal::parse_outcome(&text)?);
                    complete = true;
                }
                5 if !complete => {
                    let count: u64 = decode(frame.payload)?;
                    if count != events.len() as u64 {
                        return Err("the trailer's event count disagrees".into());
                    }
                    complete = true;
                }
                _ => return Err("unexpected binary tape frame".into()),
            }
            at = frame.range.end;
            if complete && at != bytes.len() {
                return Err("bytes after complete recording".into());
            }
        }
        Ok((
            record,
            Self {
                bytes,
                events,
                complete,
                outcome,
                valid: at,
            },
        ))
    }
    pub fn len(&self) -> usize {
        self.events.len()
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
    pub fn event(&self, index: usize) -> Result<Event, String> {
        decode(
            &self.bytes[self
                .events
                .get(index)
                .ok_or("missing binary tape event")?
                .clone()],
        )
    }
    pub fn events(&self) -> Result<Vec<Event>, String> {
        (0..self.len()).map(|i| self.event(i)).collect()
    }
}

pub fn parse_prefix(bytes: &[u8]) -> Result<(Recording, bool), String> {
    if !bytes.starts_with(MAGIC) {
        return crate::launch::parse_recording_prefix(
            std::str::from_utf8(bytes).map_err(|_| "bad recording UTF-8")?,
        );
    }
    let (mut record, tape) = Tape::read(bytes.to_vec().into())?;
    record.events = tape.events()?;
    Ok((record, tape.complete))
}
pub fn parse(bytes: &[u8]) -> Result<Recording, String> {
    let (record, complete) = parse_prefix(bytes)?;
    if !complete {
        return Err("incomplete ordinary recording: unknown suffix".into());
    }
    Ok(record)
}
/// Text exists only at the inspection boundary (including test assertions).
pub fn text(bytes: &[u8]) -> Result<String, String> {
    let (record, complete) = parse_prefix(bytes)?;
    let text = crate::launch::render_recording(&record)?;
    if complete {
        Ok(text)
    } else {
        Ok(text
            .rsplit_once("complete ")
            .expect("recording trailer")
            .0
            .to_owned())
    }
}

/// A reusable frame buffer and a buffered destination. Flushes remain exactly at
/// the old ordinary tape's header, each completion and trailer boundaries.
pub struct Writer<W: Write> {
    writer: io::BufWriter<W>,
    buffer: Vec<u8>,
    count: u64,
    finished: bool,
}
impl<W: Write> Writer<W> {
    pub fn new(writer: W, record: &Recording) -> io::Result<Self> {
        if !record.events.is_empty() {
            return Err(io::Error::other("stream header contains events"));
        }
        if let Some(placement) = record.binding.as_ref().and_then(|b| b.placement.as_ref()) {
            placement.validate().map_err(io::Error::other)?;
        }
        let mut value = Self {
            writer: io::BufWriter::new(writer),
            buffer: vec![],
            count: 0,
            finished: false,
        };
        value.writer.write_all(MAGIC)?;
        value.write(0, 1, record)?;
        Ok(value)
    }
    fn write<T: Wire>(&mut self, seq: u64, kind: u8, payload: &T) -> io::Result<()> {
        self.buffer.clear();
        let start = tape::begin(&mut self.buffer);
        payload.encode(&mut self.buffer);
        tape::finish(&mut self.buffer, start, seq, kind).map_err(io::Error::other)?;
        self.writer.write_all(&self.buffer)?;
        self.writer.flush()
    }
    pub fn append(&mut self, event: &Event) -> io::Result<()> {
        if self.finished {
            return Err(io::Error::other("finished recording"));
        }
        self.write(self.count + 1, 3, event)?;
        self.count += 1;
        Ok(())
    }
    /// The production trailer preserves the run's confirmed ending (DET-1).
    /// In particular an exhausted input prefix alone never proves Blocked.
    pub fn finish_with_outcome(
        &mut self,
        outcome: &hirsel_run_schema::RunOutcome,
    ) -> io::Result<()> {
        if self.finished {
            return Err(io::Error::other("finished recording"));
        }
        self.write(
            self.count + 1,
            6,
            &(self.count, crate::journal::render_outcome(outcome)),
        )?;
        self.finished = true;
        self.writer.flush()
    }
    pub fn finish(&mut self) -> io::Result<()> {
        if self.finished {
            return Err(io::Error::other("finished recording"));
        }
        let count = self.count;
        self.write(count + 1, 5, &count)?;
        self.finished = true;
        Ok(())
    }
}

/// Validate the same semantic header constraints as the inspection parser,
/// without rendering or duplicating the initial file contents.
pub(crate) fn validate_header(record: &Recording) -> Result<(), String> {
    use crate::io::FileKind;
    let tree = &record.tree;
    let confined = |p: &String| hirsel_run_schema::confined_relative("", p).as_ref() == Some(p);
    if !tree.directories.contains("") || tree.directories.iter().any(|p| !confined(p)) {
        return Err("bad recorded directory".into());
    }
    for path in tree.files.keys() {
        if path.is_empty() || !confined(path) || tree.directories.contains(path) {
            return Err("invalid recorded initial tree".into());
        }
        let mut path = path.as_str();
        while let Some((parent, _)) = path.rsplit_once('/') {
            if !tree.directories.contains(parent) {
                return Err("missing recorded directory".into());
            }
            path = parent;
        }
    }
    for path in tree.links.keys() {
        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
        if path.is_empty()
            || !confined(path)
            || tree.directories.contains(path)
            || tree.files.contains_key(path)
            || !tree.directories.contains(parent)
        {
            return Err("recorded link collision or missing parent".into());
        }
    }
    let regular = tree.regular_paths();
    for (alias, canonical) in &tree.file_aliases {
        if alias <= canonical
            || !regular.contains(alias)
            || !regular.contains(canonical)
            || matches!((tree.files.get(alias),tree.files.get(canonical)),(Some(a),Some(b)) if a!=b)
            || tree.file_aliases.contains_key(canonical)
        {
            return Err("bad recorded shared file identity".into());
        }
    }
    for (path, meta) in &tree.metadata {
        if !confined(path) {
            return Err("bad recorded metadata".into());
        }
        let kind = if tree.directories.contains(path) {
            FileKind::Directory
        } else if tree.files.contains_key(path) {
            FileKind::Regular
        } else if tree.links.contains_key(path) {
            FileKind::Symlink
        } else {
            meta.kind
        };
        if kind != meta.kind {
            return Err("recorded metadata kind mismatch".into());
        }
    }
    if let Some(placement) = record.binding.as_ref().and_then(|b| b.placement.as_ref()) {
        placement.validate().map_err(|e| e.to_string())?;
    }
    Ok(())
}
/// Inspect a flushed prefix after interruption: only a partial final frame can
/// be omitted. A complete frame with an invalid CRC remains corruption.
pub fn confirmed_prefix(bytes: &[u8]) -> Result<usize, String> {
    if !bytes.starts_with(MAGIC) {
        return Ok(bytes.iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1));
    }
    let mut at = MAGIC.len();
    let mut seq = 0;
    while at < bytes.len() {
        let rest = &bytes[at..];
        if rest.len() < tape::FRAME {
            break;
        }
        let len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
        if len > rest.len() - tape::FRAME {
            break;
        }
        let frame = tape::read(bytes, at, seq)?;
        at = frame.range.end;
        seq += 1;
    }
    Ok(at)
}

/// Load a complete replay, retaining the original allocation for binary events.
/// Legacy text imports materialize their events through the inspection parser.
pub fn replay(bytes: Vec<u8>) -> Result<(Recording, Option<Tape>), String> {
    if !bytes.starts_with(MAGIC) {
        return parse(&bytes).map(|record| (record, None));
    }
    let (record, tape) = Tape::read(bytes.into())?;
    if !tape.complete {
        return Err("incomplete ordinary recording: unknown suffix".into());
    }
    Ok((record, Some(tape)))
}
