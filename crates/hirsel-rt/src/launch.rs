//! P8 launch: run one compiled entry in a fresh context with explicit grants,
//! then report what it observably did. Launch and tool failures use their own exit codes
//! and never impersonate a language outcome.
//!
//! The entry is a compiled `fn(rt: Io, args: Args) -> Result[(), E]` at the Hirsel ABI v1:
//! context, unused caller destination, the inline result, then the two handles by address according to their declared passing modes
//! (borrow, take or mut). The process exits 0 for `Ok`, 1 for `Error` and [`EXIT_FAILURE`] for a language
//! failure, with nothing printed beyond the program's own output.
//!
//! Grants are the launcher's explicit inputs, read once here and never by a semantic kernel:
//! `HIRSEL_RT_ROOT` (the granted directory, default the working directory),
//! `HIRSEL_RT_DIAGNOSTIC` (poisoning and header checks) and `HIRSEL_RT_OBSERVATION` (a file
//! that receives the observation record below), `HIRSEL_RT_TAPE` (a file that receives the
//! real Io completion tape) and `HIRSEL_RT_REPLAY` (a recorded tape: Io is then the
//! simulated adapter, which accepts exactly the recorded requests in order, and a mismatch,
//! a missing event and an unconsumed event all refuse the launch), and `HIRSEL_RT_NET`
//! (the root Net's grant: the JSON of a RunConfig's `io.net`, admitted by
//! `hirsel_run_config::validate_net_grant`; absent, the root Net permits nothing). A
//! `net.script@2` grant's peers serve the root Net's world on record and on replay; its
//! rules alone decide policy. Replay re-derives every network grant decision from it.
use crate::context::{
    Audit, Config as RuntimeConfig, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy,
    hirsel_rt_v2_task_exit,
};
use crate::failure::{
    FailureRecord, hirsel_rt_v2_failure_cause, hirsel_rt_v2_failure_cause_count,
    hirsel_rt_v2_failure_primary,
};
use crate::io::{
    Adapter, AdapterError, Binding, ClockError, Completion, Event, IoError, Request, Tree,
    hirsel_rt_v2_io_close,
};
use crate::io_real::RealIo;
use crate::io_sim::SimIo;
use crate::net::NetRule;
use crate::{ABI_VERSION, Layout, RunCtx, STATUS_FAILURE, STATUS_OK};
use std::cell::RefCell;
use std::fmt::Write as _;
use std::rc::Rc;

/// The observation record's format version (`hirsel-observation <n>` on its first line).
pub const OBSERVATION_VERSION: u32 = 1;
/// Process exit code of a language failure (FAIL-1).
pub const EXIT_FAILURE: i32 = 2;
/// Process exit code of a refused launch: bad grants, ABI or entry contract.
pub const EXIT_LAUNCH: i32 = 3;
/// The logical identity of the `Io` root (`Handles::default`).
pub const IO_HANDLE: u64 = 1;
/// The logical identity of the launch argument-table capability.
pub const ARGS_HANDLE: u64 = 0;
/// Sole root of a Test launch; its recorded seed is an explicit launcher input.
pub const TEST_HANDLE: u64 = 1;
pub type TestEntry = unsafe extern "C" fn(*mut RunCtx, *mut u8, *mut u8, *const u64) -> u8;
/// A root-free entry, with only the ordinary hidden context and result addresses.
pub type ZeroEntry = unsafe extern "C" fn(*mut RunCtx, *mut u8, *mut u8) -> u8;
enum Launch {
    Zero(ZeroEntry),
    Main(Entry),
    Test(TestEntry),
}
/// The largest inline result an entry may return.
pub const RESULT_MAX: u64 = 64;

/// A compiled entry at the Hirsel ABI v1 (module documentation).
pub type Entry = unsafe extern "C" fn(
    ctx: *mut RunCtx,
    destination: *mut u8,
    out: *mut u8,
    rt: *const u64,
    args: *const u64,
) -> u8;

/// The tape file's first line.
pub const TAPE_HEADER: &str = "hirsel-io-tape 4";

/// An adapter the launcher still holds after binding it.
struct Shared<A>(Rc<RefCell<A>>);

impl<A: Adapter> Adapter for Shared<A> {
    fn tls_roots(&mut self, pem: &[u8]) {
        self.0.borrow_mut().tls_roots(pem);
    }
    fn replay_stdout(&self) -> Vec<u8> {
        self.0.borrow().replay_stdout()
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        self.0.borrow_mut().call(request)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Complete typed completion tape. Hex fields preserve empty paths and arbitrary bytes.
/// Faults include their committed prefix, so replay reconstructs partial effects exactly.
pub fn render_tape(tape: &[Event]) -> Result<String, String> {
    fn outcome<T>(value: &Result<T, IoError>, ok: impl FnOnce(&T) -> String) -> String {
        match value {
            Ok(value) => format!("ok {}", ok(value)),
            Err(IoError::Fault(code)) => format!("fault {code}"),
            Err(IoError::Canceled(code)) => format!("canceled {code}"),
            Err(IoError::NotGranted) => "not-granted".into(),
        }
    }
    let mut text = format!("{TAPE_HEADER}\n");
    for event in tape {
        let request = match &event.request {
            Request::Stdout { io } => format!("stdout {io}"),
            Request::MonotonicClock { io } => format!("monotonic_clock {io}"),
            Request::WallClock { io } => format!("wall_clock {io}"),
            Request::Monotonic { clock } => format!("monotonic {clock}"),
            Request::Wall { clock } => format!("wall {clock}"),
            Request::Random { io } => format!("random {io}"),
            Request::Tasks { io } => format!("tasks {io}"),
            Request::Net { io } => format!("net {io}"),
            Request::Directory { io, path } => format!("directory {io} {}", hex(path.as_bytes())),
            Request::NarrowDir { directory, path } => {
                format!("narrow_dir {directory} {}", hex(path.as_bytes()))
            }
            Request::Print { stdout, bytes } => format!("print {stdout} {}", hex(bytes)),
            Request::Open { directory, path } => {
                format!("open {directory} {}", hex(path.as_bytes()))
            }
            Request::NarrowFile { directory, file } => format!("narrow_file {directory} {file}"),
            Request::Walk { directory } => format!("walk {directory}"),
            Request::Size { directory, path } => {
                format!("size {directory} {}", hex(path.as_bytes()))
            }
            Request::Read { directory, path } => {
                format!("read {directory} {}", hex(path.as_bytes()))
            }
            Request::Write {
                directory,
                path,
                bytes,
            } => format!("write {directory} {} {}", hex(path.as_bytes()), hex(bytes)),
            Request::Remove { directory, path } => {
                format!("remove {directory} {}", hex(path.as_bytes()))
            }
            Request::CreateAtomic { directory, path } => {
                format!("atomic {directory} {}", hex(path.as_bytes()))
            }
            Request::WriteAll {
                directory,
                file,
                bytes,
            } => format!("write_all {directory} {file} {}", hex(bytes)),
            Request::Commit { directory, file } => format!("commit {directory} {file}"),
            Request::Close { handle } => format!("close {handle}"),
            Request::TestCheck { test, condition } => {
                format!("check {test} {}", u8::from(*condition))
            }
            Request::TestCheckCase {
                test,
                condition,
                case,
            } => {
                format!("check_case {test} {} {case}", u8::from(*condition))
            }
            Request::TestFail { test } => format!("fail {test}"),
            Request::TestDir { test, entries } => format!(
                "test_dir {test} {}",
                entries
                    .iter()
                    .map(|(p, b)| format!("{}:{}", hex(p.as_bytes()), hex(b)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            request => crate::net::tape::render_request(request),
        };
        let completion = match &event.completion {
            Completion::ClockGrant(v) => match v {
                Ok(id) => format!("clock_grant ok {id}"),
                Err(()) => "clock_grant denied".into(),
            },
            Completion::Monotonic(v) => clock_outcome("monotonic", v),
            Completion::Wall(v) => clock_outcome("wall", v),
            Completion::Handle(v) => format!("handle {}", outcome(v, u64::to_string)),
            Completion::OptionalFile(v) => format!(
                "optional {}",
                v.map_or_else(|| "none".into(), |n| n.to_string())
            ),
            Completion::Unit(v) => format!("unit {}", outcome(v, |()| "".into())),
            Completion::Size(v) => format!("size {}", outcome(v, u64::to_string)),
            Completion::Bytes(v) => format!("bytes {}", outcome(v, |b| hex(b))),
            Completion::Paths(v) => format!(
                "paths {}",
                outcome(v, |paths| paths
                    .iter()
                    .map(|p| hex(p.as_bytes()))
                    .collect::<Vec<_>>()
                    .join(","))
            ),
            Completion::Checked(v) => format!("checked {}", u8::from(*v)),
            Completion::Refused(id) => format!("refused {id}"),
            completion => crate::net::tape::render_completion(completion),
        };
        let fault = event
            .fault
            .as_ref()
            .map(|f| match f.error {
                IoError::Fault(code) => format!(" ! fault {code} {}", f.partial),
                IoError::Canceled(code) => format!(" ! canceled {code} {}", f.partial),
                IoError::NotGranted => " ! not-granted 0 0".into(),
            })
            .unwrap_or_default();
        let _ = writeln!(text, "{request} -> {completion}{fault}");
    }
    Ok(text)
}
/// Strict inverse of render_tape, including fault inputs.
pub fn parse_tape(text: &str) -> Result<Vec<Event>, String> {
    fn number<T: std::str::FromStr>(word: &str) -> Result<T, String> {
        word.parse()
            .map_err(|_| format!("bad tape number {word:?}"))
    }
    fn bytes(word: &str) -> Result<Vec<u8>, String> {
        if !word.is_ascii() || word.len() % 2 != 0 {
            return Err("bad tape bytes".into());
        }
        (0..word.len() / 2)
            .map(|i| {
                u8::from_str_radix(&word[2 * i..2 * i + 2], 16).map_err(|_| "bad tape bytes".into())
            })
            .collect()
    }
    fn string(word: &str) -> Result<String, String> {
        String::from_utf8(bytes(word)?).map_err(|_| "bad tape UTF-8".into())
    }
    fn boolean(word: &str) -> Result<bool, String> {
        match word {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err("bad tape boolean".into()),
        }
    }
    fn result<T>(
        words: &[&str],
        ok: impl FnOnce(&str) -> Result<T, String>,
    ) -> Result<Result<T, IoError>, String> {
        match words {
            ["ok", v] => Ok(Ok(ok(v)?)),
            ["not-granted"] => Ok(Err(IoError::NotGranted)),
            ["fault", code] => Ok(Err(IoError::Fault(number(code)?))),
            ["canceled", code] => Ok(Err(IoError::Canceled(number(code)?))),
            _ => Err("bad tape result".into()),
        }
    }
    let mut lines = text.lines();
    if lines.next() != Some(TAPE_HEADER) {
        return Err(format!(
            "not an Io tape: the first line is not `{TAPE_HEADER}`"
        ));
    }
    lines
        .map(|line| {
            let (line, fault) = match line.split_once(" ! ") {
                None => (line, None),
                Some((line, fault)) => {
                    let w: Vec<_> = fault.split(' ').collect();
                    let f = match w.as_slice() {
                        [kind, code, partial] => crate::io::Fault {
                            error: match *kind {
                                "fault" => IoError::Fault(number(code)?),
                                "canceled" => IoError::Canceled(number(code)?),
                                "not-granted" if *code == "0" => IoError::NotGranted,
                                _ => return Err("bad tape fault".into()),
                            },
                            partial: number(partial)?,
                        },
                        _ => return Err("bad tape fault".into()),
                    };
                    (line, Some(f))
                }
            };
            let (req, comp) = line.split_once(" -> ").ok_or("bad tape delimiter")?;
            let w: Vec<_> = req.split(' ').collect();
            let request = match w.as_slice() {
                ["stdout", io] => Request::Stdout { io: number(io)? },
                ["monotonic_clock", io] => Request::MonotonicClock { io: number(io)? },
                ["wall_clock", io] => Request::WallClock { io: number(io)? },
                ["monotonic", clock] => Request::Monotonic {
                    clock: number(clock)?,
                },
                ["wall", clock] => Request::Wall {
                    clock: number(clock)?,
                },
                ["random", io] => Request::Random { io: number(io)? },
                ["tasks", io] => Request::Tasks { io: number(io)? },
                ["net", io] => Request::Net { io: number(io)? },
                ["directory", io, p] => Request::Directory {
                    io: number(io)?,
                    path: string(p)?,
                },
                ["narrow_dir", d, p] => Request::NarrowDir {
                    directory: number(d)?,
                    path: string(p)?,
                },
                ["print", s, b] => Request::Print {
                    stdout: number(s)?,
                    bytes: bytes(b)?,
                },
                ["open", d, p] => Request::Open {
                    directory: number(d)?,
                    path: string(p)?,
                },
                ["narrow_file", d, f] => Request::NarrowFile {
                    directory: number(d)?,
                    file: number(f)?,
                },
                ["walk", d] => Request::Walk {
                    directory: number(d)?,
                },
                ["size", d, p] => Request::Size {
                    directory: number(d)?,
                    path: string(p)?,
                },
                ["read", d, p] => Request::Read {
                    directory: number(d)?,
                    path: string(p)?,
                },
                ["write", d, p, b] => Request::Write {
                    directory: number(d)?,
                    path: string(p)?,
                    bytes: bytes(b)?,
                },
                ["remove", d, p] => Request::Remove {
                    directory: number(d)?,
                    path: string(p)?,
                },
                ["atomic", d, p] => Request::CreateAtomic {
                    directory: number(d)?,
                    path: string(p)?,
                },
                ["write_all", d, f, b] => Request::WriteAll {
                    directory: number(d)?,
                    file: number(f)?,
                    bytes: bytes(b)?,
                },
                ["commit", d, f] => Request::Commit {
                    directory: number(d)?,
                    file: number(f)?,
                },
                ["close", h] => Request::Close { handle: number(h)? },
                ["check", t, c] => Request::TestCheck {
                    test: number(t)?,
                    condition: boolean(c)?,
                },
                ["check_case", t, c, index] => Request::TestCheckCase {
                    test: number(t)?,
                    condition: boolean(c)?,
                    case: number(index)?,
                },
                ["fail", t] => Request::TestFail { test: number(t)? },
                ["test_dir", t, entries] => Request::TestDir {
                    test: number(t)?,
                    entries: if entries.is_empty() {
                        vec![]
                    } else {
                        entries
                            .split(',')
                            .map(|e| {
                                let (p, b) = e.split_once(':').ok_or("bad tape test entry")?;
                                Ok((string(p)?, bytes(b)?))
                            })
                            .collect::<Result<_, String>>()?
                    },
                },
                words => match crate::net::tape::parse_request(words)? {
                    Some(request) => request,
                    None => return Err("bad tape request".into()),
                },
            };
            let w: Vec<_> = comp.split(' ').collect();
            let completion = match w.as_slice() {
                ["clock_grant", "ok", id] => Completion::ClockGrant(Ok(number(id)?)),
                ["clock_grant", "denied"] => Completion::ClockGrant(Err(())),
                ["monotonic", "ok", ticks] => Completion::Monotonic(Ok(number(ticks)?)),
                ["wall", "ok", ticks] => Completion::Wall(Ok(number(ticks)?)),
                ["monotonic", "unavailable"] => Completion::Monotonic(Err(ClockError::Unavailable)),
                ["wall", "unavailable"] => Completion::Wall(Err(ClockError::Unavailable)),
                ["monotonic", "out_of_range"] => Completion::Monotonic(Err(ClockError::OutOfRange)),
                ["wall", "out_of_range"] => Completion::Wall(Err(ClockError::OutOfRange)),
                ["handle", r @ ..] => Completion::Handle(result(r, number)?),
                ["optional", "none"] => Completion::OptionalFile(None),
                ["optional", id] => Completion::OptionalFile(Some(number(id)?)),
                ["unit", r @ ..] => Completion::Unit(result(r, |v| {
                    if v.is_empty() {
                        Ok(())
                    } else {
                        Err("bad unit result".into())
                    }
                })?),
                ["size", r @ ..] => Completion::Size(result(r, number)?),
                ["bytes", r @ ..] => Completion::Bytes(result(r, bytes)?),
                ["paths", r @ ..] => Completion::Paths(result(r, |v| {
                    if v.is_empty() {
                        Ok(vec![])
                    } else {
                        v.split(',').map(string).collect()
                    }
                })?),
                ["checked", v] => Completion::Checked(boolean(v)?),
                ["refused", id] => Completion::Refused(number(id)?),
                words => match crate::net::tape::parse_completion(words)? {
                    Some(completion) => completion,
                    None => return Err("bad tape completion".into()),
                },
            };
            Ok(Event {
                request,
                completion,
                fault,
            })
        })
        .collect()
}

/// A recorded launch: semantic inputs and initial tree accompany every completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recording {
    pub arguments: Vec<String>,
    pub run_identity: [u8; 32],
    pub hash_seed: u64,
    pub random_seed: u64,
    pub test_seed: u64,
    pub tree: Tree,
    pub events: Vec<Event>,
}
pub fn render_recording(record: &Recording) -> Result<String, String> {
    let mut text = format!(
        "{TAPE_HEADER}\nrun_identity {}\nhash_seed {}\nrandom_seed {}\ntest_seed {}\n",
        hex(&record.run_identity),
        record.hash_seed,
        record.random_seed,
        record.test_seed
    );
    for argument in &record.arguments {
        let _ = writeln!(text, "argument {}", hex(argument.as_bytes()));
    }
    for directory in &record.tree.directories {
        let _ = writeln!(text, "directory {}", hex(directory.as_bytes()));
    }
    for (path, bytes) in &record.tree.files {
        let _ = writeln!(text, "file {} {}", hex(path.as_bytes()), hex(bytes));
    }
    for (path, target) in &record.tree.links {
        let _ = writeln!(
            text,
            "link {} {}",
            hex(path.as_bytes()),
            hex(target.as_bytes())
        );
    }
    text.push_str("events\n");
    text.push_str(render_tape(&record.events)?.split_once('\n').unwrap().1);
    let _ = writeln!(text, "complete {}", record.events.len());
    Ok(text)
}
pub fn parse_recording(text: &str) -> Result<Recording, String> {
    let (recording, complete) = parse_recording_prefix(text)?;
    if !complete {
        return Err("incomplete ordinary recording: unknown suffix".into());
    }
    Ok(recording)
}
/// Inspect confirmed completions after an abort. Never authorizes host continuation.
/// Torn final records are rejected, not silently converted into a complete tape.
pub fn parse_recording_prefix(text: &str) -> Result<(Recording, bool), String> {
    fn bytes(word: &str) -> Result<Vec<u8>, String> {
        if !word.is_ascii() || word.len() % 2 != 0 {
            return Err("bad recording bytes".into());
        }
        (0..word.len() / 2)
            .map(|i| {
                u8::from_str_radix(&word[i * 2..i * 2 + 2], 16)
                    .map_err(|_| "bad recording bytes".into())
            })
            .collect()
    }
    fn string(word: &str) -> Result<String, String> {
        String::from_utf8(bytes(word)?).map_err(|_| "bad recording UTF-8".into())
    }
    let (metadata, events) = text
        .split_once("\nevents\n")
        .ok_or("recording without events delimiter")?;
    let mut lines = metadata.lines();
    if lines.next() != Some(TAPE_HEADER) {
        return Err(format!(
            "not a recording: the first line is not `{TAPE_HEADER}`"
        ));
    }
    let mut run_identity = None;
    let (mut hash_seed, mut random_seed, mut test_seed) = (None, None, None);
    let mut arguments = vec![];
    let mut files = std::collections::BTreeMap::new();
    let mut links = std::collections::BTreeMap::new();
    let mut directories = std::collections::BTreeSet::new();
    for line in lines {
        let words: Vec<_> = line.split(' ').collect();
        match words.as_slice() {
            ["run_identity", value] if run_identity.is_none() => {
                if value.len() != 64
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err("bad run identity digest".into());
                }
                run_identity = Some(
                    bytes(value)?
                        .try_into()
                        .map_err(|_| "bad run identity digest")?,
                );
            }
            ["hash_seed", value] if hash_seed.is_none() => {
                hash_seed = Some(value.parse().map_err(|_| "bad recorded hash seed")?)
            }
            ["random_seed", value] if random_seed.is_none() => {
                random_seed = Some(value.parse().map_err(|_| "bad recorded random seed")?)
            }
            ["test_seed", value] if test_seed.is_none() => {
                test_seed = Some(value.parse().map_err(|_| "bad recorded test seed")?)
            }
            ["argument", value] => arguments.push(string(value)?),
            ["directory", value] => {
                let path = string(value)?;
                if crate::io::relative("", &path).as_ref() != Some(&path)
                    || !directories.insert(path)
                {
                    return Err("bad recorded directory".into());
                }
            }
            ["link", path, target] => {
                let path = string(path)?;
                if path.is_empty()
                    || crate::io::relative("", &path).as_ref() != Some(&path)
                    || links.insert(path, string(target)?).is_some()
                {
                    return Err("bad recorded link".into());
                }
            }
            ["file", path, data] => {
                let path = string(path)?;
                if files.insert(path, bytes(data)?).is_some() {
                    return Err("duplicate recorded file".into());
                }
            }
            _ => return Err("bad recording input".into()),
        }
    }
    let mut tree = Tree::new(files).map_err(|_| "invalid recorded initial tree")?;
    if directories.iter().any(|d| tree.files.contains_key(d)) {
        return Err("recorded file/directory collision".into());
    }
    tree.directories.extend(directories);
    for path in links.keys() {
        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
        if tree.directories.contains(path)
            || tree.files.contains_key(path)
            || !tree.directories.contains(parent)
        {
            return Err("recorded link collision or missing parent".into());
        }
    }
    tree.links = links;
    if !text.ends_with('\n') {
        return Err("truncated recording record".into());
    }
    let (events, count) = match events.rsplit_once("complete ") {
        Some((events, count)) if events.is_empty() || events.ends_with('\n') => {
            let count = count.strip_suffix('\n').ok_or("bad complete trailer")?;
            (
                events,
                Some(count.parse::<usize>().map_err(|_| "bad complete count")?),
            )
        }
        _ => (events, None),
    };
    let events = parse_tape(&format!("{TAPE_HEADER}\n{events}"))?;
    if count.is_some_and(|count| count != events.len()) {
        return Err("complete count mismatch".into());
    }
    Ok((
        Recording {
            arguments,
            run_identity: run_identity.ok_or("missing run identity")?,
            hash_seed: hash_seed.ok_or("missing hash seed")?,
            random_seed: random_seed.ok_or("missing random seed")?,
            test_seed: test_seed.ok_or("missing test seed")?,
            tree,
            events,
        },
        count.is_some(),
    ))
}

/// Explicit launcher inputs, separate from allocation configuration. The driver
/// supplies these now; validated RunConfig admission will supply them at its seam.
/// Seeds never come from the environment or from one another.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Config {
    pub hash_seed: u64,
    pub random_seed: u64,
    pub test_seed: u64,
    pub run_identity: [u8; 32],
}

fn refuse(message: &str) -> i32 {
    eprintln!("hirsel-rt: launch refused: {message}");
    EXIT_LAUNCH
}

/// Launch a zero-parameter entry without acquiring any host root or argv snapshot.
/// # Safety
/// The entry has the ZeroEntry ABI and config points to a live Config.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_launch_zero(
    entry: ZeroEntry,
    result_size: u64,
    layouts: *const *const Layout,
    layout_count: u32,
    config: *const Config,
) -> i32 {
    unsafe {
        launch(
            Launch::Zero(entry),
            result_size,
            layouts,
            layout_count,
            config,
        )
    }
}

/// Launch `entry`, whose inline result is `result_size` bytes and starts with the u32
/// ordinal of its `Result`.
///
/// # Safety
/// `entry` is a compiled entry of the documented signature whose result fits `result_size`.
/// A non-null `config` points to a live, aligned Config for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_launch(
    entry: Entry,
    result_size: u64,
    layouts: *const *const Layout,
    layout_count: u32,
    config: *const Config,
) -> i32 {
    unsafe {
        launch(
            Launch::Main(entry),
            result_size,
            layouts,
            layout_count,
            config,
        )
    }
}
/// Launch an ordinary selected Test function with explicit independent seeds.
/// # Safety
/// The entry has the TestEntry ABI and config points to a live Config.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_launch_test(
    entry: TestEntry,
    result_size: u64,
    layouts: *const *const Layout,
    layout_count: u32,
    config: *const Config,
) -> i32 {
    unsafe {
        launch(
            Launch::Test(entry),
            result_size,
            layouts,
            layout_count,
            config,
        )
    }
}
unsafe fn launch(
    entry: Launch,
    result_size: u64,
    layouts: *const *const Layout,
    layout_count: u32,
    launch_config: *const Config,
) -> i32 {
    let Some(launch_config) = (unsafe { launch_config.as_ref() }) else {
        return refuse("missing launch configuration");
    };
    let seed = launch_config.test_seed;
    if !(4..=RESULT_MAX).contains(&result_size) {
        return refuse("the entry result is not a Result of at most 64 bytes");
    }
    let inherited_outputs = match crate::host_paths::inherited_output_pins() {
        Ok(pins) => pins,
        Err(error) => return refuse(&error.to_string()),
    };
    let output_grants: Vec<_> = inherited_outputs
        .iter()
        .map(|(name, file)| (*name, file))
        .collect();
    let mut arguments = if matches!(entry, Launch::Zero(_)) {
        vec![]
    } else {
        match std::env::args_os()
            .skip(1)
            .enumerate()
            .map(|(i, a)| a.into_string().map_err(|_| i))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(arguments) => arguments,
            Err(index) => return refuse(&format!("argument {index} is not UTF-8")),
        }
    };
    let net = match std::env::var("HIRSEL_RT_NET") {
        Ok(text) => match hirsel_run_config::validate_net_grant(text.as_bytes()) {
            Ok(grant) => Some(grant),
            Err(error) => return refuse(&format!("the network grant: {error}")),
        },
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return refuse("the network grant is not UTF-8");
        }
    };
    // net.script@2 peers serve the root Net's world; the rules alone decide policy.
    let net_rules = net.as_ref().map(NetRule::from_config).unwrap_or_default();
    let net_peers = net.and_then(|grant| grant.peers);
    let mut initial_tree = Tree::new(Default::default()).expect("empty tree");
    let mut real = None;
    let mut zero_recording = None;
    let mut replay = None;
    let adapter: Box<dyn Adapter> = match std::env::var_os("HIRSEL_RT_REPLAY") {
        Some(path) => {
            let tape = match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| parse_recording(&text))
            {
                Ok(recording) => recording,
                Err(error) => return refuse(&format!("the replayed tape: {error}")),
            };
            if tape.run_identity != launch_config.run_identity
                || tape.hash_seed != launch_config.hash_seed
                || tape.random_seed != launch_config.random_seed
                || tape.test_seed != launch_config.test_seed
            {
                return refuse("recording configuration mismatch");
            }
            arguments = tape.arguments;
            let mut io = SimIo::new(tape.tree, tape.events);
            io.set_net_rules(net_rules.clone());
            if let Some(peers) = net_peers.clone()
                && let Err(error) = io.set_net_script(peers)
            {
                return refuse(&format!("the network script: {error:?}"));
            }
            if matches!(entry, Launch::Test(_)) {
                io.test_context(seed);
            }
            let io = Rc::new(RefCell::new(io));
            replay = Some(io.clone());
            Box::new(Shared(io))
        }
        None if matches!(entry, Launch::Zero(_)) => {
            if let Some(path) = std::env::var_os("HIRSEL_RT_TAPE") {
                let record = Recording {
                    arguments: vec![],
                    run_identity: launch_config.run_identity,
                    hash_seed: launch_config.hash_seed,
                    random_seed: launch_config.random_seed,
                    test_seed: launch_config.test_seed,
                    tree: initial_tree.clone(),
                    events: vec![],
                };
                let file =
                    match crate::host_paths::RecorderDestination::pin(std::path::Path::new(&path))
                        .and_then(|destination| destination.open_with_files(&[], &output_grants))
                    {
                        Ok(file) => file,
                        Err(error) => return refuse(&error.to_string()),
                    };
                match RecordingWriter::new(file, &record) {
                    Ok(writer) => zero_recording = Some(writer),
                    Err(error) => return refuse(&format!("the Io tape: {error}")),
                }
            }
            Box::new(SimIo::new(initial_tree.clone(), vec![]))
        }
        None => match RealIo::grant_with_output(
            std::path::Path::new(&std::env::var_os("HIRSEL_RT_ROOT").unwrap_or_else(|| ".".into())),
            RawStdout,
        ) {
            Ok(mut io) => {
                io.set_net_rules(net_rules.clone());
                if let Some(peers) = net_peers.clone()
                    && let Err(error) = io.set_net_script(peers)
                {
                    return refuse(&format!("the network script: {error:?}"));
                }
                let tape = match std::env::var_os("HIRSEL_RT_TAPE") {
                    Some(path) => match io
                        .prepare_recording_with_files(std::path::Path::new(&path), &output_grants)
                    {
                        Ok(file) => Some(file),
                        Err(error) => return refuse(&error.to_string()),
                    },
                    None => None,
                };
                if tape.is_some() {
                    initial_tree = match io.snapshot() {
                        Ok(tree) => tree,
                        Err(error) => return refuse(&format!("initial recording tree: {error:?}")),
                    };
                }
                if let Some(file) = tape {
                    let record = Recording {
                        arguments: arguments.clone(),
                        run_identity: launch_config.run_identity,
                        hash_seed: launch_config.hash_seed,
                        random_seed: launch_config.random_seed,
                        test_seed: launch_config.test_seed,
                        tree: initial_tree.clone(),
                        events: vec![],
                    };
                    let writer = RecordingWriter::new(file, &record);
                    match writer {
                        Ok(writer) => io.record_to(writer),
                        Err(error) => return refuse(&format!("the Io tape: {error}")),
                    }
                }
                if matches!(entry, Launch::Test(_)) {
                    io.test_context(seed);
                }
                io.discard_history();
                let io = Rc::new(RefCell::new(io));
                real = Some(io.clone());
                Box::new(Shared(io))
            }
            Err(error) => return refuse(&format!("the granted directory: {error}")),
        },
    };
    let prepare_tool_file = |path: &std::path::Path| {
        if let Some(io) = &real {
            io.borrow()
                .prepare_recording_with_files(path, &output_grants)
        } else {
            crate::host_paths::RecorderDestination::pin(path)?.open_with_files(&[], &output_grants)
        }
    };
    let mut observation = match std::env::var_os("HIRSEL_RT_OBSERVATION")
        .map(|path| prepare_tool_file(std::path::Path::new(&path)))
        .transpose()
    {
        Ok(file) => file,
        Err(error) => return refuse(&error.to_string()),
    };
    #[cfg(feature = "measurement")]
    let measurement = match std::env::var_os("HIRSEL_RT_MEASUREMENT")
        .map(|path| prepare_tool_file(std::path::Path::new(&path)))
        .transpose()
    {
        Ok(file) => file,
        Err(error) => return refuse(&error.to_string()),
    };
    #[cfg(feature = "measurement")]
    let profile = match std::env::var_os("HIRSEL_RT_PROFILE")
        .or_else(|| {
            std::env::var_os("HIRSEL_RT_MEASUREMENT").map(|mut path| {
                path.push(".sites.json");
                path
            })
        })
        .map(|path| prepare_tool_file(std::path::Path::new(&path)))
        .transpose()
    {
        Ok(file) => file,
        Err(error) => return refuse(&error.to_string()),
    };
    let config = RuntimeConfig {
        layouts,
        layout_count,
        abi_version: ABI_VERSION,
        flags: if std::env::var_os("HIRSEL_RT_DIAGNOSTIC").is_some() {
            crate::context::CONFIG_DIAGNOSTIC
        } else {
            0
        },
        count_max: 0,
        heap_idle_cap: 0,
        close_handle: Some(hirsel_rt_v2_io_close),
        // Only placement receives the hash seed (SEM-5).
        hash_seed: launch_config.hash_seed,
        ..RuntimeConfig::new()
    };
    let mut ctx = std::ptr::null_mut();
    if unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) } != 0 {
        return refuse("the runtime configuration");
    }
    #[cfg(feature = "measurement")]
    if let Some(file) = measurement {
        unsafe { &mut *ctx }.set_measurement_output(file);
    }
    #[cfg(feature = "measurement")]
    if let Some(file) = profile {
        unsafe { &mut *ctx }.set_profile_output(file);
    }
    let mut binding = Box::new(Binding::from_box(adapter).with_arguments(arguments.clone()));
    unsafe { binding.bind(&mut *ctx) };
    // Zeroed and 16-aligned: the entry writes a result of any supported alignment.
    let mut result = [0u128; RESULT_MAX as usize / 16];
    let (mut rt, mut args) = (IO_HANDLE, ARGS_HANDLE);
    let status = unsafe {
        match entry {
            Launch::Zero(entry) => entry(ctx, std::ptr::null_mut(), result.as_mut_ptr().cast()),
            Launch::Main(entry) => entry(
                ctx,
                std::ptr::null_mut(),
                result.as_mut_ptr().cast(),
                &mut rt,
                &mut args,
            ),
            Launch::Test(entry) => entry(
                ctx,
                std::ptr::null_mut(),
                result.as_mut_ptr().cast(),
                &TEST_HANDLE,
            ),
        }
    };
    let bytes: Vec<u8> = result
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .take(result_size as usize)
        .collect();
    let mut record = format!("hirsel-observation {OBSERVATION_VERSION}\nstatus {status}\n");
    let exit = match status {
        STATUS_OK => {
            record.push_str("result ");
            for b in &bytes {
                let _ = write!(record, "{b:02x}");
            }
            record.push('\n');
            i32::from(bytes[..4] != [0, 0, 0, 0])
        }
        STATUS_FAILURE => {
            let mut failure = FailureRecord {
                kind: 0,
                rule: 0,
                site: 0,
            };
            if unsafe { hirsel_rt_v2_failure_primary(ctx, &mut failure) } == 0 {
                crate::fault("a failure status without a recorded failure");
            }
            let _ = writeln!(
                record,
                "failure {} {} {:016x}",
                failure.kind, failure.rule, failure.site
            );
            for index in 0..unsafe { hirsel_rt_v2_failure_cause_count(ctx) } {
                if unsafe { hirsel_rt_v2_failure_cause(ctx, index, &mut failure) } == 0 {
                    crate::fault("a failure cause out of range");
                }
                let _ = writeln!(
                    record,
                    "failure {} {} {:016x}",
                    failure.kind, failure.rule, failure.site
                );
            }
            EXIT_FAILURE
        }
        _ => crate::fault("an entry returned a status other than 0 or 1"),
    };
    let mut audit = Audit {
        live_blocks: 0,
        live_bytes: 0,
        immortalized_blocks: 0,
        active_regions: 0,
        retained_bytes: 0,
    };
    unsafe { hirsel_rt_v2_task_exit(ctx, &mut audit) };
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    let _ = writeln!(
        record,
        "audit {} {} {} {} {}",
        audit.live_blocks,
        audit.live_bytes,
        audit.immortalized_blocks,
        audit.active_regions,
        audit.retained_bytes
    );
    #[cfg(not(any(feature = "diagnostic", feature = "measurement")))]
    record.push_str("audit unavailable\n");
    unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
    drop(binding);
    if let Some(io) = &replay {
        // The simulated adapter holds what the program printed; every recorded event
        // must have been consumed.
        let io = io.borrow();
        if let Err(error) = std::io::Write::write_all(&mut std::io::stdout(), io.stdout()) {
            return refuse(&format!("standard output: {error}"));
        }
        if let Err(error) = io.finish() {
            let _ = std::io::Write::flush(&mut std::io::stdout());
            return refuse(&format!("the replayed tape: {error:?}"));
        }
    }
    // Rust's stdout is line buffered and a C `main` returning does not flush it: output
    // after the last newline would be lost.
    if let Err(error) = std::io::Write::flush(&mut std::io::stdout()) {
        return refuse(&format!("standard output: {error}"));
    }
    if let Some(io) = &real {
        if let Err(error) = io.borrow_mut().finish_recording() {
            return refuse(&format!("the Io tape: {error}"));
        }
    }
    if let Some(writer) = &mut zero_recording
        && let Err(error) = writer.finish()
    {
        return refuse(&format!("the Io tape: {error}"));
    }
    if let Some(file) = &mut observation
        && let Err(error) = std::io::Write::write_all(file, record.as_bytes())
            .and_then(|()| std::io::Write::flush(file))
    {
        return refuse(&format!("the observation file: {error}"));
    }
    exit
}

/// Ordinary flushed streaming tape, explicitly nonresumable after interruption.
/// The header is flushed before entry; each completion is flushed before delivery.
pub struct RecordingWriter<W: std::io::Write> {
    writer: W,
    count: usize,
    finished: bool,
}
impl<W: std::io::Write> RecordingWriter<W> {
    pub fn new(mut writer: W, record: &Recording) -> std::io::Result<Self> {
        if !record.events.is_empty() {
            return Err(std::io::Error::other("stream header contains events"));
        }
        let text = render_recording(record).map_err(std::io::Error::other)?;
        let header = text
            .strip_suffix("complete 0\n")
            .expect("empty recording trailer");
        writer.write_all(header.as_bytes())?;
        writer.flush()?;
        Ok(Self {
            writer,
            count: 0,
            finished: false,
        })
    }
    pub fn append(&mut self, event: &Event) -> std::io::Result<()> {
        if self.finished {
            return Err(std::io::Error::other("finished recording"));
        }
        let text = render_tape(std::slice::from_ref(event)).map_err(std::io::Error::other)?;
        self.writer
            .write_all(text.split_once('\n').unwrap().1.as_bytes())?;
        self.writer.flush()?;
        self.count += 1;
        Ok(())
    }
    pub fn finish(&mut self) -> std::io::Result<()> {
        if self.finished {
            return Err(std::io::Error::other("finished recording"));
        }
        writeln!(self.writer, "complete {}", self.count)?;
        self.writer.flush()?;
        self.finished = true;
        Ok(())
    }
}

/// Unbuffered OS boundary: unlike std::io::stdout, EBADF is never suppressed.
struct RawStdout;
impl std::io::Write for RawStdout {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        // SAFETY: borrowed readable bytes; libc never retains their pointer.
        let n = unsafe { libc::write(libc::STDOUT_FILENO, bytes.as_ptr().cast(), bytes.len()) };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn clock_outcome<T: std::fmt::Display>(kind: &str, value: &Result<T, ClockError>) -> String {
    match value {
        Ok(n) => format!("{kind} ok {n}"),
        Err(ClockError::Unavailable) => format!("{kind} unavailable"),
        Err(ClockError::OutOfRange) => format!("{kind} out_of_range"),
    }
}
