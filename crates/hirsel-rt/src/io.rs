//! Typed request/completion protocol. Expected I/O errors are data, never language failures.
use crate::handles::{Grant, Handles};
use crate::net::is_net_request;
pub use crate::net::{
    NetError, NetOperation, NetProvider, NetRule, NetTimeout, NetWorld, RootsDigest, ShutdownHow,
    SocketAddr, TimeoutKind, TlsFailure,
};
use hirsel_run_config::confined_relative;
use hirsel_run_config::{Access, RunRefusal};
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
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Request {
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
    TestProcess {
        test: u64,
        runs: Vec<hirsel_run_config::ProcessRun>,
        children: Vec<hirsel_run_config::ChildRun>,
    },
    /// `proc.narrow`: a Proc of `rules`, each within a rule of `process`, in its world.
    ProcessNarrow {
        process: u64,
        rules: Vec<hirsel_run_config::ProcessRule>,
    },
    /// `proc.run`.
    ProcessRun {
        process: u64,
        command: hirsel_run_config::Command,
        stdin: Vec<u8>,
        max_output: u64,
        deadline: Option<crate::process::Deadline>,
    },
    /// `proc.pipeline`.
    ProcessPipeline {
        process: u64,
        stages: Vec<hirsel_run_config::Command>,
        stdin: Vec<u8>,
        max_output: u64,
        deadline: Option<crate::process::Deadline>,
    },
    /// `proc.spawn`.
    ProcessSpawn {
        process: u64,
        command: hirsel_run_config::Command,
    },
    /// `proc.spawn_pipeline`.
    ProcessSpawnPipeline {
        process: u64,
        stages: Vec<hirsel_run_config::Command>,
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
        stream: hirsel_run_config::ChildStream,
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
        signal: hirsel_run_config::Signal,
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
        peers: Vec<hirsel_run_config::NetPeer>,
        listeners: Vec<hirsel_run_config::NetListener>,
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
    Walk {
        directory: u64,
    },
    ReadDir {
        directory: u64,
        path: String,
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
    TlsConnect {
        net: u64,
        stream: u64,
        server_name: String,
        roots: RootsDigest,
        /// `tls.connect_as`: the SHA-256 of the client identity's certificate-chain PEM; the
        /// key never appears.
        identity: Option<[u8; 32]>,
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
    Input(Result<Vec<u8>, InputError>),
    Env(Result<Option<String>, EnvError>),
    ClockGrant(Result<u64, ()>),
    Monotonic(Result<u64, ClockError>),
    Wall(Result<i64, ClockError>),
    Random(Result<Vec<u8>, RandomError>),
    /// A completed timer wait.
    Wait(Result<(), WaitError>),
    /// Deterministic capability refusal; the operation issued no host effect.
    Refused(u64),
    Checked(bool),
    Handle(Result<u64, IoError>),
    OptionalFile(Option<u64>),
    Unit(Result<(), IoError>),
    Size(Result<u64, IoError>),
    Bytes(Result<Vec<u8>, IoError>),
    Paths(Result<Vec<String>, IoError>),
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
    Address(Result<SocketAddr, NetError>),
    Accepted(Result<(u64, SocketAddr), NetError>),
    Process(hirsel_run_config::ProcessOutcome),
    /// `proc.pipeline`.
    Pipeline(hirsel_run_config::PipelineOutcome),
    /// `proc.spawn` and `proc.spawn_pipeline`: the Child or ChildGroup handle.
    Spawned(Result<u64, hirsel_run_config::ProcessFailure>),
    /// `child.write`, `child.close_stdin` and `child.kill`.
    ChildUnit(Result<(), hirsel_run_config::ProcessFailure>),
    /// A child read: the bytes it delivered, empty at end of stream.
    ChildBytes(Result<Vec<u8>, hirsel_run_config::ProcessFailure>),
    ChildExit(Result<hirsel_run_config::ExitStatus, hirsel_run_config::ProcessFailure>),
    /// `proc.wait_group`.
    GroupExit(Result<hirsel_run_config::GroupOutput, hirsel_run_config::ProcessFailure>),
    /// The request would block and its task parked; `committed` when it already made part
    /// of its effect, so cancellation no longer completes it.
    Blocked {
        committed: bool,
    },
    /// The completion of a scheduling input, `Request::Ready` or `Request::Schedule`.
    Ready,
}
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
    fn read_file(&mut self, id: u64, offset: u64, max: u64) -> Result<Vec<u8>, IoError>;
    fn write_file(&mut self, id: u64, bytes: &[u8], all: bool) -> Result<u64, IoError>;
    fn transfer_file(&mut self, _from: u64, _to: Option<u64>) {}
    fn directory(&mut self, path: &str, id: u64) -> Result<(), IoError>;
    fn read(&mut self, path: &str) -> Result<Vec<u8>, IoError>;
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
    fn metadata(&mut self, path: &str) -> Result<Metadata, IoError>;
    fn read_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, IoError>;
    fn walk(&mut self, root: &str) -> Result<Vec<String>, IoError>;
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
    /// The admitted rules of the root Proc and the world serving it.
    pub process_rules: Vec<hirsel_run_config::ProcessRule>,
    pub process_world: crate::process::ProcessWorld,
    /// Unused entries of each scripted world, keyed by world.
    pub process_scripts: BTreeMap<u64, Vec<Option<hirsel_run_config::ProcessRun>>>,
    /// Unused streaming-child entries of each scripted world, keyed by world.
    pub child_scripts: BTreeMap<u64, Vec<Option<hirsel_run_config::ChildRun>>>,
    /// Spawned scripted children, keyed by Child handle.
    pub script_children: BTreeMap<u64, hirsel_run_config::ScriptChild>,
    /// Spawned scripted pipelines, keyed by ChildGroup handle.
    pub script_groups: BTreeMap<u64, hirsel_run_config::ScriptGroup>,
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
            process_rules: vec![],
            process_world: crate::process::ProcessWorld::Host,
            process_scripts: BTreeMap::new(),
            child_scripts: BTreeMap::new(),
            script_children: BTreeMap::new(),
            script_groups: BTreeMap::new(),
            projections: [
                "stdin", "stderr", "stdout", "tasks", "net", "process", "random", "dir",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}
impl State {
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
    pub(crate) fn is_fake_request(&self, request: &Request) -> bool {
        self.fake_tree(request).is_some()
    }
    fn fake_tree(&self, request: &Request) -> Option<u64> {
        let directory = match request {
            Request::NarrowDir { directory, .. }
            | Request::ReadOnlyDir { directory }
            | Request::Open { directory, .. }
            | Request::FileRead { directory, .. }
            | Request::FileSeek { directory, .. }
            | Request::NarrowFile { directory, .. }
            | Request::Walk { directory }
            | Request::Size { directory, .. }
            | Request::ReadDir { directory, .. }
            | Request::Metadata { directory, .. }
            | Request::Read { directory, .. }
            | Request::Write { directory, .. }
            | Request::Remove { directory, .. }
            | Request::Mkdir { directory, .. }
            | Request::Rename { directory, .. }
            | Request::RemoveDir { directory, .. }
            | Request::RemoveAll { directory, .. }
            | Request::SetPermissions { directory, .. }
            | Request::Create { directory, .. }
            | Request::FileWrite { directory, .. }
            | Request::FileAppend { directory, .. }
            | Request::CreateAtomic { directory, .. }
            | Request::WriteAll { directory, .. }
            | Request::Commit { directory, .. }
            | Request::Sync { directory, .. }
            | Request::PutNew { directory, .. } => *directory,
            Request::Close { handle } => match self.handles.rows.get(handle) {
                Some(Grant::File { directory, .. } | Grant::AtomicFile { directory, .. }) => {
                    *directory
                }
                _ => return None,
            },
            _ => return None,
        };
        self.fake_directories.get(&directory).copied()
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
                Some(Grant::Stream { .. } | Grant::Listener { .. } | Grant::UdpSocket { .. })
            ))
        {
            // Network outcomes are provider inputs, never injected file faults.
            if fault.is_some() {
                return Err(AdapterError::InvalidCompletion);
            }
            return self.execute_net(net, request);
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
            Request::ReadDir { .. }
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
                Request::NarrowDir { .. } | Request::ReadOnlyDir { .. }
            ) && let Ok(Completion::Handle(Ok(id))) = &result
            {
                self.fake_directories.insert(*id, tree_id);
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
            R::ReadStdin { .. } => Err(AdapterError::InvalidCompletion),
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
                        Grant::File { directory, .. } | Grant::AtomicFile { directory, .. }
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
            R::Walk { directory } => {
                let root = self.directory(*directory)?;
                storage.activate(*directory, &root);
                let result = error.map_or_else(|| storage.walk(&root), Err);
                Ok(C::Paths(result))
            }
            _ => {
                let (directory, path) = match request {
                    R::Open { directory, path }
                    | R::Create { directory, path }
                    | R::Size { directory, path }
                    | R::ReadDir { directory, path }
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
                    } => (*directory, path),
                    _ => unreachable!(),
                };
                let root = self.directory(directory)?;
                storage.activate(directory, &root);
                let path = confined_relative(&root, path).ok_or(IoError::Fault(1));
                match request {
                    R::Open { .. } | R::Create { .. } => Ok(C::Handle(if let Some(e) = error {
                        Err(e)
                    } else {
                        path.and_then(|path| {
                            let writable = matches!(request, R::Create { .. });
                            if writable {
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
                    })),
                    R::ReadDir { .. } => Ok(C::Entries(if let Some(e) = error {
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
                            storage.read_dir(&p).map(|entries| {
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
                        path.and_then(|p| storage.read(&p).map(|b| b.len() as u64))
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
pub trait Adapter {
    /// Check argument validity and authority without an effect or tape event.
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError>;
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
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError>;
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
    /// A seeded scheduler's choice among runnable candidates; replay supplies the recorded choice.
    fn schedule(&mut self, _count: u64, choice: u64) -> Result<u64, AdapterError> {
        Ok(choice)
    }
    /// A scheduling point with a parked request: the requests to re-issue now. A
    /// cooperative host answers the tokens whose interest is ready, in token order; the
    /// default (a scripted world) re-issues every parked request.
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        Ok(Ready::Every)
    }
    /// With no runnable task, wait until some parked request may be ready. False when no
    /// source can make one ready.
    fn wait(&mut self) -> Result<bool, AdapterError> {
        Ok(false)
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

/// Stable address owned by launch, bound explicitly to RunCtx for generated thunks.
/// The binding outlives every release and cannot be accessed while a thunk runs.
pub struct Binding {
    pub(crate) adapter: Box<dyn Adapter>,
    paths: Vec<String>,
    arguments: Vec<String>,
    /// The requests parked because their adapter answered `Blocked`.
    pub(crate) waits: crate::host_waits::Waits,
}
impl Binding {
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
    ) {
        self.adapter.tls_server(stream, identities, clients);
    }
    pub fn new(adapter: impl Adapter + 'static) -> Self {
        Self {
            adapter: Box::new(adapter),
            paths: Vec::new(),
            arguments: Vec::new(),
            waits: Default::default(),
        }
    }
    pub(crate) fn from_box(adapter: Box<dyn Adapter>) -> Self {
        Self {
            adapter,
            paths: Vec::new(),
            arguments: Vec::new(),
            waits: Default::default(),
        }
    }
    /// Explicit launch argument table; Path identities belong to this binding only.
    pub fn with_arguments(mut self, arguments: Vec<String>) -> Self {
        self.arguments = arguments;
        self
    }
    pub fn intern_path(&mut self, path: &str) -> u64 {
        if let Some(index) = self.paths.iter().position(|p| p == path) {
            return index as u64 + 1;
        }
        self.paths.push(path.into());
        self.paths.len() as u64
    }
    fn path_text(&self, id: u64) -> &str {
        id.checked_sub(1)
            .and_then(|i| self.paths.get(i as usize))
            .map(String::as_str)
            .unwrap_or_else(|| crate::fault("unknown run-context Path identity"))
    }
    pub(crate) fn path(&self, id: u64) -> String {
        self.path_text(id).to_owned()
    }
    /// # Safety
    /// This binding remains at its current address until it is unbound, and outlives
    /// every release on ctx. Close may run only while bound; access is exclusive.
    pub unsafe fn bind(&mut self, ctx: &mut crate::RunCtx) {
        unsafe { ctx.bind_io_data((self as *mut Self).cast()) };
    }
    /// # Safety
    /// ctx is live and holds this adapter's Binding with exclusive access.
    pub(crate) unsafe fn get<'a>(ctx: *mut crate::RunCtx) -> &'a mut Self {
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
    ctx: *mut crate::RunCtx,
    handle: u64,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    let binding = unsafe { Binding::get(ctx) };
    match binding.adapter.call(Request::Close { handle }) {
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    s: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    let stdout = unsafe { s.read() };
    let start =
        usize::try_from(start).unwrap_or_else(|_| crate::fault("Io start exceeds target usize"));
    let data = crate::alloc::payload(owner)
        .wrapping_add(8)
        .wrapping_add(start);
    // String's validity was established when constructed; output needs only bytes.
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    s: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
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
    }
}
// Prints cannot park. Keep their borrowed bytes out of the owned wait protocol,
// while retaining the same cancellation checks and post-effect scheduling point.
unsafe fn native_print(
    ctx: *mut crate::RunCtx,
    handle: u64,
    stderr: bool,
    bytes: &[u8],
) -> Completion {
    if unsafe { crate::tasks::canceled(ctx) }.is_some() {
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
    let result = unsafe { Binding::get(ctx) }
        .adapter
        .print(handle, stderr, bytes);
    let completion = unsafe { crate::stop::check(ctx, result.map_err(crate::stop::Stop::refusal)) };
    unsafe { crate::tasks::yield_now(ctx) };
    completion
}

unsafe fn native_call(ctx: *mut crate::RunCtx, request: Request) -> Completion {
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
    if operation && !parked {
        unsafe { crate::tasks::yield_now(ctx) };
    }
    completion
}
/// A refused typed capability is CAP-2. The compiled caller records its anchor.
///
/// Every capability operation is a scheduling point (CONC-1): after its effect the running
/// task yields to the runnable queue. A request that parked yielded or parked there instead
/// (`crate::host_waits`). A handle's implicit close at its drop is not one.
pub(crate) unsafe fn capability_call(
    ctx: *mut crate::RunCtx,
    request: Request,
) -> Result<Completion, ()> {
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
    if operation && !parked {
        unsafe { crate::tasks::yield_now(ctx) };
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
            ctx: *mut crate::RunCtx,
            _destination: *mut u8,
            out: *mut $result,
            parent: *const u64,
            path: *const u64,
        ) -> u8 {
            let request = Request::$request {
                $field: unsafe { parent.read() },
                path: unsafe { Binding::get(ctx) }.path(unsafe { path.read() }),
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    directory: *const u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    directory: *const u64,
    file: *mut u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    file: *mut u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    directory: *const u64,
    file: *mut u64,
    buffer: *mut *mut crate::Header,
    max: u64,
) -> u8 {
    let request = Request::FileRead {
        directory: unsafe { directory.read() },
        file: unsafe { file.read() },
        max,
    };
    let Ok(Completion::Bytes(value)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let value = value.map(|bytes| {
        unsafe { append_byte_list(ctx, buffer, &bytes) };
        bytes.len() as u64
    });
    unsafe { out.write(HandleResult::of(value)) };
    crate::STATUS_OK
}
/// # Safety
/// Context bound, directory/file live, out writable; file is exclusively borrowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_seek(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    file: *mut u64,
    offset: u64,
) -> u8 {
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_write(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    directory: *const u64,
    file: *mut u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    file: *mut u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    path: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    let request = Request::Write {
        directory: unsafe { directory.read() },
        path: unsafe { Binding::get(ctx) }.path(unsafe { path.read() }),
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    file: *mut u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    path: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
    mode: *const u32,
) -> u8 {
    let durable = match unsafe { mode.read() } {
        0 => true,
        1 => false,
        _ => crate::fault("Durability tag out of range"),
    };
    let request = Request::PutNew {
        directory: unsafe { directory.read() },
        path: unsafe { Binding::get(ctx) }.path(unsafe { path.read() }),
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    file: *mut u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    let text = std::str::from_utf8(unsafe { slice_bytes(owner, start, len) })
        .unwrap_or_else(|_| crate::fault("String operand is not UTF-8"));
    let id = unsafe { Binding::get(ctx) }.intern_path(text);
    unsafe { out.write(id) };
    crate::STATUS_OK
}
/// Extract the original lexical spelling without a capability operation.
/// # Safety
/// The Path belongs to the bound context; output is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_path_text(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut crate::views::Slice,
    path: *const u64,
) -> u8 {
    let text = unsafe { Binding::get(ctx) }.path_text(unsafe { path.read() });
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    args: *const u64,
    index: u64,
) -> u8 {
    if unsafe { args.read() } != crate::launch::ARGS_HANDLE {
        return crate::STATUS_FAILURE;
    }
    let binding = unsafe { Binding::get(ctx) };
    let text = usize::try_from(index)
        .ok()
        .and_then(|i| binding.arguments.get(i))
        .cloned();
    let value = match text {
        Some(text) => HandleResult::of(Ok(binding.intern_path(&text))),
        None => HandleResult {
            tag: 1,
            padding: 0,
            value: 0,
        },
    };
    unsafe { out.write(value) };
    unsafe { crate::tasks::yield_now(ctx) };
    crate::STATUS_OK
}

/// # Safety
/// Args names the launch snapshot and out is writable scalar storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_arg_len(
    ctx: *mut crate::RunCtx,
    out: *mut u64,
    args: *const u64,
) -> u8 {
    if unsafe { args.read() } != crate::launch::ARGS_HANDLE {
        return crate::STATUS_FAILURE;
    }
    unsafe { out.write(Binding::get(ctx).arguments.len() as u64) };
    unsafe { crate::tasks::yield_now(ctx) };
    crate::STATUS_OK
}

/// # Safety
/// Args names the launch snapshot and out is writable Result[String, ArgError] storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_arg_text(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut crate::text::StringResult,
    args: *const u64,
    index: u64,
) -> u8 {
    if unsafe { args.read() } != crate::launch::ARGS_HANDLE {
        return crate::STATUS_FAILURE;
    }
    let text = usize::try_from(index)
        .ok()
        .and_then(|i| unsafe { Binding::get(ctx) }.arguments.get(i))
        .cloned();
    // Zero initializes the MissingArgument tag and all padding on the error arm.
    let mut result: crate::text::StringResult = unsafe { std::mem::zeroed() };
    match text {
        Some(text) => result.value = unsafe { crate::text::allocate_bytes(ctx, text.as_bytes()) },
        None => result.tag = 1,
    }
    unsafe { out.write(result) };
    unsafe { crate::tasks::yield_now(ctx) };
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
            ctx: *mut crate::RunCtx,
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
grant_projection_export!(hirsel_rt_v2_io_tasks, Tasks);
grant_projection_export!(hirsel_rt_v2_io_net, Net);
grant_projection_export!(hirsel_rt_v2_io_process, Process);

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
            ctx: *mut crate::RunCtx,
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
    _ctx: *mut crate::RunCtx,
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
/// # Safety
/// Context is exclusively bound, the clock and Instant words readable and the result
/// writable. Without a cooperative scheduler the thread blocks until the deadline.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_clock_sleep_until(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    clock: *const u64,
    deadline: *const u64,
) -> u8 {
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
    let result = match value {
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
    };
    unsafe { out.write(result) };
    crate::STATUS_OK
}
/// # Safety
/// Context is bound, the Random word readable and the result writable. A zero count
/// returns empty Bytes without a request, as the reference does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_random_bytes(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut BytesResult,
    random: *const u64,
    count: u64,
) -> u8 {
    let value = if count == 0 {
        Ok(vec![])
    } else {
        let request = Request::RandomBytes {
            random: unsafe { random.read() },
            count,
        };
        let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
            return crate::STATUS_FAILURE;
        };
        let Completion::Random(value) = completion else {
            crate::fault("random completion mismatch");
        };
        value
    };
    // Initialize padding and inactive bytes; RandomError has no payload.
    unsafe {
        out.cast::<u8>()
            .write_bytes(0, std::mem::size_of::<BytesResult>())
    };
    match value {
        Ok(bytes) => unsafe { (*out).value = crate::text::allocate_bytes(ctx, &bytes) },
        Err(RandomError::Unavailable) => unsafe { (*out).tag = 1 },
    }
    crate::STATUS_OK
}
/// # Safety
/// The sampled Instant word is readable and the scalar result writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_instant_ticks(
    _ctx: *mut crate::RunCtx,
    out: *mut u64,
    instant: *const u64,
) -> u8 {
    unsafe { out.write(instant.read()) };
    crate::STATUS_OK
}

/// # Safety
/// A live Test grant is borrowed. The boolean is the ABI's zero-extended byte.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_check(
    ctx: *mut crate::RunCtx,
    test: *const u64,
    condition: u8,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    test: *const u64,
    condition: u8,
    case: u64,
) -> u8 {
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
pub unsafe extern "C" fn hirsel_rt_v2_test_fail(ctx: *mut crate::RunCtx, test: *const u64) -> u8 {
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
    ctx: *mut crate::RunCtx,
    test: *const u64,
    directory: *const u64,
    seed: u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    out: *mut u64,
    test: *const u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    test: *const u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    test: *const u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    test: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    test: *const u64,
    name_owner: *mut crate::Header,
    name_start: u64,
    name_len: u64,
    text_owner: *mut crate::Header,
    text_start: u64,
    text_len: u64,
) -> u8 {
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    test: *const u64,
    entries: *mut crate::Header,
) -> u8 {
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
        let path = unsafe { Binding::get(ctx) }.path(unsafe { entry.cast::<u64>().read() });
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut BytesResult,
    directory: *const u64,
    path: *const u64,
) -> u8 {
    let request = Request::Read {
        directory: unsafe { directory.read() },
        path: unsafe { Binding::get(ctx) }.path(unsafe { path.read() }),
    };
    let Ok(completion) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Bytes(value) = completion else {
        crate::fault("Io completion type mismatch");
    };
    // Initialize padding and inactive bytes without manufacturing a pointer from error bits.
    unsafe {
        out.cast::<u8>()
            .write_bytes(0, std::mem::size_of::<BytesResult>())
    };
    match value {
        Ok(bytes) => unsafe { (*out).value = crate::text::allocate_bytes(ctx, &bytes) },
        Err(error) => unsafe {
            (*out).tag = 1;
            std::ptr::copy_nonoverlapping(
                error_word(error).to_le_bytes().as_ptr(),
                std::ptr::addr_of_mut!((*out).value).cast::<u8>(),
                8,
            )
        },
    }
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
/// Pointer-free runtime environment: position, count, inline Result[Path,FileError] items.
#[repr(transparent)]
pub struct WalkDescriptor(pub crate::Layout);
// SAFETY: this immutable descriptor has no steps or mutable pointees.
unsafe impl Sync for WalkDescriptor {}
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static hirsel_rt_v2_file_walk_descriptor: WalkDescriptor = WalkDescriptor(crate::Layout {
    magic: crate::LAYOUT_MAGIC,
    version: crate::LAYOUT_VERSION,
    size: 16,
    align: 8,
    finalizer: None,
    steps: std::ptr::null(),
    step_count: 0,
    field_count: 0,
    flags: crate::LAYOUT_DYNAMIC,
    logical: crate::LOGICAL_UNSUPPORTED,
    logical_count: std::ptr::null(),
});
/// # Safety
/// Context live; each item contains only a run-context Path or FileError.
pub unsafe fn walk_environment(
    ctx: *mut crate::RunCtx,
    items: &[HandleResult],
) -> *mut crate::Header {
    let size = (items.len() as u64)
        .checked_mul(16)
        .and_then(|n| n.checked_add(16))
        .unwrap_or_else(|| crate::fault("walk extent overflow"));
    let environment =
        unsafe { crate::alloc::hirsel_rt_v2_alloc(ctx, 1, size, 8, crate::DOMAIN_HEAP, 0) };
    unsafe {
        let payload = crate::alloc::payload(environment);
        payload.cast::<u64>().write(0);
        payload.add(8).cast::<u64>().write(items.len() as u64);
        std::ptr::copy_nonoverlapping(
            items.as_ptr().cast::<u8>(),
            payload.add(16),
            items.len() * 16,
        );
    }
    environment
}
/// # Safety
/// Context bound; directory borrowed and result writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_walk(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut WalkResult,
    directory: *const u64,
) -> u8 {
    let Ok(completion) = (unsafe {
        capability_call(
            ctx,
            Request::Walk {
                directory: directory.read(),
            },
        )
    }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Paths(value) = completion else {
        crate::fault("walk completion mismatch");
    };
    unsafe {
        out.cast::<u8>()
            .write_bytes(0, std::mem::size_of::<WalkResult>())
    };
    match value {
        Ok(mut paths) => {
            paths.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            let items: Vec<_> = paths
                .iter()
                .map(|p| HandleResult::of(Ok(unsafe { Binding::get(ctx) }.intern_path(p))))
                .collect();
            unsafe {
                (*out).value = WalkIterator {
                    code: hirsel_rt_v2_file_walk_step as *const std::ffi::c_void,
                    environment: walk_environment(ctx, &items),
                    drop: std::ptr::null(),
                }
            };
        }
        Err(error) => unsafe {
            (*out).tag = 1;
            std::ptr::copy_nonoverlapping(
                error_word(error).to_le_bytes().as_ptr(),
                std::ptr::addr_of_mut!((*out).value).cast::<u8>(),
                8,
            )
        },
    }
    crate::STATUS_OK
}
/// IterNext closure ABI: context, environment, the inert caller and operation
/// site words every closure and iterator callable carries, hidden destination,
/// output.
/// # Safety
/// The environment is an exclusive live walk environment and out is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_walk_step(
    _ctx: *mut crate::RunCtx,
    environment: *mut crate::Header,
    _caller_site: u64,
    _operation_site: u64,
    _destination: *mut u8,
    out: *mut WalkOption,
) -> u8 {
    let payload = crate::alloc::payload(environment);
    let position = unsafe { payload.cast::<u64>().read() };
    let count = unsafe { payload.add(8).cast::<u64>().read() };
    let mut result = WalkOption::default();
    if position < count {
        let offset = position
            .checked_mul(16)
            .and_then(|n| n.checked_add(16))
            .filter(|&n| n <= isize::MAX as u64)
            .unwrap_or_else(|| crate::fault("walk position overflow"));
        let value = unsafe { payload.add(offset as usize).cast::<HandleResult>().read() };
        result.tag = 1;
        result.value = value;
        unsafe {
            payload
                .cast::<u64>()
                .write(if value.tag == 1 { count } else { position + 1 })
        };
    }
    unsafe { out.write(result) };
    crate::STATUS_OK
}

/// Structural Path order, lexicographically over its exact UTF-8 text.
/// # Safety
/// The context is bound and both IDs were interned in it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_path_compare(ctx: *mut crate::RunCtx, a: u64, b: u64) -> i32 {
    use std::cmp::Ordering;
    let binding = unsafe { Binding::get(ctx) };
    match binding
        .path_text(a)
        .as_bytes()
        .cmp(binding.path_text(b).as_bytes())
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    stdin: *const u64,
    buffer: *mut *mut crate::Header,
    max: u64,
) -> u8 {
    let request = Request::ReadStdin {
        stdin: unsafe { stdin.read() },
        max,
    };
    let Ok(Completion::Input(value)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let result = match value {
        Ok(bytes) => {
            unsafe { append_byte_list(ctx, buffer, &bytes) };
            HandleResult {
                tag: 0,
                padding: 0,
                value: bytes.len() as u64,
            }
        }
        Err(error) => {
            let (tag, payload) = match error {
                InputError::Fault(code) => (0u32, code),
                InputError::Canceled(cause) => (1, cause),
                InputError::InvalidInput => (2, 0),
                InputError::InvalidUtf8 => (3, 0),
            };
            HandleResult {
                tag: 1,
                padding: 0,
                value: u64::from(tag) | u64::from(payload) << 32,
            }
        }
    };
    unsafe { out.write(result) };
    crate::STATUS_OK
}

/// Result[Option[String], EnvError]: nested Option starts at byte 8, String at byte 16.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EnvResult {
    pub tag: u32,
    pub padding: u32,
    pub option: u32,
    pub option_padding: u32,
    pub value: crate::views::Slice,
}
/// # Safety
/// Context and borrowed Env/String are live; out is writable nested Result storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_env_get(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut EnvResult,
    env: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    let data = crate::alloc::payload(owner)
        .wrapping_add(8)
        .wrapping_add(start as usize);
    let name = unsafe { native_text(data, len) }.to_owned();
    let Ok(Completion::Env(value)) = (unsafe {
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
    let mut result: EnvResult = unsafe { std::mem::zeroed() };
    match value {
        Ok(Some(text)) => {
            result.option = 1;
            result.value = unsafe { crate::text::allocate_bytes(ctx, text.as_bytes()) };
        }
        Ok(None) => {}
        Err(error) => {
            result.tag = 1;
            result.option = match error {
                EnvError::NotGranted => 0,
                EnvError::InvalidUtf8 => 1,
            };
        }
    }
    unsafe { out.write(result) };
    crate::STATUS_OK
}

/// `env.narrow(e: Env, names: List[String]) -> Result[Env, EnvError]`; each name is a
/// 24-byte String slice.
/// # Safety
/// Context bound; env readable; names a live List[String] block; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_env_narrow(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut GrantResult,
    env: *const u64,
    names: *mut crate::Header,
) -> u8 {
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
pub(crate) unsafe fn append_byte_list(
    ctx: *mut crate::RunCtx,
    buffer: *mut *mut crate::Header,
    bytes: &[u8],
) {
    if !bytes.is_empty() {
        unsafe {
            let block = buffer.read();
            let count = crate::alloc::payload(block).cast::<u64>().read();
            let total = count
                .checked_add(bytes.len() as u64)
                .unwrap_or_else(|| crate::fault("List[u8] length overflow"));
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
            let payload = crate::alloc::payload(block);
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                payload.add(crate::alloc::sequence_elements_offset(1) as usize + count as usize),
                bytes.len(),
            );
            payload.cast::<u64>().write(total);
            buffer.write(block);
        }
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    path: *const u64,
    permissions: u32,
) -> u8 {
    let request = Request::SetPermissions {
        directory: unsafe { directory.read() },
        path: unsafe { Binding::get(ctx) }.path(unsafe { path.read() }),
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut UnitResult,
    directory: *const u64,
    from: *const u64,
    to: *const u64,
) -> u8 {
    let request = Request::Rename {
        directory: unsafe { directory.read() },
        from: unsafe { Binding::get(ctx) }.path(unsafe { from.read() }),
        to: unsafe { Binding::get(ctx) }.path(unsafe { to.read() }),
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
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut MetadataResult,
    directory: *const u64,
    path: *const u64,
) -> u8 {
    let request = Request::Metadata {
        directory: unsafe { directory.read() },
        path: unsafe { Binding::get(ctx) }.path(unsafe { path.read() }),
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
/// Context bound, directory and path borrowed, out writable for Result[List[DirEntry],FileError].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_read_dir(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut HandleResult,
    directory: *const u64,
    path: *const u64,
) -> u8 {
    let request = Request::ReadDir {
        directory: unsafe { directory.read() },
        path: unsafe { Binding::get(ctx) }.path(unsafe { path.read() }),
    };
    let Ok(Completion::Entries(result)) = (unsafe { capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let result = result.map(|entries| {
        // DirEntry is pointer-free (Path @0, FileKind @8), like the runtime address list.
        let block = unsafe { crate::alloc::allocate_sequence(ctx, 0, entries.len() as u64, 16, 8) };
        let first = crate::alloc::sequence_elements_offset(8) as usize;
        for (index, entry) in entries.into_iter().enumerate() {
            let id = unsafe { Binding::get(ctx) }.intern_path(&entry.path);
            unsafe {
                let slot = crate::alloc::payload(block).add(first + index * 16);
                slot.cast::<u64>().write(id);
                slot.add(8).cast::<u64>().write(entry.kind as u64);
            }
        }
        block as u64
    });
    unsafe { out.write(HandleResult::of(result)) };
    crate::STATUS_OK
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
        fn metadata(&mut self, _: &str) -> Result<Metadata, IoError> {
            unreachable!()
        }
        fn read_dir(&mut self, _: &str) -> Result<Vec<DirEntry>, IoError> {
            unreachable!()
        }
        fn walk(&mut self, _: &str) -> Result<Vec<String>, IoError> {
            unreachable!()
        }
    }
    #[test]
    fn denied_mutations_never_activate_or_call_storage() {
        let mut state = State::default();
        let directory = state.handles.insert(Grant::Directory {
            root: String::new(),
            access: Access::Read,
        });
        for request in [
            Request::SetPermissions {
                directory,
                path: "../outside".into(),
                permissions: 0o777,
            },
            Request::RemoveAll {
                directory,
                path: "missing/child".into(),
            },
            Request::Mkdir {
                directory,
                path: "../outside".into(),
            },
            Request::RemoveDir {
                directory,
                path: "missing/child".into(),
            },
            Request::Rename {
                directory,
                from: "missing".into(),
                to: "/outside".into(),
            },
            Request::PutNew {
                directory,
                path: "../outside".into(),
                bytes: b"x".to_vec(),
                durable: true,
            },
        ] {
            assert_eq!(
                state.execute_storage(&mut DeniedStorage, &request, None),
                Ok(Completion::Unit(Err(IoError::NotGranted)))
            );
        }
    }
}
