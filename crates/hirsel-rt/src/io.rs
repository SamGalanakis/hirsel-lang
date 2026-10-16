//! Typed request/completion protocol. Expected I/O errors are data, never language failures.
use crate::handles::{Grant, Handles};
use crate::net::is_net_request;
pub use crate::net::{
    NetError, NetOperation, NetProvider, NetRule, NetTimeout, NetWorld, RootsDigest, ShutdownHow,
    SocketAddr, TimeoutKind, TlsFailure,
};
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
    NotGranted,
}
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
    Tasks {
        io: u64,
    },
    Net {
        io: u64,
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
    TestNet {
        test: u64,
        peers: Vec<hirsel_run_config::NetPeer>,
    },
    TestDir {
        test: u64,
        entries: Vec<(String, Vec<u8>)>,
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
    Print {
        stdout: u64,
        bytes: Vec<u8>,
    },
    Open {
        directory: u64,
        path: String,
    },
    NarrowFile {
        directory: u64,
        file: u64,
    },
    Walk {
        directory: u64,
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
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClockError {
    Unavailable,
    OutOfRange,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Completion {
    ClockGrant(Result<u64, ()>),
    Monotonic(Result<u64, ClockError>),
    Wall(Result<i64, ClockError>),
    /// Deterministic capability refusal; the operation issued no host effect.
    Refused(u64),
    Checked(bool),
    Handle(Result<u64, IoError>),
    OptionalFile(Option<u64>),
    Unit(Result<(), IoError>),
    Size(Result<u64, IoError>),
    Bytes(Result<Vec<u8>, IoError>),
    Paths(Result<Vec<String>, IoError>),
    NetHandle(Result<u64, NetError>),
    Addresses(Result<Vec<SocketAddr>, NetError>),
    NetBytes(Result<Vec<u8>, NetError>),
    Count(Result<u64, NetError>),
    NetUnit(Result<(), NetError>),
    Address(Result<SocketAddr, NetError>),
    Accepted(Result<(u64, SocketAddr), NetError>),
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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Tree {
    pub directories: BTreeSet<String>,
    pub files: BTreeMap<String, Vec<u8>>,
    /// Physical relative links; targets remain data until a confined operation resolves them.
    pub links: BTreeMap<String, String>,
}
impl Tree {
    pub fn new(files: BTreeMap<String, Vec<u8>>) -> Result<Self, IoError> {
        let mut tree = Self {
            directories: BTreeSet::from([String::new()]),
            files: BTreeMap::new(),
            links: BTreeMap::new(),
        };
        for (path, bytes) in files {
            let path = relative("", &path)
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
                return Err(IoError::Fault(2));
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
    fn walk(&mut self, root: &str) -> Result<Vec<String>, IoError>;
}
pub(crate) fn relative(root: &str, path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains('\0') || path.contains('\\') {
        return None;
    }
    let mut parts: Vec<_> = root.split('/').filter(|p| !p.is_empty()).collect();
    let base = parts.len();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.len() == base {
                    return None;
                }
                parts.pop();
            }
            _ => parts.push(p),
        }
    }
    Some(parts.join("/"))
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct State {
    pub handles: Handles,
    pub(crate) monotonic_granted: bool,
    pub(crate) wall_granted: bool,
    last_monotonic: Option<u64>,
    pub stdout: Vec<u8>,
    pub fake_trees: BTreeMap<u64, Tree>,
    pub fake_directories: BTreeMap<u64, u64>,
    fake_files: BTreeMap<u64, crate::io_sim::FileIdentities>,
    pub projections: BTreeSet<String>,
    pub mounts: BTreeMap<String, Access>,
    /// The admitted RunConfig rule set of the root Net; replay re-derives from it.
    pub net_rules: BTreeSet<NetRule>,
    pub net_world: NetWorld,
    pub net_scripts: BTreeMap<u64, crate::io_net_script::ScriptNet>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            handles: Handles::default(),
            monotonic_granted: false,
            wall_granted: false,
            last_monotonic: None,
            stdout: vec![],
            fake_trees: BTreeMap::new(),
            fake_directories: BTreeMap::new(),
            fake_files: BTreeMap::new(),
            mounts: BTreeMap::from([(String::new(), Access::ReadWrite)]),
            net_rules: BTreeSet::new(),
            net_world: NetWorld::Host,
            net_scripts: BTreeMap::new(),
            projections: ["stdout", "tasks", "net", "random", "dir"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }
}
impl State {
    pub fn denied_projection(&self, request: &Request) -> bool {
        match request {
            Request::Directory { path, .. } if self.mount(path).is_none() => return true,
            Request::Write { directory, .. }
            | Request::Remove { directory, .. }
            | Request::CreateAtomic { directory, .. }
            | Request::WriteAll { directory, .. }
            | Request::Commit { directory, .. } => {
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
            Request::Stdout { .. } => "stdout",
            Request::Tasks { .. } => "tasks",
            Request::Net { .. } => "net",
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
    fn access(&self, id: u64) -> Result<Access, AdapterError> {
        match self.handles.rows.get(&id) {
            Some(Grant::Directory { access, .. }) => Ok(*access),
            _ => Err(AdapterError::InvalidHandle(id)),
        }
    }
    fn directory(&self, id: u64) -> Result<String, AdapterError> {
        match self.handles.rows.get(&id) {
            Some(Grant::Directory { root, .. }) => Ok(root.clone()),
            _ => Err(AdapterError::InvalidHandle(id)),
        }
    }
    fn expect(&self, id: u64, grant: Grant) -> Result<(), AdapterError> {
        if self.handles.rows.get(&id) == Some(&grant) {
            Ok(())
        } else {
            Err(AdapterError::InvalidHandle(id))
        }
    }
    pub fn grant_test(&mut self, seed: u64) -> u64 {
        self.handles.insert(Grant::Test { seed })
    }
    pub fn is_fake_request(&self, request: &Request) -> bool {
        self.fake_tree(request).is_some()
    }
    fn fake_tree(&self, request: &Request) -> Option<u64> {
        let directory = match request {
            Request::NarrowDir { directory, .. }
            | Request::Open { directory, .. }
            | Request::NarrowFile { directory, .. }
            | Request::Walk { directory }
            | Request::Size { directory, .. }
            | Request::Read { directory, .. }
            | Request::Write { directory, .. }
            | Request::Remove { directory, .. }
            | Request::CreateAtomic { directory, .. }
            | Request::WriteAll { directory, .. }
            | Request::Commit { directory, .. } => *directory,
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
                Some(Grant::Stream { .. } | Grant::Listener { .. })
            ))
        {
            // Network outcomes are provider inputs, never injected file faults.
            if fault.is_some() {
                return Err(AdapterError::InvalidCompletion);
            }
            return self.execute_net(net, request);
        }
        // Policy denial is independently derived, never an injected tape fault.
        if fault.is_some_and(|f| f.error == IoError::NotGranted)
            || (self.denied_projection(request) && fault.is_some())
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
            let result = self.execute_storage(
                &mut crate::io_sim::MemoryTree {
                    tree: &mut tree,
                    identities: &mut identities,
                },
                request,
                fault,
            );
            self.fake_trees.insert(tree_id, tree);
            self.fake_files.insert(tree_id, identities);
            if matches!(request, Request::NarrowDir { .. })
                && let Ok(Completion::Handle(Ok(id))) = &result
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
    /// Validate provider inputs before publishing any replay or epoch state.
    pub(crate) fn sample(
        &mut self,
        request: &Request,
        input: Completion,
    ) -> Result<Completion, AdapterError> {
        match (request, &input) {
            (Request::Monotonic { clock }, Completion::Monotonic(value)) => {
                self.expect(*clock, Grant::MonotonicClock)?;
                if let Ok(ticks) = value {
                    if self.last_monotonic.is_some_and(|last| *ticks < last) {
                        return Err(AdapterError::InvalidCompletion);
                    }
                    self.last_monotonic = Some(*ticks);
                }
            }
            (Request::Wall { clock }, Completion::Wall(_)) => {
                self.expect(*clock, Grant::WallClock)?
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
        let error = fault.map(|f| f.error.clone());
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
            R::Monotonic { .. } | R::Wall { .. } => Err(AdapterError::InvalidCompletion),
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
            R::TestDir { test, entries } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                let mut files = BTreeMap::new();
                for (path, bytes) in entries {
                    let path = relative("", path).ok_or(AdapterError::InvalidCompletion)?;
                    files.insert(path, bytes.clone());
                }
                let tree = Tree::new(files).map_err(|_| AdapterError::InvalidCompletion)?;
                let id = self.handles.insert(Grant::Directory {
                    root: String::new(),
                    access: Access::ReadWrite,
                });
                self.fake_files
                    .insert(id, crate::io_sim::FileIdentities::new(&tree));
                self.fake_trees.insert(id, tree);
                self.fake_directories.insert(id, id);
                Ok(C::Handle(Ok(id)))
            }
            R::Stdout { io } => {
                self.expect(*io, Grant::Io)?;
                if !self.projections.contains("stdout") {
                    return Ok(C::Handle(Err(IoError::NotGranted)));
                }
                Ok(C::Handle(Ok(self.handles.insert(Grant::Stdout))))
            }
            R::Directory { io, path }
            | R::NarrowDir {
                directory: io,
                path,
            } => {
                let (root, path, access) = if matches!(request, R::Directory { .. }) {
                    self.expect(*io, Grant::Io)?;
                    if !self.projections.contains("dir") {
                        return Ok(C::Handle(Err(IoError::NotGranted)));
                    }
                    let Some((selector, local, access)) = self.mount(path) else {
                        return Ok(C::Handle(Err(IoError::NotGranted)));
                    };
                    (selector.to_owned(), local, access)
                } else {
                    (self.directory(*io)?, path.as_str(), self.access(*io)?)
                };
                storage.activate(
                    if matches!(request, R::Directory { .. }) {
                        0
                    } else {
                        *io
                    },
                    &root,
                );
                let result = if let Some(e) = error {
                    Err(e)
                } else {
                    relative(&root, path)
                        .ok_or(IoError::Fault(1))
                        .and_then(|root| {
                            storage.directory(&root, self.handles.next())?;
                            Ok(self.handles.insert(Grant::Directory { root, access }))
                        })
                };
                Ok(C::Handle(result))
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
                    })
                });
                storage.transfer_file(*file, new);
                Ok(C::OptionalFile(new))
            }
            R::WriteAll {
                directory, file, ..
            }
            | R::Commit { directory, file } => {
                let root = self.directory(*directory)?;
                if self.access(*directory)? == Access::Read {
                    return Ok(C::Unit(Err(IoError::NotGranted)));
                }
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
                let result = if matches!(request, R::Commit { .. }) {
                    self.handles.rows.remove(file);
                    if let Some(e) = error {
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
            R::Walk { directory } => {
                let root = self.directory(*directory)?;
                storage.activate(*directory, &root);
                let result = error.map_or_else(|| storage.walk(&root), Err);
                Ok(C::Paths(result))
            }
            _ => {
                let (directory, path) = match request {
                    R::Open { directory, path }
                    | R::Size { directory, path }
                    | R::Read { directory, path }
                    | R::Write {
                        directory, path, ..
                    }
                    | R::Remove { directory, path }
                    | R::CreateAtomic { directory, path } => (*directory, path),
                    _ => unreachable!(),
                };
                let root = self.directory(directory)?;
                if self.access(directory)? == Access::Read {
                    match request {
                        R::Write { .. } | R::Remove { .. } => {
                            return Ok(C::Unit(Err(IoError::NotGranted)));
                        }
                        R::CreateAtomic { .. } => return Ok(C::Handle(Err(IoError::NotGranted))),
                        _ => {}
                    }
                }
                storage.activate(directory, &root);
                let path = relative(&root, path).ok_or(IoError::Fault(1));
                match request {
                    R::Open { .. } => Ok(C::Handle(if let Some(e) = error {
                        Err(e)
                    } else {
                        path.and_then(|path| {
                            storage.open_file(&path, self.handles.next())?;
                            Ok(self.handles.insert(Grant::File { path, directory }))
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
                                        Err(IoError::Fault(3)) => {
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
    /// Supply the next handshake's PEM material without adding it to the tape.
    /// Replay and scripted worlds use only the digest in TlsConnect.
    fn tls_roots(&mut self, _pem: &[u8]) {}
    /// Confirmed replay output, used only when reporting a tool refusal.
    fn replay_stdout(&self) -> Vec<u8> {
        vec![]
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError>;
}

/// Stable address owned by launch, bound explicitly to RunCtx for generated thunks.
/// The binding outlives every release and cannot be accessed while a thunk runs.
pub struct Binding {
    adapter: Box<dyn Adapter>,
    paths: Vec<String>,
    arguments: Vec<String>,
}
impl Binding {
    pub(crate) fn tls_roots(&mut self, pem: &[u8]) {
        self.adapter.tls_roots(pem);
    }
    pub fn new(adapter: impl Adapter + 'static) -> Self {
        Self {
            adapter: Box::new(adapter),
            paths: Vec::new(),
            arguments: Vec::new(),
        }
    }
    pub fn from_box(adapter: Box<dyn Adapter>) -> Self {
        Self {
            adapter,
            paths: Vec::new(),
            arguments: Vec::new(),
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
    fn path(&self, id: u64) -> String {
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
    _anchor: u64,
) -> u8 {
    let binding = unsafe { Binding::get(ctx) };
    match binding.adapter.call(Request::Close { handle }) {
        Ok(Completion::Unit(Ok(()))) => {}
        Err(error) => adapter_refusal(binding, error),
        _ => crate::fault("invalid handle cleanup completion"),
    }
    crate::STATUS_OK
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
            Err(IoError::NotGranted) => Self {
                tag: 1,
                error: 2,
                code: 0,
            },
        }
    }
}
/// `out.print` at the Hirsel ABI v1 of `fn(s: Stdout, text: String) -> Result[(), OutputError]`
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
    let bytes = unsafe { native_text(data, len) }.as_bytes().to_vec();
    let Completion::Unit(value) = (unsafe { native_call(ctx, Request::Print { stdout, bytes }) })
    else {
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
    }
}
unsafe fn native_call(ctx: *mut crate::RunCtx, request: Request) -> Completion {
    let binding = unsafe { Binding::get(ctx) };
    match binding.adapter.call(request) {
        Ok(value) => value,
        Err(error) => adapter_refusal(binding, error),
    }
}
/// A refused typed capability is CAP-2. The compiled caller records its anchor.
pub(crate) unsafe fn capability_call(
    ctx: *mut crate::RunCtx,
    request: Request,
) -> Result<Completion, ()> {
    let binding = unsafe { Binding::get(ctx) };
    match binding.adapter.call(request) {
        Ok(value) => Ok(value),
        Err(AdapterError::InvalidHandle(_)) => Err(()),
        Err(error) => adapter_refusal(binding, error),
    }
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
        /// Hirsel ABI v1. Expected errors are results; refused authority returns status 1.
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
path_export!(
    hirsel_rt_v2_file_open,
    Open,
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
        crate::release::hirsel_rt_v2_release(ctx, owner, (*owner).index(), std::ptr::null(), 0)
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
grant_projection_export!(hirsel_rt_v2_io_stdout, Stdout);
grant_projection_export!(hirsel_rt_v2_io_random, Random);
grant_projection_export!(hirsel_rt_v2_io_tasks, Tasks);
grant_projection_export!(hirsel_rt_v2_io_net, Net);

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
/// IterNext closure ABI: context, environment, hidden destination, output.
/// # Safety
/// The environment is an exclusive live walk environment and out is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_file_walk_step(
    _ctx: *mut crate::RunCtx,
    environment: *mut crate::Header,
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

/// A replay refusal is a tool outcome, even when it happens in cleanup.
fn adapter_refusal(binding: &mut Binding, error: AdapterError) -> ! {
    use std::io::Write;
    let mut stdout = std::io::stdout();
    let output = binding.adapter.replay_stdout();
    if let Err(output_error) = stdout.write_all(&output).and_then(|()| stdout.flush()) {
        eprintln!("hirsel-rt: replay output: {output_error}");
    }
    eprintln!("hirsel-rt: Io refused: {error:?}");
    std::process::exit(crate::launch::EXIT_LAUNCH)
}
