//! Typed request/completion protocol. Expected I/O errors are data, never language failures.
use crate::handles::{Grant, Handles};
use crate::net::is_net_request;
pub use crate::net::{
    NetError, NetOperation, NetProvider, NetRule, NetTimeout, NetWorld, RootsDigest, ShutdownHow,
    SocketAddr, TimeoutKind, TlsFailure,
};
use hirsel_run_schema::confined_relative;
use hirsel_run_schema::{Access, RunRefusal};
use std::collections::{BTreeMap, BTreeSet};
pub(crate) fn mount_access(
    mounts: impl IntoIterator<Item = (String, Access)>,
) -> Result<BTreeMap<String, Access>, RunRefusal> {
    let mut result: BTreeMap<String, Access> = BTreeMap::new();
    for (path, access) in mounts {
        if (!path.is_empty()
            && path
                .split('/')
                .any(|p| p.is_empty() || matches!(p, "." | "..") || p.contains(['\\', '\0'])))
            || result.keys().any(|p| {
                p == &path
                    || p.is_empty()
                    || path.is_empty()
                    || p.strip_prefix(&path).is_some_and(|s| s.starts_with('/'))
                    || path.strip_prefix(p).is_some_and(|s| s.starts_with('/'))
            })
        {
            return Err(RunRefusal::Invalid {
                field: "io.dirs".into(),
                detail: "invalid, duplicate or overlapping virtual mount".into(),
            });
        }
        result.insert(path, access);
    }
    Ok(result)
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IoError {
    Fault(u32),
    Canceled(u32),
    NotFound,
    PermissionDenied,
    AlreadyExists,
    IsDirectory,
    NotDirectory,
    DirectoryNotEmpty,
    CrossDevice,
    NotGranted,
    /// A mapped file is now shorter than its mapping (`mapped.with`).
    Truncated,
}
/// Ordinals follow core InputError; host errno is translated to the existing fault input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputError {
    Fault(u32),
    Canceled(u32),
    InvalidInput,
    InvalidUtf8,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvError {
    NotGranted,
    InvalidUtf8,
}
/// The root Env identity is separate from every adapter-allocated logical handle.
pub const ENV_HANDLE: u64 = u64::MAX;
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdapterError {
    InvalidHandle(u64),
    ReplayMismatch(usize),
    MissingInput(usize),
    UnconsumedInput(usize),
    InvalidCompletion,
    RecordingFailed(String),
    /// CONC-8: the simulated world can never answer the request (no timeout given).
    Blocked,
    /// The provider of this run cannot perform the admitted operation.
    Unsupported(String),
    ProcessFailed {
        child: u64,
        site: u64,
        operation_site: u64,
        path: Vec<u32>,
    },
    /// A scripted signal took its default action, which ends the run.
    Signaled(hirsel_run_schema::SignalKind),
    /// Replay reached the recorded point where the run's place stopped it.
    Stopped,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Request {
    /// New-provider-only virtual advance. Old requests keep their wire tags.
    ProcessTime {
        from: u64,
        to: u64,
    },
    ProcessSettle {
        canceled: Vec<u64>,
    },
    Stdin {
        io: u64,
    },
    Stderr {
        io: u64,
    },
    ReadStdin {
        stdin: u64,
        max: u64,
    },
    PrintStderr {
        stderr: u64,
        bytes: Vec<u8>,
    },
    EnvGet {
        env: u64,
        name: String,
    },
    /// `env.narrow`: an Env granting exactly `names`, each granted by `env`.
    EnvNarrow {
        env: u64,
        names: Vec<String>,
    },
    MonotonicClock {
        io: u64,
    },
    WallClock {
        io: u64,
    },
    Monotonic {
        clock: u64,
    },
    Wall {
        clock: u64,
    },
    Random {
        io: u64,
    },
    /// `random.bytes` with a positive count; a zero count issues no request.
    RandomBytes {
        random: u64,
        count: u64,
    },
    /// `io.telemetry`.
    Telemetry {
        io: u64,
    },
    /// `capture.begin`: a span under `parent` (all-zero IDs for a root) described by
    /// `spec`; the provider supplies its identities.
    TelemetryBegin {
        telemetry: u64,
        parent: Vec<u8>,
        spec: Vec<u8>,
    },
    /// `capture.event`.
    TelemetryEvent {
        telemetry: u64,
        span: u64,
        payload: Vec<u8>,
    },
    /// `capture.finish`: the span ends with `status`.
    TelemetryFinish {
        telemetry: u64,
        span: u64,
        status: Vec<u8>,
    },
    /// `capture.log`.
    TelemetryLog {
        telemetry: u64,
        payload: Vec<u8>,
    },
    /// `capture.measure`.
    TelemetryMeasure {
        telemetry: u64,
        payload: Vec<u8>,
    },
    /// `clock.sleep_until`; the deadline is the Instant's nanosecond ticks.
    SleepUntil {
        clock: u64,
        deadline: u64,
    },
    Tasks {
        io: u64,
    },
    Net {
        io: u64,
    },
    Process {
        io: u64,
    },
    Signals {
        io: u64,
    },
    /// `signal.wait`: the kinds as bits (`hirsel_run_schema::SignalKind::bit`).
    SignalWait {
        signals: u64,
        kinds: u8,
    },
    TestProcess {
        test: u64,
        runs: Vec<hirsel_run_schema::ProcessRun>,
        children: Vec<hirsel_run_schema::ChildRun>,
    },
    /// `proc.narrow`: a Proc of `rules`, each within a rule of `process`, in its world.
    ProcessNarrow {
        process: u64,
        rules: Vec<hirsel_run_schema::ProcessRule>,
    },
    /// `proc.run`.
    ProcessRun {
        process: u64,
        command: hirsel_run_schema::Command,
        stdin: Vec<u8>,
        max_output: u64,
        deadline: Option<crate::process::Deadline>,
    },
    /// `proc.pipeline`.
    ProcessPipeline {
        process: u64,
        stages: Vec<hirsel_run_schema::Command>,
        stdin: Vec<u8>,
        max_output: u64,
        deadline: Option<crate::process::Deadline>,
    },
    /// `proc.spawn`.
    ProcessSpawn {
        process: u64,
        command: hirsel_run_schema::Command,
    },
    /// `proc.spawn_pipeline`.
    ProcessSpawnPipeline {
        process: u64,
        stages: Vec<hirsel_run_schema::Command>,
        max_stderr: u64,
    },
    /// The streaming child operations name their Proc, as stream operations name their Net.
    /// `group.write`, `group.close_stdin`, `group.read_stdout` and `group.kill` are these
    /// requests on a ChildGroup handle.
    ChildWrite {
        process: u64,
        child: u64,
        bytes: Vec<u8>,
    },
    ChildCloseStdin {
        process: u64,
        child: u64,
    },
    ChildRead {
        process: u64,
        child: u64,
        stream: hirsel_run_schema::ChildStream,
        max: u64,
    },
    ChildWait {
        process: u64,
        child: u64,
        deadline: Option<crate::process::Deadline>,
    },
    ChildKill {
        process: u64,
        child: u64,
        signal: hirsel_run_schema::Signal,
    },
    /// `proc.wait_group`.
    GroupWait {
        process: u64,
        group: u64,
        deadline: Option<crate::process::Deadline>,
    },
    TestCheck {
        test: u64,
        condition: bool,
    },
    TestCheckCase {
        test: u64,
        condition: bool,
        case: u64,
    },
    /// `test.fail`: an unconditional LANG-9 failure.
    TestFail {
        test: u64,
    },
    /// A test-local simulated monotonic clock.
    TestClock {
        test: u64,
    },
    TestSeed {
        test: u64,
    },
    /// `test.tasks`: a Tasks capability for the test's own tasks, under the run's scheduler.
    TestTasks {
        test: u64,
    },
    /// `test.log`: one escaped Test report record.
    TestLog {
        test: u64,
        text: String,
    },
    /// `test.expect_snapshot`: an invalid name fails LANG-9 without a record.
    TestSnapshot {
        test: u64,
        name: String,
        text: String,
    },
    TestNet {
        test: u64,
        peers: Vec<hirsel_run_schema::NetPeer>,
        listeners: Vec<hirsel_run_schema::NetListener>,
    },
    TestDir {
        test: u64,
        entries: Vec<(String, Vec<u8>)>,
    },
    /// `test.crash`: the simulated tree of `directory` becomes its durable state.
    TestCrash {
        test: u64,
        directory: u64,
        seed: u64,
    },
    Stdout {
        io: u64,
    },
    Directory {
        io: u64,
        path: String,
    },
    NarrowDir {
        directory: u64,
        path: String,
    },
    /// `dir.read_only`: the directory's own pinned root, with read access.
    ReadOnlyDir {
        directory: u64,
    },
    Print {
        stdout: u64,
        bytes: Vec<u8>,
    },
    Open {
        directory: u64,
        path: String,
    },
    OpenAppend {
        directory: u64,
        path: String,
    },
    Create {
        directory: u64,
        path: String,
    },
    FileRead {
        directory: u64,
        file: u64,
        max: u64,
    },
    FileSeek {
        directory: u64,
        file: u64,
        offset: u64,
    },
    FileWrite {
        directory: u64,
        file: u64,
        bytes: Vec<u8>,
    },
    FileAppend {
        directory: u64,
        file: u64,
        bytes: Vec<u8>,
    },
    NarrowFile {
        directory: u64,
        file: u64,
    },
    ReadDirPage {
        directory: u64,
        path: String,
        after: String,
        recursive: bool,
    },
    Metadata {
        directory: u64,
        path: String,
    },
    Mkdir {
        directory: u64,
        path: String,
    },
    Rename {
        directory: u64,
        from: String,
        to: String,
    },
    RemoveDir {
        directory: u64,
        path: String,
    },
    SetPermissions {
        directory: u64,
        path: String,
        permissions: u32,
    },
    RemoveAll {
        directory: u64,
        path: String,
    },
    Size {
        directory: u64,
        path: String,
    },
    Read {
        directory: u64,
        path: String,
    },
    Write {
        directory: u64,
        path: String,
        bytes: Vec<u8>,
    },
    Remove {
        directory: u64,
        path: String,
    },
    CreateAtomic {
        directory: u64,
        path: String,
    },
    WriteAll {
        directory: u64,
        file: u64,
        bytes: Vec<u8>,
    },
    Commit {
        directory: u64,
        file: u64,
    },
    /// `file.sync`: the file's bytes and its directory chain become durable.
    Sync {
        directory: u64,
        file: u64,
    },
    /// `file.put_new`: create-if-absent, never partial to a reader.
    PutNew {
        directory: u64,
        path: String,
        bytes: Vec<u8>,
        durable: bool,
    },
    /// `file.map`: a read-only mapping of the regular file's current contents.
    Map {
        directory: u64,
        path: String,
    },
    /// `mapped.with`'s view of the mapping's bytes, before its body runs.
    MapView {
        directory: u64,
        mapped: u64,
    },
    /// `mapped.with`'s check, after its body returns: is the file still at least as long?
    MapCheck {
        directory: u64,
        mapped: u64,
    },
    /// `file.watch`: watch a directory's entries (with `recursive`, every path below it) or
    /// a file.
    Watch {
        directory: u64,
        path: String,
        recursive: bool,
    },
    /// `watcher.next`: the watcher's next change event; it parks while none has arrived.
    WatchNext {
        directory: u64,
        watcher: u64,
    },
    /// `test.dir_event`: deliver `event` (its paths relative to the Dir) to the watchers of
    /// a test tree.
    TestDirEvent {
        test: u64,
        directory: u64,
        event: crate::watch::WatchEvent,
    },
    Close {
        handle: u64,
    },
    // Network. Stream and listener operations name their Net, as file
    // operations name their Dir; handles are tape-local ordinals.
    NetNarrow {
        net: u64,
        rules: Vec<NetRule>,
    },
    Resolve {
        net: u64,
        host: String,
        port: u16,
    },
    Connect {
        net: u64,
        host: String,
        port: u16,
        timeout: NetTimeout,
    },
    UdpBind {
        net: u64,
        addr: SocketAddr,
    },
    UdpSend {
        net: u64,
        socket: u64,
        bytes: Vec<u8>,
        addr: SocketAddr,
    },
    UdpRecv {
        net: u64,
        socket: u64,
        max: u64,
    },
    ConnectAddr {
        net: u64,
        addr: SocketAddr,
        timeout: NetTimeout,
    },
    /// `net.read` and `net.read_into` alike; only where the bytes land differs.
    NetRead {
        net: u64,
        stream: u64,
        max: u64,
    },
    NetWrite {
        net: u64,
        stream: u64,
        bytes: Vec<u8>,
    },
    NetWriteAll {
        net: u64,
        stream: u64,
        bytes: Vec<u8>,
    },
    Shutdown {
        net: u64,
        stream: u64,
        how: ShutdownHow,
    },
    SetTimeout {
        net: u64,
        stream: u64,
        which: TimeoutKind,
        timeout: NetTimeout,
    },
    PeerAddr {
        net: u64,
        stream: u64,
    },
    /// `net.split`: the stream's halves, read then write.
    NetSplit {
        net: u64,
        stream: u64,
    },
    /// `net.join`: the halves' connection again, when they are one connection's halves.
    NetJoin {
        net: u64,
        read: u64,
        write: u64,
    },
    TlsConnect {
        net: u64,
        stream: u64,
        server_name: String,
        roots: RootsDigest,
        /// `tls.connect_as`: the SHA-256 of the client identity's certificate-chain PEM; the
        /// key never appears.
        identity: Option<[u8; 32]>,
        /// `tls.connect_with`: the ALPN protocols offered, in order.
        alpn: Vec<String>,
    },
    TlsAccept {
        net: u64,
        stream: u64,
        server: crate::net::TlsServerDigest,
    },
    TlsPeer {
        net: u64,
        stream: u64,
    },
    /// `tls.negotiated_protocol`: answered from the stream's recorded handshake.
    TlsProtocol {
        net: u64,
        stream: u64,
    },
    TlsClose {
        net: u64,
        stream: u64,
    },
    Listen {
        net: u64,
        addr: SocketAddr,
    },
    Accept {
        net: u64,
        listener: u64,
    },
    LocalAddr {
        net: u64,
        listener: u64,
    },
    /// `net.unix_connect`: the socket path as the program spelled it.
    UnixConnect {
        net: u64,
        path: String,
    },
    UnixListen {
        net: u64,
        path: String,
    },
    /// A runner call (RUN-11): legacy providers wait for its outcome;
    /// qualified transport acknowledges handoff with Ready, then RunnerPoll
    /// observes the outcome. An immediate refusal is returned directly.
    RunnerCall(Box<hirsel_run_schema::RunnerRequest>),
    /// Qualified runner result probe; cancellation is forwarded once as part
    /// of this bounded canonical request, before its outcome is observed.
    RunnerPoll {
        key: [u8; 32],
        cancel: Option<u32>,
    },
    /// A cancellation forwarded to the call with key `key` (RUN-9), acknowledged with
    /// `Ready`.
    RunnerCancel {
        key: [u8; 32],
        cause: u32,
    },
    /// A callee run's own outcome as it ended (RUN-11), acknowledged with `Ready`: replay
    /// checks it, and a joined tape check compares it with its caller's completion.
    CallReturn(hirsel_run_schema::CalleeOutcome),
    /// The run's place stopped it here, before its next request (a simulated place's crash,
    /// RUN-6 `Abandoned`), acknowledged with `Ready`: replay stops the run at this event.
    Stopped,
    /// A scheduling input of cooperative runs, never a capability request: the parked
    /// request of wait `wait` (creation ordinal) was ready at the scheduling point after the
    /// `points` it waited through since it parked or was last ready, and was re-issued there.
    Ready {
        wait: u64,
        points: u64,
    },
    /// A scheduling input of a seeded scheduler (`inputs.scheduler`), never a capability
    /// request: candidate `choice` of the `count` runnable tasks ran next.
    Schedule {
        count: u64,
        choice: u64,
    },
    /// Scoped CPU admission/drain receipt, not a capability request.
    CpuControl {
        width: u32,
        action: u32,
        count: u64,
        outcome: u32,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClockError {
    Unavailable,
    OutOfRange,
}
/// A timer wait's failure: its clock's, or a cancellation (CONC-3) with its cause.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WaitError {
    Clock(ClockError),
    Canceled(u32),
}
/// `SignalError`, in its core variant order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SignalError {
    Canceled(u32),
    NotGranted,
    InvalidInput,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RandomError {
    Unavailable,
}
/// The admitted Random provider. Only the system provider reads the host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RandomSource {
    /// `random.splitmix64@1`: the next SplitMix64 state.
    SplitMix64(u64),
    /// `random.system@1`: the host's cryptographically secure generator.
    System,
}
impl RandomSource {
    /// Successive SplitMix64 words, little-endian, the last cut short; or `count`
    /// bytes from getrandom(2), retried on EINTR and short reads. The fill loop is
    /// adapted from Rust library/std/src/sys/random/linux.rs (third-party/NOTICES.md).
    pub fn draw(&mut self, count: u64) -> Result<Vec<u8>, RandomError> {
        let count = usize::try_from(count).map_err(|_| RandomError::Unavailable)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(count)
            .map_err(|_| RandomError::Unavailable)?;
        match self {
            Self::SplitMix64(state) => {
                while bytes.len() < count {
                    *state = state.wrapping_add(0x9e3779b97f4a7c15);
                    let mut z = *state;
                    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
                    let word = (z ^ (z >> 31)).to_le_bytes();
                    let take = (count - bytes.len()).min(8);
                    bytes.extend_from_slice(&word[..take]);
                }
            }
            Self::System => {
                bytes.resize(count, 0);
                let mut filled = 0;
                while filled < count {
                    // SAFETY: the unfilled tail of `bytes` is writable for its length.
                    let n = unsafe {
                        libc::getrandom(bytes[filled..].as_mut_ptr().cast(), count - filled, 0)
                    };
                    if n < 0 {
                        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                        {
                            continue;
                        }
                        return Err(RandomError::Unavailable);
                    }
                    filled += n as usize;
                }
            }
        }
        Ok(bytes)
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Completion {
    /// Confirmed host file input; never emitted by a simulated tree.
    FileInput(FileInput),
    Input(Result<Vec<u8>, InputError>),
    Env(Result<Option<String>, EnvError>),
    ClockGrant(Result<u64, ()>),
    Monotonic(Result<u64, ClockError>),
    Wall(Result<i64, ClockError>),
    Random(Result<Vec<u8>, RandomError>),
    /// A completed timer wait.
    Wait(Result<(), WaitError>),
    /// A completed `signal.wait`.
    Signal(Result<hirsel_run_schema::SignalKind, SignalError>),
    /// Deterministic capability refusal; the operation issued no host effect.
    Refused(u64),
    Checked(bool),
    Handle(Result<u64, IoError>),
    OptionalFile(Option<u64>),
    Unit(Result<(), IoError>),
    Size(Result<u64, IoError>),
    Bytes(Result<Vec<u8>, IoError>),
    Entries(Result<Vec<DirEntry>, IoError>),
    Metadata(Result<Metadata, IoError>),
    NetHandle(Result<u64, NetError>),
    Listened(Result<(u64, SocketAddr), NetError>),
    Addresses(Result<Vec<SocketAddr>, NetError>),
    NetBytes(Result<Vec<u8>, NetError>),
    Datagram(Result<(Vec<u8>, SocketAddr, bool), NetError>),
    Count(Result<u64, NetError>),
    NetUnit(Result<(), NetError>),
    /// `tls.peer`: the verified client chain's DER certificates, concatenated.
    Certificates(Result<Option<Vec<u8>>, NetError>),
    /// A TLS handshake's outcome, or `tls.negotiated_protocol`'s answer: the ALPN protocol
    /// negotiated, if any.
    Protocol(Result<Option<String>, NetError>),
    Address(Result<SocketAddr, NetError>),
    /// `net.split`: the read half and the write half.
    Halves(Result<(u64, u64), NetError>),
    Accepted(Result<(u64, SocketAddr), NetError>),
    Process(hirsel_run_schema::ProcessOutcome),
    /// `proc.pipeline`.
    Pipeline(hirsel_run_schema::PipelineOutcome),
    /// `proc.spawn` and `proc.spawn_pipeline`: the Child or ChildGroup handle.
    Spawned(Result<u64, hirsel_run_schema::ProcessFailure>),
    /// `child.write`, `child.close_stdin` and `child.kill`.
    ChildUnit(Result<(), hirsel_run_schema::ProcessFailure>),
    /// A child read: the bytes it delivered, empty at end of stream.
    ChildBytes(Result<Vec<u8>, hirsel_run_schema::ProcessFailure>),
    ChildExit(Result<hirsel_run_schema::ExitStatus, hirsel_run_schema::ProcessFailure>),
    /// `proc.wait_group`.
    GroupExit(Result<hirsel_run_schema::GroupOutput, hirsel_run_schema::ProcessFailure>),
    /// A durable journal barrier parked the request before provider issue.
    Held,
    /// The issued request would block and its task parked; `committed` when it already made part
    /// of its effect, so cancellation no longer completes it.
    Blocked {
        committed: bool,
    },
    /// The completion of a scheduling input, `Request::Ready` or `Request::Schedule`, and
    /// a forwarded cancellation's acknowledgement.
    Ready,
    /// A runner call's outcome.
    Runner(hirsel_run_schema::RunnerCompletion),
    /// `mapped.with`'s view.
    View(MapView),
    /// `watcher.next`'s event, its paths relative to the watcher's Dir.
    Watch(Result<crate::watch::WatchEvent, IoError>),
    /// `capture.begin`'s span: its handle and the identities its provider supplied.
    Span {
        handle: u64,
        trace: [u8; 16],
        span: [u8; 8],
    },
}
/// Successful observations of host files. Request, authority and handle allocation
/// remain independently checked; these values replace only the storage provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileInput {
    Opened(u64),
    Size(u64),
    Bytes(Vec<u8>),
}
impl FileInput {
    pub(crate) fn capture(request: &Request, completion: &Completion) -> Option<Self> {
        match (request, completion) {
            (Request::Open { .. }, Completion::Handle(Ok(id))) => Some(Self::Opened(*id)),
            (Request::Size { .. }, Completion::Size(Ok(size))) => Some(Self::Size(*size)),
            (Request::Read { .. } | Request::FileRead { .. }, Completion::Bytes(Ok(bytes))) => {
                Some(Self::Bytes(bytes.clone()))
            }
            _ => None,
        }
    }
    pub(crate) fn completion(&self, request: &Request) -> Result<Completion, AdapterError> {
        match (request, self) {
            (Request::Open { .. }, Self::Opened(id)) => Ok(Completion::Handle(Ok(*id))),
            (Request::Size { .. }, Self::Size(size)) => Ok(Completion::Size(Ok(*size))),
            (Request::Read { .. }, Self::Bytes(bytes)) => Ok(Completion::Bytes(Ok(bytes.clone()))),
            (Request::FileRead { max, .. }, Self::Bytes(bytes))
                if bytes.len() as u64 <= (*max).min(65536) =>
            {
                Ok(Completion::Bytes(Ok(bytes.clone())))
            }
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
}
/// The bytes a mapping lends `mapped.with`'s body. A host mapping that nothing records
/// keeps them where they are mapped: `block` is then the address of the static block in
/// front of them (`crate::mapping`), and `bytes` is absent. Equality compares the bytes.
#[derive(Clone, Debug)]
pub struct MapView {
    pub length: u64,
    pub bytes: Option<Vec<u8>>,
    pub(crate) block: usize,
}
impl MapView {
    pub fn of(bytes: Vec<u8>) -> Self {
        Self {
            length: bytes.len() as u64,
            bytes: Some(bytes),
            block: 0,
        }
    }
    /// A copy of the viewed bytes, read before the mapping that lends them is closed.
    pub fn contents(&self) -> Vec<u8> {
        match &self.bytes {
            Some(bytes) => bytes.clone(),
            None => crate::mapping::contents(self.block, self.length),
        }
    }
}
impl PartialEq for MapView {
    fn eq(&self, other: &Self) -> bool {
        self.length == other.length && self.bytes == other.bytes
    }
}
impl Eq for MapView {}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fault {
    pub error: IoError,
    pub partial: usize,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Event {
    pub request: Request,
    pub completion: Completion,
    pub fault: Option<Fault>,
    /// The keyed recorder's resource footprint of this request event; None
    /// where the recorder declared none (scheduling inputs, coupled adapters).
    pub resources: Option<Box<crate::io_keys::Footprint>>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum FileKind {
    Regular,
    Directory,
    Symlink,
    Other,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metadata {
    pub kind: FileKind,
    pub size: u64,
    pub modified_unix_ns: Option<i128>,
    pub permissions: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirEntry {
    pub path: String,
    pub kind: FileKind,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Tree {
    pub directories: BTreeSet<String>,
    pub files: BTreeMap<String, Vec<u8>>,
    /// Initial/shared regular-file identity, alias to its lexically first name.
    pub file_aliases: BTreeMap<String, String>,
    /// Physical relative links; targets remain data until a confined operation resolves them.
    pub links: BTreeMap<String, String>,
    /// Snapshot provider metadata, including special entries. New simulated entries omit it.
    pub metadata: BTreeMap<String, Metadata>,
}
impl Tree {
    pub(crate) fn regular_paths(&self) -> BTreeSet<&String> {
        self.files
            .keys()
            .chain(
                self.metadata
                    .iter()
                    .filter_map(|(p, m)| (m.kind == FileKind::Regular).then_some(p)),
            )
            .collect()
    }
    pub fn new(files: BTreeMap<String, Vec<u8>>) -> Result<Self, IoError> {
        let mut tree = Self {
            directories: BTreeSet::from([String::new()]),
            files: BTreeMap::new(),
            file_aliases: BTreeMap::new(),
            links: BTreeMap::new(),
            metadata: BTreeMap::new(),
        };
        for (path, bytes) in files {
            let path = confined_relative("", &path)
                .filter(|p| !p.is_empty())
                .ok_or(IoError::Fault(1))?;
            let mut p = path.as_str();
            while let Some((parent, _)) = p.rsplit_once('/') {
                tree.directories.insert(parent.into());
                p = parent;
            }
            tree.files.insert(path, bytes);
        }
        if tree.files.keys().any(|p| tree.directories.contains(p)) {
            return Err(IoError::Fault(1));
        }
        Ok(tree)
    }
    /// Build an isolated test snapshot with SimulationConfig's virtual mount selectors.
    pub fn fixture(
        files: BTreeMap<String, Vec<u8>>,
        mounts: BTreeMap<String, Access>,
    ) -> Result<Self, hirsel_run_schema::RunRefusal> {
        let invalid = || hirsel_run_schema::RunRefusal::Invalid {
            field: "fixture.files".into(),
            detail: "invalid snapshot path or mount collision".into(),
        };
        let mounts = mount_access(mounts)?;
        let mut tree = Self::new(files).map_err(|_| invalid())?;
        for selector in mounts.keys() {
            let mut path = selector.as_str();
            loop {
                if tree.files.contains_key(path) {
                    return Err(invalid());
                }
                tree.directories.insert(path.into());
                let Some((parent, _)) = path.rsplit_once('/') else {
                    break;
                };
                path = parent;
            }
        }
        Ok(tree)
    }
    /// Resolve a relative link using an explicit granted root. The snapshot
    /// contains no host paths or handles; invalid targets remain file faults.
    pub(crate) fn resolve(
        &self,
        root: &str,
        path: &str,
        follow_leaf: bool,
    ) -> Result<String, IoError> {
        use std::collections::VecDeque;
        if path.starts_with('/') || path.contains(['\\', '\0']) {
            return Err(IoError::Fault(1));
        }
        let mut parts: Vec<String> = root
            .split('/')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect();
        let base = parts.len();
        let mut pending: VecDeque<String> = path.split('/').map(str::to_owned).collect();
        let mut links = 0;
        while let Some(part) = pending.pop_front() {
            match part.as_str() {
                "" | "." => continue,
                ".." => {
                    if parts.len() == base {
                        return Err(IoError::Fault(1));
                    }
                    parts.pop();
                    continue;
                }
                _ => {}
            }
            parts.push(part);
            let current = parts.join("/");
            if (follow_leaf || !pending.is_empty())
                && let Some(target) = self.links.get(&current)
            {
                links += 1;
                if links > 40 || target.starts_with('/') || target.contains(['\\', '\0']) {
                    return Err(IoError::Fault(1));
                }
                parts.pop();
                for part in target.split('/').rev() {
                    pending.push_front(part.into());
                }
            } else if !pending.is_empty() && !self.directories.contains(&current) {
                return Err(if self.files.contains_key(&current) {
                    IoError::NotDirectory
                } else {
                    IoError::NotFound
                });
            }
        }
        Ok(parts.join("/"))
    }
}
/// Storage is always relative to the explicit root granted at launch.
pub(crate) trait Storage {
    fn destination(&mut self, _path: &str) -> Result<(), IoError> {
        Ok(())
    }
    fn activate(&mut self, _id: u64, _root: &str) {}
    fn file_fits(&mut self, id: u64, path: &str) -> bool;
    fn open_file(&mut self, path: &str, id: u64) -> Result<(), IoError>;
    fn create_file(&mut self, path: &str, id: u64) -> Result<(), IoError>;
    fn open_append_file(&mut self, path: &str, id: u64) -> Result<(), IoError>;
    fn read_file(&mut self, id: u64, offset: u64, max: u64) -> Result<Vec<u8>, IoError>;
    fn write_file(&mut self, id: u64, bytes: &[u8], all: bool) -> Result<u64, IoError>;
    fn transfer_file(&mut self, _from: u64, _to: Option<u64>) {}
    fn directory(&mut self, path: &str, id: u64) -> Result<(), IoError>;
    fn read(&mut self, path: &str) -> Result<Vec<u8>, IoError>;
    fn size(&mut self, path: &str) -> Result<u64, IoError> {
        self.read(path).map(|bytes| bytes.len() as u64)
    }
    fn write(
        &mut self,
        path: &str,
        bytes: &[u8],
        append: bool,
        exclusive: bool,
    ) -> Result<(), IoError>;
    fn remove(&mut self, path: &str) -> Result<(), IoError>;
    fn rename(&mut self, from: &str, to: &str) -> Result<(), IoError>;
    fn move_entry(&mut self, from: &str, to: &str) -> Result<(), IoError>;
    fn mkdir(&mut self, path: &str) -> Result<(), IoError>;
    fn remove_dir(&mut self, path: &str) -> Result<(), IoError>;
    fn set_permissions(&mut self, path: &str, permissions: u32) -> Result<(), IoError>;
    /// Make open file `id`'s bytes, and every directory from the parent of `path` (where it
    /// was opened) up to the mount root, durable.
    fn sync_file(&mut self, id: u64, path: &str) -> Result<(), IoError>;
    /// Create `path` holding exactly `bytes` unless an entry exists; equal bytes succeed.
    fn put_new(&mut self, path: &str, bytes: &[u8], durable: bool) -> Result<(), IoError>;
    /// Whether open handle `id` predates its simulated tree's last crash.
    fn stale(&self, _id: u64) -> bool {
        false
    }
    fn remove_all(&mut self, path: &str, limit: Option<usize>) -> Result<(), IoError>;
    /// Map the regular file at `path` read-only as mapping `id`; its length.
    fn map_file(&mut self, path: &str, id: u64) -> Result<u64, IoError>;
    /// The `length` bytes of mapping `id`, as `mapped.with` lends them: the file's current
    /// bytes, zero past its end. `capture` copies host bytes for the tape.
    fn map_view(&mut self, id: u64, length: u64, capture: bool) -> MapView;
    /// Whether mapping `id`'s file is now shorter than `length`, or a read past its end
    /// was answered with zeros.
    fn map_truncated(&mut self, id: u64, length: u64) -> bool;
    fn unmap(&mut self, id: u64);
    /// Watch the directory or file at `path` as watcher `id`; with `recursive`, every
    /// directory below it too.
    fn watch(&mut self, path: &str, id: u64, recursive: bool) -> Result<(), IoError>;
    /// Watcher `id`'s next event, its paths relative to Dir root `root`, or `None` when none
    /// has arrived: a cooperative host provider then records its descriptor interest.
    fn watch_next(
        &mut self,
        id: u64,
        root: &str,
    ) -> Result<Option<crate::watch::WatchEvent>, IoError>;
    fn unwatch(&mut self, id: u64);
    fn metadata(&mut self, path: &str) -> Result<Metadata, IoError>;
    fn read_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, IoError>;
    fn read_dir_page(
        &mut self,
        _path: &str,
        _after: &str,
        _recursive: bool,
    ) -> Result<Vec<DirEntry>, IoError> {
        Err(IoError::Fault(1))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct State {
    pub handles: Handles,
    pub(crate) monotonic_granted: bool,
    pub(crate) wall_granted: bool,
    pub(crate) last_monotonic: Option<u64>,
    pub(crate) test_last_monotonic: Option<u64>,
    pub(crate) test_clocks: BTreeSet<u64>,
    pub(crate) random: RandomSource,
    /// The kinds the run's Signals observes, as bits; zero without the grant.
    pub(crate) signal_kinds: u8,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub env: BTreeMap<String, Option<String>>,
    pub fake_trees: BTreeMap<u64, Tree>,
    pub fake_directories: BTreeMap<u64, u64>,
    fake_files: BTreeMap<u64, crate::io_sim::FileIdentities>,
    fake_roots: BTreeMap<u64, BTreeMap<u64, String>>,
    fake_detached: BTreeMap<u64, BTreeSet<u64>>,
    pub projections: BTreeSet<String>,
    pub mounts: BTreeMap<String, Access>,
    /// The admitted RunConfig rule set of the root Net; replay re-derives from it.
    pub net_rules: BTreeSet<NetRule>,
    pub net_world: NetWorld,
    pub net_scripts: BTreeMap<u64, crate::io_net_script::ScriptNet>,
    pub(crate) pem_validity: BTreeMap<u64, ([u8; 32], bool)>,
    /// The identity material supplied for each stream's pending `tls.accept`.
    pub(crate) tls_identities: BTreeMap<u64, crate::net::TlsIdentity>,
    pub(crate) tls_servers: BTreeMap<u64, crate::net::TlsServer>,
    /// The TLS engine, once a TLS entry handed it over (`Adapter::tls`).
    pub(crate) tls: crate::net_tls::Engine,
    /// The Stream grant of each split connection, keyed by stream, with the number of its
    /// halves still alive.
    pub(crate) split_streams: crate::id_map::IdMap<(crate::handles::Grant, u8)>,
    /// The admitted rules of the root Proc and the world serving it.
    pub process_rules: Vec<hirsel_run_schema::ProcessRule>,
    pub timed_process: bool,
    pub timed_waits: BTreeMap<u64, Option<u64>>,
    pub timed_requests: BTreeMap<u64, Request>,
    pub process_batch_pending: bool,
    pub claimed_children: BTreeSet<u64>,
    pub process_sites: BTreeMap<u64, (u64, u64, Vec<u32>)>,
    pub timed_sleeps: BTreeMap<u64, u64>,
    pub process_world: crate::process::ProcessWorld,
    /// Unused entries of each scripted world, keyed by world.
    pub process_scripts: BTreeMap<u64, Vec<Option<hirsel_run_schema::ProcessRun>>>,
    /// Unused streaming-child entries of each scripted world, keyed by world.
    pub child_scripts: BTreeMap<u64, Vec<Option<hirsel_run_schema::TimedChildRun>>>,
    /// Spawned scripted children, keyed by Child handle.
    pub script_children: BTreeMap<u64, hirsel_run_schema::ScriptChild>,
    /// Spawned scripted pipelines, keyed by ChildGroup handle.
    pub script_groups: BTreeMap<u64, hirsel_run_schema::ScriptGroup>,
    /// Whether a host mapping's view copies its bytes, for a tape that keeps them.
    pub(crate) capture_views: bool,
    /// The queue of each test tree's watcher, with its tree; `test.dir_event` fills them.
    fake_watches: BTreeMap<u64, (u64, crate::watch::WatchQueue)>,
    /// The run's Telemetry provider, delivered identities and simulated exporter.
    pub telemetry: crate::telemetry::Capture,
}
impl Default for State {
    fn default() -> Self {
        Self {
            handles: Handles::default(),
            monotonic_granted: false,
            wall_granted: false,
            last_monotonic: None,
            test_last_monotonic: None,
            test_clocks: Default::default(),
            random: RandomSource::SplitMix64(0),
            signal_kinds: 0,
            stdout: vec![],
            stderr: Vec::new(),
            env: BTreeMap::new(),
            fake_trees: BTreeMap::new(),
            fake_directories: BTreeMap::new(),
            fake_files: BTreeMap::new(),
            fake_roots: BTreeMap::new(),
            fake_detached: BTreeMap::new(),
            mounts: BTreeMap::from([(String::new(), Access::ReadWrite)]),
            net_rules: BTreeSet::new(),
            net_world: NetWorld::Host,
            net_scripts: BTreeMap::new(),
            pem_validity: BTreeMap::new(),
            tls_identities: BTreeMap::new(),
            tls_servers: BTreeMap::new(),
            tls: Default::default(),
            split_streams: Default::default(),
            process_rules: vec![],
            timed_process: false,
            timed_waits: BTreeMap::new(),
            timed_requests: BTreeMap::new(),
            process_batch_pending: false,
            claimed_children: BTreeSet::new(),
            process_sites: BTreeMap::new(),
            timed_sleeps: BTreeMap::new(),
            process_world: crate::process::ProcessWorld::Host,
            process_scripts: BTreeMap::new(),
            child_scripts: BTreeMap::new(),
            script_children: BTreeMap::new(),
            script_groups: BTreeMap::new(),
            capture_views: true,
            fake_watches: BTreeMap::new(),
            telemetry: Default::default(),
            projections: [
                "stdin",
                "stderr",
                "stdout",
                "tasks",
                "net",
                "process",
                "random",
                "dir",
                "telemetry",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}
impl State {
    pub(crate) fn fixture(&mut self, tree: Tree, mounts: BTreeMap<String, Access>) {
        self.mounts = mounts;
        self.fake_files
            .insert(0, crate::io_sim::FileIdentities::new(&tree));
        self.fake_trees.insert(0, tree);
    }
    pub(crate) fn denied_projection(&self, request: &Request) -> bool {
        match request {
            Request::Directory { path, .. } if self.mount(path).is_none() => return true,
            Request::Write { directory, .. }
            | Request::Remove { directory, .. }
            | Request::Mkdir { directory, .. }
            | Request::Rename { directory, .. }
            | Request::RemoveDir { directory, .. }
            | Request::RemoveAll { directory, .. }
            | Request::SetPermissions { directory, .. }
            | Request::OpenAppend { directory, .. }
            | Request::Create { directory, .. }
            | Request::FileWrite { directory, .. }
            | Request::FileAppend { directory, .. }
            | Request::CreateAtomic { directory, .. }
            | Request::WriteAll { directory, .. }
            | Request::Commit { directory, .. }
            | Request::PutNew { directory, .. } => {
                return matches!(
                    self.handles.rows.get(directory),
                    Some(Grant::Directory {
                        access: Access::Read,
                        ..
                    })
                );
            }
            _ => {}
        }
        let name = match request {
            Request::Stdin { .. } => "stdin",
            Request::Stderr { .. } => "stderr",
            Request::Stdout { .. } => "stdout",
            Request::Tasks { .. } => "tasks",
            Request::Net { .. } => "net",
            Request::Process { .. } => "process",
            Request::Random { .. } => "random",
            Request::Telemetry { .. } => "telemetry",
            Request::Signals { .. } => "signals",
            Request::Directory { .. } => "dir",
            _ => return false,
        };
        !self.projections.contains(name)
    }
    fn mount<'a>(&'a self, path: &'a str) -> Option<(&'a str, &'a str, Access)> {
        self.mounts.iter().find_map(|(selector, access)| {
            let local = if selector.is_empty() {
                Some(path)
            } else if path == selector {
                Some("")
            } else {
                path.strip_prefix(selector)?.strip_prefix('/')
            }?;
            Some((selector.as_str(), local, *access))
        })
    }
    /// `signal.wait`'s program checks, in order: an empty set, then an unobserved kind.
    pub(crate) fn signal_check(&self, kinds: u8) -> Option<SignalError> {
        if kinds == 0 {
            Some(SignalError::InvalidInput)
        } else if kinds & !self.signal_kinds != 0 {
            Some(SignalError::NotGranted)
        } else {
            None
        }
    }
    /// Whether Env `env` grants `name`: the root grants every name of the run's Env.
    pub(crate) fn env_granted(&self, env: u64, name: &str) -> Result<bool, AdapterError> {
        match self.handles.rows.get(&env) {
            _ if env == ENV_HANDLE => Ok(self.env.contains_key(name)),
            Some(Grant::Env { names }) => Ok(names.contains(name)),
            _ => Err(AdapterError::InvalidHandle(env)),
        }
    }
    pub(crate) fn access(&self, id: u64) -> Result<Access, AdapterError> {
        match self.handles.rows.get(&id) {
            Some(Grant::Directory { access, .. }) => Ok(*access),
            _ => Err(AdapterError::InvalidHandle(id)),
        }
    }
    /// The confined directory, root and path at which execute_storage performs
    /// one of the path syscalls a concurrent caller may run without its lock.
    /// None when execute_storage would refuse before reaching storage.
    pub(crate) fn path_target(&self, request: &Request) -> Option<(u64, String, String)> {
        use Request as R;
        let (directory, path) = match request {
            R::Open { directory, path }
            | R::OpenAppend { directory, path }
            | R::Create { directory, path }
            | R::Size { directory, path }
            | R::Metadata { directory, path }
            | R::Mkdir { directory, path }
            | R::RemoveDir { directory, path }
            | R::Read { directory, path }
            | R::Remove { directory, path } => (*directory, path),
            _ => return None,
        };
        let root = self.directory(directory).ok()?;
        let path = confined_relative(&root, path)?;
        if path == root && matches!(request, R::Mkdir { .. } | R::RemoveDir { .. }) {
            return None;
        }
        Some((directory, root, path))
    }
    pub(crate) fn directory(&self, id: u64) -> Result<String, AdapterError> {
        match self.handles.rows.get(&id) {
            Some(Grant::Directory { root, .. }) => Ok(root.clone()),
            _ => Err(AdapterError::InvalidHandle(id)),
        }
    }
    pub(crate) fn expect(&self, id: u64, grant: Grant) -> Result<(), AdapterError> {
        if self.handles.rows.get(&id) == Some(&grant) {
            Ok(())
        } else {
            Err(AdapterError::InvalidHandle(id))
        }
    }
    pub fn grant_test(&mut self, seed: u64) -> u64 {
        self.handles.insert(Grant::Test { seed })
    }
    /// A `test.dir` tree's watcher asked for its next event. Only `test.dir_event` fills its
    /// queue, which recording and replay derive alike, so its wait is never recorded: every
    /// scheduling point re-issues it ([`Adapter::injected_wait`]).
    pub(crate) fn injected_wait(&self, request: &Request) -> bool {
        matches!(request, Request::WatchNext { directory, watcher }
            if matches!(self.handles.rows.get(watcher), Some(Grant::Watcher { directory: owner, .. }) if owner == directory)
                && self.fake_watches.contains_key(watcher))
    }
    /// An injected wait with nothing queued: both adapters answer `Blocked` without
    /// recording or consuming an event.
    pub(crate) fn awaits_injection(&self, request: &Request) -> bool {
        self.injected_wait(request)
            && matches!(request, Request::WatchNext { watcher, .. }
                if self.fake_watches.get(watcher).is_some_and(|(_, queue)| queue.is_empty()))
    }
    pub(crate) fn is_fake_request(&self, request: &Request) -> bool {
        self.fake_tree(request).is_some()
    }
    pub(crate) fn fake_file_stale(&self, directory: u64, file: u64) -> bool {
        self.fake_directories
            .get(&directory)
            .and_then(|tree| self.fake_files.get(tree))
            .is_some_and(|files| files.is_stale(file))
    }
    fn fake_tree(&self, request: &Request) -> Option<u64> {
        if matches!(request, Request::Directory { .. }) && self.fake_trees.contains_key(&0) {
            return Some(0);
        }
        let directory = self.file_directory(request)?;
        self.fake_directories.get(&directory).copied()
    }
    pub(crate) fn file_directory(&self, request: &Request) -> Option<u64> {
        match request {
            Request::NarrowDir { directory, .. }
            | Request::ReadOnlyDir { directory }
            | Request::Open { directory, .. }
            | Request::FileRead { directory, .. }
            | Request::FileSeek { directory, .. }
            | Request::NarrowFile { directory, .. }
            | Request::Size { directory, .. }
            | Request::ReadDirPage { directory, .. }
            | Request::Metadata { directory, .. }
            | Request::Read { directory, .. }
            | Request::Write { directory, .. }
            | Request::Remove { directory, .. }
            | Request::Mkdir { directory, .. }
            | Request::Rename { directory, .. }
            | Request::RemoveDir { directory, .. }
            | Request::RemoveAll { directory, .. }
            | Request::SetPermissions { directory, .. }
            | Request::OpenAppend { directory, .. }
            | Request::Create { directory, .. }
            | Request::FileWrite { directory, .. }
            | Request::FileAppend { directory, .. }
            | Request::CreateAtomic { directory, .. }
            | Request::WriteAll { directory, .. }
            | Request::Commit { directory, .. }
            | Request::Sync { directory, .. }
            | Request::PutNew { directory, .. } => Some(*directory),
            Request::Map { directory, .. }
            | Request::MapView { directory, .. }
            | Request::MapCheck { directory, .. }
            | Request::Watch { directory, .. }
            | Request::WatchNext { directory, .. } => Some(*directory),
            Request::Close { handle } => match self.handles.rows.get(handle) {
                Some(
                    Grant::File { directory, .. }
                    | Grant::AtomicFile { directory, .. }
                    | Grant::Mapped { directory, .. }
                    | Grant::Watcher { directory, .. },
                ) => Some(*directory),
                _ => return None,
            },
            _ => return None,
        }
    }
    pub fn execute(
        &mut self,
        storage: &mut impl Storage,
        net: &mut dyn NetProvider,
        request: &Request,
        fault: Option<&Fault>,
    ) -> Result<Completion, AdapterError> {
        if is_net_request(request)
            || matches!(request, Request::Close { handle } if matches!(
                self.handles.rows.get(handle),
                Some(
                    Grant::Stream { .. }
                        | Grant::Half { .. }
                        | Grant::Listener { .. }
                        | Grant::UdpSocket { .. }
                )
            ))
        {
            // Network outcomes are provider inputs, never injected file faults.
            if fault.is_some() {
                return Err(AdapterError::InvalidCompletion);
            }
            return self.execute_net(net, request);
        }
        // Telemetry observations are accepted, never faulted: capture is not delivery.
        if matches!(request, Request::Telemetry { .. })
            || crate::telemetry::is_telemetry(request)
            || matches!(request, Request::Close { handle }
                if matches!(self.handles.rows.get(handle), Some(Grant::Span { .. })))
        {
            if fault.is_some() {
                return Err(AdapterError::InvalidCompletion);
            }
            return self.execute_telemetry(request);
        }
        // Process outcomes are provider inputs too; only a host adapter can spawn.
        if self.is_process(request) {
            if fault.is_some() {
                return Err(AdapterError::InvalidCompletion);
            }
            return self.execute_process(
                &mut |_| {
                    Err(AdapterError::Unsupported(
                        "this provider cannot run host programs".into(),
                    ))
                },
                request,
            );
        }
        if matches!(
            request,
            Request::FileRead { .. }
                | Request::Read { .. }
                | Request::Open { .. }
                | Request::Create { .. }
                | Request::OpenAppend { .. }
                | Request::CreateAtomic { .. }
                | Request::Commit { .. }
        ) && fault.is_some_and(|f| f.partial != 0 && !matches!(f.error, IoError::Canceled(_)))
        {
            return Err(AdapterError::InvalidCompletion);
        }
        if matches!(
            request,
            Request::ReadDirPage { .. }
                | Request::Metadata { .. }
                | Request::Mkdir { .. }
                | Request::RemoveDir { .. }
                | Request::SetPermissions { .. }
                | Request::Rename { .. }
                | Request::Sync { .. }
                | Request::PutNew { .. }
        ) && fault.is_some_and(|f| f.partial != 0)
        {
            return Err(AdapterError::InvalidCompletion);
        }
        // Policy denial is independently derived, never an injected tape fault.
        if fault.is_some_and(|f| f.error == IoError::NotGranted)
            || (self.denied_projection(request) && fault.is_some())
        {
            return Err(AdapterError::InvalidCompletion);
        }
        if matches!(request, Request::Print { .. })
            && fault.is_some_and(|f| {
                matches!(
                    f.error,
                    IoError::NotFound
                        | IoError::PermissionDenied
                        | IoError::AlreadyExists
                        | IoError::IsDirectory
                        | IoError::NotDirectory
                        | IoError::DirectoryNotEmpty
                        | IoError::CrossDevice
                        | IoError::Truncated
                )
            })
        {
            return Err(AdapterError::InvalidCompletion);
        }
        let source_tree = if let Request::NarrowFile { file, .. } = request {
            match self.handles.rows.get(file) {
                Some(Grant::File { directory, .. }) => {
                    self.fake_directories.get(directory).copied()
                }
                _ => None,
            }
        } else {
            None
        };
        let target_tree = self.fake_tree(request);
        let result = if let Some(tree_id) = target_tree {
            let mut tree = self.fake_trees.remove(&tree_id).expect("fake tree");
            let mut identities = self
                .fake_files
                .remove(&tree_id)
                .expect("fake file identities");
            let mut roots = self.fake_roots.remove(&tree_id).unwrap_or_default();
            let mut detached = self.fake_detached.remove(&tree_id).unwrap_or_default();
            let result = self.execute_storage(
                &mut crate::io_sim::LinkedTree::new(
                    &mut tree,
                    &mut roots,
                    &mut identities,
                    &mut detached,
                ),
                request,
                fault,
            );
            self.fake_trees.insert(tree_id, tree);
            self.fake_files.insert(tree_id, identities);
            self.fake_roots.insert(tree_id, roots);
            self.fake_detached.insert(tree_id, detached);
            if matches!(
                request,
                Request::Directory { .. } | Request::NarrowDir { .. } | Request::ReadOnlyDir { .. }
            ) && let Ok(Completion::Handle(Ok(id))) = &result
            {
                self.fake_directories.insert(*id, tree_id);
            }
            if matches!(request, Request::Watch { .. })
                && let Ok(Completion::Handle(Ok(id))) = &result
            {
                self.fake_watches.insert(*id, (tree_id, Default::default()));
            }
            result
        } else {
            self.execute_storage(storage, request, fault)
        };
        if source_tree != target_tree
            && let Request::NarrowFile { file, .. } = request
            && result.is_ok()
        {
            if let Some(tree) = source_tree {
                self.fake_files
                    .get_mut(&tree)
                    .expect("source file identities")
                    .close(*file);
            } else {
                storage.transfer_file(*file, None);
            }
        }
        result
    }
    fn monotonic_floor(&mut self, clock: u64) -> &mut Option<u64> {
        if self.test_clocks.contains(&clock) {
            &mut self.test_last_monotonic
        } else {
            &mut self.last_monotonic
        }
    }
    /// Validate provider inputs before publishing any replay or epoch state.
    pub(crate) fn sample(
        &mut self,
        request: &Request,
        input: Completion,
    ) -> Result<Completion, AdapterError> {
        if let Request::ReadStdin { stdin, .. } = request {
            self.expect(*stdin, Grant::Stdin)?;
        }
        match (request, &input) {
            (Request::SignalWait { signals, kinds }, Completion::Signal(value)) => {
                self.expect(*signals, Grant::Signals)?;
                let check = self.signal_check(*kinds);
                let valid = match value {
                    Ok(kind) => check.is_none() && kinds & kind.bit() != 0,
                    Err(SignalError::Canceled(cause)) => {
                        check.is_none() && *cause <= crate::tasks::CANCEL_DEADLINE
                    }
                    Err(error) => check.as_ref() == Some(error),
                };
                if !valid {
                    return Err(AdapterError::InvalidCompletion);
                }
            }
            (Request::ReadStdin { max, .. }, Completion::Input(value)) => {
                let valid = match value {
                    Err(InputError::InvalidInput) => *max == 0,
                    Ok(bytes) => *max > 0 && bytes.len() as u64 <= *max,
                    Err(InputError::Fault(_)) => *max > 0,
                    Err(InputError::Canceled(cause)) => {
                        *max > 0 && *cause <= crate::tasks::CANCEL_DEADLINE
                    }
                    Err(InputError::InvalidUtf8) => false,
                };
                if !valid {
                    return Err(AdapterError::InvalidCompletion);
                }
            }
            (Request::Monotonic { clock }, Completion::Monotonic(value)) => {
                self.expect(*clock, Grant::MonotonicClock)?;
                if let Ok(ticks) = value {
                    let last = self.monotonic_floor(*clock);
                    if last.is_some_and(|last| *ticks < last) {
                        return Err(AdapterError::InvalidCompletion);
                    }
                    *last = Some(*ticks);
                }
            }
            (Request::Wall { clock }, Completion::Wall(_)) => {
                self.expect(*clock, Grant::WallClock)?
            }
            (Request::RandomBytes { random, count }, Completion::Random(value)) => {
                self.expect(*random, Grant::Random)?;
                if *count == 0 || value.as_ref().is_ok_and(|b| b.len() as u64 != *count) {
                    return Err(AdapterError::InvalidCompletion);
                }
            }
            (Request::TelemetryBegin { .. }, Completion::Span { .. }) => {
                return self.telemetry_sample(request, input);
            }
            // A completed wait proves the clock reached its deadline: a later
            // recorded sample below it is not a monotonic reading.
            (Request::SleepUntil { clock, deadline }, Completion::Wait(value)) => {
                self.expect(*clock, Grant::MonotonicClock)?;
                if value.is_ok() {
                    let last = self.monotonic_floor(*clock);
                    *last = Some(last.unwrap_or(0).max(*deadline));
                }
            }
            _ => return Err(AdapterError::InvalidCompletion),
        }
        Ok(input)
    }
    fn execute_storage(
        &mut self,
        storage: &mut impl Storage,
        request: &Request,
        fault: Option<&Fault>,
    ) -> Result<Completion, AdapterError> {
        use Completion as C;
        use Request as R;
        if let Some(error) = self.preflight(request)? {
            return Ok(error);
        }
        let error = fault.map(|f| f.error.clone());
        let partial = fault.map_or(0, |f| f.partial);
        let prefix = |bytes: &[u8]| {
            let n = match fault {
                Some(Fault {
                    error: IoError::Canceled(_),
                    ..
                }) => 0,
                Some(f) => f.partial.min(bytes.len()),
                None => bytes.len(),
            };
            bytes[..n].to_vec()
        };
        match request {
            R::Ready { .. }
            | R::Schedule { .. }
            | R::CpuControl { .. }
            | R::ProcessTime { .. }
            | R::ProcessSettle { .. } => Err(AdapterError::InvalidCompletion),
            R::MonotonicClock { io } | R::WallClock { io } => {
                self.expect(*io, Grant::Io)?;
                let monotonic = matches!(request, R::MonotonicClock { .. });
                if if monotonic {
                    self.monotonic_granted
                } else {
                    self.wall_granted
                } {
                    Ok(C::ClockGrant(Ok(self.handles.insert(if monotonic {
                        Grant::MonotonicClock
                    } else {
                        Grant::WallClock
                    }))))
                } else {
                    Ok(C::ClockGrant(Err(())))
                }
            }
            R::Monotonic { .. } | R::Wall { .. } | R::RandomBytes { .. } | R::SleepUntil { .. } => {
                Err(AdapterError::InvalidCompletion)
            }
            R::Random { io } | R::Tasks { io } | R::Net { io } => {
                self.expect(*io, Grant::Io)?;
                let (name, grant) = match request {
                    R::Random { .. } => ("random", Grant::Random),
                    R::Tasks { .. } => ("tasks", Grant::Tasks),
                    R::Net { .. } => (
                        "net",
                        Grant::Net {
                            rules: self.net_rules.clone(),
                            world: self.net_world,
                        },
                    ),
                    _ => unreachable!(),
                };
                if !self.projections.contains(name) {
                    return Ok(C::Handle(Err(IoError::NotGranted)));
                }
                Ok(C::Handle(Ok(self.handles.insert(grant))))
            }
            R::TestCheck { test, condition } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                Ok(C::Checked(*condition))
            }
            R::TestCheckCase {
                test,
                condition,
                case,
            } => {
                let Some(Grant::Test { seed }) = self.handles.rows.get(test) else {
                    return Err(AdapterError::InvalidHandle(*test));
                };
                if !condition {
                    self.stdout.extend_from_slice(
                        format!("test failure: seed={seed} case={case}\n").as_bytes(),
                    );
                }
                Ok(C::Checked(*condition))
            }
            R::TestFail { test } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                Ok(C::Checked(false))
            }
            R::TestClock { test } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                let clock = self.handles.insert(Grant::MonotonicClock);
                self.test_clocks.insert(clock);
                Ok(C::Handle(Ok(clock)))
            }
            R::TestSeed { test } => {
                let Some(Grant::Test { seed }) = self.handles.rows.get(test) else {
                    return Err(AdapterError::InvalidHandle(*test));
                };
                Ok(C::Size(Ok(*seed)))
            }
            R::TestTasks { test } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                Ok(C::Handle(Ok(self.handles.insert(Grant::Tasks))))
            }
            R::TestLog { test, text } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                self.stdout
                    .extend_from_slice(&hirsel_kernels::report::log(text));
                Ok(C::Checked(true))
            }
            R::TestSnapshot { test, name, text } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                if !hirsel_kernels::report::snapshot_name(name) {
                    return Ok(C::Checked(false));
                }
                self.stdout
                    .extend_from_slice(&hirsel_kernels::report::snapshot(name, text));
                Ok(C::Checked(true))
            }
            R::TestDirEvent {
                test,
                directory,
                event,
            } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                let root = self.directory(*directory)?;
                // An event reaches the watchers of the Dir's test tree that see it; one
                // whose path leaves the Dir reaches none, nor does one on a host Dir.
                if let Some(tree) = self.fake_directories.get(directory).copied()
                    && let Some(event) =
                        crate::watch::map_paths(event, |p| confined_relative(&root, p))
                {
                    for (id, (watched_tree, queue)) in &mut self.fake_watches {
                        let Some(Grant::Watcher {
                            root, recursive, ..
                        }) = self.handles.rows.get(id)
                        else {
                            continue;
                        };
                        if *watched_tree == tree
                            && let Some(seen) = crate::watch::seen(root, *recursive, &event)
                        {
                            queue.push(seen);
                        }
                    }
                }
                Ok(C::Unit(Ok(())))
            }
            R::TestDir { test, entries } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                let mut files = BTreeMap::new();
                for (path, bytes) in entries {
                    let path =
                        confined_relative("", path).ok_or(AdapterError::InvalidCompletion)?;
                    files.insert(path, bytes.clone());
                }
                let tree = Tree::new(files).map_err(|_| AdapterError::InvalidCompletion)?;
                let id = self.handles.insert(Grant::Directory {
                    root: String::new(),
                    access: Access::ReadWrite,
                });
                let mut files = crate::io_sim::FileIdentities::new(&tree);
                files.durable_start(&tree);
                self.fake_files.insert(id, files);
                self.fake_trees.insert(id, tree);
                self.fake_directories.insert(id, id);
                Ok(C::Handle(Ok(id)))
            }
            R::Stdin { io } | R::Stderr { io } => {
                self.expect(*io, Grant::Io)?;
                let (projection, grant) = if matches!(request, R::Stdin { .. }) {
                    ("stdin", Grant::Stdin)
                } else {
                    ("stderr", Grant::Stderr)
                };
                if !self.projections.contains(projection) {
                    return Ok(C::Handle(Err(IoError::NotGranted)));
                }
                Ok(C::Handle(Ok(self.handles.insert(grant))))
            }
            R::ReadStdin { .. } | R::SignalWait { .. } => Err(AdapterError::InvalidCompletion),
            R::Signals { io } => {
                self.expect(*io, Grant::Io)?;
                if !self.projections.contains("signals") {
                    return Ok(C::Handle(Err(IoError::NotGranted)));
                }
                Ok(C::Handle(Ok(self.handles.insert(Grant::Signals))))
            }
            R::EnvGet { env, name } => Ok(C::Env(if self.env_granted(*env, name)? {
                self.env.get(name).cloned().ok_or(EnvError::NotGranted)
            } else {
                Err(EnvError::NotGranted)
            })),
            R::EnvNarrow { env, names } => {
                self.env_granted(*env, "")?;
                for name in names {
                    if !self.env_granted(*env, name)? {
                        return Ok(C::Handle(Err(IoError::NotGranted)));
                    }
                }
                Ok(C::Handle(Ok(self.handles.insert(Grant::Env {
                    names: names.iter().cloned().collect(),
                }))))
            }
            R::Stdout { io } => {
                self.expect(*io, Grant::Io)?;
                if !self.projections.contains("stdout") {
                    return Ok(C::Handle(Err(IoError::NotGranted)));
                }
                Ok(C::Handle(Ok(self.handles.insert(Grant::Stdout))))
            }
            R::Directory { .. } | R::NarrowDir { .. } | R::ReadOnlyDir { .. } => {
                // A read-only Dir reopens its parent's pinned directory, never the path.
                let (parent, root, path, access) = match request {
                    R::Directory { io, path } => {
                        self.expect(*io, Grant::Io)?;
                        if !self.projections.contains("dir") {
                            return Ok(C::Handle(Err(IoError::NotGranted)));
                        }
                        let Some((selector, local, access)) = self.mount(path) else {
                            return Ok(C::Handle(Err(IoError::NotGranted)));
                        };
                        (0, selector.to_owned(), local, access)
                    }
                    R::NarrowDir { directory, path } => (
                        *directory,
                        self.directory(*directory)?,
                        path.as_str(),
                        self.access(*directory)?,
                    ),
                    R::ReadOnlyDir { directory } => {
                        (*directory, self.directory(*directory)?, "", Access::Read)
                    }
                    _ => unreachable!(),
                };
                storage.activate(parent, &root);
                let result = if let Some(e) = error {
                    Err(e)
                } else {
                    confined_relative(&root, path)
                        .ok_or(IoError::Fault(1))
                        .and_then(|root| {
                            storage.directory(&root, self.handles.next())?;
                            Ok(self.handles.insert(Grant::Directory { root, access }))
                        })
                };
                Ok(C::Handle(result))
            }
            R::PrintStderr { stderr, bytes } => {
                self.expect(*stderr, Grant::Stderr)?;
                self.stderr.extend(prefix(bytes));
                Ok(C::Unit(error.map_or(Ok(()), Err)))
            }
            R::Print { stdout, bytes } => {
                self.expect(*stdout, Grant::Stdout)?;
                self.stdout.extend(prefix(bytes));
                Ok(C::Unit(error.map_or(Ok(()), Err)))
            }
            R::Close { handle } => {
                let grant = self
                    .handles
                    .rows
                    .get(handle)
                    .cloned()
                    .ok_or(AdapterError::InvalidHandle(*handle))?;
                if let Grant::AtomicFile {
                    temporary,
                    directory,
                    ..
                } = &grant
                {
                    let root = self.directory(*directory)?;
                    storage.activate(*directory, &root);
                    let _ = storage.remove(temporary);
                }
                if matches!(grant, Grant::File { .. } | Grant::AtomicFile { .. }) {
                    self.handles.rows.remove(handle);
                    storage.transfer_file(*handle, None);
                } else if matches!(grant, Grant::Mapped { .. }) {
                    self.handles.rows.remove(handle);
                    storage.unmap(*handle);
                } else if matches!(grant, Grant::Watcher { .. }) {
                    self.handles.rows.remove(handle);
                    self.fake_watches.remove(handle);
                    storage.unwatch(*handle);
                } else {
                    return Err(AdapterError::InvalidHandle(*handle));
                }
                Ok(C::Unit(Ok(())))
            }
            R::NarrowFile { directory, file } => {
                let root = self.directory(*directory)?;
                let Some(Grant::File {
                    path,
                    directory: owner,
                    position,
                    writable,
                }) = self.handles.rows.get(file).cloned()
                else {
                    return Err(AdapterError::InvalidHandle(*file));
                };
                self.handles.rows.remove(file);
                storage.activate(*directory, &root);
                let fits = self.fake_directories.get(&owner)
                    == self.fake_directories.get(directory)
                    && error.is_none()
                    && (root.is_empty() || path.starts_with(&format!("{root}/")))
                    && storage.file_fits(*file, &path);
                let new = fits.then(|| {
                    self.handles.insert(Grant::File {
                        path,
                        directory: *directory,
                        position,
                        writable,
                    })
                });
                storage.transfer_file(*file, new);
                Ok(C::OptionalFile(new))
            }
            R::FileRead {
                directory,
                file,
                max,
            } => {
                self.directory(*directory)?;
                let Some(Grant::File {
                    directory: owner,
                    position,
                    ..
                }) = self.handles.rows.get(file).cloned()
                else {
                    return Err(AdapterError::InvalidHandle(*file));
                };
                if owner != *directory {
                    return Err(AdapterError::InvalidHandle(*file));
                }
                let result = if let Some(e) = error {
                    Err(e)
                } else {
                    storage.read_file(*file, position, *max)
                };
                if let Ok(bytes) = &result {
                    if bytes.len() as u64 > *max {
                        return Err(AdapterError::InvalidCompletion);
                    }
                    let Some(Grant::File { position, .. }) = self.handles.rows.get_mut(file) else {
                        unreachable!()
                    };
                    *position = position
                        .checked_add(bytes.len() as u64)
                        .ok_or(AdapterError::InvalidCompletion)?;
                }
                Ok(C::Bytes(result))
            }
            R::FileSeek {
                directory,
                file,
                offset,
            } => {
                self.directory(*directory)?;
                let Some(Grant::File {
                    directory: owner,
                    position,
                    ..
                }) = self.handles.rows.get_mut(file)
                else {
                    return Err(AdapterError::InvalidHandle(*file));
                };
                if owner != directory {
                    return Err(AdapterError::InvalidHandle(*file));
                }
                let result = if let Some(e) = error {
                    Err(e)
                } else {
                    *position = *offset;
                    Ok(())
                };
                Ok(C::Unit(result))
            }
            R::FileWrite { file, bytes, .. } | R::FileAppend { file, bytes, .. } => {
                let all = matches!(request, R::FileAppend { .. });
                let result = if matches!(error, Some(IoError::Canceled(_)))
                    || (error.is_some() && prefix(bytes).is_empty())
                {
                    Err(error.unwrap())
                } else {
                    storage
                        .write_file(*file, &prefix(bytes), all)
                        .and_then(|n| error.map_or(Ok(n), Err))
                };
                Ok(if all {
                    C::Unit(result.map(|_| ()))
                } else {
                    C::Size(result)
                })
            }
            R::Sync { directory, file } => {
                let root = self.directory(*directory)?;
                let Some(Grant::File {
                    path,
                    directory: owner,
                    ..
                }) = self.handles.rows.get(file).cloned()
                else {
                    return Err(AdapterError::InvalidHandle(*file));
                };
                if owner != *directory {
                    return Err(AdapterError::InvalidHandle(*file));
                }
                storage.activate(*directory, &root);
                Ok(C::Unit(match error {
                    Some(e) => Err(e),
                    None if storage.stale(*file) => Err(IoError::Fault(1)),
                    None => storage.sync_file(*file, &path),
                }))
            }
            R::TestCrash {
                test,
                directory,
                seed,
            } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                self.directory(*directory)?;
                let Some(&tree) = self.fake_directories.get(directory) else {
                    return Ok(C::Checked(false));
                };
                let stale: Vec<u64> = self
                    .handles
                    .rows
                    .sorted()
                    .into_iter()
                    .filter_map(|(id, grant)| match grant {
                        Grant::File { directory, .. }
                        | Grant::AtomicFile { directory, .. }
                        | Grant::Mapped { directory, .. }
                            if self.fake_directories.get(directory) == Some(&tree) =>
                        {
                            Some(id)
                        }
                        _ => None,
                    })
                    .collect();
                let files = self
                    .fake_files
                    .get_mut(&tree)
                    .expect("fake file identities");
                files.crash(
                    self.fake_trees.get_mut(&tree).expect("fake tree"),
                    *seed,
                    stale.into_iter(),
                );
                Ok(C::Checked(true))
            }
            R::WriteAll {
                directory, file, ..
            }
            | R::Commit { directory, file } => {
                let root = self.directory(*directory)?;
                storage.activate(*directory, &root);
                let Some(Grant::AtomicFile {
                    temporary,
                    destination,
                    directory: owner,
                }) = self.handles.rows.get(file).cloned()
                else {
                    return Err(AdapterError::InvalidHandle(*file));
                };
                if owner != *directory {
                    return Err(AdapterError::InvalidHandle(*file));
                }
                let stale = storage.stale(*file);
                let result = if matches!(request, R::Commit { .. }) {
                    self.handles.rows.remove(file);
                    if stale {
                        Err(IoError::Fault(1))
                    } else if let Some(e) = error {
                        let _ = storage.remove(&temporary);
                        Err(e)
                    } else {
                        let result = storage.rename(&temporary, &destination);
                        if result.is_err() {
                            let _ = storage.remove(&temporary);
                        }
                        result
                    }
                } else {
                    let bytes = if let R::WriteAll { bytes, .. } = request {
                        bytes
                    } else {
                        unreachable!()
                    };
                    let data = prefix(bytes);
                    if matches!(error, Some(IoError::Canceled(_))) {
                        Err(error.unwrap())
                    } else if stale {
                        Err(IoError::Fault(1))
                    } else if error.is_some() && data.is_empty() {
                        Err(error.unwrap())
                    } else {
                        storage
                            .write(&temporary, &data, true, false)
                            .and_then(|()| error.map_or(Ok(()), Err))
                    }
                };
                Ok(C::Unit(result))
            }
            R::Rename {
                directory,
                from,
                to,
            } => {
                let root = self.directory(*directory)?;
                storage.activate(*directory, &root);
                let result = if let Some(e) = error {
                    Err(e)
                } else {
                    confined_relative(&root, from)
                        .zip(confined_relative(&root, to))
                        .ok_or(IoError::Fault(1))
                        .and_then(|(from, to)| {
                            if from == root || to == root {
                                return Err(IoError::Fault(1));
                            }
                            storage.move_entry(&from, &to)
                        })
                };
                Ok(C::Unit(result))
            }
            R::WatchNext { directory, watcher } => {
                let root = self.directory(*directory)?;
                storage.activate(*directory, &root);
                if !matches!(self.handles.rows.get(watcher), Some(Grant::Watcher { directory: owner, .. }) if owner == directory)
                {
                    return Err(AdapterError::InvalidHandle(*watcher));
                }
                if let Some(e) = error {
                    return Ok(C::Watch(Err(e)));
                }
                // A test tree's watcher reads its injected queue; a host one its provider.
                let event = match self.fake_watches.get_mut(watcher) {
                    Some((_, queue)) => Ok(queue
                        .pop()
                        .map(|event| crate::watch::relative_to(&root, event))),
                    None => storage.watch_next(*watcher, &root),
                };
                match event {
                    Ok(Some(event)) => Ok(C::Watch(Ok(event))),
                    Ok(None) => Err(AdapterError::Blocked),
                    Err(e) => Ok(C::Watch(Err(e))),
                }
            }
            R::MapView { directory, mapped } | R::MapCheck { directory, mapped } => {
                let capture = self.capture_views;
                let root = self.directory(*directory)?;
                storage.activate(*directory, &root);
                let Some(Grant::Mapped {
                    directory: owner,
                    length,
                    truncated,
                }) = self.handles.rows.get_mut(mapped)
                else {
                    return Err(AdapterError::InvalidHandle(*mapped));
                };
                if owner != directory {
                    return Err(AdapterError::InvalidHandle(*mapped));
                }
                // A mapping from before its test tree's crash reads as zeros and fails.
                let stale = storage.stale(*mapped);
                if let R::MapView { .. } = request {
                    if error.is_some() {
                        return Err(AdapterError::InvalidCompletion);
                    }
                    if stale {
                        return Ok(C::View(MapView::of(vec![0; *length as usize])));
                    }
                    return Ok(C::View(storage.map_view(*mapped, *length, capture)));
                }
                if stale {
                    return Ok(C::Unit(Err(IoError::Fault(1))));
                }
                // A recorded truncation is a host input; a check that found none is derived.
                *truncated = match error {
                    Some(IoError::Truncated) => true,
                    Some(_) => return Err(AdapterError::InvalidCompletion),
                    None => *truncated || storage.map_truncated(*mapped, *length),
                };
                Ok(C::Unit(if *truncated {
                    Err(IoError::Truncated)
                } else {
                    Ok(())
                }))
            }
            _ => {
                let (directory, path) = match request {
                    R::Open { directory, path }
                    | R::OpenAppend { directory, path }
                    | R::Create { directory, path }
                    | R::Size { directory, path }
                    | R::ReadDirPage {
                        directory, path, ..
                    }
                    | R::Metadata { directory, path }
                    | R::Mkdir { directory, path }
                    | R::RemoveDir { directory, path }
                    | R::RemoveAll { directory, path }
                    | R::SetPermissions {
                        directory, path, ..
                    }
                    | R::Read { directory, path }
                    | R::Write {
                        directory, path, ..
                    }
                    | R::Remove { directory, path }
                    | R::CreateAtomic { directory, path }
                    | R::PutNew {
                        directory, path, ..
                    }
                    | R::Map { directory, path }
                    | R::Watch {
                        directory, path, ..
                    } => (*directory, path),
                    _ => unreachable!(),
                };
                let root = self.directory(directory)?;
                storage.activate(directory, &root);
                let path = confined_relative(&root, path).ok_or(IoError::Fault(1));
                match request {
                    R::Open { .. } | R::Create { .. } | R::OpenAppend { .. } => {
                        Ok(C::Handle(if let Some(e) = error {
                            Err(e)
                        } else {
                            path.and_then(|path| {
                                let writable = !matches!(request, R::Open { .. });
                                if matches!(request, R::OpenAppend { .. }) {
                                    storage.open_append_file(&path, self.handles.next())?;
                                } else if writable {
                                    storage.create_file(&path, self.handles.next())?;
                                } else {
                                    storage.open_file(&path, self.handles.next())?;
                                }
                                Ok(self.handles.insert(Grant::File {
                                    path,
                                    directory,
                                    position: 0,
                                    writable,
                                }))
                            })
                        }))
                    }
                    R::Watch { recursive, .. } => Ok(C::Handle(if let Some(e) = error {
                        Err(e)
                    } else {
                        path.and_then(|path| {
                            storage.watch(&path, self.handles.next(), *recursive)?;
                            Ok(self.handles.insert(Grant::Watcher {
                                directory,
                                root: path,
                                recursive: *recursive,
                            }))
                        })
                    })),
                    R::Map { .. } => Ok(C::Handle(if let Some(e) = error {
                        Err(e)
                    } else {
                        path.and_then(|path| {
                            let length = storage.map_file(&path, self.handles.next())?;
                            Ok(self.handles.insert(Grant::Mapped {
                                directory,
                                length,
                                truncated: false,
                            }))
                        })
                    })),
                    R::ReadDirPage {
                        after, recursive, ..
                    } => Ok(C::Entries(if let Some(e) = error {
                        Err(e)
                    } else {
                        path.and_then(|p| {
                            let prefix = if root.is_empty() {
                                p.as_str()
                            } else if p == root {
                                ""
                            } else {
                                p.strip_prefix(&format!("{root}/")).expect("relative path")
                            };
                            storage.read_dir_page(&p, after, *recursive).map(|entries| {
                                entries
                                    .into_iter()
                                    .map(|mut entry| {
                                        if !prefix.is_empty() {
                                            entry.path = format!("{prefix}/{}", entry.path);
                                        }
                                        entry
                                    })
                                    .collect()
                            })
                        })
                    })),
                    R::Metadata { .. } => Ok(C::Metadata(if let Some(e) = error {
                        Err(e)
                    } else {
                        path.and_then(|p| storage.metadata(&p))
                    })),
                    R::SetPermissions { permissions, .. } => Ok(C::Unit(if let Some(e) = error {
                        Err(e)
                    } else if *permissions > 0o777 {
                        Err(IoError::Fault(1))
                    } else {
                        path.and_then(|p| storage.set_permissions(&p, *permissions))
                    })),
                    R::RemoveAll { .. } => Ok(C::Unit(path.and_then(|p| {
                        if p == root {
                            return Err(IoError::Fault(1));
                        }
                        match error {
                            Some(e @ IoError::Canceled(_)) => Err(e),
                            Some(e) if partial == 0 => Err(e),
                            Some(e) => storage.remove_all(&p, Some(partial)).and_then(|()| Err(e)),
                            None => storage.remove_all(&p, None),
                        }
                    }))),
                    R::Mkdir { .. } | R::RemoveDir { .. } => Ok(C::Unit(if let Some(e) = error {
                        Err(e)
                    } else {
                        path.and_then(|p| {
                            if p == root {
                                return Err(IoError::Fault(1));
                            }
                            if matches!(request, R::Mkdir { .. }) {
                                storage.mkdir(&p)
                            } else {
                                storage.remove_dir(&p)
                            }
                        })
                    })),
                    R::Size { .. } => Ok(C::Size(if let Some(e) = error {
                        Err(e)
                    } else {
                        path.and_then(|p| storage.size(&p))
                    })),
                    R::Read { .. } => Ok(C::Bytes(if let Some(e) = error {
                        Err(e)
                    } else {
                        path.and_then(|p| storage.read(&p))
                    })),
                    R::Remove { .. } => Ok(C::Unit(if let Some(e) = error {
                        Err(e)
                    } else {
                        path.and_then(|p| storage.remove(&p))
                    })),
                    R::PutNew { bytes, durable, .. } => Ok(C::Unit(if let Some(e) = error {
                        Err(e)
                    } else {
                        path.and_then(|p| storage.put_new(&p, bytes, *durable))
                    })),
                    R::Write { bytes, .. } => {
                        let result = if matches!(error, Some(IoError::Canceled(_)))
                            || (error.is_some() && prefix(bytes).is_empty())
                        {
                            Err(error.unwrap())
                        } else {
                            path.and_then(|p| storage.write(&p, &prefix(bytes), false, false))
                                .and_then(|()| error.map_or(Ok(()), Err))
                        };
                        Ok(C::Unit(result))
                    }
                    R::CreateAtomic { .. } => {
                        let result = if let Some(e) = error {
                            Err(e)
                        } else {
                            path.and_then(|destination| {
                                storage.destination(&destination)?;
                                let mut serial = self.handles.next();
                                let temporary = loop {
                                    let candidate = format!("{destination}.hirsel-tmp-{serial}");
                                    match storage.write(&candidate, &[], false, true) {
                                        Ok(()) => break candidate,
                                        Err(IoError::AlreadyExists) => {
                                            serial = serial
                                                .checked_add(1)
                                                .expect("temporary names exhausted")
                                        }
                                        Err(e) => return Err(e),
                                    }
                                };
                                Ok(self.handles.insert(Grant::AtomicFile {
                                    temporary,
                                    destination,
                                    directory,
                                }))
                            })
                        };
                        Ok(C::Handle(result))
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
}
/// Explicit API: wrappers carry concrete operands and results, not boxed values.
/// Storage borrowed by a native network call. It remains on the issuing fiber's
/// stack until that call completes, including across readiness waits.
pub enum NetBuffer<'a> {
    Read(&'a mut [std::mem::MaybeUninit<u8>]),
    Write(&'a [u8]),
    WriteAll(&'a [u8]),
}

impl NetBuffer<'_> {
    pub(crate) fn request(&self, net: u64, stream: u64) -> Request {
        match self {
            Self::Read(bytes) => Request::NetRead {
                net,
                stream,
                max: bytes.len() as u64,
            },
            Self::Write(_) => Request::NetWrite {
                net,
                stream,
                bytes: Vec::new(),
            },
            Self::WriteAll(_) => Request::NetWriteAll {
                net,
                stream,
                bytes: Vec::new(),
            },
        }
    }
}
/// Live network operations return a count (zero for write_all), never a tape value.
pub type NetResult = Result<u64, crate::net::NetError>;

pub trait Adapter {
    /// Check argument validity and authority without an effect or tape event.
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError>;
    /// The TLS engine, which every TLS entry hands over before its material and its request:
    /// only programs that reach TLS link it (`net_tls`).
    fn tls(&mut self, _engine: &'static dyn crate::net_tls::TlsEngine) {}
    /// The host network provider's constructor, which every entry that opens host network
    /// state (a connect, resolve, listen or bind) hands over before its request: only
    /// programs that reach the network link it (`net_host::provider`).
    fn net(&mut self, _host: crate::net_host::Provider) {}
    /// The host process provider's constructor, which every entry that starts a process
    /// hands over before its request (`process::host`).
    fn process(&mut self, _host: fn() -> Box<dyn crate::process::ProcessHost>) {}
    /// Supply the next handshake's PEM material without adding it to the tape.
    /// Replay and scripted worlds use only the digest in TlsConnect.
    fn tls_roots(&mut self, _stream: u64, _pem: &[u8]) {}
    /// Supply the next `tls.connect_as` identity material without adding it to the tape.
    /// Every world loads it to check validity; the request records only the chain digest.
    fn tls_identity(&mut self, _stream: u64, _chain: &[u8], _key: &[u8]) {}
    /// Supply the next server handshake's `TlsServer` material without adding it to the
    /// tape. Every world loads it to check validity; the request records only digests.
    fn tls_server(
        &mut self,
        _stream: u64,
        _identities: &[(&[u8], &[u8])],
        _clients: crate::net::ClientAuth<&[u8]>,
        _alpn: &[String],
    ) {
    }
    /// Confirmed replay output, used only when reporting a tool refusal.
    fn replay_stdout(&self) -> Vec<u8> {
        vec![]
    }
    fn replay_stderr(&self) -> Vec<u8> {
        vec![]
    }
    /// A synchronous output borrows its bytes until completion. Adapters that retain
    /// requests use the ordinary owned protocol; a live sink can consume the view.
    fn print(
        &mut self,
        handle: u64,
        stderr: bool,
        bytes: &[u8],
    ) -> Result<Completion, AdapterError> {
        self.call(if stderr {
            Request::PrintStderr {
                stderr: handle,
                bytes: bytes.to_vec(),
            }
        } else {
            Request::Print {
                stdout: handle,
                bytes: bytes.to_vec(),
            }
        })
    }
    /// Optional declared issue footprint. None keeps legacy home coupling;
    /// Some requires complete initialized-result identity observations.
    fn keys(&self, _request: &Request) -> Option<crate::io_keys::KeySet> {
        None
    }
    /// Auxiliary domains, named by a prior handle's resource and a stable label.
    /// Aliases retain that resource's domain; raw key numbers are never names.
    fn domains(&self, _request: &Request) -> Vec<(crate::io_keys::Key, u64, String)> {
        Vec::new()
    }
    /// A live resource identity, matching the ConcurrentAdapter graph.
    fn identity(&self, _handle: u64) -> Option<(crate::io_keys::Key, crate::io_keys::Kind)> {
        None
    }
    /// A synchronous print and its exact externally written prefix, when the
    /// adapter can observe it. Unknown counts remain unknown even on success.
    fn print_observed(
        &mut self,
        handle: u64,
        stderr: bool,
        bytes: &[u8],
    ) -> (Result<Completion, AdapterError>, Option<u64>) {
        (self.print(handle, stderr, bytes), None)
    }
    /// Live adapters may execute a borrowed network operation. None declines before
    /// any effect; record/replay and scripted adapters use ordinary owned requests.
    fn net_io(
        &mut self,
        _token: u64,
        _net: u64,
        _stream: u64,
        _inline: bool,
        _buffer: &mut NetBuffer<'_>,
    ) -> Option<Result<NetResult, AdapterError>> {
        None
    }

    fn call(&mut self, request: Request) -> Result<Completion, AdapterError>;

    /// A facade can combine request metadata with its effect in one home
    /// bridge. The method order remains wants_issuer, issuer, enter, call.
    fn batches_issue(&self) -> bool {
        false
    }
    fn issue_batch(
        &mut self,
        request: Request,
        token: u64,
        task: &[u32],
    ) -> Result<Completion, AdapterError> {
        if self.wants_issuer() {
            self.issuer(task);
        }
        self.enter(token);
        self.call(request)
    }

    /// A scheduler-owned scope deadline attempt. It uses the same host/tape
    /// operation as SleepUntil, with deadline evidence rather than an issuing
    /// task's host-request evidence. Ordinary clock.sleep_until still uses call.
    fn scope_timer(&mut self, request: Request) -> Result<Completion, AdapterError> {
        self.call(request)
    }
    /// Whether the adapter records who issues each request (a durable journal).
    fn wants_issuer(&self) -> bool {
        false
    }
    /// The spawn path of the task issuing the next request.
    fn issuer(&mut self, _task: &[u32]) {}
    /// Cooperative waits (`crate::host_waits`, DET-1). Select the readiness token of the
    /// request about to be issued; a `Blocked` answer parks it under that token.
    fn enter(&mut self, _token: u64) {}
    /// The request under `token` completed or was canceled.
    fn finish(&mut self, _token: u64) {}
    /// The task of the request that blocked under `token` parked (an armed scope deadline
    /// parks at once): it waits from the next scheduling point on.
    fn parked(&mut self, _token: u64) {}
    /// Whether the request parked under `token` has committed no effect yet.
    fn cancellable(&self, _token: u64) -> bool {
        true
    }
    /// None probes replay without starting workers. A live adapter returns None;
    /// a replay consumes the bound receipt. Some records the live outcome once.
    fn cpu_control(
        &mut self,
        _width: u32,
        _action: u32,
        _count: u64,
        value: Option<u32>,
    ) -> Result<Option<u32>, AdapterError> {
        Ok(value)
    }
    /// A seeded scheduler's choice among runnable candidates; replay supplies the recorded choice.
    fn schedule(&mut self, _count: u64, choice: u64) -> Result<u64, AdapterError> {
        Ok(choice)
    }
    /// Whether parked `request` waits only on state recording and replay derive alike (a
    /// `test.dir` tree's watcher): every scheduling point re-issues it, recording nothing.
    fn injected_wait(&self, _request: &Request) -> bool {
        false
    }
    /// Span `span`'s context when Telemetry `telemetry` began it: state its begin's
    /// completion derived, read with no request.
    fn telemetry_context(&self, _telemetry: u64, _span: u64) -> Option<Vec<u8>> {
        None
    }
    /// A scheduling point with a parked request: the requests to re-issue now. A
    /// cooperative host answers the tokens whose interest is ready, in token order; the
    /// default (a scripted world) re-issues every parked request.
    /// Finish the timed provider batch after scope expiry and before publishing readiness.
    /// Canceled taking waits detach their child gates before any due script step.
    fn isolated_clock(&self) -> bool {
        false
    }
    fn process_site(&mut self, _child: u64, _site: u64, _operation_site: u64, _path: &[u32]) {}
    fn process_sources(&self) -> bool {
        false
    }
    fn process_batch(&mut self, _canceled: &[u64]) -> Result<Ready, AdapterError> {
        Ok(Ready::Tokens(Vec::new()))
    }
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        Ok(Ready::Every)
    }
    /// A conservative lower bound on the next ready token, only when all
    /// readiness sources are real timers and no asynchronous source can change
    /// them. Custom, shared, scripted and replay adapters must decline.
    fn ready_deadline(&self) -> Option<std::time::Instant> {
        None
    }
    /// Account for exactly `points` timer-only queries answered empty by an
    /// Ordered poll owner. Called on home before any subsequent adapter method.
    /// Only an adapter publishing ready_deadline may receive nonzero points.
    fn advance_ready_points(&mut self, points: u64) {
        assert_eq!(points, 0, "adapter published no timer readiness bound");
    }
    /// Whether a parked request of this run is ready now, without taking it and without
    /// waiting: an execution's idle runs ask before any of them waits (RUN-12).
    fn has_ready(&mut self) -> Result<bool, AdapterError> {
        Ok(false)
    }
    /// The cooperative readiness a runner call parks on and its callee runs share (RUN-12),
    /// made at the first call of a live adapter that has none; None in replay, which starts
    /// no callee.
    fn runner_readiness(&mut self) -> Option<crate::readiness::Readiness> {
        None
    }
    /// The outcome of the runner call parked under `token` arrived (`RealIo::deliver`).
    fn deliver(&mut self, _token: u64, _completion: hirsel_run_schema::RunnerCompletion) {}
    /// Prepare an idle wait without blocking on a driver. A source-aware
    /// adapter flushes/delivers its idle observations here, then exposes all
    /// remaining drivers so another adapter's sources can be waited together.
    /// None preserves an opaque custom adapter's existing wait implementation.
    fn wait_sources(&mut self) -> Result<Option<WaitSources>, AdapterError> {
        Ok(None)
    }
    /// With no runnable task, wait until some parked request may be ready. False when no
    /// source can make one ready.
    fn wait(&mut self) -> Result<bool, AdapterError> {
        Ok(false)
    }
    /// Whether a script still holds input the run's idle points deliver (signals), so an
    /// idle run with nothing parked is not yet Blocked.
    fn scripted_input(&self) -> bool {
        false
    }
    /// End this run's idle [`wait`](Adapter::wait) whenever `fd` becomes readable, readying no
    /// request: a host's stop (`crate::host::RunControl`). Whether the adapter watches it; one
    /// whose waits do not block on the host needs not.
    fn wake_on(&mut self, _fd: std::os::fd::RawFd) -> bool {
        false
    }
}

/// The bounded preparation of an adapter's idle wait.
pub enum WaitSources {
    /// An idle observation or a script advanced; return to the scheduler.
    Ready,
    /// All remaining event sources. Empty means no external progress source.
    Drivers(Vec<crate::readiness::Readiness>),
}
impl WaitSources {
    pub(crate) fn wait(self) -> Result<bool, AdapterError> {
        match self {
            Self::Ready => Ok(true),
            Self::Drivers(drivers) if drivers.len() == 1 => drivers[0].wait(),
            Self::Drivers(drivers) => {
                crate::readiness::Readiness::wait_many(&drivers.iter().collect::<Vec<_>>())
            }
        }
    }
}

/// The parked requests a scheduling point re-issues ([`Adapter::ready`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ready {
    /// Every parked request.
    Every,
    /// Exactly these tokens, in token order.
    Tokens(Vec<u64>),
}
impl Ready {
    pub fn contains(&self, token: u64) -> bool {
        match self {
            Ready::Every => true,
            Ready::Tokens(tokens) => tokens.binary_search(&token).is_ok(),
        }
    }
}

/// Stable address owned by launch, bound explicitly to TaskCtx for generated thunks.
/// The binding outlives every release and cannot be accessed while a thunk runs.
pub struct Binding {
    pub(crate) adapter: Box<dyn Adapter>,
    names: std::sync::Arc<crate::paths::Names>,
    /// The requests parked because their adapter answered `Blocked`.
    pub(crate) waits: crate::host_waits::Waits,
}
impl Binding {
    pub(crate) fn tls(&mut self, engine: &'static dyn crate::net_tls::TlsEngine) {
        self.adapter.tls(engine);
    }
    pub(crate) fn net(&mut self, host: crate::net_host::Provider) {
        self.adapter.net(host);
    }
    pub(crate) fn process(&mut self, host: fn() -> Box<dyn crate::process::ProcessHost>) {
        self.adapter.process(host);
    }
    pub(crate) fn tls_roots(&mut self, stream: u64, pem: &[u8]) {
        self.adapter.tls_roots(stream, pem);
    }
    pub(crate) fn tls_identity(&mut self, stream: u64, chain: &[u8], key: &[u8]) {
        self.adapter.tls_identity(stream, chain, key);
    }
    pub(crate) fn tls_server(
        &mut self,
        stream: u64,
        identities: &[(&[u8], &[u8])],
        clients: crate::net::ClientAuth<&[u8]>,
        alpn: &[String],
    ) {
        self.adapter.tls_server(stream, identities, clients, alpn);
    }
    pub fn new(adapter: impl Adapter + 'static) -> Self {
        Self {
            adapter: Box::new(adapter),
            names: Default::default(),
            waits: Default::default(),
        }
    }
    pub(crate) fn from_box(adapter: Box<dyn Adapter>) -> Self {
        Self {
            adapter,
            names: Default::default(),
            waits: Default::default(),
        }
    }
    /// Explicit launch argument table; Path identities belong to this binding only.
    pub fn with_arguments(mut self, arguments: Vec<String>) -> Self {
        std::sync::Arc::get_mut(&mut self.names)
            .unwrap_or_else(|| crate::fault("launch arguments set after the binding was bound"))
            .arguments = arguments;
        self
    }
    pub fn intern_path(&self, path: &str) -> u64 {
        self.names.paths.intern(path)
    }
    fn path_text(&self, id: u64) -> &str {
        self.names.path_text(id)
    }
    pub(crate) fn path(&self, id: u64) -> String {
        self.path_text(id).to_owned()
    }
    /// # Safety
    /// This binding remains at its current address until it is unbound, and outlives
    /// every release on ctx. Close may run only while bound; access is exclusive.
    pub unsafe fn bind(&mut self, ctx: &mut crate::TaskCtx) {
        unsafe { ctx.bind_io_data((self as *mut Self).cast()) };
        ctx.run().names = Some(std::sync::Arc::clone(&self.names));
    }
    /// # Safety
    /// ctx is live and holds this adapter's Binding with exclusive access.
    pub(crate) unsafe fn get<'a>(ctx: *mut crate::TaskCtx) -> &'a mut Self {
        // SAFETY: a launched run's binding is its run-owned facade; the
        // actual adapter is accessed only by HomeAdapter on admission.
        unsafe {
            crate::stop::check(ctx, crate::stop::observe(ctx));
            Self::raw(ctx)
        }
    }

    /// Split off a run-owned facade before root code starts. The original
    /// binding (and its non-Send adapter) stays on the launcher's stack.
    pub(crate) unsafe fn bridge(ctx: *mut crate::TaskCtx) {
        unsafe {
            let run = (*ctx).run_ptr();
            if (*ctx).io_data().is_null() || (*(*run).io_facade.get()).is_some() {
                return;
            }
            let home = Self::raw(ctx);
            let mut facade = Box::new(Self {
                adapter: Box::new(crate::io_home::HomeAdapter {
                    adapter: &mut *home.adapter,
                    run,
                }),
                names: std::sync::Arc::clone(&home.names),
                waits: std::mem::take(&mut home.waits),
            });
            (*ctx).bind_io_data((&mut *facade as *mut Self).cast());
            (*(*run).io_facade.get()) = Some(facade);
        }
    }

    /// The bound binding where the calling thread is, without moving the running task: a
    /// read by the thread that runs the run, between its scheduling points.
    ///
    /// # Safety
    /// As for [`Binding::get`].
    pub(crate) unsafe fn raw<'a>(ctx: *mut crate::TaskCtx) -> &'a mut Self {
        let p = unsafe { (*ctx).io_data() }.cast::<Self>();
        if p.is_null() {
            crate::fault("Io call without an explicitly bound adapter");
        }
        unsafe { &mut *p }
    }
}
/// File closes and atomic aborts enter the same typed recorded protocol as explicit calls.
/// # Safety
/// ctx is live; its bound Binding has exclusive access and outlives every release.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_io_close(
    ctx: *mut crate::TaskCtx,
    handle: u64,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = crate::tasks::profile::Operation::new(ctx, "Close");
    let closed = if crate::tasks::free::host::native::enabled(ctx)
        || crate::tasks::ordered::engine::on(ctx)
    {
        unsafe {
            crate::stop::check(
                ctx,
                crate::host_waits::issue(ctx, Request::Close { handle }),
            )
        }
        .0
    } else {
        let binding = unsafe { Binding::get(ctx) };
        binding.adapter.call(Request::Close { handle })
    };
    // SAFETY: a close runs in a release, which holds nothing a stop must drop.
    unsafe { crate::tasks::leave_home(ctx) };
    match closed {
        Ok(Completion::Unit(Ok(()))) => crate::STATUS_OK,
        // A dropped accepted stream or listener left a scripted client's Expect unmet.
        Ok(Completion::Checked(false)) => {
            crate::failure::record_runtime(ctx, "LANG-9", anchor, operation_site);
            crate::STATUS_FAILURE
        }
        // SAFETY: a close runs in a release, which holds nothing a stop must drop.
        Err(error) => unsafe { crate::stop::raise(ctx, crate::stop::Stop::refusal(error)) },
        _ => crate::fault("invalid handle cleanup completion"),
    }
}
/// The inline bytes of `Result[(), E]` for `E = Fault(u32) | Canceled(CancelCause)`
/// (`OutputError`, `FileError`): three u32 ordinals, the Result's (0 Ok, 1 Error), the
/// error's (0 fault, 1 canceled) and the fault code or cancel cause.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnitResult {
    pub tag: u32,
    pub error: u32,
    pub code: u32,
}
impl UnitResult {
    pub fn of(value: Result<(), IoError>) -> Self {
        match value {
            Ok(()) => Self {
                tag: 0,
                error: 0,
                code: 0,
            },
            Err(IoError::Fault(code)) => Self {
                tag: 1,
                error: 0,
                code,
            },
            Err(IoError::Canceled(code)) => Self {
                tag: 1,
                error: 1,
                code,
            },
            Err(other) => Self {
                tag: 1,
                error: error_word(other) as u32,
                code: 0,
            },
        }
    }
}
/// Site-carrying variant of the stable output export. The checked FnAbi supplies
/// source codes so a truncated replay can attribute its prepared output wait.
/// # Safety
/// As `hirsel_rt_v2_out_print`; the source codes belong to this program.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_out_print_at_site(
    ctx: *mut crate::TaskCtx,
    _caller_site: u64,
    operation_site: u64,
    destination: *mut u8,
    out: *mut UnitResult,
    s: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // Own the semantic entry before inspecting replay state or suspending.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let replay = unsafe { (*ctx).run_ref().free_replay };
    let previous =
        replay.then(|| unsafe { std::mem::replace(&mut (*ctx).operation_site, operation_site) });
    let status = unsafe { hirsel_rt_v2_out_print(ctx, destination, out, s, owner, start, len) };
    if let Some(previous) = previous {
        unsafe { (*ctx).operation_site = previous };
    }
    status
}

/// `out.print` at the Hirsel ABI v2 of `fn(s: Stdout, text: String) -> Result[(), OutputError]`
/// (`HIRSEL_ABI_EXPORTS`): the context, the unused caller destination, the inline result,
/// the borrowed `Stdout` handle by read-only address, then the String slice by value as
/// its owner block, start and length. The text is the owner's sequence payload from
/// `8 + start`. An expected output error is a successful status with an Error result.
/// # Safety
/// ctx is bound exclusively, s is readable, owner is a live String block holding
/// `start + len` bytes past its count, and out is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_out_print(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    s: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    unsafe { hirsel_rt_v2_out_write(ctx, _destination, out, s, owner, start, len) }
}

/// Write the exact borrowed bytes to Stdout.
/// # Safety
/// Context and borrowed Stdout/Bytes are live; out is writable Result storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_out_write(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    s: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let stdout = unsafe { s.read() };
    let start =
        usize::try_from(start).unwrap_or_else(|_| crate::fault("Io start exceeds target usize"));
    let data = crate::alloc::payload(owner)
        .wrapping_add(8)
        .wrapping_add(start);
    // Bytes and String share the owner/start/length slice representation.
    let bytes = unsafe { std::slice::from_raw_parts(data, len as usize) };
    let Completion::Unit(value) = (unsafe { native_print(ctx, stdout, false, bytes) }) else {
        crate::fault("Io completion type mismatch");
    };
    unsafe { out.write(UnitResult::of(value)) };
    crate::STATUS_OK
}

/// # Safety
/// Context and borrowed Stderr/String are live; out is writable Result storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_err_print(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    s: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    unsafe { hirsel_rt_v2_err_write(ctx, _destination, out, s, owner, start, len) }
}

/// Write the exact borrowed bytes to Stderr.
/// # Safety
/// Context and borrowed Stderr/Bytes are live; out is writable Result storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_err_write(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    s: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let stderr = unsafe { s.read() };
    let start =
        usize::try_from(start).unwrap_or_else(|_| crate::fault("Io start exceeds target usize"));
    let data = crate::alloc::payload(owner)
        .wrapping_add(8)
        .wrapping_add(start);
    let bytes = unsafe { std::slice::from_raw_parts(data, len as usize) };
    let Completion::Unit(value) = (unsafe { native_print(ctx, stderr, true, bytes) }) else {
        crate::fault("Io completion type mismatch");
    };
    unsafe { out.write(UnitResult::of(value)) };
    crate::STATUS_OK
}

/// Result[handle/u64, FileError]. The payload union starts at byte eight;
/// its Error arm contains the FileError tag and code, not the Result tag.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HandleResult {
    pub tag: u32,
    pub padding: u32,
    pub value: u64,
}
impl HandleResult {
    pub fn of(value: Result<u64, IoError>) -> Self {
        match value {
            Ok(value) => Self {
                tag: 0,
                padding: 0,
                value,
            },
            Err(error) => Self {
                tag: 1,
                padding: 0,
                value: error_word(error),
            },
        }
    }
}
fn error_word(error: IoError) -> u64 {
    match error {
        IoError::Fault(code) => u64::from(code) << 32,
        IoError::Canceled(cause) => 1 | (u64::from(cause) << 32),
        IoError::NotGranted => 2,
        IoError::NotFound => 3,
        IoError::PermissionDenied => 4,
        IoError::AlreadyExists => 5,
        IoError::IsDirectory => 6,
        IoError::NotDirectory => 7,
        IoError::DirectoryNotEmpty => 8,
        IoError::CrossDevice => 9,
        IoError::Truncated => 10,
    }
}
// Prints cannot park. Keep their borrowed bytes out of the owned wait protocol,
// while retaining the same cancellation checks and post-effect scheduling point.
unsafe fn native_print(
    ctx: *mut crate::TaskCtx,
    handle: u64,
    stderr: bool,
    bytes: &[u8],
) -> Completion {
    #[cfg(feature = "measurement")]
    let _segment =
        crate::tasks::profile::Operation::new(ctx, if stderr { "PrintStderr" } else { "Print" });
    if crate::tasks::free::host::native::enabled(ctx)
        || (crate::tasks::ordered::engine::on(ctx) && crate::tasks::ordered::events::observing(ctx))
    {
        let request = if stderr {
            Request::PrintStderr {
                stderr: handle,
                bytes: bytes.to_vec(),
            }
        } else {
            Request::Print {
                stdout: handle,
                bytes: bytes.to_vec(),
            }
        };
        return unsafe { native_call(ctx, request) };
    }
    if crate::tasks::ordered::engine::on(ctx) {
        unsafe { crate::stop::check(ctx, crate::tasks::ordered::host::begin(ctx, false)) };
    }
    // Free mode: a host write on the `stdout` or `stderr` key, through the session's phases.
    let canceled = unsafe { crate::tasks::canceled(ctx) };
    let free = crate::free::print(
        ctx,
        stderr,
        bytes,
        canceled,
        || {
            let result = unsafe { Binding::get(ctx) }
                .adapter
                .print(handle, stderr, bytes);
            unsafe { crate::stop::check(ctx, result.map_err(crate::stop::Stop::refusal)) }
        },
        || Completion::Unit(Ok(())),
        |cause| Completion::Unit(Err(IoError::Canceled(cause))),
    );
    if let Some(completion) = free {
        unsafe { crate::tasks::yield_now(&mut *ctx) };
        return completion;
    }
    if canceled.is_some() {
        return unsafe {
            native_call(
                ctx,
                if stderr {
                    Request::PrintStderr {
                        stderr: handle,
                        bytes: bytes.to_vec(),
                    }
                } else {
                    Request::Print {
                        stdout: handle,
                        bytes: bytes.to_vec(),
                    }
                },
            )
        };
    }
    let ordered = crate::tasks::ordered::engine::on(ctx);
    if ordered {
        crate::tasks::ordered::events::request_print(ctx, handle, stderr, bytes);
    }
    let observing = ordered && crate::tasks::ordered::events::observing(ctx);
    let (result, count) = if observing {
        let request = if stderr {
            Request::PrintStderr {
                stderr: handle,
                bytes: bytes.to_vec(),
            }
        } else {
            Request::Print {
                stdout: handle,
                bytes: bytes.to_vec(),
            }
        };
        unsafe { crate::host_waits::observe_keys(ctx, &request) };
        let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
        unsafe { Binding::get(ctx) }
            .adapter
            .print_observed(handle, stderr, bytes)
    } else {
        (
            unsafe { Binding::get(ctx) }
                .adapter
                .print(handle, stderr, bytes),
            None,
        )
    };
    if ordered {
        if observing {
            unsafe { crate::host_waits::observe_result(ctx, &result) };
            crate::tasks::ordered::events::record_host_initial(ctx, &result);
            crate::tasks::ordered::events::print_output(ctx, bytes, &result, count);
        }
        crate::tasks::ordered::events::complete_host(ctx, false, &result);
        if observing {
            let _ = crate::tasks::ordered::events::certify_host(ctx);
        }
    }
    let completion = unsafe { crate::stop::check(ctx, result.map_err(crate::stop::Stop::refusal)) };
    if ordered {
        return unsafe {
            crate::tasks::ordered::host::concurrent::finish_home(ctx, Ok(completion), true, false)
        }
        .expect("print completion");
    }
    unsafe { crate::tasks::yield_now(&mut *ctx) };
    completion
}

unsafe fn native_call(ctx: *mut crate::TaskCtx, request: Request) -> Completion {
    #[cfg(feature = "measurement")]
    let _segment =
        crate::tasks::profile::Operation::new(ctx, crate::tasks::profile::request_kind(&request));
    let operation = !matches!(request, Request::Close { .. });
    // SAFETY: the context is live; the request moved into the issue.
    let (completion, parked) = unsafe {
        crate::stop::check(
            ctx,
            crate::host_waits::issue(ctx, request).and_then(|(completion, parked)| {
                Ok((completion.map_err(crate::stop::Stop::refusal)?, parked))
            }),
        )
    };
    if crate::tasks::free::host::native::enabled(ctx) {
        return unsafe {
            crate::tasks::free::host::native::finish_result(ctx, Ok(completion), operation, parked)
        }
        .expect("native call has a completion");
    }
    if crate::tasks::ordered::engine::on(ctx) {
        return unsafe {
            crate::tasks::ordered::host::concurrent::finish_home(
                ctx,
                Ok(completion),
                operation,
                parked,
            )
        }
        .expect("native call has a completion");
    }
    if operation && !parked {
        unsafe { crate::tasks::yield_now(&mut *ctx) };
    } else if !operation {
        // SAFETY: the context is live.
        unsafe { crate::tasks::leave_home(ctx) };
    }
    completion
}
/// A refused typed capability is CAP-2. The compiled caller records its anchor.
///
/// Every capability operation is a scheduling point (CONC-1): after its effect the running
/// task yields to the runnable queue. A request that parked yielded or parked there instead
/// (`crate::host_waits`). A handle's implicit close at its drop is not one.
pub(crate) unsafe fn capability_call(
    ctx: *mut crate::TaskCtx,
    request: Request,
) -> Result<Completion, ()> {
    #[cfg(feature = "measurement")]
    let _segment =
        crate::tasks::profile::Operation::new(ctx, crate::tasks::profile::request_kind(&request));
    let operation = !matches!(request, Request::Close { .. });
    // SAFETY: the context is live; the request moved into the issue.
    let (completion, parked) = unsafe {
        crate::stop::check(
            ctx,
            crate::host_waits::issue(ctx, request).and_then(|(completion, parked)| {
                let completion = match completion {
                    Ok(value) => Ok(value),
                    Err(AdapterError::InvalidHandle(_)) => Err(()),
                    Err(error) => return Err(crate::stop::Stop::refusal(error)),
                };
                Ok((completion, parked))
            }),
        )
    };
    if crate::tasks::free::host::native::enabled(ctx) {
        return unsafe {
            crate::tasks::free::host::native::finish_result(ctx, completion, operation, parked)
        };
    }
    if crate::tasks::ordered::engine::on(ctx) {
        return unsafe {
            crate::tasks::ordered::host::concurrent::finish_home(ctx, completion, operation, parked)
        };
    }
    if operation && !parked {
        unsafe { crate::tasks::yield_now(&mut *ctx) };
    } else if !operation {
        // SAFETY: the context is live.
        unsafe { crate::tasks::leave_home(ctx) };
    }
    completion
}
unsafe fn native_bytes<'a>(data: *const u8, len: u64) -> &'a [u8] {
    let n = usize::try_from(len).unwrap_or_else(|_| crate::fault("Io length overflow"));
    if n > isize::MAX as usize {
        crate::fault("Io extent overflow");
    }
    if n == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, n) }
    }
}
unsafe fn native_text<'a>(data: *const u8, len: u64) -> &'a str {
    std::str::from_utf8(unsafe { native_bytes(data, len) })
        .unwrap_or_else(|_| crate::fault("String operand is not UTF-8"))
}
pub(crate) unsafe fn slice_bytes<'a>(owner: *mut crate::Header, start: u64, len: u64) -> &'a [u8] {
    let offset = start
        .checked_add(8)
        .filter(|&n| n <= isize::MAX as u64)
        .unwrap_or_else(|| crate::fault("Io slice offset overflow"));
    unsafe { native_bytes(crate::alloc::payload(owner).add(offset as usize), len) }
}
macro_rules! path_export {
    ($name:ident, $request:ident, $field:ident, $completion:ident, $result:ty, $convert:expr) => {
        /// Hirsel ABI v2. Expected errors are results; refused authority returns status 1.
        /// # Safety
        /// The bound context, borrowed ID addresses and writable result are live.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            ctx: *mut crate::TaskCtx,
            _destination: *mut u8,
            out: *mut $result,
            parent: *const u64,
            path: *const u64,
        ) -> u8 {
            let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
            let request = Request::$request {
                $field: unsafe { parent.read() },
                path: crate::paths::names(ctx)
                    .path_text(unsafe { path.read() })
                    .to_owned(),
            };
            let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
                return crate::STATUS_FAILURE;
            };
            let Completion::$completion(value) = completion else {
                crate::fault("Io completion type mismatch");
            };
            unsafe { out.write(($convert)(value)) };
            crate::STATUS_OK
        }
    };
}
path_export!(
    hirsel_rt_v2_io_dir,
    Directory,
    io,
    Handle,
    HandleResult,
    HandleResult::of
);
path_export!(
    hirsel_rt_v2_dir_narrow,
    NarrowDir,
    directory,
    Handle,
    HandleResult,
    HandleResult::of
);
/// Hirsel ABI v2: `dir.read_only`. Expected errors are results; refused authority returns 1.
/// # Safety
/// The bound context, borrowed directory address and writable result are live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_dir_read_only(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    directory: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::ReadOnlyDir {
        directory: unsafe { directory.read() },
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Handle(value) = completion else {
        crate::fault("Io completion type mismatch");
    };
    unsafe { out.write(HandleResult::of(value)) };
    crate::STATUS_OK
}
path_export!(
    hirsel_rt_v2_file_open,
    Open,
    directory,
    Handle,
    HandleResult,
    HandleResult::of
);
path_export!(
    hirsel_rt_v2_file_open_append,
    OpenAppend,
    directory,
    Handle,
    HandleResult,
    HandleResult::of
);
path_export!(
    hirsel_rt_v2_file_create,
    Create,
    directory,
    Handle,
    HandleResult,
    HandleResult::of
);
path_export!(
    hirsel_rt_v2_file_create_atomic,
    CreateAtomic,
    directory,
    Handle,
    HandleResult,
    HandleResult::of
);
path_export!(
    hirsel_rt_v2_file_map,
    Map,
    directory,
    Handle,
    HandleResult,
    HandleResult::of
);
/// `mapped.with`'s view: the static block over a host mapping, or new bytes.
/// # Safety
/// Context bound, directory and mapping borrowed, out writable for a Slice.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mapped_view(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut crate::views::Slice,
    directory: *const u64,
    mapped: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::MapView {
        directory: unsafe { directory.read() },
        mapped: unsafe { mapped.read() },
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe {
        crate::tasks::free::host::native::convert_result(ctx, completion, |completion| {
            let Completion::View(view) = completion else {
                crate::fault("Io completion type mismatch");
            };
            let slice = match &view.bytes {
                _ if view.block != 0 => crate::mapping::view(view.block, view.length),
                Some(bytes) => crate::text::allocate_bytes(ctx, bytes),
                None => crate::fault("a mapped view without its bytes"),
            };
            out.write(slice);
        })
    };
    crate::STATUS_OK
}
/// `mapped.with`'s check: 0, 1 when the mapped file was truncated, or 2 when the mapping
/// predates its test tree's crash.
/// # Safety
/// Context bound, directory and mapping borrowed, out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mapped_check(
    ctx: *mut crate::TaskCtx,
    out: *mut u32,
    directory: *const u64,
    mapped: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::MapCheck {
        directory: unsafe { directory.read() },
        mapped: unsafe { mapped.read() },
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let status = match completion {
        Completion::Unit(Ok(())) => 0,
        Completion::Unit(Err(IoError::Truncated)) => 1,
        Completion::Unit(Err(IoError::Fault(1))) => 2,
        _ => crate::fault("Io completion type mismatch"),
    };
    unsafe { out.write(status) };
    crate::STATUS_OK
}
/// # Safety
/// Context bound; directory and Path borrowed, out writable Result[Watcher, FileError].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_watch(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    directory: *const u64,
    path: *const u64,
    recursive: u8,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::Watch {
        directory: unsafe { directory.read() },
        path: crate::paths::names(ctx)
            .path_text(unsafe { path.read() })
            .to_owned(),
        recursive: recursive != 0,
    };
    let Ok(Completion::Handle(result)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(HandleResult::of(result)) };
    crate::STATUS_OK
}
/// Result[WatchEvent, FileError]: the Result tag at 0; the event's tag at 8 and its paths
/// at 16 and 24, or the FileError at 8.
#[repr(C)]
pub struct WatchResult {
    pub words: [u64; 4],
}
/// A WatchEvent's tag and its paths, as `watcher.next` returns and `test.dir_event` takes.
fn watch_words(ctx: *mut crate::TaskCtx, event: &crate::watch::WatchEvent) -> [u64; 3] {
    use crate::watch::WatchEvent::*;
    let paths = &crate::paths::names(ctx).paths;
    match event {
        Created(p) => [0, paths.intern(p), 0],
        Modified(p) => [1, paths.intern(p), 0],
        Removed(p) => [2, paths.intern(p), 0],
        Renamed(a, b) => [3, paths.intern(a), paths.intern(b)],
        Overflow => [4, 0, 0],
    }
}
/// # Safety
/// Context bound; directory borrowed, watcher exclusive and out writable for 32 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_watcher_next(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut WatchResult,
    directory: *const u64,
    watcher: *mut u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::WatchNext {
        directory: unsafe { directory.read() },
        watcher: unsafe { watcher.read() },
    };
    let Ok(Completion::Watch(result)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(watch_result(ctx, &result)) };
    crate::STATUS_OK
}
pub(crate) fn watch_result(
    ctx: *mut crate::TaskCtx,
    result: &Result<crate::watch::WatchEvent, IoError>,
) -> WatchResult {
    let words = match result {
        Ok(event) => {
            let [tag, first, second] = watch_words(ctx, event);
            [0, tag, first, second]
        }
        Err(error) => [1, error_word(error.clone()), 0, 0],
    };
    WatchResult { words }
}
/// # Safety
/// Context bound; Test and directory borrowed, event a readable WatchEvent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_dir_event(
    ctx: *mut crate::TaskCtx,
    test: *const u64,
    directory: *const u64,
    event: *const [u64; 3],
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    use crate::watch::WatchEvent::*;
    let [tag, first, second] = unsafe { event.read() };
    let names = crate::paths::names(ctx);
    let event = match tag as u32 {
        0 => Created(names.path_text(first).to_owned()),
        1 => Modified(names.path_text(first).to_owned()),
        2 => Removed(names.path_text(first).to_owned()),
        3 => Renamed(
            names.path_text(first).to_owned(),
            names.path_text(second).to_owned(),
        ),
        4 => Overflow,
        _ => crate::fault("invalid WatchEvent tag"),
    };
    let request = Request::TestDirEvent {
        test: unsafe { test.read() },
        directory: unsafe { directory.read() },
        event,
    };
    match unsafe { capability_call(ctx, request) } {
        Ok(Completion::Unit(Ok(()))) => crate::STATUS_OK,
        _ => crate::STATUS_FAILURE,
    }
}
path_export!(
    hirsel_rt_v2_file_size,
    Size,
    directory,
    Size,
    HandleResult,
    HandleResult::of
);
// FileError has no block but is nominal, hence Result[(), FileError] has the hidden destination too.
path_export!(
    hirsel_rt_v2_file_remove,
    Remove,
    directory,
    Unit,
    UnitResult,
    UnitResult::of
);

/// # Safety
/// The live File ID transfers at entry; the directory address is borrowed, out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_narrow(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    directory: *const u64,
    file: *mut u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::NarrowFile {
        directory: unsafe { directory.read() },
        file: unsafe { file.read() },
    };
    // Erasing the transferred physical word makes accidental runtime reuse fail closed.
    unsafe { file.write(0) };
    let transferred = match request {
        Request::NarrowFile { file, .. } => file,
        _ => unreachable!(),
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        unsafe {
            native_call(
                ctx,
                Request::Close {
                    handle: transferred,
                },
            )
        };
        return crate::STATUS_FAILURE;
    };
    let Completion::OptionalFile(value) = completion else {
        crate::fault("Io completion type mismatch");
    };
    unsafe {
        out.write(HandleResult {
            tag: u32::from(value.is_some()),
            padding: 0,
            value: value.unwrap_or(0),
        })
    };
    crate::STATUS_OK
}
/// # Safety
/// The live AtomicFile ID transfers at entry; directory is borrowed and out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_commit(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    file: *mut u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::Commit {
        directory: unsafe { directory.read() },
        file: unsafe { file.read() },
    };
    unsafe { file.write(0) };
    let transferred = match request {
        Request::Commit { file, .. } => file,
        _ => unreachable!(),
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        unsafe {
            native_call(
                ctx,
                Request::Close {
                    handle: transferred,
                },
            )
        };
        return crate::STATUS_FAILURE;
    };
    let Completion::Unit(value) = completion else {
        crate::fault("Io completion type mismatch");
    };
    unsafe { out.write(UnitResult::of(value)) };
    crate::STATUS_OK
}

/// Append delivered file bytes through the caller's mutable List[u8] slot.
/// # Safety
/// Context bound, directory/file live, buffer exclusive and out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_read(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    directory: *const u64,
    file: *mut u64,
    buffer: *mut *mut crate::Header,
    max: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::FileRead {
        directory: unsafe { directory.read() },
        file: unsafe { file.read() },
        max,
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe {
        crate::tasks::free::host::native::convert_result(ctx, completion, |completion| {
            let Completion::Bytes(value) = completion else {
                crate::fault("file read completion mismatch");
            };
            let result = match value {
                Ok(bytes) => {
                    append_byte_list(ctx, buffer, bytes);
                    Ok(bytes.len() as u64)
                }
                Err(error) => Err(error.clone()),
            };
            out.write(HandleResult::of(result));
        })
    };
    crate::STATUS_OK
}
/// # Safety
/// Context bound, directory/file live, out writable; file is exclusively borrowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_seek(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    file: *mut u64,
    offset: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::FileSeek {
        directory: unsafe { directory.read() },
        file: unsafe { file.read() },
        offset,
    };
    let Ok(Completion::Unit(value)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(UnitResult::of(value)) };
    crate::STATUS_OK
}
/// # Safety
/// Context bound, directory/file live, Bytes borrowed, out writable.
#[cfg_attr(not(feature = "operation-latency"), unsafe(no_mangle))]
pub unsafe extern "C" fn hirsel_rt_v2_file_write(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    directory: *const u64,
    file: *mut u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::FileWrite {
        directory: unsafe { directory.read() },
        file: unsafe { file.read() },
        bytes: unsafe { slice_bytes(owner, start, len) }.to_vec(),
    };
    let Ok(Completion::Size(value)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(HandleResult::of(value)) };
    crate::STATUS_OK
}
/// # Safety
/// Context bound, directory/file live, Bytes borrowed, out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_append(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    file: *mut u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::FileAppend {
        directory: unsafe { directory.read() },
        file: unsafe { file.read() },
        bytes: unsafe { slice_bytes(owner, start, len) }.to_vec(),
    };
    let Ok(Completion::Unit(value)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(UnitResult::of(value)) };
    crate::STATUS_OK
}

/// # Safety
/// Directory/Path are borrowed, the slice transfers one owner credit, out is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_write_bytes(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    path: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::Write {
        directory: unsafe { directory.read() },
        path: crate::paths::names(ctx)
            .path_text(unsafe { path.read() })
            .to_owned(),
        bytes: unsafe { slice_bytes(owner, start, len) }.to_vec(),
    };
    let completion = unsafe { capability_call(ctx, request) };
    unsafe {
        crate::release::hirsel_rt_v2_release(ctx, owner, (*owner).index(), std::ptr::null(), 0, 0)
    };
    let Ok(Completion::Unit(value)) = completion else {
        if completion.is_err() {
            return crate::STATUS_FAILURE;
        }
        crate::fault("Io completion type mismatch");
    };
    unsafe { out.write(UnitResult::of(value)) };
    crate::STATUS_OK
}
/// # Safety
/// Context bound, directory/file live, out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_sync(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    file: *mut u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::Sync {
        directory: unsafe { directory.read() },
        file: unsafe { file.read() },
    };
    let Ok(Completion::Unit(value)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(UnitResult::of(value)) };
    crate::STATUS_OK
}
/// # Safety
/// Directory/Path/Durability are borrowed, the slice transfers one owner credit, out is
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_put_new(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    path: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
    mode: *const u32,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let durable = match unsafe { mode.read() } {
        0 => true,
        1 => false,
        _ => crate::fault("Durability tag out of range"),
    };
    let request = Request::PutNew {
        directory: unsafe { directory.read() },
        path: crate::paths::names(ctx)
            .path_text(unsafe { path.read() })
            .to_owned(),
        bytes: unsafe { slice_bytes(owner, start, len) }.to_vec(),
        durable,
    };
    let completion = unsafe { capability_call(ctx, request) };
    unsafe {
        crate::release::hirsel_rt_v2_release(ctx, owner, (*owner).index(), std::ptr::null(), 0, 0)
    };
    let Ok(Completion::Unit(value)) = completion else {
        if completion.is_err() {
            return crate::STATUS_FAILURE;
        }
        crate::fault("Io completion type mismatch");
    };
    unsafe { out.write(UnitResult::of(value)) };
    crate::STATUS_OK
}
/// # Safety
/// Directory is borrowed, file is an exclusive address, the slice is borrowed, out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_write_all(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    file: *mut u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::WriteAll {
        directory: unsafe { directory.read() },
        file: unsafe { file.read() },
        bytes: unsafe { slice_bytes(owner, start, len) }.to_vec(),
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Unit(value) = completion else {
        crate::fault("Io completion type mismatch");
    };
    unsafe { out.write(UnitResult::of(value)) };
    crate::STATUS_OK
}
/// # Safety
/// The String slice is borrowed, context bound and result writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_path_of(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    let text = std::str::from_utf8(unsafe { slice_bytes(owner, start, len) })
        .unwrap_or_else(|_| crate::fault("String operand is not UTF-8"));
    let id = crate::paths::names(ctx).paths.intern(text);
    unsafe { out.write(id) };
    crate::STATUS_OK
}
/// Extract the original lexical spelling without a capability operation.
/// # Safety
/// The Path belongs to the bound context; output is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_path_text(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut crate::views::Slice,
    path: *const u64,
) -> u8 {
    let text = crate::paths::names(ctx).path_text(unsafe { path.read() });
    let value = unsafe { crate::text::allocate_bytes(ctx, text.as_bytes()) };
    unsafe {
        out.write(value);
    }
    crate::STATUS_OK
}
/// # Safety
/// Args is the borrowed launch handle and the output is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_arg_path(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    args: *const u64,
    index: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = crate::tasks::profile::Operation::new(ctx, "hirsel_rt_v2_arg_path");
    if unsafe { args.read() } != crate::launch::ARGS_HANDLE {
        return crate::STATUS_FAILURE;
    }
    let names = crate::paths::names(ctx);
    let text = usize::try_from(index)
        .ok()
        .and_then(|i| names.arguments.get(i));
    let value = match text {
        Some(text) => HandleResult::of(Ok(names.paths.intern(text))),
        None => HandleResult {
            tag: 1,
            padding: 0,
            value: 0,
        },
    };
    unsafe { out.write(value) };
    if crate::tasks::ordered::engine::on(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                crate::tasks::ordered::host::argument(ctx, "arg.path", Some(index), || {
                    text.map(|text| text.as_bytes().to_vec())
                }),
            )
        };
    } else {
        unsafe { crate::tasks::yield_now(&mut *ctx) };
    }
    crate::STATUS_OK
}

/// # Safety
/// Args names the launch snapshot and out is writable scalar storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_arg_len(
    ctx: *mut crate::TaskCtx,
    out: *mut u64,
    args: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = crate::tasks::profile::Operation::new(ctx, "hirsel_rt_v2_arg_len");
    if unsafe { args.read() } != crate::launch::ARGS_HANDLE {
        return crate::STATUS_FAILURE;
    }
    let len = crate::paths::names(ctx).arguments.len() as u64;
    unsafe { out.write(len) };
    if crate::tasks::ordered::engine::on(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                crate::tasks::ordered::host::argument(ctx, "arg.len", None, || {
                    Some(len.to_le_bytes().to_vec())
                }),
            )
        };
    } else {
        unsafe { crate::tasks::yield_now(&mut *ctx) };
    }
    crate::STATUS_OK
}

/// # Safety
/// Args names the launch snapshot and out is writable Result[String, ArgError] storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_arg_text(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut crate::text::StringResult,
    args: *const u64,
    index: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = crate::tasks::profile::Operation::new(ctx, "hirsel_rt_v2_arg_text");
    if unsafe { args.read() } != crate::launch::ARGS_HANDLE {
        return crate::STATUS_FAILURE;
    }
    let text = usize::try_from(index)
        .ok()
        .and_then(|i| crate::paths::names(ctx).arguments.get(i));
    // Zero initializes the MissingArgument tag and all padding on the error arm.
    let mut result: crate::text::StringResult = unsafe { std::mem::zeroed() };
    match text {
        Some(text) => result.value = unsafe { crate::text::allocate_bytes(ctx, text.as_bytes()) },
        None => result.tag = 1,
    }
    unsafe { out.write(result) };
    if crate::tasks::ordered::engine::on(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                crate::tasks::ordered::host::argument(ctx, "arg.text", Some(index), || {
                    text.map(|text| text.as_bytes().to_vec())
                }),
            )
        };
    } else {
        unsafe { crate::tasks::yield_now(&mut *ctx) };
    }
    crate::STATUS_OK
}

/// Inline Result[capability, GrantError]. The error is a nullary tag at offset 8.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GrantResult {
    pub tag: u32,
    pub padding: u32,
    pub value: u64,
}
impl GrantResult {
    pub fn of(value: Result<u64, IoError>) -> Self {
        match value {
            Ok(value) => Self {
                tag: 0,
                padding: 0,
                value,
            },
            Err(IoError::NotGranted) => Self {
                tag: 1,
                padding: 0,
                value: 0,
            },
            Err(_) => crate::fault("grant projection returned a non-grant error"),
        }
    }
}
macro_rules! grant_projection_export {
    ($name:ident, $request:ident) => {
        /// # Safety
        /// The context, borrowed Io address and writable Result output are live.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            ctx: *mut crate::TaskCtx,
            _destination: *mut u8,
            out: *mut GrantResult,
            io: *const u64,
        ) -> u8 {
            let Ok(completion) =
                (unsafe { capability_call(ctx, Request::$request { io: io.read() }) })
            else {
                return crate::STATUS_FAILURE;
            };
            let Completion::Handle(value) = completion else {
                crate::fault("Io projection completion mismatch");
            };
            unsafe { out.write(GrantResult::of(value)) };
            crate::STATUS_OK
        }
    };
}
grant_projection_export!(hirsel_rt_v2_io_stdin, Stdin);
grant_projection_export!(hirsel_rt_v2_io_stderr, Stderr);
grant_projection_export!(hirsel_rt_v2_io_stdout, Stdout);
grant_projection_export!(hirsel_rt_v2_io_random, Random);
grant_projection_export!(hirsel_rt_v2_io_telemetry, Telemetry);

/// `Result[Span, TelemetryError]`: the Result tag at 0, then the Span handle or the
/// error's ordinal at 8.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpanResult {
    pub tag: u32,
    pub padding: u32,
    pub value: u64,
}
/// `Result[(), TelemetryError]`: the Result tag, then the error's ordinal.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TelemetryResult {
    pub tag: u32,
    pub error: u32,
}
impl TelemetryResult {
    fn of(value: Result<(), hirsel_kernels::telemetry::Refusal>) -> Self {
        match value {
            Ok(()) => Self { tag: 0, error: 0 },
            Err(refusal) => Self {
                tag: 1,
                error: refusal.ordinal(),
            },
        }
    }
}
/// An accepted observation: a scheduling point with no cancellation check, whose
/// completion carries nothing the program reads.
unsafe fn telemetry_observe(ctx: *mut crate::TaskCtx, request: Request) -> u8 {
    match unsafe { capability_call(ctx, request) } {
        Ok(Completion::Unit(Ok(()))) => crate::STATUS_OK,
        Ok(_) => crate::fault("telemetry completion mismatch"),
        Err(()) => crate::STATUS_FAILURE,
    }
}
/// # Safety
/// Context bound, Telemetry readable, both Bytes live slices and out writable. A refused
/// parent or an oversized payload issues no request.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn hirsel_rt_v2_capture_begin(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut SpanResult,
    telemetry: *const u64,
    parent_owner: *mut crate::Header,
    parent_start: u64,
    parent_len: u64,
    spec_owner: *mut crate::Header,
    spec_start: u64,
    spec_len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    use hirsel_kernels::telemetry as kernel;
    let parent = unsafe { slice_bytes(parent_owner, parent_start, parent_len) };
    let spec = unsafe { slice_bytes(spec_owner, spec_start, spec_len) };
    let checked = kernel::bounded(&[spec]).and_then(|()| kernel::parent(parent));
    let result = match checked {
        Err(refusal) => SpanResult {
            tag: 1,
            padding: 0,
            value: refusal.ordinal().into(),
        },
        Ok(_) => {
            let request = Request::TelemetryBegin {
                telemetry: unsafe { telemetry.read() },
                parent: parent.to_vec(),
                spec: spec.to_vec(),
            };
            let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
                return crate::STATUS_FAILURE;
            };
            let Completion::Span { handle, .. } = completion else {
                crate::fault("telemetry begin completion mismatch");
            };
            SpanResult {
                tag: 0,
                padding: 0,
                value: handle,
            }
        }
    };
    unsafe { out.write(result) };
    crate::STATUS_OK
}
/// # Safety
/// Context bound, Telemetry and Span readable and the Bytes slot writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_capture_context(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut crate::views::Slice,
    telemetry: *const u64,
    span: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if crate::tasks::free::host::native::enabled(ctx) {
        let Some(bytes) = crate::tasks::free::host::native::span_context(
            ctx,
            unsafe { telemetry.read() },
            unsafe { span.read() },
        ) else {
            return crate::STATUS_FAILURE;
        };
        unsafe {
            crate::tasks::free::host::native::convert_result(
                ctx,
                Completion::Bytes(Ok(bytes)),
                |completion| {
                    let Completion::Bytes(Ok(bytes)) = completion else {
                        unreachable!()
                    };
                    out.write(crate::text::allocate_bytes(ctx, bytes));
                },
            )
        };
        return crate::STATUS_OK;
    }
    let binding = unsafe { Binding::get(ctx) };
    let Some(context) = binding
        .adapter
        .telemetry_context(unsafe { telemetry.read() }, unsafe { span.read() })
    else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(crate::text::allocate_bytes(ctx, &context)) };
    crate::STATUS_OK
}
/// # Safety
/// Context bound, Telemetry and Span readable, payload a live slice, out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_capture_event(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut TelemetryResult,
    telemetry: *const u64,
    span: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let payload = unsafe { slice_bytes(owner, start, len) };
    let checked = hirsel_kernels::telemetry::bounded(&[payload]);
    if checked.is_ok() {
        let request = Request::TelemetryEvent {
            telemetry: unsafe { telemetry.read() },
            span: unsafe { span.read() },
            payload: payload.to_vec(),
        };
        if unsafe { telemetry_observe(ctx, request) } != crate::STATUS_OK {
            return crate::STATUS_FAILURE;
        }
    }
    unsafe { out.write(TelemetryResult::of(checked)) };
    crate::STATUS_OK
}
/// # Safety
/// Context bound, Telemetry readable, the taken Span readable, status a live slice and
/// out writable. A refused status abandons the span, as its drop would.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_capture_finish(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut TelemetryResult,
    telemetry: *const u64,
    span: *mut u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let (telemetry, span) = unsafe { (telemetry.read(), span.read()) };
    let status = unsafe { slice_bytes(owner, start, len) };
    let checked = hirsel_kernels::telemetry::bounded(&[status]);
    let request = match checked {
        Ok(()) => Request::TelemetryFinish {
            telemetry,
            span,
            status: status.to_vec(),
        },
        Err(_) => {
            // CAP-2 still precedes the refusal: only the span's own Telemetry abandons it.
            let valid = if crate::tasks::free::host::native::enabled(ctx) {
                crate::tasks::free::host::native::span_valid(ctx, telemetry, span)
            } else {
                unsafe { Binding::get(ctx) }
                    .adapter
                    .telemetry_context(telemetry, span)
                    .is_some()
            };
            if !valid {
                return crate::STATUS_FAILURE;
            }
            Request::Close { handle: span }
        }
    };
    if unsafe { telemetry_observe(ctx, request) } != crate::STATUS_OK {
        return crate::STATUS_FAILURE;
    }
    unsafe { out.write(TelemetryResult::of(checked)) };
    crate::STATUS_OK
}
macro_rules! telemetry_record_export {
    ($name:ident, $request:ident) => {
        /// # Safety
        /// Context bound, Telemetry readable, payload a live slice and out writable.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            ctx: *mut crate::TaskCtx,
            _destination: *mut u8,
            out: *mut TelemetryResult,
            telemetry: *const u64,
            owner: *mut crate::Header,
            start: u64,
            len: u64,
        ) -> u8 {
            let payload = unsafe { slice_bytes(owner, start, len) };
            let checked = hirsel_kernels::telemetry::bounded(&[payload]);
            if checked.is_ok() {
                let request = Request::$request {
                    telemetry: unsafe { telemetry.read() },
                    payload: payload.to_vec(),
                };
                if unsafe { telemetry_observe(ctx, request) } != crate::STATUS_OK {
                    return crate::STATUS_FAILURE;
                }
            }
            unsafe { out.write(TelemetryResult::of(checked)) };
            crate::STATUS_OK
        }
    };
}
telemetry_record_export!(hirsel_rt_v2_capture_log, TelemetryLog);
telemetry_record_export!(hirsel_rt_v2_capture_measure, TelemetryMeasure);
grant_projection_export!(hirsel_rt_v2_io_tasks, Tasks);
grant_projection_export!(hirsel_rt_v2_io_net, Net);
grant_projection_export!(hirsel_rt_v2_io_process, Process);
grant_projection_export!(hirsel_rt_v2_io_signals, Signals);

/// Result[SignalKind, SignalError]: the Ok kind's ordinal, or the SignalError tag and its
/// CancelCause ordinal, each a word after the Result tag.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalResult {
    pub tag: u32,
    pub value: u32,
    pub cause: u32,
}
/// `signal.wait`: the kinds are a List[SignalKind], each element its u32 variant ordinal.
/// # Safety
/// Context, borrowed Signals word and List are live; out is writable Result storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_signal_wait(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut SignalResult,
    signals: *const u64,
    kinds: *mut crate::Header,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let kinds = unsafe {
        crate::process::list(kinds, 4, |at| {
            hirsel_run_schema::SignalKind::from_ordinal(at.cast::<u32>().read())
                .unwrap_or_else(|| crate::fault("SignalKind tag"))
                .bit()
        })
    }
    .into_iter()
    .fold(0, |bits, bit| bits | bit);
    let request = Request::SignalWait {
        signals: unsafe { signals.read() },
        kinds,
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Signal(value) = completion else {
        crate::fault("signal wait completion mismatch");
    };
    let result = match value {
        Ok(kind) => SignalResult {
            tag: 0,
            value: kind.ordinal(),
            cause: 0,
        },
        Err(SignalError::Canceled(cause)) => SignalResult {
            tag: 1,
            value: 0,
            cause,
        },
        Err(SignalError::NotGranted) => SignalResult {
            tag: 1,
            value: 1,
            cause: 0,
        },
        Err(SignalError::InvalidInput) => SignalResult {
            tag: 1,
            value: 2,
            cause: 0,
        },
    };
    unsafe { out.write(result) };
    crate::STATUS_OK
}

/// Result with an eight-byte success payload and a nullary error tag.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockResult {
    pub tag: u32,
    pub padding: u32,
    pub value: u64,
}
impl ClockResult {
    fn sample(value: Result<u64, ClockError>) -> Self {
        match value {
            Ok(value) => Self {
                tag: 0,
                padding: 0,
                value,
            },
            Err(error) => Self {
                tag: 1,
                padding: 0,
                value: match error {
                    ClockError::Unavailable => 0,
                    ClockError::OutOfRange => 1,
                },
            },
        }
    }
}
macro_rules! clock_export {
    ($name:ident, $request:ident, $field:ident, $completion:ident, $convert:expr) => {
        /// # Safety
        /// Context is exclusively bound, the grant readable and the result writable.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            ctx: *mut crate::TaskCtx,
            _destination: *mut u8,
            out: *mut ClockResult,
            grant: *const u64,
        ) -> u8 {
            let Ok(completion) = (unsafe {
                capability_call(
                    ctx,
                    Request::$request {
                        $field: grant.read(),
                    },
                )
            }) else {
                return crate::STATUS_FAILURE;
            };
            let Completion::$completion(value) = completion else {
                crate::fault("clock completion mismatch");
            };
            let converted: Result<u64, ClockError> = ($convert)(value);
            unsafe { out.write(ClockResult::sample(converted)) };
            crate::STATUS_OK
        }
    };
}
clock_export!(
    hirsel_rt_v2_io_monotonic_clock,
    MonotonicClock,
    io,
    ClockGrant,
    |v: Result<u64, ()>| v.map_err(|()| ClockError::Unavailable)
);
clock_export!(
    hirsel_rt_v2_io_wall_clock,
    WallClock,
    io,
    ClockGrant,
    |v: Result<u64, ()>| v.map_err(|()| ClockError::Unavailable)
);
clock_export!(
    hirsel_rt_v2_clock_monotonic,
    Monotonic,
    clock,
    Monotonic,
    |v| v
);
clock_export!(hirsel_rt_v2_clock_wall, Wall, clock, Wall, |v: Result<
    i64,
    ClockError,
>| v
    .map(|n| n as u64));
/// `Option[Instant]`: tag 1 carries the advanced ticks.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstantOption {
    pub tag: u32,
    pub padding: u32,
    pub ticks: u64,
}
/// # Safety
/// The clock and Instant words are readable and the result writable. Pure: no request.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_instant_checked_add(
    _ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut InstantOption,
    _clock: *const u64,
    start: *const u64,
    nanoseconds: u64,
) -> u8 {
    let value = match unsafe { start.read() }.checked_add(nanoseconds) {
        Some(ticks) => InstantOption {
            tag: 1,
            padding: 0,
            ticks,
        },
        None => InstantOption {
            tag: 0,
            padding: 0,
            ticks: 0,
        },
    };
    unsafe { out.write(value) };
    crate::STATUS_OK
}
/// Site-carrying variant of the stable clock wait export. Existing callers keep
/// their ABI. The ordinary checked FnAbi carries these two inert source codes;
/// no compiler helper invocation or additional semantic operation is introduced.
/// # Safety
/// As `hirsel_rt_v2_clock_sleep_until`; the source codes belong to this program.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_clock_sleep_until_at_site(
    ctx: *mut crate::TaskCtx,
    _caller_site: u64,
    operation_site: u64,
    destination: *mut u8,
    out: *mut UnitResult,
    clock: *const u64,
    deadline: *const u64,
) -> u8 {
    // Own the semantic entry before inspecting replay state or suspending.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let replay = unsafe { (*ctx).run_ref().free_replay };
    let previous =
        replay.then(|| unsafe { std::mem::replace(&mut (*ctx).operation_site, operation_site) });
    let status = unsafe { hirsel_rt_v2_clock_sleep_until(ctx, destination, out, clock, deadline) };
    if let Some(previous) = previous {
        unsafe { (*ctx).operation_site = previous };
    }
    status
}
/// # Safety
/// Context is exclusively bound, the clock and Instant words readable and the result
/// writable. Without a cooperative scheduler the thread blocks until the deadline.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_clock_sleep_until(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    clock: *const u64,
    deadline: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::SleepUntil {
        clock: unsafe { clock.read() },
        deadline: unsafe { deadline.read() },
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Wait(value) = completion else {
        crate::fault("clock wait completion mismatch");
    };
    // WaitError::Clock(e) is error 0 with the ClockError ordinal as its payload;
    // WaitError::Canceled(cause) is error 1 with the cause.
    let result = wait_result(value);
    unsafe { out.write(result) };
    crate::STATUS_OK
}
pub(crate) fn wait_result(value: Result<(), WaitError>) -> UnitResult {
    match value {
        Ok(()) => UnitResult {
            tag: 0,
            error: 0,
            code: 0,
        },
        Err(WaitError::Clock(e)) => UnitResult {
            tag: 1,
            error: 0,
            code: match e {
                ClockError::Unavailable => 0,
                ClockError::OutOfRange => 1,
            },
        },
        Err(WaitError::Canceled(cause)) => UnitResult {
            tag: 1,
            error: 1,
            code: cause,
        },
    }
}
/// # Safety
/// Context is bound, the Random word readable and the result writable. A zero count
/// returns empty Bytes without a request, as the reference does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_random_bytes(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut BytesResult,
    random: *const u64,
    count: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let completion = if count == 0 {
        Completion::Random(Ok(vec![]))
    } else {
        let request = Request::RandomBytes {
            random: unsafe { random.read() },
            count,
        };
        let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
            return crate::STATUS_FAILURE;
        };
        completion
    };
    unsafe {
        crate::tasks::free::host::native::convert_result(ctx, completion, |completion| {
            let Completion::Random(value) = completion else {
                crate::fault("random completion mismatch");
            };
            // Initialize padding and inactive bytes; RandomError has no payload.
            out.cast::<u8>()
                .write_bytes(0, std::mem::size_of::<BytesResult>());
            match value {
                Ok(bytes) => (*out).value = crate::text::allocate_bytes(ctx, bytes),
                Err(RandomError::Unavailable) => (*out).tag = 1,
            }
        })
    };
    crate::STATUS_OK
}
/// # Safety
/// The sampled Instant word is readable and the scalar result writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_instant_ticks(
    _ctx: *mut crate::TaskCtx,
    out: *mut u64,
    instant: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(_ctx) };
    unsafe { out.write(instant.read()) };
    crate::STATUS_OK
}

/// # Safety
/// A live Test grant is borrowed. The boolean is the ABI's zero-extended byte.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_check(
    ctx: *mut crate::TaskCtx,
    test: *const u64,
    condition: u8,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let Ok(completion) = (unsafe {
        capability_call(
            ctx,
            Request::TestCheck {
                test: test.read(),
                condition: condition != 0,
            },
        )
    }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Checked(condition) = completion else {
        crate::fault("Test completion mismatch");
    };
    if condition {
        crate::STATUS_OK
    } else {
        crate::STATUS_FAILURE
    }
}
/// # Safety
/// A live Test grant is borrowed; the boolean is a zero-extended byte.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_check_case(
    ctx: *mut crate::TaskCtx,
    test: *const u64,
    condition: u8,
    case: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let Ok(completion) = (unsafe {
        capability_call(
            ctx,
            Request::TestCheckCase {
                test: test.read(),
                condition: condition != 0,
                case,
            },
        )
    }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Checked(condition) = completion else {
        crate::fault("Test completion mismatch");
    };
    if condition {
        crate::STATUS_OK
    } else {
        crate::STATUS_FAILURE
    }
}

/// # Safety
/// A live Test grant is borrowed. The callee result is Never: the only status
/// it returns is failure, along the diverging call's failure edge.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_fail(ctx: *mut crate::TaskCtx, test: *const u64) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let Ok(completion) = (unsafe { capability_call(ctx, Request::TestFail { test: test.read() }) })
    else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Checked(false) = completion else {
        crate::fault("Test completion mismatch");
    };
    crate::STATUS_FAILURE
}

/// # Safety
/// A live Test grant and Dir are borrowed. A Dir that `test.dir` did not make fails LANG-9.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_crash(
    ctx: *mut crate::TaskCtx,
    test: *const u64,
    directory: *const u64,
    seed: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::TestCrash {
        test: unsafe { test.read() },
        directory: unsafe { directory.read() },
        seed,
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Checked(simulated) = completion else {
        crate::fault("Test completion mismatch");
    };
    if simulated {
        crate::STATUS_OK
    } else {
        crate::STATUS_FAILURE
    }
}
/// # Safety
/// A live Test grant is borrowed and out is writable scalar storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_seed(
    ctx: *mut crate::TaskCtx,
    out: *mut u64,
    test: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let Ok(completion) = (unsafe { capability_call(ctx, Request::TestSeed { test: test.read() }) })
    else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Size(Ok(seed)) = completion else {
        crate::fault("Test completion mismatch");
    };
    unsafe { out.write(seed) };
    crate::STATUS_OK
}

/// # Safety
/// A live Test grant is borrowed and out is writable handle storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_tasks(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    test: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let Ok(completion) =
        (unsafe { capability_call(ctx, Request::TestTasks { test: test.read() }) })
    else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Handle(Ok(tasks)) = completion else {
        crate::fault("Test completion mismatch");
    };
    unsafe { out.write(tasks) };
    crate::STATUS_OK
}

/// # Safety
/// A live Test grant is borrowed; out is writable capability storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_clock(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    test: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let Ok(Completion::Handle(Ok(clock))) =
        (unsafe { capability_call(ctx, Request::TestClock { test: test.read() }) })
    else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(clock) };
    crate::STATUS_OK
}

/// The borrowed String's text: its owner block, byte start and length.
unsafe fn borrowed_text(owner: *mut crate::Header, start: u64, len: u64) -> String {
    let start =
        usize::try_from(start).unwrap_or_else(|_| crate::fault("Test start exceeds target usize"));
    let data = crate::alloc::payload(owner)
        .wrapping_add(8)
        .wrapping_add(start);
    unsafe { native_text(data, len) }.to_owned()
}

/// # Safety
/// A live Test grant and String are borrowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_log(
    ctx: *mut crate::TaskCtx,
    test: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let text = unsafe { borrowed_text(owner, start, len) };
    let Ok(completion) = (unsafe {
        capability_call(
            ctx,
            Request::TestLog {
                test: test.read(),
                text,
            },
        )
    }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Checked(true) = completion else {
        crate::fault("Test completion mismatch");
    };
    crate::STATUS_OK
}

/// # Safety
/// A live Test grant and both Strings are borrowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_expect_snapshot(
    ctx: *mut crate::TaskCtx,
    test: *const u64,
    name_owner: *mut crate::Header,
    name_start: u64,
    name_len: u64,
    text_owner: *mut crate::Header,
    text_start: u64,
    text_len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let name = unsafe { borrowed_text(name_owner, name_start, name_len) };
    let text = unsafe { borrowed_text(text_owner, text_start, text_len) };
    let Ok(completion) = (unsafe {
        capability_call(
            ctx,
            Request::TestSnapshot {
                test: test.read(),
                name,
                text,
            },
        )
    }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Checked(recorded) = completion else {
        crate::fault("Test completion mismatch");
    };
    if recorded {
        crate::STATUS_OK
    } else {
        crate::STATUS_FAILURE
    }
}

/// # Safety
/// Test and entries are borrowed; entries is a live List[(Path, Bytes)] block.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_dir(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    test: *const u64,
    entries: *mut crate::Header,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let payload = crate::alloc::payload(entries);
    let count = unsafe { payload.cast::<u64>().read() };
    let count = usize::try_from(count).unwrap_or_else(|_| crate::fault("Test entries overflow"));
    let first = crate::alloc::sequence_elements_offset(8) as usize;
    let mut files = Vec::with_capacity(count);
    for index in 0..count {
        let offset = index
            .checked_mul(32)
            .and_then(|n| n.checked_add(first))
            .unwrap_or_else(|| crate::fault("Test entry offset overflow"));
        let entry = unsafe { payload.add(offset) };
        let path = crate::paths::names(ctx)
            .path_text(unsafe { entry.cast::<u64>().read() })
            .to_owned();
        let slice = unsafe { entry.add(8).cast::<crate::views::Slice>().read() };
        let bytes = unsafe { slice_bytes(slice.owner, slice.start, slice.length) }.to_vec();
        files.push((path, bytes));
    }
    let Ok(completion) = (unsafe {
        capability_call(
            ctx,
            Request::TestDir {
                test: test.read(),
                entries: files,
            },
        )
    }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Handle(Ok(value)) = completion else {
        crate::fault("Test completion mismatch");
    };
    unsafe { out.write(value) };
    crate::STATUS_OK
}

/// Result[Bytes, FileError]: the payload is a Slice or the first eight error bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BytesResult {
    pub tag: u32,
    pub padding: u32,
    pub value: crate::views::Slice,
}
/// # Safety
/// Context is bound, directory/Path borrowed, and output writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_read_bytes(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut BytesResult,
    directory: *const u64,
    path: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::Read {
        directory: unsafe { directory.read() },
        path: crate::paths::names(ctx)
            .path_text(unsafe { path.read() })
            .to_owned(),
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe {
        crate::tasks::free::host::native::convert_result(ctx, completion, |completion| {
            let Completion::Bytes(value) = completion else {
                crate::fault("Io completion type mismatch");
            };
            // Initialize padding and inactive bytes without manufacturing a pointer from error bits.
            out.cast::<u8>()
                .write_bytes(0, std::mem::size_of::<BytesResult>());
            match value {
                Ok(bytes) => (*out).value = crate::text::allocate_bytes(ctx, bytes),
                Err(error) => {
                    (*out).tag = 1;
                    std::ptr::copy_nonoverlapping(
                        error_word(error.clone()).to_le_bytes().as_ptr(),
                        std::ptr::addr_of_mut!((*out).value).cast::<u8>(),
                        8,
                    );
                }
            }
        })
    };
    crate::STATUS_OK
}

/// Iter's three words. The environment is counted; the unused drop word is null.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WalkIterator {
    pub environment: *mut crate::Header,
    pub code: *const std::ffi::c_void,
    pub drop: *const std::ffi::c_void,
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WalkResult {
    pub tag: u32,
    pub padding: u32,
    pub value: WalkIterator,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WalkOption {
    pub tag: u32,
    pub padding: u32,
    pub value: HandleResult,
}
/// Private runtime-owned cursor environment. The public Iter ABI is unchanged;
/// its counted environment owns the sorted directory pages until drop.
pub(crate) struct StreamingWalk {
    pub(crate) directory: u64,
    pub(crate) walk: hirsel_kernels::walk::Walk,
    pub(crate) recursive: bool,
}
unsafe extern "C" fn drop_walk(
    _ctx: *mut crate::TaskCtx,
    value: *mut u8,
    _anchor: u64,
    _site: u64,
) -> u8 {
    // SAFETY: the constructor installs this owned pointer; Iter is affine,
    // so the environment has exactly one final release.
    unsafe {
        drop(Box::from_raw(value.cast::<*mut StreamingWalk>().read()));
    }
    crate::STATUS_OK
}
fn walk_entries(entries: &[DirEntry]) -> Vec<hirsel_kernels::walk::Entry> {
    entries
        .iter()
        .map(|e| hirsel_kernels::walk::Entry {
            path: e.path.clone(),
            kind: e.kind as u32,
        })
        .collect()
}
#[repr(transparent)]
pub struct WalkDescriptor(pub crate::Layout);
// SAFETY: this immutable descriptor has no steps or mutable pointees.
unsafe impl Sync for WalkDescriptor {}
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static hirsel_rt_v2_file_walk_descriptor: WalkDescriptor = WalkDescriptor(crate::Layout {
    magic: crate::LAYOUT_MAGIC,
    version: crate::LAYOUT_VERSION,
    size: 8,
    align: 8,
    finalizer: Some(drop_walk),
    steps: std::ptr::null(),
    step_count: 0,
    field_count: 0,
    flags: crate::LAYOUT_DYNAMIC | crate::LAYOUT_IMPLEMENTATION_FINALIZER,
    logical: crate::LOGICAL_UNSUPPORTED,
    logical_count: std::ptr::null(),
    scan: std::ptr::null(),
    scan_words: 0,
});
/// # Safety
/// Context bound; directory borrowed and result writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_walk(
    ctx: *mut crate::TaskCtx,
    destination: *mut u8,
    out: *mut WalkResult,
    directory: *const u64,
) -> u8 {
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    unsafe { directory_iterator(ctx, destination, out, directory.read(), String::new(), true) }
}
unsafe fn directory_iterator(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut WalkResult,
    directory: u64,
    path: String,
    recursive: bool,
) -> u8 {
    let Ok(completion) = (unsafe {
        capability_call(
            ctx,
            Request::ReadDirPage {
                directory,
                path: path.clone(),
                after: String::new(),
                recursive,
            },
        )
    }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe {
        crate::tasks::free::host::native::convert_result(ctx, completion, |completion| {
            let Completion::Entries(value) = completion else {
                crate::fault("directory root completion mismatch")
            };
            out.cast::<u8>()
                .write_bytes(0, std::mem::size_of::<WalkResult>());
            match value {
                Ok(entries) => {
                    let mut walk = hirsel_kernels::walk::Walk::default();
                    walk.push_page(&path, walk_entries(entries), recursive);
                    let environment =
                        crate::alloc::hirsel_rt_v2_alloc(ctx, 1, 8, 8, crate::DOMAIN_HEAP, 0);
                    crate::alloc::payload(environment)
                        .cast::<*mut StreamingWalk>()
                        .write(Box::into_raw(Box::new(StreamingWalk {
                            directory,
                            walk,
                            recursive,
                        })));
                    (*out).value = WalkIterator {
                        environment,
                        code: if recursive {
                            hirsel_rt_v2_file_walk_step as *const std::ffi::c_void
                        } else {
                            hirsel_rt_v2_file_read_dir_step as *const std::ffi::c_void
                        },
                        drop: std::ptr::null(),
                    };
                }
                Err(error) => {
                    (*out).tag = 1;
                    std::ptr::addr_of_mut!((*out).value)
                        .cast::<u64>()
                        .write(error_word(error.clone()));
                }
            }
        });
    }
    crate::STATUS_OK
}
unsafe fn directory_next(
    ctx: *mut crate::TaskCtx,
    environment: *mut crate::Header,
) -> Result<Option<Result<hirsel_kernels::walk::Entry, IoError>>, ()> {
    let state = unsafe {
        crate::alloc::payload(environment)
            .cast::<*mut StreamingWalk>()
            .read()
    };
    while let Some(entry) = unsafe { (*state).walk.next_entry() } {
        if entry.kind == 4 || (entry.kind == 1 && unsafe { (*state).recursive }) {
            let (path, after) = if entry.kind == 4 {
                entry.path.split_once('\0').expect("page cursor")
            } else {
                (entry.path.as_str(), "")
            };
            let Ok(completion) = (unsafe {
                capability_call(
                    ctx,
                    Request::ReadDirPage {
                        directory: (*state).directory,
                        path: path.into(),
                        after: after.into(),
                        recursive: (*state).recursive,
                    },
                )
            }) else {
                return Err(());
            };
            let mut failure = None;
            unsafe {
                crate::tasks::free::host::native::convert_result(ctx, completion, |completion| {
                    let Completion::Entries(entries) = completion else {
                        crate::fault("directory page completion mismatch")
                    };
                    match entries {
                        Ok(entries) => {
                            (*state)
                                .walk
                                .push_page(path, walk_entries(entries), (*state).recursive)
                        }
                        Err(error) => {
                            (*state).walk.stop();
                            failure = Some(error.clone());
                        }
                    }
                });
            }
            if let Some(error) = failure {
                return Ok(Some(Err(error)));
            }
        } else if entry.kind <= 3 {
            if entry.kind == 3 && unsafe { (*state).recursive } {
                unsafe {
                    (*state).walk.stop();
                }
                return Ok(Some(Err(IoError::Fault(1))));
            }
            return Ok(Some(Ok(entry)));
        } else {
            crate::fault("invalid directory cursor");
        }
    }
    Ok(None)
}
/// # Safety
/// Live exclusive iterator environment; out is writable Option[Result[Path,FileError]].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_walk_step(
    ctx: *mut crate::TaskCtx,
    environment: *mut crate::Header,
    _caller_site: u64,
    _operation_site: u64,
    _destination: *mut u8,
    out: *mut WalkOption,
) -> u8 {
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let Ok(item) = (unsafe { directory_next(ctx, environment) }) else {
        return crate::STATUS_FAILURE;
    };
    let mut result = WalkOption::default();
    if let Some(item) = item {
        result.tag = 1;
        result.value =
            HandleResult::of(item.map(|e| crate::paths::names(ctx).paths.intern(&e.path)));
    }
    unsafe {
        out.write(result);
    }
    crate::STATUS_OK
}
#[repr(C)]
#[derive(Default)]
pub struct DirEntryOption {
    pub tag: u32,
    pub padding: u32,
    pub result: u64,
    pub path: u64,
    pub kind: u64,
}
/// # Safety
/// Live exclusive iterator environment; out is writable Option[Result[DirEntry,FileError]].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_read_dir_step(
    ctx: *mut crate::TaskCtx,
    environment: *mut crate::Header,
    _caller_site: u64,
    _operation_site: u64,
    _destination: *mut u8,
    out: *mut DirEntryOption,
) -> u8 {
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let Ok(item) = (unsafe { directory_next(ctx, environment) }) else {
        return crate::STATUS_FAILURE;
    };
    let mut result = DirEntryOption::default();
    match item {
        Some(Ok(e)) => {
            result.tag = 1;
            result.path = crate::paths::names(ctx).paths.intern(&e.path);
            result.kind = e.kind as u64;
        }
        Some(Err(e)) => {
            result.tag = 1;
            result.result = 1;
            result.path = error_word(e);
        }
        None => (),
    }
    unsafe {
        out.write(result);
    }
    crate::STATUS_OK
}

/// Structural Path order, lexicographically over its exact UTF-8 text.
/// # Safety
/// The context is bound and both IDs were interned in it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_path_compare(
    ctx: *mut crate::TaskCtx,
    a: u64,
    b: u64,
) -> i32 {
    use std::cmp::Ordering;
    let names = crate::paths::names(ctx);
    match names
        .path_text(a)
        .as_bytes()
        .cmp(names.path_text(b).as_bytes())
    {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// # Safety
/// Context is bound; Stdin and list addresses are live, list exclusive; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_stdin_read(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    stdin: *const u64,
    buffer: *mut *mut crate::Header,
    max: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::ReadStdin {
        stdin: unsafe { stdin.read() },
        max,
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe {
        crate::tasks::free::host::native::convert_result(ctx, completion, |completion| {
            let Completion::Input(value) = completion else {
                crate::fault("stdin read completion mismatch");
            };
            let result = input_result(ctx, buffer, value);
            out.write(result);
        })
    };
    crate::STATUS_OK
}

/// Convert the retained owned input once, using the existing list growth policy.
/// # Safety
/// The buffer slot is an exclusive live List[u8] address for this conversion.
pub(crate) unsafe fn input_result(
    ctx: *mut crate::TaskCtx,
    buffer: *mut *mut crate::Header,
    value: &Result<Vec<u8>, InputError>,
) -> HandleResult {
    match value {
        Ok(bytes) => {
            unsafe { append_byte_list(ctx, buffer, bytes) };
            HandleResult {
                tag: 0,
                padding: 0,
                value: bytes.len() as u64,
            }
        }
        Err(error) => {
            let (tag, payload) = match error {
                InputError::Fault(code) => (0u32, *code),
                InputError::Canceled(cause) => (1, *cause),
                InputError::InvalidInput => (2, 0),
                InputError::InvalidUtf8 => (3, 0),
            };
            HandleResult {
                tag: 1,
                padding: 0,
                value: u64::from(tag) | u64::from(payload) << 32,
            }
        }
    }
}

/// Result[Option[String], EnvError]: the tag, then at 8 either the Option, a String
/// whose null owner word is `None` (its niche), or the EnvError ordinal; 32 bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EnvResult {
    pub tag: u32,
    pub padding: u32,
    pub value: crate::views::Slice,
}
/// # Safety
/// Context and borrowed Env/String are live; out is writable nested Result storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_env_get(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut EnvResult,
    env: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let data = crate::alloc::payload(owner)
        .wrapping_add(8)
        .wrapping_add(start as usize);
    let name = unsafe { native_text(data, len) }.to_owned();
    let Ok(completion) = (unsafe {
        capability_call(
            ctx,
            Request::EnvGet {
                env: env.read(),
                name,
            },
        )
    }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe {
        crate::tasks::free::host::native::convert_result(ctx, completion, |completion| {
            let Completion::Env(value) = completion else {
                crate::fault("Env completion mismatch")
            };
            // Zero bytes are `Ok(None)`; an error ordinal never becomes a pointer value.
            out.cast::<u8>()
                .write_bytes(0, std::mem::size_of::<EnvResult>());
            match value {
                Ok(Some(text)) => {
                    (*out).value = crate::text::allocate_bytes(ctx, text.as_bytes());
                }
                Ok(None) => {}
                Err(error) => {
                    (*out).tag = 1;
                    std::ptr::addr_of_mut!((*out).value)
                        .cast::<u32>()
                        .write(match error {
                            EnvError::NotGranted => 0,
                            EnvError::InvalidUtf8 => 1,
                        });
                }
            }
        })
    };
    crate::STATUS_OK
}

/// `env.narrow(e: Env, names: List[String]) -> Result[Env, EnvError]`; each name is a
/// 24-byte String slice.
/// # Safety
/// Context bound; env readable; names a live List[String] block; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_env_narrow(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut GrantResult,
    env: *const u64,
    names: *mut crate::Header,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::EnvNarrow {
        env: unsafe { env.read() },
        names: unsafe { crate::process::list(names, 24, |at| crate::process::text(at)) },
    };
    let Ok(Completion::Handle(value)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    // EnvError's NotGranted() and GrantError's share ordinal 0.
    unsafe { out.write(GrantResult::of(value)) };
    crate::STATUS_OK
}

/// Append initialized bytes to an exclusive physical List slot through ordinary COW growth.
/// A tracked arena List grows within its arena, uncharged, and stays as shared as it was: a
/// read appends no element MEM-6 charges.
/// # Safety
/// Context is live; buffer is an exclusive address of a live List[u8].
#[inline]
pub(crate) unsafe fn append_byte_list(
    ctx: *mut crate::TaskCtx,
    buffer: *mut *mut crate::Header,
    bytes: &[u8],
) {
    if !bytes.is_empty() {
        unsafe {
            let (block, count) = reserve_byte_list(ctx, buffer, bytes.len() as u64);
            let payload = crate::alloc::payload(block);
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                payload.add(crate::alloc::sequence_elements_offset(1) as usize + count as usize),
                bytes.len(),
            );
            payload.cast::<u64>().write(count + bytes.len() as u64);
        }
    }
}

/// Shared byte-list growth policy for borrowed appends and LZ history copies.
/// # Safety
/// As `append_byte_list`; additional is nonzero and the total fits the extent.
#[inline(always)]
pub(crate) unsafe fn reserve_byte_list(
    ctx: *mut crate::TaskCtx,
    buffer: *mut *mut crate::Header,
    additional: u64,
) -> (*mut crate::Header, u64) {
    unsafe {
        let block = buffer.read();
        let count = crate::alloc::payload(block).cast::<u64>().read();
        let total = count
            .checked_add(additional)
            .unwrap_or_else(|| crate::fault("List[u8] length overflow"));
        // Count one excludes tracked arenas and immortal roots. Keep the
        // ordinary reserve/copy/publish path together (Rust Vec::append_elements,
        // alloc/src/vec/mod.rs:2995-3031, f6c1107f8be1; MIT/Apache-2.0).
        if (*block).count == 1 {
            let capacity = crate::alloc::payload(block)
                .add(crate::alloc::SEQUENCE_CAPACITY_OFFSET as usize)
                .cast::<u64>()
                .read();
            if capacity >= total {
                return (block, count);
            }
        }
        (
            reserve_byte_list_slow(ctx, buffer, block, count, total),
            count,
        )
    }
}

/// Preserve tracked-arena, shared-root and growth policy off the common path.
/// # Safety
/// As `reserve_byte_list`; block and count are the current buffer and length.
#[cold]
#[inline(never)]
unsafe fn reserve_byte_list_slow(
    ctx: *mut crate::TaskCtx,
    buffer: *mut *mut crate::Header,
    block: *mut crate::Header,
    count: u64,
    total: u64,
) -> *mut crate::Header {
    unsafe {
        let block = if crate::region::is_tracked(block) {
            let capacity = crate::alloc::payload(block)
                .add(crate::alloc::SEQUENCE_CAPACITY_OFFSET as usize)
                .cast::<u64>()
                .read();
            let shared = (*block).count == crate::COUNT_ARENA_SHARED;
            if shared || capacity < total {
                let slots = total.max(count.saturating_mul(2)).max(4);
                let count = (*block).count;
                crate::region::tracked_move(ctx, buffer, slots, 1, 1, count)
            } else {
                block
            }
        } else {
            let capacity = crate::alloc::payload(block)
                .add(crate::alloc::SEQUENCE_CAPACITY_OFFSET as usize)
                .cast::<u64>()
                .read();
            if (*block).count == 1 && capacity >= total {
                block
            } else {
                let slots = if capacity < total {
                    total.max(count.saturating_mul(2)).max(4)
                } else {
                    total
                };
                crate::alloc::hirsel_rt_v2_seq_grow(ctx, block, 1, 1, slots)
            }
        };
        buffer.write(block);
        block
    }
}

path_export!(
    hirsel_rt_v2_file_mkdir,
    Mkdir,
    directory,
    Unit,
    UnitResult,
    UnitResult::of
);
path_export!(
    hirsel_rt_v2_file_remove_dir,
    RemoveDir,
    directory,
    Unit,
    UnitResult,
    UnitResult::of
);

path_export!(
    hirsel_rt_v2_dir_remove_all,
    RemoveAll,
    directory,
    Unit,
    UnitResult,
    UnitResult::of
);

/// # Safety
/// Context bound, directory and path borrowed, out writable for UnitResult.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_set_permissions(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    path: *const u64,
    permissions: u32,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::SetPermissions {
        directory: unsafe { directory.read() },
        path: crate::paths::names(ctx)
            .path_text(unsafe { path.read() })
            .to_owned(),
        permissions,
    };
    let Ok(Completion::Unit(result)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(UnitResult::of(result)) };
    crate::STATUS_OK
}

/// # Safety
/// The bound context and borrowed handles/paths are live; out is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_rename(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    from: *const u64,
    to: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::Rename {
        directory: unsafe { directory.read() },
        from: crate::paths::names(ctx)
            .path_text(unsafe { from.read() })
            .to_owned(),
        to: crate::paths::names(ctx)
            .path_text(unsafe { to.read() })
            .to_owned(),
    };
    let Ok(Completion::Unit(result)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    unsafe { out.write(UnitResult::of(result)) };
    crate::STATUS_OK
}

/// Metadata result: tag at 0, Metadata at 16, Option[i128] at 32.
#[repr(C, align(16))]
pub struct MetadataResult {
    pub words: [u64; 10],
}
/// # Safety
/// Context bound; directory/Path borrowed and out writable for 80 aligned bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_metadata(
    ctx: *mut crate::TaskCtx,
    _destination: *mut u8,
    out: *mut MetadataResult,
    directory: *const u64,
    path: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::Metadata {
        directory: unsafe { directory.read() },
        path: crate::paths::names(ctx)
            .path_text(unsafe { path.read() })
            .to_owned(),
    };
    let Ok(Completion::Metadata(result)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let mut words = [0; 10];
    match result {
        Ok(meta) => {
            words[2] = meta.kind as u64;
            words[3] = meta.size;
            words[8] = u64::from(meta.permissions);
            if let Some(ns) = meta.modified_unix_ns {
                words[4] = 1;
                words[6] = ns as u64;
                words[7] = (ns >> 64) as u64;
            }
        }
        Err(error) => {
            words[0] = 1;
            words[2] = error_word(error);
        }
    }
    unsafe { out.write(MetadataResult { words }) };
    crate::STATUS_OK
}
/// # Safety
/// Context bound, directory and path borrowed, out writable for Result[Iter[Result[DirEntry,FileError]],FileError].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_read_dir(
    ctx: *mut crate::TaskCtx,
    destination: *mut u8,
    out: *mut WalkResult,
    directory: *const u64,
    path: *const u64,
) -> u8 {
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let p = crate::paths::names(ctx)
        .path_text(unsafe { path.read() })
        .to_owned();
    unsafe { directory_iterator(ctx, destination, out, directory.read(), p, false) }
}

#[cfg(test)]
mod filesystem_authority_tests {
    use super::*;
    struct DeniedStorage;
    impl Storage for DeniedStorage {
        fn activate(&mut self, _: u64, _: &str) {
            panic!("denied operation activated storage")
        }
        fn file_fits(&mut self, _: u64, _: &str) -> bool {
            unreachable!()
        }
        fn open_file(&mut self, _: &str, _: u64) -> Result<(), IoError> {
            unreachable!()
        }
        fn open_append_file(&mut self, _: &str, _: u64) -> Result<(), IoError> {
            unreachable!()
        }
        fn create_file(&mut self, _: &str, _: u64) -> Result<(), IoError> {
            unreachable!()
        }
        fn read_file(&mut self, _: u64, _: u64, _: u64) -> Result<Vec<u8>, IoError> {
            unreachable!()
        }
        fn write_file(&mut self, _: u64, _: &[u8], _: bool) -> Result<u64, IoError> {
            unreachable!()
        }
        fn directory(&mut self, _: &str, _: u64) -> Result<(), IoError> {
            unreachable!()
        }
        fn read(&mut self, _: &str) -> Result<Vec<u8>, IoError> {
            unreachable!()
        }
        fn write(&mut self, _: &str, _: &[u8], _: bool, _: bool) -> Result<(), IoError> {
            unreachable!()
        }
        fn remove(&mut self, _: &str) -> Result<(), IoError> {
            unreachable!()
        }
        fn rename(&mut self, _: &str, _: &str) -> Result<(), IoError> {
            unreachable!()
        }
        fn move_entry(&mut self, _: &str, _: &str) -> Result<(), IoError> {
            unreachable!()
        }
        fn mkdir(&mut self, _: &str) -> Result<(), IoError> {
            unreachable!()
        }
        fn remove_dir(&mut self, _: &str) -> Result<(), IoError> {
            unreachable!()
        }
        fn set_permissions(&mut self, _: &str, _: u32) -> Result<(), IoError> {
            unreachable!()
        }
        fn sync_file(&mut self, _: u64, _: &str) -> Result<(), IoError> {
            unreachable!()
        }
        fn put_new(&mut self, _: &str, _: &[u8], _: bool) -> Result<(), IoError> {
            unreachable!()
        }
        fn remove_all(&mut self, _: &str, _: Option<usize>) -> Result<(), IoError> {
            unreachable!()
        }
        fn map_file(&mut self, _: &str, _: u64) -> Result<u64, IoError> {
            unreachable!()
        }
        fn map_view(&mut self, _: u64, _: u64, _: bool) -> MapView {
            unreachable!()
        }
        fn map_truncated(&mut self, _: u64, _: u64) -> bool {
            unreachable!()
        }
        fn unmap(&mut self, _: u64) {
            unreachable!()
        }
        fn watch(&mut self, _: &str, _: u64, _: bool) -> Result<(), IoError> {
            unreachable!()
        }
        fn watch_next(
            &mut self,
            _: u64,
            _: &str,
        ) -> Result<Option<crate::watch::WatchEvent>, IoError> {
            unreachable!()
        }
        fn unwatch(&mut self, _: u64) {
            unreachable!()
        }
        fn metadata(&mut self, _: &str) -> Result<Metadata, IoError> {
            unreachable!()
        }
        fn read_dir(&mut self, _: &str) -> Result<Vec<DirEntry>, IoError> {
            unreachable!()
        }
    }
}
