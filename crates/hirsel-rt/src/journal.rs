//! The recording seam (determinism.md, Recording and resuming). Every capability request of a
//! host run passes through its [`Journaled`] gate, whose [`Journal`] is the recording level the
//! run record selects: [`NoJournal`] (none), [`TapeJournal`] (record: the v9 binary tape), or [`DurableJournal`] (durable: framed, checksummed records over a [`JournalFile`],
//! the host's file or the simulated disk's [`SimJournalFile`]). The performer (`RealIo`) only
//! performs; the gate decides when a request may be issued and when its completion may be
//! delivered. [`recover`] reads a durable journal back: a torn tail is cut, earlier damage is
//! `Corrupt`, and a `Prepared` request with no completion is in doubt.
use crate::io::{Completion, Event, Request};
use crate::launch::{Recording, RecordingWriter, render_tape};
use crate::tape_wire::{Decode, Wire, decode};
use hirsel_run_schema::RunOutcome;
use std::collections::BTreeMap;
use std::io;
use std::time::{Duration, Instant};

/// A request made durable before it is issued: its run-local ordinal, the issuing task's spawn
/// path and issue ordinal (its k-th request), and the request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prepared {
    pub id: u64,
    pub task: Vec<u32>,
    pub issue: u64,
    pub request: Request,
}

/// A recording level: where a run's requests and completions go.
pub trait Journal {
    /// Whether the level records anything.
    fn records(&self) -> bool {
        true
    }
    /// Whether requests are made durable before issue and completions before delivery.
    fn durable(&self) -> bool {
        false
    }
    fn prepare(&mut self, _prepared: &Prepared) -> io::Result<()> {
        Ok(())
    }
    /// A tape event: a completion, a park or a ready event. `id` is the request it belongs to.
    fn event(&mut self, id: Option<u64>, event: &Event) -> io::Result<()>;
    /// A prepared request ended without its effect (a cancellation before issue).
    fn withdraw(&mut self, _id: u64) -> io::Result<()> {
        Ok(())
    }
    /// Make everything appended so far durable: one barrier.
    fn barrier(&mut self) -> io::Result<()> {
        Ok(())
    }
    /// Bytes appended since the last barrier.
    fn pending(&self) -> usize {
        0
    }
    /// The terminal trailer, where the level has one.
    fn end(&mut self, outcome: &RunOutcome) -> io::Result<()>;
}

impl<T: Journal + ?Sized> Journal for Box<T> {
    fn records(&self) -> bool {
        (**self).records()
    }
    fn durable(&self) -> bool {
        (**self).durable()
    }
    fn prepare(&mut self, prepared: &Prepared) -> io::Result<()> {
        (**self).prepare(prepared)
    }
    fn event(&mut self, id: Option<u64>, event: &Event) -> io::Result<()> {
        (**self).event(id, event)
    }
    fn withdraw(&mut self, id: u64) -> io::Result<()> {
        (**self).withdraw(id)
    }
    fn barrier(&mut self) -> io::Result<()> {
        (**self).barrier()
    }
    fn pending(&self) -> usize {
        (**self).pending()
    }
    fn end(&mut self, outcome: &RunOutcome) -> io::Result<()> {
        (**self).end(outcome)
    }
}

/// Recording level `none`.
pub struct NoJournal;
impl Journal for NoJournal {
    fn records(&self) -> bool {
        false
    }
    fn event(&mut self, _: Option<u64>, _: &Event) -> io::Result<()> {
        Ok(())
    }
    fn end(&mut self, _: &RunOutcome) -> io::Result<()> {
        Ok(())
    }
}

/// Recording level `record`: the v9 binary tape, each completion flushed before delivery and
/// labelled with an unknown suffix until its terminal frame.
pub struct TapeJournal<W: io::Write>(pub RecordingWriter<W>);
impl<W: io::Write> Journal for TapeJournal<W> {
    fn event(&mut self, _: Option<u64>, event: &Event) -> io::Result<()> {
        self.0.append(event)
    }
    fn end(&mut self, outcome: &RunOutcome) -> io::Result<()> {
        self.0.finish_with_outcome(outcome)
    }
}

/// Storage under a durable journal: appends, and a barrier that makes them durable.
pub trait JournalFile {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn sync(&mut self) -> io::Result<()>;
}
/// The host's journal file; its creation and directory entry were synced with the header.
impl JournalFile for std::fs::File {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        io::Write::write_all(self, bytes)
    }
    fn sync(&mut self) -> io::Result<()> {
        self.sync_data()
    }
}

/// The first bytes of every durable journal.
pub const MAGIC: &[u8] = b"hirsel-journal 4\n";
/// Frame header: payload length, CRC-32C, sequence number, kind.
const FRAME: usize = 4 + 4 + 8 + 1;
/// Group commit bounds (determinism.md, Durable journal).
pub const BATCH_RECORDS: usize = 64;
pub const BATCH_BYTES: usize = 1 << 20;
pub const BATCH_WINDOW: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum Kind {
    Header = 1,
    Prepared = 2,
    Event = 3,
    Withdrawn = 4,
    Trailer = 5,
}
impl Kind {
    fn of(byte: u8) -> Option<Self> {
        [
            Self::Header,
            Self::Prepared,
            Self::Event,
            Self::Withdrawn,
            Self::Trailer,
        ]
        .into_iter()
        .find(|kind| *kind as u8 == byte)
    }
}

/// Recording level `durable`.
pub struct DurableJournal<F: JournalFile> {
    file: F,
    seq: u64,
    buffer: Vec<u8>,
    events: u64,
    ended: bool,
}
impl<F: JournalFile> DurableJournal<F> {
    /// Write the header and make it durable before the run enters. The caller syncs the
    /// directory entry of a newly created host file.
    pub fn create(mut file: F, header: &Recording) -> io::Result<Self> {
        if !header.events.is_empty() {
            return Err(io::Error::other("journal header contains events"));
        }
        let mut journal = Self {
            file: {
                file.append(MAGIC)?;
                file
            },
            seq: 0,
            buffer: vec![],
            events: 0,
            ended: false,
        };
        journal.frame(Kind::Header, header)?;
        journal.barrier()?;
        Ok(journal)
    }
    pub fn file(&self) -> &F {
        &self.file
    }
    fn frame<T: Wire>(&mut self, kind: Kind, payload: &T) -> io::Result<()> {
        let start = hirsel_run_schema::tape::begin(&mut self.buffer);
        payload.encode(&mut self.buffer);
        hirsel_run_schema::tape::finish(&mut self.buffer, start, self.seq, kind as u8)
            .map_err(io::Error::other)?;
        self.seq += 1;
        Ok(())
    }
    fn open(&self) -> io::Result<()> {
        if self.ended {
            return Err(io::Error::other("finished journal"));
        }
        Ok(())
    }
}
impl<F: JournalFile> Journal for DurableJournal<F> {
    fn durable(&self) -> bool {
        true
    }
    fn prepare(&mut self, prepared: &Prepared) -> io::Result<()> {
        self.open()?;
        self.frame(Kind::Prepared, prepared)?;
        Ok(())
    }
    fn event(&mut self, id: Option<u64>, event: &Event) -> io::Result<()> {
        self.open()?;
        let start = hirsel_run_schema::tape::begin(&mut self.buffer);
        id.encode(&mut self.buffer);
        event.encode(&mut self.buffer);
        hirsel_run_schema::tape::finish(&mut self.buffer, start, self.seq, Kind::Event as u8)
            .map_err(io::Error::other)?;
        self.seq += 1;
        self.events += 1;
        Ok(())
    }
    fn withdraw(&mut self, id: u64) -> io::Result<()> {
        self.open()?;
        self.frame(Kind::Withdrawn, &id)?;
        Ok(())
    }
    fn barrier(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        self.file.append(&self.buffer)?;
        self.buffer.clear();
        self.file.sync()
    }
    fn pending(&self) -> usize {
        self.buffer.len()
    }
    fn end(&mut self, outcome: &RunOutcome) -> io::Result<()> {
        self.open()?;
        let start = hirsel_run_schema::tape::begin(&mut self.buffer);
        self.events.encode(&mut self.buffer);
        outcome.encode(&mut self.buffer);
        hirsel_run_schema::tape::finish(&mut self.buffer, start, self.seq, Kind::Trailer as u8)
            .map_err(io::Error::other)?;
        self.seq += 1;
        self.ended = true;
        self.barrier()
    }
}

/// One tape event line, without the tape header and newline.
fn event_line(event: &Event) -> io::Result<String> {
    let text = render_tape(std::slice::from_ref(event)).map_err(io::Error::other)?;
    let (_, line) = text.split_once('\n').expect("tape header");
    Ok(line.trim_end_matches('\n').to_owned())
}
fn task_text(task: &[u32]) -> String {
    if task.is_empty() {
        "-".into()
    } else {
        task.iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(".")
    }
}
fn canonical_number(word: &str) -> Result<u64, String> {
    word.parse::<u64>()
        .ok()
        .filter(|n| n.to_string() == word)
        .ok_or_else(|| format!("bad journal number {word:?}"))
}

/// CRC-32C (Castagnoli, reflected) over the encoded length, sequence number, kind and payload.
fn checksum(seq: u64, kind: u8, payload: &[u8]) -> u32 {
    use hirsel_run_schema::tape::crc32c_append;
    let crc = crc32c_append(0, &(payload.len() as u32).to_le_bytes());
    let crc = crc32c_append(crc, &seq.to_le_bytes());
    crc32c_append(crc32c_append(crc, &[kind]), payload)
}

fn hex(bytes: &[u8]) -> String {
    hirsel_run_schema::hex(bytes)
}
fn unhex(word: &str) -> Result<String, String> {
    if !word.is_ascii() || !word.len().is_multiple_of(2) {
        return Err("bad journal bytes".into());
    }
    let bytes = (0..word.len() / 2)
        .map(|i| u8::from_str_radix(&word[2 * i..2 * i + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "bad journal bytes")?;
    let text = String::from_utf8(bytes).map_err(|_| "bad journal UTF-8")?;
    (hex(text.as_bytes()) == word)
        .then_some(text)
        .ok_or_else(|| "bad journal bytes".into())
}

/// The trailer's outcome text: one word for the class, then hex strings and decimal numbers.
pub fn render_outcome(outcome: &RunOutcome) -> String {
    use hirsel_run_schema::{BudgetKind, Environmental, Returned};
    fn site(site: &hirsel_run_schema::Site) -> String {
        format!(
            "{} {} {} {}",
            hex(site.definition.as_bytes()),
            hex(site.file.as_bytes()),
            site.line,
            site.column
        )
    }
    fn failure(f: &hirsel_run_schema::Failure) -> String {
        let operation = f
            .operation
            .as_ref()
            .map_or_else(|| "-".into(), |s| format!("+ {}", site(s)));
        format!("{} {} {operation}", hex(f.rule.as_bytes()), site(&f.site))
    }
    fn diagnostic(d: &hirsel_run_schema::ExecutionDiagnostic) -> String {
        format!(
            "{} {} {}",
            hex(d.rule.as_bytes()),
            site(&d.site),
            hex(d.message.as_bytes())
        )
    }
    match outcome {
        RunOutcome::Returned(Returned::Ok) => "returned ok".into(),
        RunOutcome::Returned(Returned::Error) => "returned error".into(),
        RunOutcome::Returned(Returned::ExitCode(code)) => format!("returned code {code}"),
        RunOutcome::Failed { primary, causes } => {
            let mut text = format!("failed {} {}", causes.len(), failure(primary));
            for cause in causes {
                text.push(' ');
                text.push_str(&failure(cause));
            }
            text
        }
        RunOutcome::Blocked(waits) => format!(
            "blocked {}",
            hex(hirsel_run_schema::Wait::encode(waits).as_bytes())
        ),
        RunOutcome::Signaled(kind) => format!("signaled {}", kind.name()),
        RunOutcome::BudgetExceeded(BudgetKind::Fuel) => "budget fuel".into(),
        RunOutcome::BudgetExceeded(BudgetKind::Memory) => "budget memory".into(),
        RunOutcome::Refused(why) => format!("refused {}", hex(why.as_bytes())),
        RunOutcome::Diverged(why) => format!("diverged {}", hex(why.as_bytes())),
        RunOutcome::Environmental(Environmental::CodecLimit) => "environmental codec".into(),
        RunOutcome::Environmental(Environmental::Recording(why)) => {
            format!("environmental recording {}", hex(why.as_bytes()))
        }
        RunOutcome::Environmental(Environmental::Output(why)) => {
            format!("environmental output {}", hex(why.as_bytes()))
        }
        RunOutcome::Environmental(Environmental::Stopped) => "environmental stopped".into(),
        RunOutcome::Tooling(why) => format!("tooling {}", hex(why.as_bytes())),
        RunOutcome::ToolingLimit(limit) => format!(
            "tooling_limit {} {} {}",
            hex(limit.kind.as_bytes()),
            limit.limit,
            limit
                .site
                .as_ref()
                .map_or_else(|| "-".into(), |s| format!("+ {}", site(s)))
        ),
        RunOutcome::UndefinedBehavior(d) => format!("undefined_behavior {}", diagnostic(d)),
        RunOutcome::Trapped(d) => format!("trapped {}", diagnostic(d)),
    }
}
/// Strict inverse of [`render_outcome`].
pub fn parse_outcome(text: &str) -> Result<RunOutcome, String> {
    use hirsel_run_schema::{BudgetKind, Environmental, Failure, Returned, Site};
    fn site(next: &mut dyn FnMut() -> Result<String, String>) -> Result<Site, String> {
        Ok(Site {
            definition: unhex(&next()?)?,
            file: unhex(&next()?)?,
            line: canonical_number(&next()?)?,
            column: canonical_number(&next()?)?,
        })
    }
    let words: Vec<&str> = text.split(' ').collect();
    let mut at = 0;
    let mut next = || -> Result<&str, String> {
        let word = words.get(at).copied().ok_or("short journal outcome")?;
        at += 1;
        Ok(word)
    };
    let outcome = match next()? {
        "returned" => match next()? {
            "ok" => RunOutcome::Returned(Returned::Ok),
            "error" => RunOutcome::Returned(Returned::Error),
            "code" => RunOutcome::Returned(Returned::ExitCode(
                u8::try_from(canonical_number(next()?)?).map_err(|_| "exit code outside u8")?,
            )),
            _ => return Err("bad journal outcome".into()),
        },
        "failed" => {
            let count = canonical_number(next()?)?;
            let mut failures = vec![];
            for _ in 0..=count {
                let mut take = || next().map(str::to_owned);
                let rule = unhex(&take()?)?;
                let primary = site(&mut take)?;
                let operation = match take()?.as_str() {
                    "-" => None,
                    "+" => Some(site(&mut take)?),
                    _ => return Err("bad journal failure".into()),
                };
                failures.push(Failure {
                    rule,
                    site: primary,
                    operation,
                });
            }
            let primary = failures.remove(0);
            RunOutcome::Failed {
                primary,
                causes: failures,
            }
        }
        "blocked" => RunOutcome::Blocked(hirsel_run_schema::Wait::decode(&unhex(next()?)?)?),
        "signaled" => RunOutcome::Signaled(
            hirsel_run_schema::SignalKind::parse(next()?).ok_or("bad journal signal")?,
        ),
        "budget" => match next()? {
            "fuel" => RunOutcome::BudgetExceeded(BudgetKind::Fuel),
            "memory" => RunOutcome::BudgetExceeded(BudgetKind::Memory),
            _ => return Err("bad journal budget".into()),
        },
        "refused" => RunOutcome::Refused(unhex(next()?)?),
        "diverged" => RunOutcome::Diverged(unhex(next()?)?),
        "environmental" => match next()? {
            "codec" => RunOutcome::Environmental(Environmental::CodecLimit),
            "recording" => RunOutcome::Environmental(Environmental::Recording(unhex(next()?)?)),
            "output" => RunOutcome::Environmental(Environmental::Output(unhex(next()?)?)),
            "stopped" => RunOutcome::Environmental(Environmental::Stopped),
            _ => return Err("bad journal environment".into()),
        },
        "tooling" => RunOutcome::Tooling(unhex(next()?)?),
        "tooling_limit" => {
            let kind = unhex(next()?)?;
            let limit = canonical_number(next()?)?;
            let site = match next()? {
                "-" => None,
                "+" => Some(site(&mut || next().map(str::to_owned))?),
                _ => return Err("bad journal tooling limit".into()),
            };
            RunOutcome::ToolingLimit(hirsel_run_schema::ToolingLimit { kind, limit, site })
        }
        kind @ ("undefined_behavior" | "trapped") => {
            let rule = unhex(next()?)?;
            let site = site(&mut || next().map(str::to_owned))?;
            let message = unhex(next()?)?;
            let diagnostic = hirsel_run_schema::ExecutionDiagnostic {
                rule,
                site,
                message,
            };
            if kind == "trapped" {
                RunOutcome::Trapped(diagnostic)
            } else {
                RunOutcome::UndefinedBehavior(diagnostic)
            }
        }
        _ => return Err("bad journal outcome".into()),
    };
    if at != words.len() {
        return Err("trailing journal outcome words".into());
    }
    if render_outcome(&outcome) != text {
        return Err("noncanonical journal outcome".into());
    }
    Ok(outcome)
}

/// Why a durable journal cannot be recovered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Unrecoverable {
    /// Not a durable journal.
    NotJournal,
    /// The header never became durable: the run never entered.
    NoHeader,
    /// A record before the last is damaged, or the records disagree.
    Corrupt { offset: usize, detail: String },
}
impl std::fmt::Display for Unrecoverable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotJournal => f.write_str("not a durable journal"),
            Self::NoHeader => f.write_str("the journal has no durable header"),
            Self::Corrupt { offset, detail } => {
                write!(f, "the journal is corrupt at byte {offset}: {detail}")
            }
        }
    }
}

/// What a durable journal proves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recovered {
    /// The header and every tape event, in order: the replayable prefix.
    pub recording: Recording,
    pub prepared: Vec<Prepared>,
    /// Exact Prepared association for each replayable event; scheduling inputs have none.
    pub event_prepared: Vec<Option<u64>>,
    /// Prepared requests with no completion or withdrawal, in preparation order.
    pub in_doubt: Vec<Prepared>,
    /// The trailer's outcome: a complete recording.
    pub outcome: Option<RunOutcome>,
    /// Length of the valid prefix; a torn tail lies past it.
    pub valid: usize,
}
impl Recovered {
    /// A complete recording with nothing in doubt, which replays like a tape.
    pub fn complete(&self) -> bool {
        self.outcome.is_some() && self.in_doubt.is_empty()
    }
}

/// Why an incomplete journal does not replay: it has no trailer, or names the requests in
/// doubt by ordinal, issuing task and issue ordinal, and request.
pub fn incomplete(recovered: &Recovered) -> String {
    let mut text = match &recovered.outcome {
        Some(_) => "the durable journal is complete but leaves requests in doubt".to_owned(),
        None => "the durable journal has no trailer (the run did not finish)".to_owned(),
    };
    if recovered.in_doubt.is_empty() {
        text.push_str("; no request is in doubt");
    }
    for prepared in &recovered.in_doubt {
        let line = event_line(&Event {
            request: prepared.request.clone(),
            completion: Completion::Checked(false),
            fault: None,
            resources: None,
        })
        .unwrap_or_default();
        text.push_str(&format!(
            "; in doubt: request {} of task {} (its request {}): {}",
            prepared.id,
            task_text(&prepared.task),
            prepared.issue,
            line.split_once(" -> ").map_or(line.as_str(), |(r, _)| r)
        ));
    }
    text
}

/// Whether `bytes` begin a durable journal (rather than an ordinary tape).
pub fn is_journal(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC) || (!bytes.is_empty() && MAGIC.starts_with(bytes))
}

/// Read a durable journal back (determinism.md, Durable journal). The last record may be torn
/// and is cut; damage to an earlier record, a sequence gap or a record after the trailer is
/// `Corrupt`.
pub fn recover(bytes: &[u8]) -> Result<Recovered, Unrecoverable> {
    // An empty file is a journal created but never synced.
    if bytes.is_empty() {
        return Err(Unrecoverable::NoHeader);
    }
    if !is_journal(bytes) {
        return Err(Unrecoverable::NotJournal);
    }
    let mut offset = MAGIC.len().min(bytes.len());
    let mut records = vec![];
    while offset < bytes.len() {
        let rest = &bytes[offset..];
        if rest.len() < FRAME {
            break;
        }
        let len = u32::from_le_bytes(rest[0..4].try_into().unwrap()) as usize;
        let end = FRAME.checked_add(len).filter(|end| *end <= rest.len());
        let Some(end) = end else {
            // The record runs past the end: the last write was torn.
            break;
        };
        let crc = u32::from_le_bytes(rest[4..8].try_into().unwrap());
        let seq = u64::from_le_bytes(rest[8..16].try_into().unwrap());
        let kind = rest[16];
        let payload = &rest[FRAME..end];
        let last = end == rest.len();
        let damage = if checksum(seq, kind, payload) != crc {
            Some("checksum mismatch")
        } else if seq != records.len() as u64 {
            Some("sequence gap")
        } else if Kind::of(kind).is_none() {
            Some("unknown record kind")
        } else {
            None
        };
        if let Some(detail) = damage {
            if last {
                break;
            }
            return Err(Unrecoverable::Corrupt {
                offset,
                detail: detail.into(),
            });
        }
        records.push((offset, Kind::of(kind).unwrap(), payload));
        offset += end;
    }
    let valid = records
        .last()
        .map_or(MAGIC.len().min(bytes.len()), |(at, _, payload)| {
            at + FRAME + payload.len()
        });
    let corrupt = |offset: usize, detail: String| Unrecoverable::Corrupt { offset, detail };
    let mut records = records.into_iter();
    let Some((at, Kind::Header, header)) = records.next() else {
        return Err(Unrecoverable::NoHeader);
    };
    let mut recording: Recording = decode(header).map_err(|e| corrupt(at, e))?;
    crate::tape_binary::validate_header(&recording).map_err(|e| corrupt(at, e))?;
    if !recording.events.is_empty() {
        return Err(corrupt(at, "header contains events".into()));
    }
    let mut prepared = vec![];
    let mut event_prepared = vec![];
    let mut open: BTreeMap<u64, Prepared> = BTreeMap::new();
    let mut outcome = None;
    for (at, kind, payload) in records {
        if outcome.is_some() {
            return Err(corrupt(at, "a record after the trailer".into()));
        }
        match kind {
            Kind::Header => return Err(corrupt(at, "a second header".into())),
            Kind::Prepared => {
                let record: Prepared = decode(payload).map_err(|e| corrupt(at, e))?;
                let id = record.id;
                if id != prepared.len() as u64 {
                    return Err(corrupt(at, "Prepared ordinals out of order".into()));
                }
                open.insert(id, record.clone());
                prepared.push(record);
            }
            Kind::Event => {
                let mut input = Decode(payload);
                let id = Option::<u64>::decode(&mut input).map_err(|e| corrupt(at, e))?;
                let event = Event::decode(&mut input).map_err(|e| corrupt(at, e))?;
                if !input.0.is_empty() {
                    return Err(corrupt(at, "trailing event payload".into()));
                }
                if let Some(id) = id {
                    let Some(record) = prepared.get(id as usize) else {
                        return Err(corrupt(at, "an event of no Prepared request".into()));
                    };
                    if record.request != event.request {
                        return Err(corrupt(at, "an event of another request".into()));
                    }
                    if !matches!(
                        event.completion,
                        Completion::Blocked { .. } | Completion::Held
                    ) && open.remove(&id).is_none()
                    {
                        return Err(corrupt(at, "a request completed twice".into()));
                    }
                }
                event_prepared.push(id);
                recording.events.push(event);
            }
            Kind::Withdrawn => {
                let id = decode(payload).map_err(|e| corrupt(at, e))?;
                if open.remove(&id).is_none() {
                    return Err(corrupt(at, "a withdrawal of no open request".into()));
                }
            }
            Kind::Trailer => {
                let (count, end): (u64, RunOutcome) =
                    decode(payload).map_err(|e| corrupt(at, e))?;
                if count != recording.events.len() as u64 {
                    return Err(corrupt(at, "the trailer's event count disagrees".into()));
                }
                outcome = Some(end);
            }
        }
    }
    Ok(Recovered {
        recording,
        prepared,
        event_prepared,
        in_doubt: open.into_values().collect(),
        outcome,
        valid,
    })
}

/// A journal on the simulated disk: the crash model of every simulated `Dir`
/// (`hirsel_kernels::disk`), over one durable file.
#[derive(Clone, Debug)]
pub struct SimJournalFile {
    disk: hirsel_kernels::disk::Durable,
    live: Vec<u8>,
}
impl Default for SimJournalFile {
    fn default() -> Self {
        let directories = std::collections::BTreeSet::from([String::new()]);
        let files = BTreeMap::from([("journal".to_string(), (1, vec![]))]);
        Self {
            disk: hirsel_kernels::disk::Durable::of(&directories, &files),
            live: vec![],
        }
    }
}
impl SimJournalFile {
    /// Every byte appended, durable or not.
    pub fn bytes(&self) -> &[u8] {
        &self.live
    }
    /// The journal a crash leaves: its durable bytes and a seeded prefix of the last write.
    pub fn crash(&self, seed: u64) -> Vec<u8> {
        self.disk.crash(seed).files["journal"].1.clone()
    }
}
impl JournalFile for SimJournalFile {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.disk.wrote(1, self.live.len(), bytes);
        self.live.extend_from_slice(bytes);
        Ok(())
    }
    fn sync(&mut self) -> io::Result<()> {
        let directories = std::collections::BTreeSet::from([String::new()]);
        let files = BTreeMap::from([("journal".to_string(), 1)]);
        self.disk
            .sync(1, &self.live, "journal", &directories, &files);
        Ok(())
    }
}
/// A shared simulated journal file, so a test can crash it while the run writes.
impl JournalFile for std::rc::Rc<std::cell::RefCell<SimJournalFile>> {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.borrow_mut().append(bytes)
    }
    fn sync(&mut self) -> io::Result<()> {
        self.borrow_mut().sync()
    }
}
impl JournalFile for std::sync::Arc<std::sync::Mutex<SimJournalFile>> {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.lock().unwrap().append(bytes)
    }
    fn sync(&mut self) -> io::Result<()> {
        self.lock().unwrap().sync()
    }
}

/// Whether the gate lets a request through now or parks it until a barrier covers it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Gate {
    Issue,
    Hold,
}
struct Ticket {
    id: u64,
    request: Request,
    durable: bool,
    resolved: bool,
    /// Held by the gate until a barrier, and its task parked: due for release.
    held: bool,
    parked: bool,
}

/// The journal gate in front of a performer: request ordinals, the issuer of each request,
/// and group commit of `Prepared` records for requests that may park.
pub struct Journaled<J: Journal = Box<dyn Journal>> {
    journal: J,
    /// The default has no destination. An installed journal retains its dynamic
    /// records() contract, even when that contract changes during a run.
    absent: bool,
    next: u64,
    issues: BTreeMap<Vec<u32>, u64>,
    issuer: Vec<u32>,
    /// Requests that may park, by wait token.
    tickets: BTreeMap<u64, Ticket>,
    /// The request being performed.
    current: Option<u64>,
    /// Records since the last barrier and when the first was appended.
    batch: usize,
    since: Option<Instant>,
    /// A recording failure no caller could take; the next request reports it.
    failed: Option<io::Error>,
}
impl Default for Journaled {
    fn default() -> Self {
        let mut gate = Self::new(Box::new(NoJournal));
        gate.absent = true;
        gate
    }
}
impl Journaled {
    pub fn new(journal: Box<dyn Journal>) -> Self {
        Self::with_destination(journal)
    }
}
impl<J: Journal> Journaled<J> {
    pub(crate) fn with_destination(journal: J) -> Self {
        Self {
            journal,
            absent: false,
            next: 0,
            issues: BTreeMap::new(),
            issuer: vec![],
            tickets: BTreeMap::new(),
            current: None,
            batch: 0,
            since: None,
            failed: None,
        }
    }
    pub(crate) fn unrecorded(journal: J) -> Self {
        let mut gate = Self::with_destination(journal);
        gate.absent = true;
        gate
    }
    pub(crate) fn destination(&self) -> &J {
        &self.journal
    }
    pub(crate) fn map_destination<K: Journal>(self, map: impl FnOnce(J) -> K) -> Journaled<K> {
        let Self {
            journal,
            absent,
            next,
            issues,
            issuer,
            tickets,
            current,
            batch,
            since,
            failed,
        } = self;
        Journaled {
            journal: map(journal),
            absent,
            next,
            issues,
            issuer,
            tickets,
            current,
            batch,
            since,
            failed,
        }
    }
    /// Keep a recording failure for the next request, which it prevents.
    pub fn fail(&mut self, error: io::Error) {
        self.failed.get_or_insert(error);
    }
    pub(crate) fn absent(&self) -> bool {
        self.absent
    }
    pub fn records(&self) -> bool {
        !self.absent && self.journal.records()
    }
    pub fn durable(&self) -> bool {
        !self.absent && self.journal.durable()
    }
    /// The spawn path of the task issuing the next request.
    pub fn issuer(&mut self, task: &[u32]) {
        if !self.durable() {
            return;
        }
        task.clone_into(&mut self.issuer);
    }
    fn barrier(&mut self) -> io::Result<()> {
        self.journal.barrier()?;
        self.batch = 0;
        self.since = None;
        for ticket in self.tickets.values_mut() {
            ticket.durable = true;
        }
        Ok(())
    }
    /// Before `request` is issued, `token` its wait when it may park its task. A durable
    /// journal prepares a new request; one that may park is held until a barrier covers its
    /// record, and any other is covered by a barrier now.
    pub fn before(&mut self, request: &Request, token: Option<u64>) -> io::Result<Gate> {
        if let Some(error) = self.failed.take() {
            return Err(error);
        }
        if !self.durable() {
            return Ok(Gate::Issue);
        }
        if let Some(token) = token
            && let Some(ticket) = self.tickets.get(&token)
            && ticket.request == *request
        {
            self.current = Some(ticket.id);
            return Ok(if ticket.durable {
                Gate::Issue
            } else {
                Gate::Hold
            });
        }
        let id = self.next;
        self.next += 1;
        let issues = self.issues.entry(self.issuer.clone()).or_default();
        let issue = *issues;
        *issues += 1;
        self.journal.prepare(&Prepared {
            id,
            task: self.issuer.clone(),
            issue,
            request: request.clone(),
        })?;
        self.current = Some(id);
        self.batch += 1;
        self.since.get_or_insert_with(Instant::now);
        let Some(token) = token else {
            self.barrier()?;
            return Ok(Gate::Issue);
        };
        self.tickets.insert(
            token,
            Ticket {
                id,
                request: request.clone(),
                durable: false,
                resolved: false,
                held: false,
                parked: false,
            },
        );
        if self.batch >= BATCH_RECORDS || self.journal.pending() >= BATCH_BYTES {
            self.barrier()?;
        }
        // Every newly prepared parkable request reports Held, even when this
        // request closed the batch. Thus new journals containing an issued
        // Blocked wait always identify their pre-issue parks explicitly.
        self.tickets.get_mut(&token).expect("the ticket").held = true;
        Ok(Gate::Hold)
    }
    /// A tape event of the current request (or a scheduling input). A durable journal makes a
    /// completion durable before it is delivered; the same barrier covers held records.
    pub fn record(&mut self, event: &Event) -> io::Result<()> {
        if self.absent {
            return Ok(());
        }
        // A scheduling input (a ready wait, a seeded schedule choice) belongs to no request.
        let ready = matches!(
            event.request,
            Request::Ready { .. } | Request::Schedule { .. } | Request::CpuControl { .. }
        );
        let id = if ready { None } else { self.current };
        self.journal.event(id, event)?;
        if ready
            || matches!(
                event.completion,
                Completion::Blocked { .. } | Completion::Held
            )
        {
            return Ok(());
        }
        self.current = None;
        if let Some(id) = id
            && let Some(ticket) = self.tickets.values_mut().find(|t| t.id == id)
        {
            ticket.resolved = true;
        }
        if self.durable() {
            self.barrier()?;
        }
        Ok(())
    }
    /// The task of the request parked under `token` parked.
    pub fn parked(&mut self, token: u64) {
        if self.absent {
            return;
        }
        if let Some(ticket) = self.tickets.get_mut(&token) {
            ticket.parked = true;
        }
    }
    /// A scheduling point: the parked held requests a barrier has released, in token order,
    /// each to be re-issued now. A batch whose first record is older than the batch window
    /// is made durable here.
    pub fn released(&mut self) -> io::Result<Vec<u64>> {
        if self.absent {
            return Ok(vec![]);
        }
        if self
            .since
            .is_some_and(|since| since.elapsed() >= BATCH_WINDOW)
            && self.tickets.values().any(|t| t.held && !t.durable)
        {
            self.barrier()?;
        }
        Ok(self
            .tickets
            .iter_mut()
            .filter(|(_, t)| t.held && t.parked && t.durable)
            .map(|(&token, t)| {
                t.held = false;
                t.parked = false;
                token
            })
            .collect())
    }
    /// The run would wait for the host: the open batch is made durable, and true when a
    /// parked held request is now due for release.
    pub fn idle(&mut self) -> io::Result<bool> {
        if self.absent {
            return Ok(false);
        }
        if self.tickets.values().any(|t| !t.durable) {
            self.barrier()?;
        }
        Ok(self.tickets.values().any(|t| t.held && t.parked))
    }
    /// The wait `token` ended; a prepared request it never completed is withdrawn.
    pub fn finish(&mut self, token: u64) -> io::Result<()> {
        if self.absent {
            return Ok(());
        }
        if let Some(ticket) = self.tickets.remove(&token)
            && !ticket.resolved
        {
            self.journal.withdraw(ticket.id)?;
            if self.current == Some(ticket.id) {
                self.current = None;
            }
        }
        Ok(())
    }
    /// The run ended: its trailer, durable.
    pub fn end(&mut self, outcome: &RunOutcome) -> io::Result<()> {
        if self.absent {
            return Ok(());
        }
        self.journal.end(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{IoError, Tree};
    use hirsel_run_schema::{Failure, Returned, Site};
    use std::sync::{Arc, Mutex};

    fn header() -> Recording {
        Recording {
            arguments: vec!["a".into()],
            run_identity: [7; 32],
            hash_seed: 1,
            random_seed: 2,
            test_seed: 3,
            binding: None,
            tree: Tree::new(BTreeMap::from([("f".into(), b"x".to_vec())])).unwrap(),
            events: vec![],
        }
    }
    fn print(bytes: &[u8]) -> Request {
        Request::Print {
            stdout: 2,
            bytes: bytes.to_vec(),
        }
    }
    fn printed(bytes: &[u8]) -> Event {
        Event {
            request: print(bytes),
            completion: Completion::Unit(Ok(())),
            fault: None,
            resources: None,
        }
    }

    /// A run of three requests: two sequential prints and a parked one withdrawn.
    fn run(file: Arc<Mutex<SimJournalFile>>) -> Journaled {
        let mut gate = Journaled::new(Box::new(DurableJournal::create(file, &header()).unwrap()));
        gate.issuer(&[]);
        assert_eq!(gate.before(&print(b"a"), None).unwrap(), Gate::Issue);
        gate.record(&printed(b"a")).unwrap();
        gate.issuer(&[0]);
        let read = Request::ReadStdin { stdin: 3, max: 4 };
        assert_eq!(gate.before(&read, Some(9)).unwrap(), Gate::Hold);
        gate.record(&Event {
            request: read.clone(),
            completion: Completion::Held,
            fault: None,
            resources: None,
        })
        .unwrap();
        gate.parked(9);
        assert!(gate.idle().unwrap());
        assert_eq!(gate.released().unwrap(), [9]);
        gate.finish(9).unwrap();
        gate.issuer(&[]);
        assert_eq!(gate.before(&print(b"b"), None).unwrap(), Gate::Issue);
        gate.record(&Event {
            request: print(b"b"),
            completion: Completion::Unit(Err(IoError::Fault(5))),
            fault: None,
            resources: None,
        })
        .unwrap();
        gate
    }

    #[test]
    fn a_complete_journal_recovers_its_tape_and_outcome() {
        let file = Arc::new(Mutex::new(SimJournalFile::default()));
        let mut gate = run(file.clone());
        let outcome = RunOutcome::Failed {
            primary: Failure {
                rule: "FAIL-1".into(),
                site: Site {
                    definition: "main".into(),
                    file: "é.hir".into(),
                    line: 3,
                    column: 4,
                },
                operation: None,
            },
            causes: vec![Failure {
                rule: "LANG-9".into(),
                site: Site::default(),
                operation: Some(Site::default()),
            }],
        };
        gate.end(&outcome).unwrap();
        let recovered = recover(file.lock().unwrap().bytes()).unwrap();
        assert_eq!(recovered.outcome, Some(outcome));
        assert!(recovered.complete());
        assert_eq!(recovered.recording.tree, header().tree);
        assert_eq!(recovered.recording.events.len(), 3);
        assert_eq!(recovered.prepared.len(), 3);
        assert_eq!(recovered.prepared[1].task, vec![0]);
        assert_eq!(recovered.prepared[2].issue, 1);
        assert_eq!(recovered.valid, file.lock().unwrap().bytes().len());
    }

    #[test]
    fn outcomes_round_trip() {
        use hirsel_run_schema::{
            BudgetKind, Environmental, ExecutionDiagnostic, ToolingLimit, Wait,
        };
        let site = Site {
            definition: "main.worker".into(),
            file: "é.hir".into(),
            line: 3,
            column: 4,
        };
        let diagnostic = ExecutionDiagnostic {
            rule: "MEM-1".into(),
            site: site.clone(),
            message: "invalid memory access".into(),
        };
        for outcome in [
            RunOutcome::Returned(Returned::Ok),
            RunOutcome::Returned(Returned::Error),
            RunOutcome::Returned(Returned::ExitCode(2)),
            RunOutcome::Returned(Returned::ExitCode(255)),
            RunOutcome::Blocked(vec![]),
            RunOutcome::Blocked(vec![Wait {
                spawn_path: vec![0, 2],
                site: site.clone(),
                kind: "channel.receive".into(),
                targets: vec![3],
            }]),
            RunOutcome::Signaled(hirsel_run_schema::SignalKind::Hangup),
            RunOutcome::BudgetExceeded(BudgetKind::Memory),
            RunOutcome::Refused("".into()),
            RunOutcome::Diverged("x y".into()),
            RunOutcome::Environmental(Environmental::CodecLimit),
            RunOutcome::Environmental(Environmental::Recording("disk".into())),
            RunOutcome::Environmental(Environmental::Output("o".into())),
            RunOutcome::Environmental(Environmental::Stopped),
            RunOutcome::Tooling("t".into()),
            RunOutcome::ToolingLimit(ToolingLimit {
                kind: "steps".into(),
                limit: 123,
                site: Some(site),
            }),
            RunOutcome::ToolingLimit(ToolingLimit {
                kind: "calls".into(),
                limit: 456,
                site: None,
            }),
            RunOutcome::UndefinedBehavior(diagnostic.clone()),
            RunOutcome::Trapped(diagnostic),
        ] {
            assert_eq!(parse_outcome(&render_outcome(&outcome)), Ok(outcome));
        }
        assert!(parse_outcome("returned ok extra").is_err());
        assert!(parse_outcome("returned code 256").is_err());
        assert!(parse_outcome("returned code -1").is_err());
    }

    #[test]
    fn every_record_boundary_and_torn_byte_recovers_a_valid_prefix() {
        let file = Arc::new(Mutex::new(SimJournalFile::default()));
        let mut gate = run(file.clone());
        gate.end(&RunOutcome::Returned(Returned::Ok)).unwrap();
        let bytes = file.lock().unwrap().bytes().to_vec();
        let complete = recover(&bytes).unwrap();
        let header_end = MAGIC.len()
            + FRAME
            + u32::from_le_bytes(bytes[MAGIC.len()..MAGIC.len() + 4].try_into().unwrap()) as usize;
        for cut in 0..bytes.len() {
            match recover(&bytes[..cut]) {
                Ok(prefix) => {
                    assert!(cut >= header_end, "{cut}");
                    assert!(prefix.valid <= cut);
                    assert!(
                        complete
                            .recording
                            .events
                            .starts_with(&prefix.recording.events)
                    );
                    assert!(complete.prepared.starts_with(&prefix.prepared));
                    assert_eq!(prefix.outcome, None);
                    // In doubt: exactly the prepared requests the prefix does not resolve.
                    let resolved = prefix.prepared.len() - prefix.in_doubt.len();
                    assert!(resolved <= prefix.recording.events.len());
                }
                Err(error) => {
                    assert!(cut < header_end, "{cut}: {error}");
                    assert_eq!(error, Unrecoverable::NoHeader);
                }
            }
        }
        // A crash keeps the durable prefix and a seeded part of the last unsynced write: here
        // a completion appended whose barrier never returned.
        let mut journal = DurableJournal::create(SimJournalFile::default(), &header()).unwrap();
        journal.prepare(&complete.prepared[0]).unwrap();
        journal.barrier().unwrap();
        journal.event(Some(0), &printed(b"a")).unwrap();
        let unsynced = std::mem::take(&mut journal.buffer);
        journal.file.append(&unsynced).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        // A crash keeps a seeded prefix of the unsynced record, rarely all of it, so the
        // complete survivor is also checked directly: every appended byte reached the disk.
        let survivors = (0..64).map(|seed| journal.file().crash(seed));
        for crashed in survivors.chain([journal.file().bytes().to_vec()]) {
            let recovered = recover(&crashed).unwrap();
            seen.insert(recovered.recording.events.len());
            if recovered.recording.events.is_empty() {
                assert_eq!(recovered.in_doubt, complete.prepared[..1]);
            } else {
                assert!(recovered.in_doubt.is_empty());
            }
        }
        assert_eq!(seen, [0, 1].into());
    }

    #[test]
    fn damage_before_the_last_record_is_corrupt_and_a_torn_last_record_is_cut() {
        let file = Arc::new(Mutex::new(SimJournalFile::default()));
        let mut gate = run(file.clone());
        gate.end(&RunOutcome::Returned(Returned::Ok)).unwrap();
        let bytes = file.lock().unwrap().bytes().to_vec();
        let mut last = bytes.clone();
        *last.last_mut().unwrap() ^= 1;
        let cut = recover(&last).unwrap();
        assert_eq!(cut.outcome, None);
        assert!(cut.valid < bytes.len());
        let mut early = bytes.clone();
        early[MAGIC.len() + FRAME + 2] ^= 1;
        assert!(matches!(
            recover(&early),
            Err(Unrecoverable::Corrupt { .. })
        ));
        assert_eq!(recover(b"hirsel-tape 7\n"), Err(Unrecoverable::NotJournal));
    }

    #[test]
    fn the_gate_holds_parked_requests_until_a_barrier_and_bounds_the_batch() {
        let file = Arc::new(Mutex::new(SimJournalFile::default()));
        let mut gate = Journaled::new(Box::new(
            DurableJournal::create(file.clone(), &header()).unwrap(),
        ));
        let durable = file.lock().unwrap().bytes().len();
        let read = |n| Request::ReadStdin { stdin: 3, max: n };
        for token in 0..BATCH_RECORDS as u64 - 1 {
            assert_eq!(gate.before(&read(token), Some(token)).unwrap(), Gate::Hold);
        }
        // Nothing reached the file: the batch is still open.
        assert_eq!(file.lock().unwrap().bytes().len(), durable);
        let last = BATCH_RECORDS as u64 - 1;
        assert_eq!(gate.before(&read(last), Some(last)).unwrap(), Gate::Hold);
        assert!(file.lock().unwrap().bytes().len() > durable);
        // Only the requests that were held and whose tasks parked are released.
        for token in 0..last {
            gate.parked(token);
        }
        assert_eq!(gate.released().unwrap(), (0..last).collect::<Vec<_>>());
        assert!(gate.released().unwrap().is_empty());
        assert_eq!(gate.before(&read(0), Some(0)).unwrap(), Gate::Issue);
        assert!(!gate.idle().unwrap());
        // The other levels never hold.
        let mut tape = Journaled::default();
        assert_eq!(tape.before(&read(0), Some(1)).unwrap(), Gate::Issue);
    }
}
