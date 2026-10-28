//! P8 launch: run one compiled entry in a fresh context with explicit grants,
//! then report what it observably did. Launch and tool failures use their own exit codes
//! and never impersonate a language outcome.
//!
//! The entry is a compiled `fn(rt: Io, args: Args) -> Result[(), E]` at the Hirsel ABI v2:
//! context, unused caller destination, the inline result, then the two handles by address according to their declared passing modes
//! (borrow, take or mut). The process exits 0 for `Ok`, 1 for `Error` and [`EXIT_FAILURE`] for a language
//! failure, with nothing printed beyond the program's own output.
//!
//! A production launch takes its grants from a run record (`--hirsel-run`, or a template
//! with `--hirsel-grants`), optionally preceded by `--hirsel-record <tape>`, or replays a
//! tape alone with `--hirsel-replay <tape>`; the tape binds its own run record.
//!
//! Harness launches without a record take explicit inputs from the environment, read once
//! here and never by a semantic kernel:
//! `HIRSEL_RT_ROOT` (the fixture profile's read-write directory beside every Io projection;
//! absent, the root Io grants no mount and no projection),
//! `HIRSEL_RT_DIAGNOSTIC` (poisoning and header checks; with it, in the diagnostic archive,
//! `HIRSEL_RT_TRACE_ROOTS` names a file receiving every live `Shared` root's count at every
//! scheduling point, `crate::root_trace`, `HIRSEL_RT_COMPACT_TRACE` names a file receiving
//! every compact frame's leave and activation, `crate::compact_trace`, and
//! `HIRSEL_RT_COUNT_MAX` sets the run's count bound,
//! `Config::count_max`, so roots reach the exact overflow table) and `HIRSEL_RT_OBSERVATION` (a file
//! that receives the observation record below), `HIRSEL_RT_TAPE` (a file that receives the
//! real Io completion tape) and `HIRSEL_RT_REPLAY` (a recorded tape: Io is then the
//! simulated adapter, which accepts exactly the recorded requests in order, and a mismatch,
//! a missing event and an unconsumed event all refuse the launch), and `HIRSEL_RT_NET`
//! (the root Net's grant: the JSON of a RunConfig's `io.net`, admitted by
//! `hirsel_run_config::validate_net_grant`; absent, the root Net permits nothing). A
//! `net.script@2` grant's peers serve the root Net's world on record and on replay; its
//! rules alone decide policy. Replay re-derives every network grant decision from it.
//! `HIRSEL_RT_FREE_PROJECTION` optionally writes the full normalized Free
//! projection as a host-test diagnostic. It never selects admission, scheduling,
//! recording contents or replay decisions; it is compiled in ordinary archives.
use crate::context::{
    Audit, Config as RuntimeConfig, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy,
    hirsel_rt_v2_task_exit,
};
use crate::failure::{
    FailureRecord, hirsel_rt_v2_failure_cause, hirsel_rt_v2_failure_cause_count,
    hirsel_rt_v2_failure_primary,
};
use crate::io::{
    Adapter, AdapterError, Binding, ClockError, Completion, Event, IoError, Ready, Request, Tree,
    hirsel_rt_v2_io_close,
};
use crate::io_real::RealIo;
use crate::io_sim::SimIo;
use crate::net::NetRule;
use crate::program::{Code, Program, ProgramDescriptor};
use crate::{ABI_VERSION, Layout, STATUS_FAILURE, STATUS_OK, TaskCtx};
use hirsel_run_schema::{RunOutcome, hex};
use std::cell::RefCell;
use std::fmt::Write as _;
use std::io::Write as _;
use std::rc::Rc;

/// The observation record's format version (`hirsel-observation <n>` on its first line).
pub const OBSERVATION_VERSION: u32 = 7;

/// The logical identity of the `Io` root (`Handles::default`).
pub const IO_HANDLE: u64 = 1;
/// The logical identity of the launch argument-table capability.
pub const ARGS_HANDLE: u64 = 0;
/// Sole root of a Test launch; its recorded seed is an explicit launcher input.
pub(crate) const TEST_HANDLE: u64 = 1;
pub type TestEntry = unsafe extern "C" fn(*mut TaskCtx, *mut u8, *mut u8, *const u64) -> u8;
/// A root-free entry, with only the ordinary hidden context and result addresses.
pub type ZeroEntry = unsafe extern "C" fn(*mut TaskCtx, *mut u8, *mut u8) -> u8;
/// Uniform compiler-generated adapter: roots are [Io, Args, Env, Unsafe, Test].
pub type RootsEntry = unsafe extern "C" fn(*mut TaskCtx, *mut u8, *mut u8, *mut u64) -> u8;
/// The largest inline result an entry may return.
pub const RESULT_MAX: u64 = 64;

/// A compiled entry at the Hirsel ABI v2 (module documentation).
pub type Entry = unsafe extern "C" fn(
    ctx: *mut TaskCtx,
    destination: *mut u8,
    out: *mut u8,
    rt: *const u64,
    args: *const u64,
) -> u8;

/// The legacy inspection syntax. On-disk writers use binary format 9; this renderer
/// remains readable for humans and for v6/v7 text imports.
pub(crate) const TAPE_HEADER: &str = "hirsel-io-tape 7";
const TAPE_HEADER_6: &str = "hirsel-io-tape 6";
fn tape_header(line: Option<&str>) -> bool {
    line.is_some_and(|line| line == TAPE_HEADER || line == TAPE_HEADER_6)
}

/// The home binding of a live Ordered run whose RealIo moved to its concurrent
/// service. Requests enter the service; scope deadlines, in-process runner
/// calls and their delivery still address the home binding and reach the same
/// core, under home tokens disjoint from the service's.
struct OrderedHome {
    io: std::sync::Arc<crate::io_real::ConcurrentIo>,
    /// The entered home token, as `RealIo::enter` keeps its current one.
    current: u64,
}

impl Adapter for OrderedHome {
    fn cpu_summary(&mut self, summary: &crate::io::CpuSummary) -> Result<(), AdapterError> {
        self.io.cpu_summary(summary)
    }

    fn telemetry_context(&self, telemetry: u64, span: u64) -> Option<Vec<u8>> {
        self.io.telemetry_context(telemetry, span)
    }

    fn cpu_control(
        &mut self,
        width: u32,
        action: u32,
        count: u64,
        value: Option<u32>,
    ) -> Result<Option<u32>, AdapterError> {
        self.io.cpu_control(width, action, count, value)
    }

    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        crate::io_concurrent::ConcurrentAdapter::preflight(&*self.io, request)
    }
    fn enter(&mut self, token: u64) {
        self.current = token | crate::io_real::HOME_TOKEN;
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        crate::io_concurrent::ConcurrentAdapter::call_at(&*self.io, self.current, request)
    }
    fn parked(&mut self, token: u64) {
        crate::io_concurrent::ConcurrentAdapter::parked(
            &*self.io,
            token | crate::io_real::HOME_TOKEN,
        );
    }
    fn finish(&mut self, token: u64) {
        crate::io_concurrent::ConcurrentAdapter::finish(
            &*self.io,
            token | crate::io_real::HOME_TOKEN,
        );
    }
    fn cancellable(&self, token: u64) -> bool {
        crate::io_concurrent::ConcurrentAdapter::cancellable(
            &*self.io,
            token | crate::io_real::HOME_TOKEN,
        )
    }
    fn runner_readiness(&mut self) -> Option<crate::readiness::Readiness> {
        crate::io_concurrent::ConcurrentAdapter::readiness(&*self.io)
    }
    /// Only the home binding's own ready tokens, as RealIo reported them: a
    /// conservative Every would re-issue, and record, every armed timer.
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        self.io.ready_for(true)
    }
    fn deliver(&mut self, token: u64, completion: hirsel_run_schema::RunnerCompletion) {
        self.io
            .deliver(token | crate::io_real::HOME_TOKEN, completion);
    }
    /// With no service row pending, home waits here for its own timers and
    /// runner deliveries: the same core sources, journal flush included.
    fn wait(&mut self) -> Result<bool, AdapterError> {
        crate::io_concurrent::ConcurrentAdapter::wait_sources(&*self.io)?.wait()
    }
    fn wake_on(&mut self, fd: std::os::fd::RawFd) -> bool {
        crate::io_concurrent::ConcurrentAdapter::readiness(&*self.io)
            .is_some_and(|readiness| readiness.wake_on(fd).is_ok())
    }
    /// Home waits (scope deadlines, runner sends) register on the readiness
    /// the service shares, so a mixed wait exposes that one driver.
    fn wait_sources(&mut self) -> Result<Option<crate::io::WaitSources>, AdapterError> {
        Ok(Some(crate::io::WaitSources::Drivers(
            crate::io_concurrent::ConcurrentAdapter::readiness(&*self.io)
                .into_iter()
                .collect(),
        )))
    }
}

/// An adapter the launcher still holds after binding it.
pub(crate) struct Shared<A>(pub(crate) Rc<RefCell<A>>);

impl<A: Adapter> Adapter for Shared<A> {
    fn batches_issue(&self) -> bool {
        self.0.borrow().batches_issue()
    }
    fn issue_batch(
        &mut self,
        request: Request,
        token: u64,
        task: &[u32],
    ) -> Result<Completion, AdapterError> {
        self.0.borrow_mut().issue_batch(request, token, task)
    }

    fn keys(&self, request: &Request) -> Option<crate::io_keys::KeySet> {
        self.0.borrow().keys(request)
    }
    fn domains(&self, request: &Request) -> Vec<(crate::io_keys::Key, u64, String)> {
        self.0.borrow().domains(request)
    }
    fn identity(&self, handle: u64) -> Option<(crate::io_keys::Key, crate::io_keys::Kind)> {
        self.0.borrow().identity(handle)
    }
    fn print_observed(
        &mut self,
        handle: u64,
        stderr: bool,
        bytes: &[u8],
    ) -> (Result<Completion, AdapterError>, Option<u64>) {
        self.0.borrow_mut().print_observed(handle, stderr, bytes)
    }
    fn print(
        &mut self,
        handle: u64,
        stderr: bool,
        bytes: &[u8],
    ) -> Result<Completion, AdapterError> {
        self.0.borrow_mut().print(handle, stderr, bytes)
    }
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        self.0.borrow().preflight(request)
    }
    fn tls(&mut self, engine: &'static dyn crate::net_tls::TlsEngine) {
        self.0.borrow_mut().tls(engine);
    }
    fn net(&mut self, host: crate::net_host::Provider) {
        self.0.borrow_mut().net(host);
    }
    fn process(&mut self, host: fn() -> Box<dyn crate::process::ProcessHost>) {
        self.0.borrow_mut().process(host);
    }
    fn tls_roots(&mut self, stream: u64, pem: &[u8]) {
        self.0.borrow_mut().tls_roots(stream, pem);
    }
    fn tls_identity(&mut self, stream: u64, chain: &[u8], key: &[u8]) {
        self.0.borrow_mut().tls_identity(stream, chain, key);
    }
    fn tls_server(
        &mut self,
        stream: u64,
        identities: &[(&[u8], &[u8])],
        clients: crate::net::ClientAuth<&[u8]>,
        alpn: &[String],
    ) {
        self.0
            .borrow_mut()
            .tls_server(stream, identities, clients, alpn);
    }
    fn replay_stderr(&self) -> Vec<u8> {
        self.0.borrow().replay_stderr()
    }
    fn replay_stdout(&self) -> Vec<u8> {
        self.0.borrow().replay_stdout()
    }
    fn net_io(
        &mut self,
        token: u64,
        net: u64,
        stream: u64,
        inline: bool,
        buffer: &mut crate::io::NetBuffer<'_>,
    ) -> Option<Result<crate::io::NetResult, AdapterError>> {
        self.0
            .borrow_mut()
            .net_io(token, net, stream, inline, buffer)
    }

    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        self.0.borrow_mut().call(request)
    }
    fn wants_issuer(&self) -> bool {
        self.0.borrow().wants_issuer()
    }
    fn issuer(&mut self, task: &[u32]) {
        self.0.borrow_mut().issuer(task);
    }
    fn enter(&mut self, token: u64) {
        self.0.borrow_mut().enter(token);
    }
    fn finish(&mut self, token: u64) {
        self.0.borrow_mut().finish(token);
    }
    fn cancellable(&self, token: u64) -> bool {
        self.0.borrow().cancellable(token)
    }
    fn cpu_summary(&mut self, summary: &crate::io::CpuSummary) -> Result<(), AdapterError> {
        self.0.borrow_mut().cpu_summary(summary)
    }
    fn cpu_control(
        &mut self,
        width: u32,
        action: u32,
        count: u64,
        value: Option<u32>,
    ) -> Result<Option<u32>, AdapterError> {
        self.0.borrow_mut().cpu_control(width, action, count, value)
    }
    fn schedule(&mut self, count: u64, choice: u64) -> Result<u64, AdapterError> {
        self.0.borrow_mut().schedule(count, choice)
    }
    fn parked(&mut self, token: u64) {
        self.0.borrow_mut().parked(token);
    }
    fn injected_wait(&self, request: &Request) -> bool {
        self.0.borrow().injected_wait(request)
    }
    fn telemetry_context(&self, telemetry: u64, span: u64) -> Option<Vec<u8>> {
        self.0.borrow().telemetry_context(telemetry, span)
    }
    fn isolated_clock(&self) -> bool {
        self.0.borrow().isolated_clock()
    }
    fn process_site(&mut self, child: u64, site: u64, operation_site: u64, path: &[u32]) {
        self.0
            .borrow_mut()
            .process_site(child, site, operation_site, path)
    }
    fn process_sources(&self) -> bool {
        self.0.borrow().process_sources()
    }
    fn process_batch(&mut self, canceled: &[u64]) -> Result<Ready, AdapterError> {
        self.0.borrow_mut().process_batch(canceled)
    }
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        self.0.borrow_mut().ready()
    }
    fn ready_deadline(&self) -> Option<std::time::Instant> {
        self.0.borrow().ready_deadline()
    }
    fn advance_ready_points(&mut self, points: u64) {
        self.0.borrow_mut().advance_ready_points(points);
    }
    fn has_ready(&mut self) -> Result<bool, AdapterError> {
        self.0.borrow_mut().has_ready()
    }
    fn runner_readiness(&mut self) -> Option<crate::readiness::Readiness> {
        self.0.borrow_mut().runner_readiness()
    }
    fn deliver(&mut self, token: u64, completion: hirsel_run_schema::RunnerCompletion) {
        self.0.borrow_mut().deliver(token, completion);
    }
    fn wait_sources(&mut self) -> Result<Option<crate::io::WaitSources>, AdapterError> {
        self.0.borrow_mut().wait_sources()
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        self.0.borrow_mut().wait()
    }
    fn scripted_input(&self) -> bool {
        self.0.borrow().scripted_input()
    }
}

/// Complete typed completion tape. Hex fields preserve empty paths and arbitrary bytes.
/// Faults include their committed prefix, so replay reconstructs partial effects exactly.
fn render_metadata(meta: &crate::io::Metadata) -> String {
    format!(
        "{},{},{},{}",
        meta.kind as u32,
        meta.size,
        meta.modified_unix_ns
            .map_or_else(|| "none".into(), |n| n.to_string()),
        meta.permissions
    )
}
fn parse_kind(text: &str) -> Result<crate::io::FileKind, String> {
    use crate::io::FileKind::*;
    match text {
        "0" => Ok(Regular),
        "1" => Ok(Directory),
        "2" => Ok(Symlink),
        "3" => Ok(Other),
        _ => Err("bad file kind".into()),
    }
}
fn parse_metadata(text: &str) -> Result<crate::io::Metadata, String> {
    let fields: Vec<_> = text.split(',').collect();
    let [kind, size, modified, permissions] = fields.as_slice() else {
        return Err("bad metadata".into());
    };
    Ok(crate::io::Metadata {
        permissions: permissions
            .parse()
            .map_err(|_| "bad metadata permissions")?,
        kind: parse_kind(kind)?,
        size: size.parse().map_err(|_| "bad metadata size")?,
        modified_unix_ns: if *modified == "none" {
            None
        } else {
            Some(modified.parse().map_err(|_| "bad metadata timestamp")?)
        },
    })
}
/// A watch event as one tape word: its kind, then each path in hex, `:`-separated.
fn render_watch_event(event: &crate::watch::WatchEvent) -> String {
    use crate::watch::WatchEvent::*;
    match event {
        Created(p) => format!("created:{}", hex(p.as_bytes())),
        Modified(p) => format!("modified:{}", hex(p.as_bytes())),
        Removed(p) => format!("removed:{}", hex(p.as_bytes())),
        Renamed(a, b) => format!("renamed:{}:{}", hex(a.as_bytes()), hex(b.as_bytes())),
        Overflow => "overflow".into(),
    }
}
fn parse_watch_event(word: &str) -> Result<crate::watch::WatchEvent, String> {
    use crate::watch::WatchEvent::*;
    let path = |word: &str| -> Result<String, String> {
        if !word.is_ascii() || word.len() % 2 != 0 {
            return Err("bad tape watch path".into());
        }
        let bytes = (0..word.len() / 2)
            .map(|i| u8::from_str_radix(&word[2 * i..2 * i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(|_| "bad tape watch path")?;
        String::from_utf8(bytes).map_err(|_| "bad tape watch path".into())
    };
    Ok(match word.split(':').collect::<Vec<_>>().as_slice() {
        ["created", p] => Created(path(p)?),
        ["modified", p] => Modified(path(p)?),
        ["removed", p] => Removed(path(p)?),
        ["renamed", a, b] => Renamed(path(a)?, path(b)?),
        ["overflow"] => Overflow,
        _ => return Err("bad tape watch event".into()),
    })
}
pub fn render_tape(tape: &[Event]) -> Result<String, String> {
    fn outcome<T>(value: &Result<T, IoError>, ok: impl FnOnce(&T) -> String) -> String {
        match value {
            Ok(value) => format!("ok {}", ok(value)),
            Err(IoError::Fault(code)) => format!("fault {code}"),
            Err(IoError::Canceled(code)) => format!("canceled {code}"),
            Err(IoError::NotGranted) => "not-granted".into(),
            Err(IoError::NotFound) => "NotFound".into(),
            Err(IoError::PermissionDenied) => "PermissionDenied".into(),
            Err(IoError::AlreadyExists) => "AlreadyExists".into(),
            Err(IoError::IsDirectory) => "IsDirectory".into(),
            Err(IoError::NotDirectory) => "NotDirectory".into(),
            Err(IoError::DirectoryNotEmpty) => "DirectoryNotEmpty".into(),
            Err(IoError::CrossDevice) => "CrossDevice".into(),
            Err(IoError::Truncated) => "Truncated".into(),
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
            Request::ReadOnlyDir { directory } => format!("read_only_dir {directory}"),
            Request::Print { stdout, bytes } => format!("print {stdout} {}", hex(bytes)),
            Request::Open { directory, path } => {
                format!("open {directory} {}", hex(path.as_bytes()))
            }
            Request::OpenAppend { directory, path } => {
                format!("open_append {directory} {}", hex(path.as_bytes()))
            }
            Request::Create { directory, path } => {
                format!("create {directory} {}", hex(path.as_bytes()))
            }
            Request::Map { directory, path } => {
                format!("map {directory} {}", hex(path.as_bytes()))
            }
            Request::MapView { directory, mapped } => format!("map_view {directory} {mapped}"),
            Request::MapCheck { directory, mapped } => format!("map_check {directory} {mapped}"),
            Request::Watch {
                directory,
                path,
                recursive,
            } => format!(
                "watch {directory} {} {}",
                hex(path.as_bytes()),
                u8::from(*recursive)
            ),
            Request::WatchNext { directory, watcher } => {
                format!("watch_next {directory} {watcher}")
            }
            Request::TestDirEvent {
                test,
                directory,
                event,
            } => format!(
                "test_dir_event {test} {directory} {}",
                render_watch_event(event)
            ),
            Request::FileRead {
                directory,
                file,
                max,
            } => format!("file_read {directory} {file} {max}"),
            Request::FileSeek {
                directory,
                file,
                offset,
            } => format!("file_seek {directory} {file} {offset}"),
            Request::FileWrite {
                directory,
                file,
                bytes,
            } => format!("file_write {directory} {file} {}", hex(bytes)),
            Request::FileAppend {
                directory,
                file,
                bytes,
            } => format!("file_append {directory} {file} {}", hex(bytes)),
            Request::NarrowFile { directory, file } => format!("narrow_file {directory} {file}"),
            Request::ReadDirPage {
                directory,
                path,
                after,
                recursive,
            } => {
                format!(
                    "read_dir_page {directory} {} {} {}",
                    hex(path.as_bytes()),
                    hex(after.as_bytes()),
                    u8::from(*recursive)
                )
            }
            Request::Metadata { directory, path } => {
                format!("metadata {directory} {}", hex(path.as_bytes()))
            }
            Request::Mkdir { directory, path } => {
                format!("mkdir {directory} {}", hex(path.as_bytes()))
            }
            Request::SetPermissions {
                directory,
                path,
                permissions,
            } => {
                format!(
                    "set_permissions {directory} {} {permissions}",
                    hex(path.as_bytes())
                )
            }
            Request::RemoveAll { directory, path } => {
                format!("remove_all {directory} {}", hex(path.as_bytes()))
            }
            Request::RemoveDir { directory, path } => {
                format!("remove_dir {directory} {}", hex(path.as_bytes()))
            }
            Request::Rename {
                directory,
                from,
                to,
            } => format!(
                "rename {directory} {} {}",
                hex(from.as_bytes()),
                hex(to.as_bytes())
            ),
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
            Request::Sync { directory, file } => format!("sync {directory} {file}"),
            Request::PutNew {
                directory,
                path,
                bytes,
                durable,
            } => format!(
                "put_new {directory} {} {} {}",
                hex(path.as_bytes()),
                hex(bytes),
                u8::from(*durable)
            ),
            Request::TestCrash {
                test,
                directory,
                seed,
            } => format!("test_crash {test} {directory} {seed}"),
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
            Request::TestClock { test } => format!("test_clock {test}"),
            Request::TestSeed { test } => format!("test_seed {test}"),
            Request::TestTasks { test } => format!("test_tasks {test}"),
            Request::TestLog { test, text } => format!("test_log {test} {}", hex(text.as_bytes())),
            Request::TestSnapshot { test, name, text } => format!(
                "test_snapshot {test} {} {}",
                hex(name.as_bytes()),
                hex(text.as_bytes())
            ),
            Request::TestDir { test, entries } => format!(
                "test_dir {test} {}",
                entries
                    .iter()
                    .map(|(p, b)| format!("{}:{}", hex(p.as_bytes()), hex(b)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            request => crate::host_io_tape::render_request(request)
                .unwrap_or_else(|| crate::net::tape::render_request(request)),
        };
        let completion = match &event.completion {
            Completion::FileInput(input) => match input {
                crate::io::FileInput::Opened(id) => format!("file_input opened {id}"),
                crate::io::FileInput::Size(size) => format!("file_input size {size}"),
                crate::io::FileInput::Bytes(bytes) => format!("file_input bytes {}", hex(bytes)),
            },
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
            Completion::Metadata(v) => format!("metadata {}", outcome(v, render_metadata)),
            Completion::Entries(v) => format!(
                "entries {}",
                outcome(v, |entries| entries
                    .iter()
                    .map(|e| format!("{}:{}", hex(e.path.as_bytes()), e.kind as u32))
                    .collect::<Vec<_>>()
                    .join(","))
            ),
            Completion::Checked(v) => format!("checked {}", u8::from(*v)),
            Completion::Refused(id) => format!("refused {id}"),
            Completion::Watch(v) => format!("watch {}", outcome(v, render_watch_event)),
            Completion::View(view) => format!(
                "view {}",
                hex(view
                    .bytes
                    .as_ref()
                    .ok_or("a mapped view whose bytes were not captured")?)
            ),
            completion => crate::host_io_tape::render_completion(completion)
                .unwrap_or_else(|| crate::net::tape::render_completion(completion)),
        };
        let fault = event
            .fault
            .as_ref()
            .map(|f| match f.error {
                IoError::Fault(code) => format!(" ! fault {code} {}", f.partial),
                IoError::Canceled(code) => format!(" ! canceled {code} {}", f.partial),
                IoError::NotGranted => " ! not-granted 0 0".into(),
                IoError::NotFound => format!(" ! NotFound 0 {}", f.partial),
                IoError::PermissionDenied => format!(" ! PermissionDenied 0 {}", f.partial),
                IoError::AlreadyExists => format!(" ! AlreadyExists 0 {}", f.partial),
                IoError::IsDirectory => format!(" ! IsDirectory 0 {}", f.partial),
                IoError::NotDirectory => format!(" ! NotDirectory 0 {}", f.partial),
                IoError::DirectoryNotEmpty => format!(" ! DirectoryNotEmpty 0 {}", f.partial),
                IoError::CrossDevice => format!(" ! CrossDevice 0 {}", f.partial),
                IoError::Truncated => format!(" ! Truncated 0 {}", f.partial),
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
            ["NotFound"] => Ok(Err(IoError::NotFound)),
            ["PermissionDenied"] => Ok(Err(IoError::PermissionDenied)),
            ["AlreadyExists"] => Ok(Err(IoError::AlreadyExists)),
            ["IsDirectory"] => Ok(Err(IoError::IsDirectory)),
            ["NotDirectory"] => Ok(Err(IoError::NotDirectory)),
            ["DirectoryNotEmpty"] => Ok(Err(IoError::DirectoryNotEmpty)),
            ["CrossDevice"] => Ok(Err(IoError::CrossDevice)),
            ["Truncated"] => Ok(Err(IoError::Truncated)),
            ["fault", code] => Ok(Err(IoError::Fault(number(code)?))),
            ["canceled", code] => Ok(Err(IoError::Canceled(number(code)?))),
            _ => Err("bad tape result".into()),
        }
    }
    let mut lines = text.lines();
    if !tape_header(lines.next()) {
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
                                "NotFound" if *code == "0" => IoError::NotFound,
                                "PermissionDenied" if *code == "0" => IoError::PermissionDenied,
                                "AlreadyExists" if *code == "0" => IoError::AlreadyExists,
                                "IsDirectory" if *code == "0" => IoError::IsDirectory,
                                "NotDirectory" if *code == "0" => IoError::NotDirectory,
                                "DirectoryNotEmpty" if *code == "0" => IoError::DirectoryNotEmpty,
                                "CrossDevice" if *code == "0" => IoError::CrossDevice,
                                "Truncated" if *code == "0" => IoError::Truncated,
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
            let request =
                match w.as_slice() {
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
                    ["read_only_dir", d] => Request::ReadOnlyDir {
                        directory: number(d)?,
                    },
                    ["print", s, b] => Request::Print {
                        stdout: number(s)?,
                        bytes: bytes(b)?,
                    },
                    ["open", d, p] => Request::Open {
                        directory: number(d)?,
                        path: string(p)?,
                    },
                    ["open_append", d, p] => Request::OpenAppend {
                        directory: number(d)?,
                        path: string(p)?,
                    },
                    ["create", d, p] => Request::Create {
                        directory: number(d)?,
                        path: string(p)?,
                    },
                    ["map", d, p] => Request::Map {
                        directory: number(d)?,
                        path: string(p)?,
                    },
                    ["map_view", d, m] => Request::MapView {
                        directory: number(d)?,
                        mapped: number(m)?,
                    },
                    ["map_check", d, m] => Request::MapCheck {
                        directory: number(d)?,
                        mapped: number(m)?,
                    },
                    ["watch", d, p, r] => Request::Watch {
                        directory: number(d)?,
                        path: string(p)?,
                        recursive: boolean(r)?,
                    },
                    ["watch_next", d, w] => Request::WatchNext {
                        directory: number(d)?,
                        watcher: number(w)?,
                    },
                    ["test_dir_event", t, d, e] => Request::TestDirEvent {
                        test: number(t)?,
                        directory: number(d)?,
                        event: parse_watch_event(e)?,
                    },
                    ["file_read", d, f, n] => Request::FileRead {
                        directory: number(d)?,
                        file: number(f)?,
                        max: number(n)?,
                    },
                    ["file_seek", d, f, n] => Request::FileSeek {
                        directory: number(d)?,
                        file: number(f)?,
                        offset: number(n)?,
                    },
                    ["file_write", d, f, b] => Request::FileWrite {
                        directory: number(d)?,
                        file: number(f)?,
                        bytes: bytes(b)?,
                    },
                    ["file_append", d, f, b] => Request::FileAppend {
                        directory: number(d)?,
                        file: number(f)?,
                        bytes: bytes(b)?,
                    },
                    ["narrow_file", d, f] => Request::NarrowFile {
                        directory: number(d)?,
                        file: number(f)?,
                    },
                    ["walk", ..] => {
                        return Err(
                            "retired Request::Walk recording; re-record with streaming file.walk"
                                .into(),
                        );
                    }
                    ["read_dir", ..] => return Err(
                        "retired eager read_dir recording; re-record with streaming file.read_dir"
                            .into(),
                    ),
                    ["read_dir_page", d, p, after, recursive] => Request::ReadDirPage {
                        directory: number(d)?,
                        path: string(p)?,
                        after: string(after)?,
                        recursive: match *recursive {
                            "0" => false,
                            "1" => true,
                            _ => return Err("read_dir_page order".into()),
                        },
                    },
                    ["metadata", d, p] => Request::Metadata {
                        directory: number(d)?,
                        path: string(p)?,
                    },
                    ["mkdir", d, p] => Request::Mkdir {
                        directory: number(d)?,
                        path: string(p)?,
                    },
                    ["set_permissions", d, p, permissions] => Request::SetPermissions {
                        directory: number(d)?,
                        path: string(p)?,
                        permissions: number(permissions)?,
                    },
                    ["remove_all", d, p] => Request::RemoveAll {
                        directory: number(d)?,
                        path: string(p)?,
                    },
                    ["remove_dir", d, p] => Request::RemoveDir {
                        directory: number(d)?,
                        path: string(p)?,
                    },
                    ["rename", d, a, b] => Request::Rename {
                        directory: number(d)?,
                        from: string(a)?,
                        to: string(b)?,
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
                    ["sync", d, f] => Request::Sync {
                        directory: number(d)?,
                        file: number(f)?,
                    },
                    ["put_new", d, p, b, durable] => Request::PutNew {
                        directory: number(d)?,
                        path: string(p)?,
                        bytes: bytes(b)?,
                        durable: boolean(durable)?,
                    },
                    ["test_crash", t, d, seed] => Request::TestCrash {
                        test: number(t)?,
                        directory: number(d)?,
                        seed: number(seed)?,
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
                    ["test_clock", t] => Request::TestClock { test: number(t)? },
                    ["test_seed", t] => Request::TestSeed { test: number(t)? },
                    ["test_tasks", t] => Request::TestTasks { test: number(t)? },
                    ["test_log", t, text] => Request::TestLog {
                        test: number(t)?,
                        text: string(text)?,
                    },
                    ["test_snapshot", t, name, text] => Request::TestSnapshot {
                        test: number(t)?,
                        name: string(name)?,
                        text: string(text)?,
                    },
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
                    words => match crate::host_io_tape::parse_request(words)?
                        .or(crate::net::tape::parse_request(words)?)
                    {
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
                ["file_input", "opened", id] => {
                    Completion::FileInput(crate::io::FileInput::Opened(number(id)?))
                }
                ["file_input", "size", size] => {
                    Completion::FileInput(crate::io::FileInput::Size(number(size)?))
                }
                ["file_input", "bytes", data] => {
                    Completion::FileInput(crate::io::FileInput::Bytes(bytes(data)?))
                }
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
                ["metadata", r @ ..] => Completion::Metadata(result(r, parse_metadata)?),
                ["entries", r @ ..] => Completion::Entries(result(r, |v| {
                    if v.is_empty() {
                        return Ok(vec![]);
                    }
                    v.split(',')
                        .map(|entry| {
                            let (path, kind) =
                                entry.split_once(':').ok_or("bad directory entry")?;
                            Ok(crate::io::DirEntry {
                                path: string(path)?,
                                kind: parse_kind(kind)?,
                            })
                        })
                        .collect()
                })?),
                ["checked", v] => Completion::Checked(boolean(v)?),
                ["refused", id] => Completion::Refused(number(id)?),
                ["view", b] => Completion::View(crate::io::MapView::of(bytes(b)?)),
                ["watch", r @ ..] => Completion::Watch(result(r, parse_watch_event)?),
                words => match crate::host_io_tape::parse_completion(words)?
                    .or(crate::net::tape::parse_completion(words)?)
                {
                    Some(completion) => completion,
                    None => return Err("bad tape completion".into()),
                },
            };
            Ok(Event {
                request,
                completion,
                fault,
                resources: None,
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
    /// The admitted run record a production launch bound; harness launches have none.
    pub binding: Option<RecordedRun>,
    pub tree: Tree,
    pub events: Vec<Event>,
}
/// The canonical validated run record and the admission evidence it was accepted with
/// (DET-1): replay rebuilds every grant decision from these bytes and needs no grants.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedRun {
    pub canonical: Vec<u8>,
    /// Validated request and immutable startup evidence, excluded from semantic
    /// identity. Replay checks this provenance without applying it to the host.
    pub placement: Option<hirsel_run_config::RecordedPlacement>,
    pub admitted_at: i64,
    pub revocation: Option<hirsel_run_config::RevocationEvidence>,
    /// The run instance fixed at admission (RUN-11), for a run that calls runners or
    /// answers a call; its header line is absent otherwise, so other tapes keep their bytes.
    pub instance: Option<[u8; 16]>,
}
// Empty default fixture grants contain no scripted world. A configured peer or
// listener is explicit scripted-provider input, separate from actual worker startup.
fn scripted_world(run: &hirsel_run_config::ValidatedRun) -> bool {
    run.net_grant().is_some_and(|net| {
        net.provider == "net.script@2"
            && (!net.listeners.is_empty() || net.peers.as_ref().is_some_and(|p| !p.is_empty()))
    })
}
impl Recording {
    /// The bound run, checked against this header and its recorded admission without
    /// consulting a clock, revocation source or host provider.
    pub fn bound_run(
        &self,
    ) -> Result<hirsel_run_config::ValidatedRun, hirsel_run_schema::RunRefusal> {
        let invalid = |detail: &str| hirsel_run_schema::RunRefusal::Invalid {
            field: "recording".into(),
            detail: detail.into(),
        };
        let binding = self
            .binding
            .as_ref()
            .ok_or_else(|| invalid("the recording binds no run record"))?;
        let run = hirsel_run_config::RunConfig::validate(&binding.canonical)?;
        run.check_replay(
            &binding.canonical,
            self.run_identity,
            &hirsel_run_config::AdmissionEvidence {
                at: binding.admitted_at,
                revocation: binding.revocation.clone(),
            },
        )?;
        let expected = run.instance()?;
        if binding.instance.is_some() != (expected.is_some() || run.calls())
            || expected.is_some_and(|instance| binding.instance != Some(instance))
        {
            return Err(invalid("the run instance disagrees with its run record"));
        }
        if let Some(placement) = &binding.placement {
            placement.validate()?;
            if scripted_world(&run)
                && matches!(
                    placement.admission,
                    hirsel_run_config::CapacityAdmission::Physical { .. }
                )
            {
                return Err(invalid("scripted world cannot claim physical capacity"));
            }
        }
        let config = run.config();
        if (config.hash_seed(), config.random_seed(), config.test_seed())
            != (self.hash_seed, self.random_seed, self.test_seed)
            || run.record().args.clone().unwrap_or_default() != self.arguments
        {
            return Err(invalid("the header disagrees with its run record"));
        }
        Ok(run)
    }
}
pub fn render_recording(record: &Recording) -> Result<String, String> {
    let mut text = format!(
        "{TAPE_HEADER}\nrun_identity {}\nhash_seed {}\nrandom_seed {}\ntest_seed {}\n",
        hex(&record.run_identity),
        record.hash_seed,
        record.random_seed,
        record.test_seed
    );
    if let Some(binding) = &record.binding {
        let _ = writeln!(
            text,
            "run {}\nadmitted_at {}",
            hex(&binding.canonical),
            binding.admitted_at
        );
    }
    if let Some(instance) = record.binding.as_ref().and_then(|b| b.instance) {
        let _ = writeln!(text, "run_instance {}", hex(&instance));
    }
    if let Some(placement) = record.binding.as_ref().and_then(|b| b.placement.as_ref()) {
        placement.validate().map_err(|e| e.to_string())?;
        let _ = writeln!(text, "placement {}", hex(&placement.to_json()));
    }
    if let Some(proof) = record.binding.as_ref().and_then(|b| b.revocation.as_ref()) {
        let _ = writeln!(
            text,
            "revocation {} {} {}",
            hex(proof.reference.as_bytes()),
            hex(proof.revision.as_bytes()),
            u8::from(proof.revoked)
        );
    }
    for argument in &record.arguments {
        let _ = writeln!(text, "argument {}", hex(argument.as_bytes()));
    }
    for directory in &record.tree.directories {
        let _ = writeln!(text, "directory {}", hex(directory.as_bytes()));
    }
    for (path, bytes) in &record.tree.files {
        let _ = writeln!(text, "file {} {}", hex(path.as_bytes()), hex(bytes));
    }
    for (path, target) in &record.tree.file_aliases {
        let _ = writeln!(
            text,
            "hard_link {} {}",
            hex(path.as_bytes()),
            hex(target.as_bytes())
        );
    }
    for (path, target) in &record.tree.links {
        let _ = writeln!(
            text,
            "link {} {}",
            hex(path.as_bytes()),
            hex(target.as_bytes())
        );
    }
    for (path, meta) in &record.tree.metadata {
        let _ = writeln!(
            text,
            "metadata {} {}",
            hex(path.as_bytes()),
            render_metadata(meta)
        );
    }
    text.push_str("events\n");
    text.push_str(render_tape(&record.events)?.split_once('\n').unwrap().1);
    let _ = writeln!(text, "complete {}", record.events.len());
    Ok(text)
}
/// Read either a binary v9 tape or the legacy inspection syntax.
pub fn parse_recording_bytes(bytes: &[u8]) -> Result<Recording, String> {
    crate::tape_binary::parse(bytes)
}
/// Render a stored tape for human inspection and text-based fixture assertions.
/// Production replay consumes the binary buffer directly.
pub fn read_recording_text(path: impl AsRef<std::path::Path>) -> std::io::Result<String> {
    crate::tape_binary::text(&std::fs::read(path)?).map_err(std::io::Error::other)
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
    if !tape_header(lines.next()) {
        return Err(format!(
            "not a recording: the first line is not `{TAPE_HEADER}`"
        ));
    }
    let mut run_identity = None;
    let (mut hash_seed, mut random_seed, mut test_seed) = (None, None, None);
    let (mut run, mut admitted_at) = (None, None);
    let mut revocation = None;
    let mut instance = None;
    let mut placement = None;
    let mut arguments = vec![];
    let mut files = std::collections::BTreeMap::new();
    let mut links = std::collections::BTreeMap::new();
    let mut file_aliases = std::collections::BTreeMap::new();
    let mut directories = std::collections::BTreeSet::new();
    let mut entry_metadata = std::collections::BTreeMap::new();
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
            ["placement", value] if placement.is_none() => {
                let data = bytes(value)?;
                let parsed = hirsel_run_config::RecordedPlacement::from_json(&data)
                    .or_else(|_| hirsel_run_config::Placement::from_json(&data).map(Into::into))
                    .map_err(|e| e.to_string())?;
                placement = Some(parsed);
            }
            ["run", value] if run.is_none() => run = Some(bytes(value)?),
            ["admitted_at", value] if admitted_at.is_none() => {
                let at: i64 = value.parse().map_err(|_| "bad recorded admission time")?;
                if at.to_string() != *value {
                    return Err("bad recorded admission time".into());
                }
                admitted_at = Some(at);
            }
            ["run_instance", value] if instance.is_none() => {
                instance = Some(
                    bytes(value)?
                        .try_into()
                        .map_err(|_| "bad recorded run instance")?,
                );
            }
            ["revocation", reference, revision, revoked @ ("0" | "1")] if revocation.is_none() => {
                revocation = Some(hirsel_run_config::RevocationEvidence {
                    reference: string(reference)?,
                    revision: string(revision)?,
                    revoked: *revoked == "1",
                });
            }
            ["argument", value] => arguments.push(string(value)?),
            ["directory", value] => {
                let path = string(value)?;
                if hirsel_run_schema::confined_relative("", &path).as_ref() != Some(&path)
                    || !directories.insert(path)
                {
                    return Err("bad recorded directory".into());
                }
            }
            ["hard_link", path, target] => {
                let path = string(path)?;
                let target = string(target)?;
                if path <= target || file_aliases.insert(path, target).is_some() {
                    return Err("bad recorded hard link".into());
                }
            }
            ["link", path, target] => {
                let path = string(path)?;
                if path.is_empty()
                    || hirsel_run_schema::confined_relative("", &path).as_ref() != Some(&path)
                    || links.insert(path, string(target)?).is_some()
                {
                    return Err("bad recorded link".into());
                }
            }
            ["metadata", path, meta] => {
                let path = string(path)?;
                if hirsel_run_schema::confined_relative("", &path).as_ref() != Some(&path)
                    || entry_metadata.insert(path, parse_metadata(meta)?).is_some()
                {
                    return Err("bad recorded metadata".into());
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
    tree.metadata = entry_metadata.clone();
    for (alias, canonical) in &file_aliases {
        let regular = tree.regular_paths();
        if !regular.contains(alias)
            || !regular.contains(canonical)
            || matches!((tree.files.get(alias), tree.files.get(canonical)), (Some(a), Some(b)) if a != b)
            || file_aliases.contains_key(canonical)
        {
            return Err("bad recorded shared file identity".into());
        }
    }
    for (path, meta) in &entry_metadata {
        let kind = if tree.directories.contains(path) {
            crate::io::FileKind::Directory
        } else if tree.files.contains_key(path) {
            crate::io::FileKind::Regular
        } else if tree.links.contains_key(path) {
            crate::io::FileKind::Symlink
        } else {
            meta.kind
        };
        if meta.kind != kind {
            return Err("recorded metadata kind mismatch".into());
        }
    }
    tree.metadata = entry_metadata;
    tree.file_aliases = file_aliases;
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
            binding: match (run, admitted_at) {
                (Some(canonical), Some(admitted_at)) => Some(RecordedRun {
                    canonical,
                    placement,
                    admitted_at,
                    revocation,
                    instance,
                }),
                (None, None)
                    if revocation.is_none() && instance.is_none() && placement.is_none() =>
                {
                    None
                }
                _ => return Err("a run record without its admission time".into()),
            },
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

const UNREPLAYABLE_REFUSAL: &str = "this program calls C outside a wrapper with a \
     deterministic model, so its runs cannot be recorded or replayed (PROG-4)";

/// A refusal before any run starts, on the process's streams.
fn refuse(message: &str) -> i32 {
    refused(&crate::host::Sinks::process(), message.into()).exit_code()
}

/// Report a launch refused before entry and return its outcome. A launcher that names its own
/// refusal file (`HIRSEL_RT_REFUSAL`) gets it there, as an observation record holding
/// `refused <reason>`; otherwise it goes to the run's report sink. The observation file itself
/// is never written for a refusal: it may be the destination refused.
pub(crate) fn refused(sinks: &crate::host::Sinks, reason: String) -> RunOutcome {
    let observed = std::env::var_os("HIRSEL_RT_REFUSAL").is_some_and(|path| {
        crate::host_paths::RecorderDestination::pin(std::path::Path::new(&path))
            .and_then(|destination| destination.open_with_files(&[], &[]))
            .is_ok_and(|mut file| {
                write!(
                    file,
                    "hirsel-observation {OBSERVATION_VERSION}\nrefused {}\n",
                    reason.replace('\n', " ")
                )
                .and_then(|()| file.flush())
                .is_ok()
            })
    });
    if !observed {
        let _ = writeln!(sinks.report.clone(), "hirsel-rt: launch refused: {reason}");
    }
    RunOutcome::Refused(reason)
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
        harness(
            Code::Zero(entry),
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
        harness(
            Code::Main(entry),
            result_size,
            layouts,
            layout_count,
            config,
        )
    }
}

/// Launch a Test entry, whose only root is the Test capability.
/// # Safety
/// `entry` has the TestEntry ABI; config and layouts are live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_launch_test(
    entry: TestEntry,
    result_size: u64,
    layouts: *const *const Layout,
    layout_count: u32,
    config: *const Config,
) -> i32 {
    unsafe {
        harness(
            Code::Test(entry),
            result_size,
            layouts,
            layout_count,
            config,
        )
    }
}

/// A harness launch of an entry with no program descriptor.
///
/// # Safety
/// As for the harness launchers.
unsafe fn harness(
    code: Code,
    result_size: u64,
    layouts: *const *const Layout,
    layout_count: u32,
    config: *const Config,
) -> i32 {
    // SAFETY: caller contract.
    let Some(config) = (unsafe { config.as_ref() }) else {
        return refuse("missing launch configuration");
    };
    let program = Program::harness(code, result_size, layouts, layout_count, *config);
    // SAFETY: caller contract.
    process_exit(unsafe { launch(&program, std::env::args_os().skip(1).collect()) })
}

/// A standalone process's exit status for `outcome`. A `Signaled` run ends the process by its
/// signal, as the host's default action would have: the default disposition is restored and
/// the signal raised. Only if the process survives that does it exit with the fallback
/// status.
fn process_exit(outcome: hirsel_run_schema::RunOutcome) -> i32 {
    if let hirsel_run_schema::RunOutcome::Signaled(kind) = &outcome {
        let _ = std::io::Write::flush(&mut std::io::stdout());
        // SAFETY: resetting a disposition and raising a signal on this process.
        unsafe {
            libc::signal(kind.number(), libc::SIG_DFL);
            libc::raise(kind.number());
        }
    }
    outcome.exit_code()
}

/// The entry of a compiled executable or program library: load its program and launch one run
/// of it, as `argv` (its host's arguments, the first naming the program) selects, and return the
/// exit status of its outcome. The run reads no other process arguments.
///
/// # Safety
/// `descriptor` is a program descriptor whose data lives for the process, and `argv` holds
/// `argc` NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_main(
    descriptor: *const ProgramDescriptor,
    argc: i32,
    argv: *const *const std::ffi::c_char,
) -> i32 {
    use std::os::unix::ffi::OsStrExt;
    let count = usize::try_from(argc).unwrap_or(0);
    let arguments = (1..count)
        .map(|i| {
            // SAFETY: caller contract.
            let argument = unsafe { std::ffi::CStr::from_ptr(*argv.add(i)) };
            std::ffi::OsStr::from_bytes(argument.to_bytes()).to_os_string()
        })
        .collect();
    // SAFETY: caller contract.
    match unsafe { Program::load(descriptor) } {
        // SAFETY: the program's data lives for the process.
        Ok(program) => process_exit(unsafe { launch(&program, arguments) }),
        Err(error) => refuse(&error),
    }
}

/// The code a run selected for `program` binds to: the program's digest and entry, or, for a
/// harness program without them, the run's own (its embedded launch configuration already binds
/// the run's identity).
fn facts(
    program: &Program,
    run: &hirsel_run_config::ValidatedRun,
) -> Result<hirsel_run_config::ProgramFacts, hirsel_run_schema::RunRefusal> {
    let (digest, entry) = if program.entry_name.is_empty() {
        (run.program(), run.record().entry.as_str())
    } else {
        (program.digest, program.entry_name.as_str())
    };
    let mut facts = crate::standalone::facts(
        digest,
        entry,
        program.code.roots(),
        program.escapes(),
        program.fueled(),
    )?;
    // Bound executables also bind the complete run identity below. A Call
    // record cannot select another entry or replace a main launch's record.
    if !program.standalone() && run.record().launch == hirsel_run_schema::LaunchKind::Call {
        facts.launch = hirsel_run_schema::LaunchKind::Call;
    }
    Ok(facts)
}

/// Select the run `arguments` ask for, bind it to `program`, admit it on the process's host
/// with the process's streams, and run it to its outcome.
///
/// # Safety
/// The program's entry and descriptors are live for the call.
unsafe fn launch(program: &Program, mut arguments: Vec<std::ffi::OsString>) -> RunOutcome {
    let host = crate::host::Host::process();
    let sinks = crate::host::Sinks::process();
    let refuse = |reason: &str| refused(&sinks, reason.into());
    let trace_events = match crate::trace_events::select(&mut arguments) {
        Ok(path) => path,
        Err(error) => return refuse(&error),
    };
    let selected = if program.standalone() {
        match crate::standalone::select(
            arguments.clone(),
            program.digest,
            &program.entry_name,
            program.code.roots(),
            program.escapes(),
            program.fueled(),
        ) {
            Ok(selected) => Some(selected),
            Err(error) => return refuse(&error.to_string()),
        }
    } else {
        match select_bound(program, &arguments) {
            Ok(selected) => selected,
            Err(error) => return refuse(&error),
        }
    };
    if program.unreplayable() && selected.as_ref().is_some_and(|s| s.taped()) {
        return refuse(UNREPLAYABLE_REFUSAL);
    }
    let config = match &selected {
        Some(selected) if program.standalone() => {
            let config = selected.run().config();
            Config {
                hash_seed: config.hash_seed(),
                random_seed: config.random_seed(),
                test_seed: config.test_seed(),
                run_identity: config.run_identity(),
            }
        }
        _ => program.config,
    };
    if let Some(run) = selected.as_ref().map(|selected| selected.run())
        && (run.identity() != config.run_identity
            || run.config().hash_seed() != config.hash_seed
            || run.config().random_seed() != config.random_seed
            || run.config().test_seed() != config.test_seed)
    {
        return refuse("run record binding mismatch");
    }
    use crate::host::Selection;
    let selection = match selected {
        None => Selection::Harness,
        Some(crate::standalone::Selected::Replay { run, io }) => Selection::Replay { run, io },
        Some(crate::standalone::Selected::Live {
            run,
            tape,
            revocations,
        }) => match host.admit(&run, revocations.as_deref(), &sinks) {
            Ok((admitted, io)) => Selection::Live { admitted, io, tape },
            Err(error) => return refuse(&error.to_string()),
        },
    };
    let mut run = host.start(program, config, selection, arguments, sinks);
    if let Some(path) = trace_events {
        run = run.with_trace_events(path);
    }
    run.wait()
}

/// A launched (not standalone) executable's selection: a replay, a run record the embedded
/// configuration binds, or nothing, when harness arguments stay ordinary Args.
fn select_bound(
    program: &Program,
    arguments: &[std::ffi::OsString],
) -> Result<Option<crate::standalone::Selected>, String> {
    if arguments.first().is_some_and(|a| a == "--hirsel-replay") {
        return match arguments {
            [_, path] => crate::host_launch::replay(std::path::Path::new(path))
                .and_then(|(io, run)| {
                    let facts = facts(program, &run)?;
                    Ok(Some(crate::standalone::Selected::Replay {
                        run: run.bind(&facts)?,
                        io,
                    }))
                })
                .map_err(|e| e.to_string()),
            _ => Err("expected only --hirsel-replay <tape>".into()),
        };
    }
    if !arguments.first().is_some_and(|a| {
        a == "--hirsel-run" || a == "--hirsel-record" || a == "--hirsel-revocations"
    }) {
        return Ok(None);
    }
    crate::standalone::live_options(arguments)
        .and_then(|options| match options.input.as_slice() {
            [flag, path] if flag == "--hirsel-run" => {
                let run = hirsel_run_config::RunConfig::read_path(std::path::Path::new(path))?;
                let facts = facts(program, &run)?;
                Ok(Some(crate::standalone::Selected::Live {
                    run: run.bind(&facts)?,
                    tape: options.tape,
                    revocations: options.revocations,
                }))
            }
            _ => Err(hirsel_run_schema::RunRefusal::Invalid {
                field: "argv".into(),
                detail: "expected --hirsel-run after launch options".into(),
            }),
        })
        .map_err(|e| e.to_string())
}

/// Run `selection` of `program` on this thread to its outcome, writing to `sinks`. `Err` is a
/// launch refused before entry (or a host report that failed after it), for the caller to
/// report.
///
/// # Safety
/// The program's entry and descriptors are live for the call.
pub(crate) unsafe fn execute(
    program: &Program,
    launch_config: &Config,
    selection: crate::host::Selection,
    process_arguments: Vec<std::ffi::OsString>,
    sinks: &crate::host::Sinks,
    options: Options,
) -> Result<RunOutcome, String> {
    let (mut width, migrate) = match options.workers {
        Some(workers) => workers,
        None => workers_from_env()?,
    };
    if !(1..=crate::pool::MAX_WIDTH).contains(&width) {
        return Err(format!(
            "the run's width {width} is outside 1 to {}",
            crate::pool::MAX_WIDTH
        ));
    }
    let entry = program.code;
    let (result_size, layouts, layout_count) =
        (program.result_size, program.layouts, program.layout_count);
    let runner_instance = match &selection {
        crate::host::Selection::Live { admitted, .. } => admitted.instance(),
        crate::host::Selection::Replay { io, .. } => io.instance,
        crate::host::Selection::Harness => None,
    };
    let (requested, production_io, production_replay) = match selection {
        crate::host::Selection::Harness => (None, None, None),
        crate::host::Selection::Live { admitted, io, tape } => (
            Some(admitted.run().clone()),
            Some((io, admitted, tape)),
            None,
        ),
        // A replay performs no host effect: no admission, clock or provider.
        crate::host::Selection::Replay { run, io } => (Some(run.run().clone()), None, Some(io)),
    };
    // Harness providers remain fixture-owned. Only an explicit validated placement
    // opts a fixture into physical execution; ordinary semantic fixtures keep their
    // existing dispatcher and recordings. Production selections never read this env.
    let harness_text = |name: &str| match std::env::var(name) {
        Ok(text) if requested.is_none() => Ok(Some(text)),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name} is not UTF-8")),
    };
    let fixture_run = harness_text("HIRSEL_RT_FIXTURE_RUN")?
        .map(|path| hirsel_run_config::RunConfig::read_path(std::path::Path::new(&path)))
        .transpose()
        .map_err(|e| format!("fixture run: {e}"))?;
    if let Some(run) = &fixture_run {
        if run.identity() != launch_config.run_identity {
            return Err("fixture run binding mismatch".into());
        }
    }
    let cpu_fixture = fixture_run
        .as_ref()
        .filter(|run| run.record().io.as_ref().is_some_and(|io| io.cpu.is_some()));
    let placement_run = requested.as_ref().or_else(|| {
        fixture_run
            .as_ref()
            .filter(|run| run.record().placement.is_some() || cpu_fixture.is_some())
    });
    // Validate call metadata before allocating a context or starting workers.
    let replay_call = requested
        .as_ref()
        .filter(|run| run.record().launch == hirsel_run_schema::LaunchKind::Call)
        .map(|run| crate::runners::native::replay_state(program, run))
        .transpose()?;
    let callee_replay = replay_call.is_some();
    // Binding refused a fuel budget this code cannot enforce (`ProgramFacts::fueled`).
    let budgets = requested.as_ref().and_then(|run| run.budgets()).cloned();
    // F0's explicit test channel replaces the logical schedule and suppresses
    // worker startup below. It cannot also install the production Ordered policy.
    let ordered = if hirsel_free::session::TestHook::from_env()?.is_some() {
        None
    } else {
        crate::tasks::ordered::admission::Admission::prepare(placement_run)?
    };
    let ordered_enabled = ordered.is_some();
    let production_free = requested
        .as_ref()
        .and_then(|run| run.record().io.as_ref())
        .and_then(|io| io.tasks.as_deref())
        == Some("tasks.free@1");
    let mut free_session = None;
    let trace_destination = options
        .trace_events
        .as_ref()
        .map(|path| crate::trace_events::pin(path, ordered_enabled || production_free))
        .transpose()?;
    let seed = launch_config.test_seed;
    if !(1..=RESULT_MAX).contains(&result_size) || (program.return_kind != 1 && result_size < 4) {
        return Err("the entry result is not a Result of at most 64 bytes".into());
    }
    let inherited_outputs = match crate::host_paths::inherited_output_pins() {
        Ok(pins) => pins,
        Err(error) => return Err(error.to_string()),
    };
    let output_grants: Vec<_> = inherited_outputs
        .iter()
        .map(|(name, file)| (*name, file))
        .collect();
    let mut free_projection = if production_free {
        if let Some(path) = &options.trace_events {
            crate::free_replay::Report::check_input(path)?;
        }
        let grants = production_io
            .as_ref()
            .map_or_else(Vec::new, |(io, _, _)| io.mount_pins());
        if let Some(path) = production_io
            .as_ref()
            .and_then(|(_, _, tape)| tape.as_deref())
        {
            crate::free_replay::Report::check_input(path)?;
        }
        crate::free_replay::Report::open(&grants, &output_grants)?
    } else {
        None
    };
    if let Some(destination) = &trace_destination {
        let io = production_io.as_ref().map(|(io, _, _)| io);
        let grants = io.map_or_else(Vec::new, |io| io.mount_pins());
        destination
            .check_with_files(&grants, &output_grants)
            .map_err(|e| e.to_string())?;
        let tape = production_io
            .as_ref()
            .and_then(|(_, _, tape)| tape.as_deref());
        let placement = requested
            .as_ref()
            .and_then(|run| run.record().placement.as_ref());
        for path in [
            tape,
            placement
                .and_then(|p| p.metrics.as_deref())
                .map(std::path::Path::new),
            placement
                .and_then(|p| p.control.as_deref())
                .map(std::path::Path::new),
        ]
        .into_iter()
        .flatten()
        {
            let other =
                crate::host_paths::RecorderDestination::pin(path).map_err(|e| e.to_string())?;
            destination
                .check_distinct(&other)
                .map_err(|e| e.to_string())?;
        }
    }
    let mut arguments = if let Some(run) = &requested {
        run.record().args.clone().unwrap_or_default()
    } else if entry.is_zero() {
        vec![]
    } else {
        match process_arguments
            .into_iter()
            .enumerate()
            .map(|(i, a)| a.into_string().map_err(|_| i))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(arguments) => arguments,
            Err(index) => return Err(format!("argument {index} is not UTF-8")),
        }
    };
    let net = if requested.is_some() {
        None
    } else {
        match std::env::var("HIRSEL_RT_NET") {
            Ok(text) => match hirsel_run_config::validate_net_grant(text.as_bytes()) {
                Ok(grant) => Some(grant),
                Err(error) => return Err(format!("the network grant: {error}")),
            },
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err("the network grant is not UTF-8".into());
            }
        }
    };
    // The launcher's root Proc grant (`HIRSEL_RT_PROCESS`), like `HIRSEL_RT_NET`.
    let process = if requested.is_some() {
        None
    } else {
        match std::env::var("HIRSEL_RT_PROCESS") {
            Ok(text) => match hirsel_run_config::validate_process_grant(text.as_bytes()) {
                Ok(grant) => Some(grant),
                Err(error) => return Err(format!("the process grant: {error}")),
            },
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err("the process grant is not UTF-8".into());
            }
        }
    };
    // The launcher's `io.signals` grant (`HIRSEL_RT_SIGNALS`), like `HIRSEL_RT_NET`.
    let signals = if requested.is_some() {
        None
    } else {
        match std::env::var("HIRSEL_RT_SIGNALS") {
            Ok(text) => match hirsel_run_config::validate_signals_grant(text.as_bytes()) {
                Ok(grant) => Some(grant),
                Err(error) => return Err(format!("the signals grant: {error}")),
            },
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err("the signals grant is not UTF-8".into());
            }
        }
    };
    // The launcher's root Env snapshot (`HIRSEL_RT_ENV`), like `HIRSEL_RT_NET`.
    let env = if requested.is_some() {
        None
    } else {
        match std::env::var("HIRSEL_RT_ENV") {
            Ok(text) => match hirsel_run_config::validate_env(text.as_bytes()) {
                Ok(env) => Some(env),
                Err(error) => return Err(format!("the Env snapshot: {error}")),
            },
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err("the Env snapshot is not UTF-8".into());
            }
        }
    };
    // The run record's scheduler, else the harness's (`HIRSEL_RT_SCHEDULER`, `name@1:seed`),
    // which a test runner sets for each launch it explores.
    let scheduler = match &requested {
        Some(run) => run.record().inputs.scheduler.clone(),
        None => match std::env::var("HIRSEL_RT_SCHEDULER") {
            Ok(text) => match hirsel_run_schema::Scheduler::parse(&text) {
                Some(scheduler) => Some(scheduler),
                None => return Err(format!("the scheduler `{text}` is not supported")),
            },
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err("the scheduler is not UTF-8".into());
            }
        },
    };
    // The harness's runners (`HIRSEL_RT_RUNNERS`, a JSON array as `io.runners` spells it),
    // the seed `sim@1` draws faults from (`HIRSEL_RT_FAULT_SEED`) and the launcher's callee
    // facts (`HIRSEL_RT_CALLEES`, `hirsel_run_schema::callees_json`). A run record's runners
    // are not offered natively yet.
    if requested
        .as_ref()
        .and_then(|run| run.record().io.as_ref())
        .is_some_and(|io| !io.runners.is_empty())
    {
        // The production execution seam remains gated until runner acceptance.
        // Check descriptor authority before considering any harness-only path.
        program.runner_callees()?;
        if !production_free {
            return Err("runners run natively only under the harness launcher".into());
        }
    }
    let runners = if production_free {
        requested
            .as_ref()
            .and_then(|run| run.record().io.as_ref())
            .map(|io| io.runners.clone())
            .unwrap_or_default()
    } else {
        match harness_text("HIRSEL_RT_RUNNERS")? {
            Some(text) => hirsel_run_config::validate_runners(text.as_bytes())
                .map_err(|error| format!("the runners: {error}"))?,
            None => Vec::new(),
        }
    };
    // Native places run in this process; another host's (remote.tls@1) is not offered.
    if let Some(entry) = runners
        .iter()
        .find(|e| !matches!(e.provider.as_str(), "local.threads@1" | "sim@1"))
    {
        return Err(format!(
            "runner {} names {}, which native code does not run",
            entry.name, entry.provider
        ));
    }
    // `runner.grants` compiles to the six flags and empty lists (RUN-2).
    if let Some(entry) = runners.iter().find(|e| {
        e.grants
            .as_ref()
            .is_some_and(|g| !g.dirs.is_empty() || g.net.is_some() || g.process.is_some())
    }) {
        return Err(format!(
            "runner {} grants directories, network or processes, which native runners do not",
            entry.name
        ));
    }
    let fault_seed = if production_free {
        requested
            .as_ref()
            .and_then(|run| run.record().inputs.fault_seed)
            .unwrap_or(0)
    } else {
        match harness_text("HIRSEL_RT_FAULT_SEED")? {
            Some(text) => text
                .parse::<u64>()
                .map_err(|_| "invalid harness fault seed".to_string())?,
            None => 0,
        }
    };
    let callees = if production_free && !runners.is_empty() {
        program.runner_callees()?.clone()
    } else {
        match harness_text("HIRSEL_RT_CALLEES")? {
            Some(text) => hirsel_run_schema::parse_callees(&text)
                .map_err(|error| format!("the callee facts: {error}"))?,
            None => Default::default(),
        }
    };
    // net.script@2 peers serve the root Net's world; the rules alone decide policy.
    let net_rules = net.as_ref().map(NetRule::from_config).unwrap_or_default();
    let net_script = net.and_then(|grant| Some((grant.peers?, grant.listeners)));
    // Only the harness selection reads fixture inputs. Production admission and RealIo
    // retain their host-file semantics even when these variables are present.
    if let Some(run) = &fixture_run {
        arguments = run.record().args.clone().unwrap_or_default();
    }
    let fixture_files = harness_text("HIRSEL_RT_FIXTURE_FILES")?
        .map(|path| -> Result<_, String> {
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            serde_json::from_slice::<(
                std::collections::BTreeMap<String, Vec<u8>>,
                std::collections::BTreeMap<String, hirsel_run_schema::Access>,
                Vec<u8>,
            )>(&bytes)
            .map_err(|e| e.to_string())
        })
        .transpose()?;
    let mut initial_tree = Tree::new(Default::default()).expect("empty tree");
    let mut real = None;
    let mut zero_recording = None;
    let mut pending_fixture_recording = None;
    let mut pending_host_journal = None;
    let mut replay = None;
    // Preserve the admitted destination before live Free moves RealIo into
    // its concurrent owner. Callee tapes belong beside this root tape.
    let runner_tape = production_io.as_ref().and_then(|(_, _, tape)| tape.clone());
    let mut adapter: Box<dyn Adapter> = if let Some((mut io, admitted, tape)) = production_io {
        if production_free {
            free_session = crate::free_launch::Session::record(
                &mut io,
                &admitted,
                tape.as_deref(),
                &output_grants,
                trace_destination.is_some(),
            )?;
        } else {
            // Attach the header after physical startup, before root publication.
            pending_host_journal = Some((admitted, tape));
        }
        if entry.is_test() {
            io.test_context(seed);
        }
        io.discard_history();
        let io = Rc::new(RefCell::new(io));
        real = Some(io.clone());
        Box::new(Shared(io))
    } else if let Some(mut io) = production_replay {
        if production_free {
            let bytes = io
                .free_tape
                .take()
                .ok_or("tasks.free@1 requires a Free phase tape")?;
            free_session = Some(crate::free_launch::Session::replay(
                &bytes,
                free_projection.is_some() || trace_destination.is_some(),
            )?);
        } else if io.free_tape.is_some() {
            return Err("Free phase tape requires tasks.free@1".into());
        }
        if entry.is_test() {
            io.test_context(seed);
        }
        let io = Rc::new(RefCell::new(io));
        replay = Some(io.clone());
        Box::new(Shared(io))
    } else {
        match std::env::var_os("HIRSEL_RT_REPLAY") {
            Some(path) => {
                let (tape, binary) = match std::fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(crate::tape_binary::replay)
                {
                    Ok(recording) => recording,
                    Err(error) => return Err(format!("the replayed tape: {error}")),
                };
                if tape.run_identity != launch_config.run_identity
                    || tape.hash_seed != launch_config.hash_seed
                    || tape.random_seed != launch_config.random_seed
                    || tape.test_seed != launch_config.test_seed
                {
                    return Err("recording configuration mismatch".into());
                }
                let bound_run = tape
                    .binding
                    .as_ref()
                    .map(|_| tape.bound_run())
                    .transpose()
                    .map_err(|e| e.to_string())?;
                arguments = tape.arguments;
                let mut io = match binary {
                    Some(binary) => SimIo::from_binary(tape.tree, binary),
                    None => SimIo::new(tape.tree, tape.events),
                };
                if let Some(run) = &bound_run {
                    crate::host_launch::configure_replay(&mut io, run)
                        .map_err(|e| e.to_string())?;
                } else {
                    // Unbound historical harness tapes keep their explicit fixture inputs.
                    if let Some(run) = &fixture_run {
                        io.set_mounts(
                            run.record()
                                .io
                                .iter()
                                .flat_map(|io| &io.dirs)
                                .map(|mount| (mount.path.clone(), mount.access)),
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    if std::env::var_os("HIRSEL_RT_DENY_OUTPUT").is_some() {
                        io.deny_output();
                    }
                    io.set_net_rules(net_rules.clone());
                    if let Some(env) = &env {
                        io.set_env(env.clone());
                    }
                    io.clock_grants(std::env::var_os("HIRSEL_RT_VIRTUAL_CLOCK").is_some(), false);
                    if let Some(grant) = &process {
                        io.set_process_grant(grant);
                    }
                    if let Some(grant) = &signals {
                        io.set_signal_kinds(grant.bits());
                    }
                    if let Some((peers, listeners)) = net_script.clone()
                        && let Err(error) = io.set_net_script(peers, listeners)
                    {
                        return Err(format!("the network script: {error:?}"));
                    }
                }
                if entry.is_test() {
                    io.test_context(seed);
                }
                let io = Rc::new(RefCell::new(io));
                replay = Some(io.clone());
                Box::new(Shared(io))
            }
            None if entry.is_zero() => {
                if let Some(path) = std::env::var_os("HIRSEL_RT_TAPE") {
                    let record = Recording {
                        arguments: vec![],
                        run_identity: launch_config.run_identity,
                        hash_seed: launch_config.hash_seed,
                        random_seed: launch_config.random_seed,
                        test_seed: launch_config.test_seed,
                        binding: None,
                        tree: initial_tree.clone(),
                        events: vec![],
                    };
                    let file = match crate::host_paths::RecorderDestination::pin(
                        std::path::Path::new(&path),
                    )
                    .and_then(|destination| destination.open_with_files(&[], &output_grants))
                    {
                        Ok(file) => file,
                        Err(error) => return Err(error.to_string()),
                    };
                    match RecordingWriter::new(file, &record) {
                        Ok(writer) => zero_recording = Some(writer),
                        Err(error) => return Err(format!("the Io tape: {error}")),
                    }
                }
                Box::new(SimIo::new(initial_tree.clone(), vec![]))
            }
            // Without `HIRSEL_RT_ROOT` the root Io grants nothing (determinism.md, Run
            // configuration): no mount and no projection.
            None => match if let Some((files, mounts, stdin)) = fixture_files {
                RealIo::fixture(files, mounts, stdin, sinks.stdout.clone())
                    .map(|mut io| {
                        io.set_stderr(sinks.stderr.clone());
                        io
                    })
                    .map_err(std::io::Error::other)
            } else {
                std::env::var_os("HIRSEL_RT_ROOT").map_or_else(
                    || {
                        let mut io = RealIo::from_pinned_mounts([], sinks.stdout.clone())
                            .expect("an empty mount set");
                        io.set_projections(Default::default());
                        Ok(io)
                    },
                    |root| {
                        RealIo::grant_with_output(std::path::Path::new(&root), sinks.stdout.clone())
                    },
                )
            } {
                Ok(mut io) => {
                    sinks.preserve_send_streams(&mut io);
                    if std::env::var_os("HIRSEL_RT_DENY_OUTPUT").is_some() {
                        io.deny_output();
                    }
                    if let Some(ticks) = std::env::var_os("HIRSEL_RT_VIRTUAL_CLOCK") {
                        let Some(ticks) = ticks.to_str().and_then(|s| s.parse::<u64>().ok()) else {
                            return Err("invalid harness virtual clock".into());
                        };
                        io.virtual_clock(ticks);
                    } else if signals.is_some() {
                        // A scripted signal parks its waits: record them and each ready poll,
                        // so replay interleaves them with the other parked requests.
                        io.set_cooperative(crate::readiness::Readiness::default());
                    }
                    if let Some(text) = harness_text("HIRSEL_RT_FILE_READS")? {
                        let scripts = serde_json::from_str(&text)
                            .map_err(|e| format!("file read script: {e}"))?;
                        io.set_file_read_script(scripts);
                    }
                    if let Some(scripts) = fixture_run
                        .as_ref()
                        .and_then(|run| run.record().inputs.fixture_file_faults.clone())
                    {
                        io.set_file_fault_script(scripts)
                            .map_err(|e| format!("file fault script: {e}"))?;
                    }
                    io.set_net_rules(net_rules.clone());
                    if let Some(env) = &env {
                        io.set_env(env.clone());
                    }
                    io.clock_grants(std::env::var_os("HIRSEL_RT_VIRTUAL_CLOCK").is_some(), false);
                    // The harness launch draws the reference's seeded simulation.
                    io.set_random(
                        hirsel_run_schema::ProviderContract::RandomSplitMix64,
                        launch_config.random_seed,
                    );
                    if let Some(grant) = &process {
                        io.set_process_grant(grant);
                    }
                    if let Some(grant) = &signals {
                        if let Err(error) = io.set_signal_grant(grant) {
                            return Err(format!("the signals grant: {error:?}"));
                        }
                    }
                    if let Some((peers, listeners)) = net_script.clone()
                        && let Err(error) = io.set_net_script(peers, listeners)
                    {
                        return Err(format!("the network script: {error:?}"));
                    }
                    // A scripted read parks until another task's request advances the
                    // script: cooperative waits park it and the tape records the wait
                    // (DET-1), as the reference's host launch does.
                    if net_script.is_some() && std::env::var_os("HIRSEL_RT_VIRTUAL_CLOCK").is_none()
                    {
                        io.set_cooperative(crate::readiness::Readiness::default());
                    }
                    let tape = match std::env::var_os("HIRSEL_RT_TAPE") {
                        Some(path) => match io.prepare_recording_with_files(
                            std::path::Path::new(&path),
                            &output_grants,
                        ) {
                            Ok(file) => Some(file),
                            Err(error) => return Err(error.to_string()),
                        },
                        None => None,
                    };
                    if tape.is_some() {
                        initial_tree = match io.snapshot() {
                            Ok(tree) => tree,
                            Err(error) => {
                                return Err(format!("initial recording tree: {error:?}"));
                            }
                        };
                    }
                    if let Some(file) = tape {
                        let record = Recording {
                            arguments: arguments.clone(),
                            run_identity: launch_config.run_identity,
                            hash_seed: launch_config.hash_seed,
                            random_seed: launch_config.random_seed,
                            test_seed: launch_config.test_seed,
                            binding: fixture_run
                                .as_ref()
                                .filter(|run| !entry.is_test() && !run.calls())
                                .map(|run| RecordedRun {
                                    canonical: run.canonical().to_vec(),
                                    placement: run.record().placement.clone().map(Into::into),
                                    admitted_at: 0,
                                    revocation: None,
                                    instance: None,
                                }),
                            tree: initial_tree.clone(),
                            events: vec![],
                        };
                        pending_fixture_recording = Some((file, record));
                    }
                    if entry.is_test() {
                        io.test_context(seed);
                    }
                    io.discard_history();
                    let io = Rc::new(RefCell::new(io));
                    real = Some(io.clone());
                    Box::new(Shared(io))
                }
                Err(error) => return Err(format!("the granted directory: {error}")),
            },
        }
    };
    // A sole root needs no physical capacity or Ordered objects. Explicit
    // observers retain their admission snapshot before they can accept requests.
    let deferred_placement = placement_run
        .filter(|_| {
            (ordered_enabled || production_free)
                && replay.is_none()
                && options.trace_events.is_none()
        })
        .map(|run| run.record().placement.clone().unwrap_or_default())
        .filter(|p| p.metrics.is_none() && p.control.is_none());
    if deferred_placement.is_some() {
        width = 1;
    }
    // Replay validates placement syntax but never captures topology or opens a sidecar.
    let managed = if replay.is_none() && deferred_placement.is_none() {
        placement_run
            .map(|run| run.record().placement.clone().unwrap_or_default())
            .map(|placement| {
                let reader = hirsel_run_config::managed_cores::LinuxLimits::capture()
                    .map_err(|e| e.to_string())?;
                let limits = reader.read().map_err(|e| e.to_string())?;
                crate::pool::init_compute_grant(&limits);
                let (process_grant, _, _) = crate::pool::compute_permits();
                let grant = hirsel_run_config::managed_cores::HostGrant {
                    max_cores: process_grant.into(),
                    cpus: limits.admission_affinity.clone(),
                };
                let manager = std::sync::Arc::new(crate::managed_cores::ManagedCores::admit(
                    placement.clone(),
                    grant,
                    limits,
                    std::time::Duration::ZERO,
                    crate::host::RunControl::from_inner(std::sync::Arc::clone(&options.control)),
                    // Both production policies and explicit worker tests use
                    // the managed physical topology.
                    options.workers.is_some() || ordered_enabled || production_free,
                )?);
                let metrics_pin = placement
                    .metrics
                    .as_ref()
                    .map(|path| {
                        crate::host_paths::RecorderDestination::pin(std::path::Path::new(path))
                    })
                    .transpose()
                    .map_err(|e| e.to_string())?;
                let control_pin = placement
                    .control
                    .as_ref()
                    .map(|path| {
                        crate::host_paths::RecorderDestination::pin(std::path::Path::new(path))
                    })
                    .transpose()
                    .map_err(|e| e.to_string())?;
                // Check both destinations before either creates or truncates anything.
                let real_io = real.as_ref().map(|io| io.borrow());
                let grants = real_io.as_ref().map_or_else(Vec::new, |io| io.mount_pins());
                if let Some(pin) = &metrics_pin {
                    pin.check_with_files(&grants, &output_grants)
                        .map_err(|e| e.to_string())?;
                }
                if let Some(pin) = &control_pin {
                    pin.check_control(&grants, &output_grants)
                        .map_err(|e| e.to_string())?;
                }
                let socket = control_pin
                    .map(|pin| pin.bind_control(&grants, &output_grants))
                    .transpose()
                    .map_err(|e| e.to_string())?;
                let writer = metrics_pin
                    .map(|pin| pin.open_with_files(&grants, &output_grants))
                    .transpose()
                    .map_err(|e| e.to_string())?;
                width = options
                    .control
                    .width
                    .load(std::sync::atomic::Ordering::Acquire);
                Ok::<_, String>((reader, manager, writer, socket))
            })
            .transpose()?
    } else {
        None
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
        Err(error) => return Err(error.to_string()),
    };
    let report_failures = observation.is_none();
    if let Some(path) = std::env::var_os("HIRSEL_RT_AHEAD_TRACE") {
        match prepare_tool_file(std::path::Path::new(&path)) {
            Ok(file) => crate::ahead_trace::open(file),
            Err(error) => return Err(error.to_string()),
        }
    }

    #[cfg(feature = "diagnostic")]
    if let Some(path) = std::env::var_os("HIRSEL_RT_TRACE_ROOTS") {
        match prepare_tool_file(std::path::Path::new(&path)) {
            Ok(file) => crate::root_trace::open(file),
            Err(error) => return Err(error.to_string()),
        }
    }
    #[cfg(feature = "diagnostic")]
    if let Some(path) = std::env::var_os("HIRSEL_RT_COMPACT_TRACE") {
        match prepare_tool_file(std::path::Path::new(&path)) {
            Ok(file) => crate::compact_trace::open(file),
            Err(error) => return Err(error.to_string()),
        }
    }
    #[cfg(feature = "measurement")]
    let measurement_segments = std::env::var_os("HIRSEL_RT_MC_PROFILE").is_some()
        && std::env::var_os("HIRSEL_RT_MEASUREMENT").is_some();
    #[cfg(feature = "measurement")]
    let measurement = match std::env::var_os("HIRSEL_RT_MEASUREMENT")
        .map(|path| prepare_tool_file(std::path::Path::new(&path)))
        .transpose()
    {
        Ok(file) => file,
        Err(error) => return Err(error.to_string()),
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
        Err(error) => return Err(error.to_string()),
    };
    // All recorder/diagnostic/sidecar paths were checked against the admitted
    // mounts while RealIo was still home-owned. Only now move its explicitly
    // transportable state to the concurrent service.
    let mut free_adapter = None;
    if production_free && replay.is_none() {
        adapter = Box::new(SimIo::new(initial_tree.clone(), Vec::new()));
        let io = Rc::try_unwrap(real.take().expect("live Free Io"))
            .map_err(|_| "Free admission retained a home adapter alias")?
            .into_inner();
        let io = io
            .into_concurrent()
            .map_err(|_| "Free admission requires transportable streams")?;
        free_adapter = Some(std::sync::Arc::new(io));
    }
    let config = RuntimeConfig {
        layouts,
        layout_count,
        abi_version: ABI_VERSION,
        flags: if std::env::var_os("HIRSEL_RT_DIAGNOSTIC").is_some() {
            crate::context::CONFIG_DIAGNOSTIC
        } else {
            0
        },
        count_max: count_max(),
        heap_idle_cap: 0,
        close_handle: Some(hirsel_rt_v2_io_close),
        // Only placement receives the hash seed (SEM-5).
        hash_seed: launch_config.hash_seed,
        ..RuntimeConfig::new()
    };
    let mut ctx = std::ptr::null_mut();
    if unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) } != 0 {
        return Err("the runtime configuration".into());
    }
    unsafe {
        (*ctx).run().runner_call = replay_call;
    }
    let control = options.control;
    // A test's host stop from another thread (`HIRSEL_RT_STOP_AFTER_MS`), as an embedding
    // host's `RunControl::stop`: the run is hosted before its budgets take their chunks.
    if let Some(delay) = stop_after_from_env()? {
        control.host();
        let control = std::sync::Arc::clone(&control);
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            control.request(crate::stop::Stop::Requested);
        });
    }
    unsafe { &mut *ctx }.run().control = std::sync::Arc::clone(&control);
    let worker_metrics = options.metrics.or_else(|| {
        managed.as_ref().and_then(|(_, _, writer, socket)| {
            (writer.is_some() || socket.is_some()).then(Default::default)
        })
    });
    unsafe { &mut *ctx }.run().worker_metrics = worker_metrics.clone();
    // Freeze the request/resolution before the live control service may resize it.
    let admission_policy = managed
        .as_ref()
        .map(|(_, manager, _, _)| manager.admission_policy());
    unsafe { &mut *ctx }.run().deferred_capacity = deferred_placement
        .clone()
        .map(crate::managed_cores::Pending::new);
    unsafe { &mut *ctx }.run().capacity = managed.map(|(reader, manager, writer, socket)| {
        crate::managed_cores::Service::new(reader, manager, writer, socket, worker_metrics)
    });
    #[cfg(feature = "measurement")]
    if let Some(file) = measurement {
        unsafe { &mut *ctx }.set_measurement_output(file);
    }
    #[cfg(feature = "measurement")]
    if let Some(file) = profile {
        unsafe { &mut *ctx }.set_profile_output(file);
    }
    if let Some(scheduler) = &scheduler {
        let algorithm = scheduler
            .code()
            .and_then(hirsel_kernels::schedule::Algorithm::from_code)
            .unwrap_or_else(|| crate::fault("an unsupported validated scheduler"));
        unsafe { &mut *ctx }.run().schedule =
            hirsel_kernels::schedule::Schedule::new(algorithm, scheduler.seed);
    }
    unsafe { &mut *ctx }.run().free_mode = requested
        .as_ref()
        .or(cpu_fixture)
        .and_then(|run| run.record().io.as_ref())
        .and_then(|io| io.tasks.as_deref())
        == Some("tasks.free@1");
    unsafe { &mut *ctx }.run().cpu_grant = requested
        .as_ref()
        .or(cpu_fixture)
        .and_then(|run| run.record().io.as_ref())
        .and_then(|io| io.cpu.clone());
    if let Some(grant) = unsafe { (*ctx).run_ref().cpu_grant.as_ref() } {
        let ledger = crate::pool::cpu_scoped::ledger::Ledger::new(grant);
        let _ = unsafe { (*ctx).heap_ref().meter.cpu.set(ledger.clone()) };
        unsafe {
            (*ctx)
                .worker()
                .account_cpu(&ledger, Some(crate::fiber::native_stack_bytes()))
        };
        unsafe { &mut *ctx }.run().cpu_ledger = Some(ledger);
    }
    if let Some(path) = std::env::var_os("HIRSEL_RT_CPU_PROBE") {
        unsafe { &mut *ctx }.run().cpu_probe = Some(
            crate::pool::cpu_scoped::probe::Probe::open(std::path::Path::new(&path))
                .map_err(|e| format!("CPU probe: {e}"))?,
        );
    }
    unsafe { &mut *ctx }.run().free_production = production_free;
    unsafe { &mut *ctx }.run().free_root_env = entry.roots() & crate::standalone::ROOT_ENV != 0;
    unsafe { &mut *ctx }.run().free_return = (program.return_kind, program.success_offset);
    unsafe { &mut *ctx }.run().value_codes = program.value_codes.clone();
    // F0's free-mode test channel, when the environment sets it, replaces the schedule.
    crate::free::install(unsafe { &mut *ctx }, &launch_config.run_identity)?;
    if cpu_fixture.is_some() && unsafe { (*ctx).run_ref().free_mode } {
        unsafe { crate::tasks::free::enable(ctx) };
    }
    if production_free {
        if replay.is_some() {
            let requested = control.width.load(std::sync::atomic::Ordering::Acquire);
            if requested != 0 {
                width = requested;
            }
        }
        if unsafe { (*ctx).run_ref().free.is_some() } {
            unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
            return Err("production Free admission cannot use the F0 test channel".into());
        }
        unsafe {
            crate::tasks::free::enable(ctx);
        }
        if let Some(session) = &free_session {
            unsafe {
                session.install(ctx);
            }
        }
    }
    let mut runner_config = crate::runners::Config {
        runners: runners.clone(),
        fault_seed,
        callees,
        request_schemas: program
            .callees
            .as_ref()
            .and_then(|m| m.request_schemas.clone()),
        entries: program.entries.clone(),
        tape: runner_tape.or_else(|| {
            std::env::var_os("HIRSEL_RT_TAPE")
                .filter(|_| requested.is_none() && real.is_some())
                .map(std::path::PathBuf::from)
        }),
        stdout: Some(sinks.stdout.clone()),
        runtime: config,
        failure_table: program.failure_table,
    };
    // The home owner stays on admission, outside RunShared: a detached worker
    // can destroy only the Send transport slot, never Rc adapters or sinks.
    let _runners_home = if production_free && !runners.is_empty() {
        let home = if let Some(io) = &free_adapter {
            let slot = crate::runners::native::Slot::new(io.clone())
                .map_err(|error| format!("native runners: {error:?}"))?;
            unsafe { (*ctx).run().runner_home = Some(slot.clone()) };
            Some(unsafe { crate::runners::native::Home::new(slot, runner_config.clone()) })
        } else {
            None
        };
        runner_config.stdout = None;
        home
    } else {
        None
    };
    unsafe { crate::runners::configure(ctx, runner_config) };
    if let Some(instance) = runner_instance {
        unsafe { crate::runners::set_instance(ctx, instance) };
    }
    crate::tasks::free::record::bind_terminal(unsafe { &*ctx });
    if let Some(io) = free_adapter.take()
        && let Err(error) = unsafe {
            let io: std::sync::Arc<dyn crate::io_concurrent::ConcurrentAdapter> =
                if let Some(slot) = (*ctx).run_ref().runner_home.as_ref() {
                    crate::runners::native::wrap(slot)
                } else {
                    io
                };
            crate::tasks::free::host::concurrent::Service::install(ctx, io)
        }
    {
        unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
        control.finish(0);
        return Err(format!("Free host admission: {error:?}"));
    }
    #[cfg(feature = "operation-latency")]
    if production_free && replay.is_none() {
        // Admission only, after replay selection and before any worker starts.
        // Replay never allocates buffers, reads observer configuration or clocks.
        unsafe { (*ctx).run().operation_latency = crate::operation_latency::Observer::from_env() };
    }
    #[cfg(feature = "ownership-trace")]
    unsafe {
        (*ctx).run().ownership_trace = crate::ownership_trace::Observer::from_env(program.digest);
    }
    // The free-mode test channel decides one worker's interleavings: it runs on one.
    // An execution of several runs (RUN-10) switches between their roots on one thread: a run
    // with runners runs on one worker too.
    if (ordered_enabled
        || cpu_fixture.is_some()
        || production_free
        || width > 1
        || (options.workers.is_some()
            && control.width.load(std::sync::atomic::Ordering::Acquire) != 0))
        && unsafe { &mut *ctx }.run().free.is_none()
        && (runners.is_empty() || production_free)
    {
        // SAFETY: the root context, on the admission thread, before the run's code runs.
        if ordered_enabled {
            unsafe { crate::pool::start_ordered_demand(ctx, width, migrate) };
        } else if production_free {
            unsafe { crate::pool::start_demand(ctx, width, migrate) };
        } else {
            unsafe { crate::pool::start(ctx, width, migrate) };
        }
    }
    if production_free || (cpu_fixture.is_some() && unsafe { (*ctx).run_ref().free_mode }) {
        unsafe { crate::pool::free::enable(ctx) };
    }
    if production_free {
        unsafe { crate::tasks::free::host::native::enable(ctx) };
    }
    // After the workers start: a run with compute workers takes its fuel in chunks, and a
    // memory budget meters every worker's heap.
    if let Some(budgets) = &budgets {
        unsafe { &mut *ctx }.set_budgets(budgets.fuel, budgets.memory_bytes);
    }
    if let Some(ordered) = ordered {
        // SAFETY: fresh root, after topology/budgets and before root publication.
        if deferred_placement.is_some() {
            unsafe { crate::pool::free::enable(ctx) };
            unsafe { (*ctx).run().deferred_ordered = Some(ordered) };
        } else {
            unsafe { ordered.install(ctx) };
        }
    }
    // Startup has published real destinations and Running synchronously. Capture
    // once before root entry: no socket round trip or program lifetime race.
    let admitted_placement = admission_policy
        .map(|(request, resolved)| {
            use hirsel_run_config::{CapacityAdmission, CapacityExclusion, RecordedPlacement};
            let run_control = crate::host::RunControl::from_inner(std::sync::Arc::clone(&control));
            let (launched, worker_phase) = run_control.worker_snapshot();
            let (grant, _, _) = crate::pool::compute_permits();
            let permitted = grant.saturating_sub(crate::pool::lost_compute_permits());
            let scripted = placement_run.is_some_and(scripted_world);
            let admission = if scripted || launched == 0 {
                CapacityAdmission::NotPhysical {
                    requested: request.cores.clone(),
                    worker_phase,
                    reason: if scripted {
                        CapacityExclusion::ScriptedWorld
                    } else {
                        CapacityExclusion::ExecutorNotObserved
                    },
                }
            } else {
                CapacityAdmission::Physical {
                    requested: request.cores.clone(),
                    resolved,
                    launched: launched.into(),
                    permitted: permitted.into(),
                    effective: launched.min(permitted).into(),
                    worker_phase,
                }
            };
            RecordedPlacement {
                schema: 1,
                request,
                admission,
            }
        })
        .or_else(|| {
            deferred_placement.map(|request| {
                use hirsel_run_config::{CapacityAdmission, CapacityExclusion, RecordedPlacement};
                RecordedPlacement {
                    schema: 1,
                    admission: CapacityAdmission::NotPhysical {
                        requested: request.cores.clone(),
                        worker_phase: hirsel_run_config::managed_cores::WorkerPhase::Running,
                        reason: CapacityExclusion::ExecutorNotObserved,
                    },
                    request,
                }
            })
        });
    let attach_journal = (|| -> Result<(), String> {
        if let Some((admitted, tape)) = pending_host_journal {
            crate::host_launch::journal_at_admission(
                &mut real.as_ref().expect("live host Io").borrow_mut(),
                &admitted,
                tape.as_deref(),
                &output_grants,
                admitted_placement.as_ref(),
            )?;
        }
        if let Some((file, mut header)) = pending_fixture_recording {
            if let Some(binding) = &mut header.binding {
                binding.placement = admitted_placement.clone().or(binding.placement.take());
            }
            let writer =
                RecordingWriter::new(file, &header).map_err(|e| format!("the Io tape: {e}"))?;
            real.as_ref()
                .expect("fixture Io")
                .borrow_mut()
                .record_to(writer);
        }
        Ok(())
    })();
    if let Err(error) = attach_journal {
        unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
        return Err(error);
    }
    // Live Ordered moves its admitted RealIo, with the attached journal, to the
    // run's concurrent service: blocking built-in calls then run as owned jobs,
    // never on a compute worker or home. Replay keeps its home tape.
    let mut ordered_live = None;
    if ordered_enabled && let Some(io) = real.take() {
        // The home binding's alias goes first; OrderedHome replaces it.
        drop(adapter);
        let moved = Rc::try_unwrap(io)
            .map_err(|_| "Ordered admission retained a home adapter alias".to_owned())
            .and_then(|io| {
                io.into_inner()
                    .into_concurrent()
                    .map_err(|_| "Ordered admission requires transportable streams".to_owned())
            })
            .map(std::sync::Arc::new)
            .and_then(|io| {
                unsafe {
                    crate::tasks::ordered::host::concurrent::Service::install(ctx, io.clone())
                }
                .map(|()| io)
                .map_err(|stop| format!("Ordered host admission: {stop:?}"))
            });
        match moved {
            Ok(io) => {
                adapter = Box::new(OrderedHome {
                    io: io.clone(),
                    current: crate::io_real::HOME_TOKEN,
                });
                ordered_live = Some(io);
            }
            Err(error) => {
                unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
                return Err(error);
            }
        }
    }
    if trace_destination.is_some() && ordered_enabled {
        crate::tasks::ordered::events::enable(ctx);
        let roots = entry.roots();
        crate::tasks::ordered::events::initial_host(
            ctx,
            (roots & crate::standalone::ROOT_IO != 0).then_some(IO_HANDLE),
            (roots & crate::standalone::ROOT_ENV != 0).then_some(u64::MAX),
        );
        if roots & crate::standalone::ROOT_ARGS != 0 {
            crate::tasks::ordered::events::initial_args(ctx, ARGS_HANDLE);
        }
    }
    let mut binding = Box::new(Binding::from_box(adapter).with_arguments(arguments.clone()));
    unsafe { binding.bind(&mut *ctx) };
    // A host's stop wakes the run's idle wait.
    if control.hosted()
        && let Some(fd) = control.wake_fd()
    {
        binding.adapter.wake_on(fd);
    }
    if let Some(table) = program.failure_table {
        crate::isolate::attach(unsafe { &mut *ctx }, table);
    }
    let roots = [IO_HANDLE, ARGS_HANDLE, u64::MAX, 0, TEST_HANDLE];
    // The replay output already echoed to the real streams.
    let echoed = std::rc::Rc::new(std::cell::Cell::new((0, 0)));
    let echo_replay = || -> Result<(), String> {
        if let Some(driver) = free_session
            .as_ref()
            .and_then(|session| session.replay_driver())
        {
            let (stdout, stderr) = driver.output();
            let (out, err) = echoed.get();
            sinks
                .stdout
                .clone()
                .write_all(&stdout[out..])
                .map_err(|e| format!("standard output: {e}"))?;
            sinks
                .stderr
                .clone()
                .write_all(&stderr[err..])
                .map_err(|e| format!("standard error: {e}"))?;
            echoed.set((stdout.len(), stderr.len()));
        } else if let Some(io) = &replay {
            let io = io.borrow();
            let (out, err) = echoed.get();
            sinks
                .stdout
                .clone()
                .write_all(&io.stdout()[out..])
                .map_err(|e| format!("standard output: {e}"))?;
            sinks
                .stderr
                .clone()
                .write_all(&io.stderr()[err..])
                .map_err(|e| format!("standard error: {e}"))?;
            echoed.set((io.stdout().len(), io.stderr().len()));
        }
        Ok(())
    };
    let mut report_refusal = None;
    // The PROG-3 report of a returned error, home state: it runs on the home thread.
    let mut report = |returned: &crate::exit_report::Returned| {
        let reported = (|| -> Result<(), String> {
            echo_replay()?;
            flush_stdout(sinks).map_err(|e| format!("standard output: {e}"))?;
            // SAFETY: the returned value is live until its cleanup, after this report.
            let text = unsafe { crate::exit_report::render(returned) };
            let mut report = sinks.report.clone();
            report
                .write_all(text.as_bytes())
                .and_then(|()| report.flush())
                .map_err(|e| format!("standard error: {e}"))
        })();
        report_refusal = reported.err();
    };
    // The run: its entry, then the PROG-3 report and the cleanup of a returned value, all on
    // the run's root fiber, so a stop anywhere in them returns here.
    let root: crate::stop::Entry = Box::new(move |ctx, words| unsafe {
        #[cfg(feature = "measurement")]
        {
            if measurement_segments {
                *(*(*ctx).run_ptr()).mc_profile.get() = Some(Default::default());
                crate::tasks::profile::resume(ctx);
            }
        }
        let result = (*words).result.as_mut_ptr().cast();
        match entry {
            Code::Zero(entry) => entry(ctx, std::ptr::null_mut(), result),
            Code::Main(entry) => entry(
                ctx,
                std::ptr::null_mut(),
                result,
                &mut (*words).rt,
                &mut (*words).args,
            ),
            Code::Test(entry) => entry(ctx, std::ptr::null_mut(), result, &TEST_HANDLE),
            Code::Roots(entry, _) => entry(
                ctx,
                std::ptr::null_mut(),
                result,
                (*words).roots.as_mut_ptr(),
            ),
        }
    });
    // SAFETY: `report` outlives the run's root, which reports only while `run_root` runs.
    let frame =
        unsafe { crate::stop::LaunchFrame::new(root, IO_HANDLE, ARGS_HANDLE, roots, &mut report) };
    let mut ran = unsafe { crate::stop::run_root(ctx, frame) };
    if callee_replay && matches!(ran, Ok(STATUS_OK)) {
        match unsafe { crate::runners::native::finish_replay(ctx) } {
            Ok(code) => unsafe {
                (*ctx).run().launch.as_mut().unwrap().words.result[0] = u128::from(code);
            },
            Err(stop) => ran = Err(stop),
        }
    }

    let mut replay_outcome = None;
    if let Some(session) = &free_session
        && session.replay_driver().is_some()
        && !matches!(
            ran,
            Err(crate::stop::Stop::Diverged(_) | crate::stop::Stop::Requested)
        )
    {
        match session.finish_replay(&mut free_projection) {
            Ok(Some(crate::free_launch::Terminal::Stop(stop))) => ran = Err(stop),
            Ok(Some(crate::free_launch::Terminal::Failed(outcome))) => {
                // FAIL-3 abandons still-owned task storage, including on replay.
                ran = Err(crate::stop::Stop::Failed);
                replay_outcome = Some(outcome);
            }
            Ok(Some(crate::free_launch::Terminal::Outcome(outcome))) => {
                ran = match &outcome {
                    RunOutcome::Returned(_) => Ok(STATUS_OK),
                    _ => ran,
                };
                replay_outcome = Some(outcome);
            }
            Ok(None) => {}
            Err(error) => ran = Err(crate::stop::Stop::Diverged(error)),
        }
    }
    // A stopped run whose workers still run its code: the launcher leaves it to them, and
    // the last to leave frees it; home state is destroyed here, on the admission thread.
    let pending = match &ran {
        Err(_) => crate::pool::busy(unsafe { (*ctx).run_ref() }),
        Ok(_) => 0,
    };
    let detached = pending > 0;
    // Copy only fully validated, quiescent evidence before any row/stack teardown.
    let mut trace = trace_destination
        .as_ref()
        .filter(|_| ordered_enabled)
        .map(|_| unsafe {
            crate::tasks::ordered::events::Trace::capture((*ctx).run_ptr(), pending)
                .map_err(str::to_owned)
        });
    let result = match &ran {
        Ok(_) => {
            unsafe { (*ctx).run_ref() }
                .launch
                .as_ref()
                .unwrap_or_else(|| crate::fault("a run without its launch frame"))
                .words
                .result
        }
        Err(_) => [0; RESULT_MAX as usize / 16],
    };
    #[cfg(feature = "measurement")]
    if !detached {
        crate::tasks::profile::finish(unsafe { &mut *ctx });
    }
    let bytes: Vec<u8> = result
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .take(result_size as usize)
        .collect();
    let mut record = format!("hirsel-observation {OBSERVATION_VERSION}\n");
    if let Some(placement) = &admitted_placement {
        let _ = writeln!(record, "placement {}", hex(&placement.to_json()));
    }
    // FAIL-3: a run a failure ended where it stood reports that failure as a failed entry,
    // but its tasks still hold their storage, so it is abandoned like a stopped run.
    let ended = matches!(ran, Err(crate::stop::Stop::Failed));
    let ran = if ended { Ok(STATUS_FAILURE) } else { ran };
    let outcome = if let Some(outcome) = replay_outcome {
        match crate::free_launch::replay_observation(program, &outcome) {
            Ok(text) => {
                record.push_str(&text);
                outcome
            }
            Err(error) => RunOutcome::Diverged(error),
        }
    } else {
        match ran {
            Ok(STATUS_OK) => {
                let _ = writeln!(record, "status {STATUS_OK}");
                record.push_str("result ");
                for b in &bytes {
                    let _ = write!(record, "{b:02x}");
                }
                record.push('\n');
                let code = if program.return_kind == 1 {
                    bytes[program.success_offset as usize]
                } else if bytes[..4] != [0, 0, 0, 0] {
                    1
                } else if program.return_kind == 2 {
                    bytes[program.success_offset as usize]
                } else {
                    0
                };
                let _ = writeln!(record, "exit_code {code}");
                hirsel_run_schema::RunOutcome::Returned(
                    hirsel_run_schema::Returned::from_exit_code(code),
                )
            }
            Ok(STATUS_FAILURE) => {
                let _ = writeln!(record, "status {STATUS_FAILURE}");
                let mut records = Vec::new();
                let mut failure = FailureRecord {
                    kind: 0,
                    rule: 0,
                    site: 0,
                    operation_site: 0,
                };
                if unsafe { hirsel_rt_v2_failure_primary(ctx, &mut failure) } == 0 {
                    crate::fault("a failure status without a recorded failure");
                }
                records.push(failure);
                for index in 0..unsafe { hirsel_rt_v2_failure_cause_count(ctx) } {
                    if unsafe { hirsel_rt_v2_failure_cause(ctx, index, &mut failure) } == 0 {
                        crate::fault("a failure cause out of range");
                    }
                    records.push(failure);
                }
                for failure in &records {
                    let _ = writeln!(
                        record,
                        "failure {} {} {:016x} {:016x}",
                        failure.kind, failure.rule, failure.site, failure.operation_site
                    );
                }
                let table = program.failure_table.unwrap_or_else(|| {
                    crate::fault("a failed run without its program's failure table")
                });
                let mut failures = records.iter().map(|record| table.failure(record));
                let primary = failures.next().expect("a primary failure");
                hirsel_run_schema::RunOutcome::Failed {
                    primary,
                    causes: failures.collect(),
                }
            }
            Ok(_) => crate::fault("an entry returned a status other than 0 or 1"),
            Err(stop) => stop.outcome(),
        }
    };
    let mut outcome = outcome;
    if let Some(session) = &free_session
        && let Err(error) = session.finish_recording(&outcome)
    {
        outcome = RunOutcome::Environmental(hirsel_run_schema::Environmental::Recording(error));
    }
    if let Some(session) = &free_session {
        session.report_recording(&mut free_projection)?;
        if trace_destination.is_some() && production_free {
            trace = Some(if pending != 0 {
                Err("canonical trace has running workers".into())
            } else {
                session.canonical_trace()
            });
        }
    }
    if !detached {
        crate::free::finish(unsafe { &mut *ctx }, &outcome);
    }
    let returned = !ended
        && matches!(
            outcome,
            hirsel_run_schema::RunOutcome::Returned(_)
                | hirsel_run_schema::RunOutcome::Failed { .. }
        );
    if returned {
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
    } else {
        // A stopped run's tasks still hold their storage: no exit audit (CONC-8).
        if !detached
            && entry.is_test()
            && matches!(outcome, hirsel_run_schema::RunOutcome::Blocked(_))
        {
            let waits = crate::tasks::test_waits(ctx);
            sinks
                .report
                .clone()
                .write_all(waits.as_bytes())
                .map_err(|e| format!("test wait report: {e}"))?;
        }
        if !detached {
            unsafe { (*ctx).abandon() };
        }
        if ended {
            record.push_str("ended\n");
        }
        match &outcome {
            hirsel_run_schema::RunOutcome::Blocked(waits) => {
                record.push_str("blocked\n");
                let _ = writeln!(record, "waits {}", hirsel_run_schema::Wait::encode(waits));
            }
            hirsel_run_schema::RunOutcome::BudgetExceeded(kind) => {
                let _ = writeln!(record, "budget_exceeded {}", kind.name());
            }
            hirsel_run_schema::RunOutcome::Signaled(kind) => {
                let _ = writeln!(record, "signaled {}", kind.name());
            }
            _ => {}
        }
        record.push_str("audit unavailable\n");
    }
    if detached {
        // The stopped run's workers hold it, not the launcher: the last to leave frees it.
        // Home state is destroyed here, on the admission thread, all the same.
        if let Some(holders) = crate::pool::holders(unsafe { (*ctx).run_ref() }) {
            control.track(holders);
        }
        unsafe { crate::context::detach(ctx) };
    } else {
        unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
    }
    drop(binding);
    control.finish(pending);
    // A stopped run's cause, as its observation records it.
    let stopped = match &outcome {
        hirsel_run_schema::RunOutcome::Refused(reason) => Some(format!("refused {reason}")),
        hirsel_run_schema::RunOutcome::Diverged(reason) => Some(format!("diverged {reason}")),
        hirsel_run_schema::RunOutcome::Environmental(cause) => Some(match cause {
            hirsel_run_schema::Environmental::CodecLimit => "environmental codec_limit".into(),
            hirsel_run_schema::Environmental::Recording(reason) => {
                format!("environmental recording {reason}")
            }
            hirsel_run_schema::Environmental::Output(reason) => {
                format!("environmental output {reason}")
            }
            hirsel_run_schema::Environmental::Stopped => "environmental stopped".into(),
        }),
        _ => None,
    };
    if let Some(stopped) = stopped {
        let _ = writeln!(record, "stopped {}", stopped.replace('\n', " "));
    }
    let mut write_observation = |record: &str| -> Result<(), String> {
        if let Some(file) = &mut observation {
            std::io::Write::write_all(file, record.as_bytes())
                .and_then(|()| std::io::Write::flush(file))
                .map_err(|e| format!("the observation file: {e}"))?;
        }
        Ok(())
    };
    // What the launch writes last: the replay's remaining output, the outputs' flush, the
    // tapes' trailers and the observation record.
    let mut finish = || -> Result<(), String> {
        echo_replay()?;
        if let Some(io) = &replay
            && let Err(error) = io.borrow().finish_outcome(&outcome)
        {
            let _ = flush_stdout(sinks);
            return Err(format!("the replayed tape: {error:?}"));
        }
        flush_stdout(sinks).map_err(|e| format!("standard output: {e}"))?;
        if let Some(io) = &real {
            io.borrow_mut()
                .finish_recording(&outcome)
                .map_err(|e| format!("the Io tape: {e}"))?;
        }
        if let Some(io) = &ordered_live {
            io.finish_recording(&outcome)
                .map_err(|e| format!("the Io tape: {e}"))?;
        }
        if let Some(writer) = &mut zero_recording {
            writer
                .finish_with_outcome(&outcome)
                .map_err(|e| format!("the Io tape: {e}"))?;
        }
        write_observation(&record)
    };
    match &outcome {
        hirsel_run_schema::RunOutcome::Returned(_)
        | hirsel_run_schema::RunOutcome::BudgetExceeded(_) => finish()?,
        hirsel_run_schema::RunOutcome::Blocked(waits) => {
            if report_failures {
                let _ = writeln!(sinks.report.clone(), "Blocked: {waits:?}");
            }
            finish()?;
        }
        hirsel_run_schema::RunOutcome::Failed { primary, causes } => {
            if report_failures {
                for failure in std::iter::once(primary).chain(causes) {
                    let _ = sinks
                        .report
                        .clone()
                        .write_all(crate::isolate::report(failure).as_bytes());
                }
            }
            finish()?
        }
        // The recording stopped short of a complete run: no trailer. A launcher that reads
        // the observation reports the stop from it; otherwise the run reports it here.
        hirsel_run_schema::RunOutcome::Refused(reason)
        | hirsel_run_schema::RunOutcome::Diverged(reason)
        | hirsel_run_schema::RunOutcome::Environmental(
            hirsel_run_schema::Environmental::Recording(reason),
        ) => {
            stopped_output(sinks, &echo_replay);
            if report_failures {
                let _ = writeln!(sinks.report.clone(), "hirsel-rt: Io refused: {reason}");
            }
            write_observation(&record)?;
        }
        hirsel_run_schema::RunOutcome::Environmental(cause) => {
            stopped_output(sinks, &echo_replay);
            if report_failures {
                let _ = writeln!(
                    sinks.report.clone(),
                    "hirsel-rt: environmental failure: {cause}"
                );
            }
            write_observation(&record)?;
        }
        // The process would have ended by the signal: its recording has no trailer.
        hirsel_run_schema::RunOutcome::Signaled(_) => {
            stopped_output(sinks, &echo_replay);
            write_observation(&record)?;
        }
        hirsel_run_schema::RunOutcome::UndefinedBehavior(diagnostic) => {
            stopped_output(sinks, &echo_replay);
            if report_failures {
                let _ = writeln!(sinks.report.clone(), "{diagnostic}");
            }
            write_observation(&record)?;
        }
        hirsel_run_schema::RunOutcome::Tooling(reason) => return Err(reason.clone()),
        hirsel_run_schema::RunOutcome::ToolingLimit(_)
        | hirsel_run_schema::RunOutcome::Trapped(_) => {
            crate::fault("an interpreter-only outcome in native execution")
        }
    }
    if let Some(error) = report_refusal {
        return Err(error);
    }
    if let (Some(destination), Some(trace)) = (trace_destination, trace) {
        let trace = trace?;
        if let Some(io) = &ordered_live {
            io.with_mount_pins(|grants| trace.publish(destination, grants, &output_grants))?;
        } else {
            let real_io = real.as_ref().map(|io| io.borrow());
            let grants = real_io.as_ref().map_or_else(Vec::new, |io| io.mount_pins());
            trace.publish(destination, &grants, &output_grants)?;
        }
    }
    Ok(outcome)
}

/// How a run executes beyond its program and selection (`crate::host::RunHandle`).
pub(crate) struct Options {
    /// The run's stop word, which a host's [`crate::host::RunControl`] shares.
    pub(crate) control: std::sync::Arc<crate::stop::Control>,
    /// The width and migrate-every-suspension mode a test asked for, or `None`.
    pub(crate) workers: Option<(u32, bool)>,
    /// The scheduling row counters, when the host asked for them.
    pub(crate) metrics: Option<std::sync::Arc<crate::pool::WorkerMetrics>>,
    pub(crate) trace_events: Option<std::path::PathBuf>,
}

/// The delay after which a test stops the run from another thread: `HIRSEL_RT_STOP_AFTER_MS`.
fn stop_after_from_env() -> Result<Option<std::time::Duration>, String> {
    match std::env::var("HIRSEL_RT_STOP_AFTER_MS") {
        Ok(text) => text
            .parse::<u64>()
            .map(|ms| Some(std::time::Duration::from_millis(ms)))
            .map_err(|_| format!("HIRSEL_RT_STOP_AFTER_MS is not a delay: {text:?}")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("HIRSEL_RT_STOP_AFTER_MS is not UTF-8".into())
        }
    }
}

/// The width and migration mode the environment asks for: `HIRSEL_RT_WORKERS` compute workers
/// beside the admission thread (1, the default, runs on it alone), and under
/// `HIRSEL_RT_MIGRATE` every task moves to another worker at every suspension, on three
/// workers unless `HIRSEL_RT_WORKERS` names at least two. Test knobs until multi-worker
/// execution is proven equal to one worker (design §9 W1).
fn workers_from_env() -> Result<(u32, bool), String> {
    let migrate = std::env::var_os("HIRSEL_RT_MIGRATE").is_some();
    let width = match std::env::var("HIRSEL_RT_WORKERS") {
        Ok(text) => text
            .parse::<u32>()
            .map_err(|_| format!("HIRSEL_RT_WORKERS is not a width: {text:?}"))?,
        Err(std::env::VarError::NotPresent) => 1,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("HIRSEL_RT_WORKERS is not UTF-8".into());
        }
    };
    Ok(match (width, migrate) {
        (0 | 1, true) => (3, true),
        other => other,
    })
}

/// A stopped run's output before its report: what the replay printed, then the flush.
fn stopped_output(sinks: &crate::host::Sinks, echo_replay: &dyn Fn() -> Result<(), String>) {
    if let Err(error) = echo_replay() {
        let _ = writeln!(sinks.report.clone(), "hirsel-rt: replay output: {error}");
    }
    if let Err(error) = flush_stdout(sinks) {
        let _ = writeln!(sinks.report.clone(), "hirsel-rt: replay output: {error}");
    }
}

/// Flush the run's stdout sink and Rust's line-buffered stdout, which a C `main` returning
/// does not flush: output after the last newline would be lost.
fn flush_stdout(sinks: &crate::host::Sinks) -> std::io::Result<()> {
    sinks.stdout.clone().flush()?;
    std::io::stdout().flush()
}

/// Ordinary flushed streaming tape, explicitly nonresumable after interruption.
/// The header is flushed before entry; each completion is flushed before delivery.
pub use crate::tape_binary::Writer as RecordingWriter;

/// The process's standard output as an unbuffered OS boundary: unlike std::io::stdout,
/// EBADF is never suppressed, so write errors reach the program as recorded outcomes.
pub struct RawStdout;
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

/// The run's count bound: `HIRSEL_RT_COUNT_MAX` in the diagnostic archive, else the full width.
fn count_max() -> u64 {
    #[cfg(feature = "diagnostic")]
    if let Some(max) = std::env::var("HIRSEL_RT_COUNT_MAX")
        .ok()
        .and_then(|max| max.parse().ok())
    {
        return max;
    }
    0
}

#[cfg(test)]
#[path = "launch_telemetry_tests.rs"]
mod telemetry_tests;
