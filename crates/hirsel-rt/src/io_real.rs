//! Linux directory-fd confinement, including component-wise relative link resolution.
//! Each Dir has its own pinned root; links cannot climb out of a narrowed grant.
use crate::handles::Handles;
use crate::io::*;
use std::collections::{BTreeMap, VecDeque};
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::rc::Rc;
pub struct RealIo {
    state: State,
    disk: Disk,
    tape: Vec<Event>,
    fault: Option<Fault>,
    output: Box<dyn Write>,
    recording: Option<crate::launch::RecordingWriter<File>>,
    retain_history: bool,
    net: Box<dyn NetProvider>,
}
impl RealIo {
    /// Configure the root Net's isolated scripted world before entry. Rules are
    /// supplied separately by set_net_rules; peer data never widens them.
    pub fn set_net_script(
        &mut self,
        peers: Vec<hirsel_run_config::NetPeer>,
    ) -> Result<(), AdapterError> {
        self.state.set_net_script(peers)
    }
    /// The admitted RunConfig rule set of the root Net (`io.net.rules`). Absent, the
    /// root Net permits nothing.
    pub fn set_net_rules(&mut self, rules: std::collections::BTreeSet<NetRule>) {
        self.state.net_rules = rules;
    }
    /// Replace the host network provider, e.g. to count provider calls in tests.
    pub fn set_net_provider(&mut self, provider: impl NetProvider + 'static) {
        self.net = Box::new(provider);
    }
    /// Admission supplies the configured projections; recordings cannot widen them.
    pub fn set_projections(&mut self, projections: std::collections::BTreeSet<String>) {
        self.state.projections = projections;
    }
    /// The caller explicitly grants this directory. No ambient directory is consulted later.
    pub fn grant(root: &Path) -> std::io::Result<Self> {
        Self::grant_with_output(root, std::io::sink())
    }
    /// Explicit output grant; write errors and committed prefixes enter the completion tape.
    pub fn grant_with_output(root: &Path, output: impl Write + 'static) -> std::io::Result<Self> {
        Ok(Self::grant_pinned_with_output(
            crate::host_paths::PinnedDirectory::open(root)?,
            output,
        ))
    }
    /// Use the exact descriptor acquired by host admission, without reopening its designator.
    pub fn grant_pinned_with_output(
        root: crate::host_paths::PinnedDirectory,
        output: impl Write + 'static,
    ) -> Self {
        Self::from_pinned_mounts(
            [(String::new(), hirsel_run_config::Access::ReadWrite, root)],
            output,
        )
        .expect("one explicit root mount")
    }
    /// Retain every admitted descriptor and its rights. Empty Io has no directory pin.
    pub fn from_pinned_mounts(
        mounts: impl IntoIterator<
            Item = (
                String,
                hirsel_run_config::Access,
                crate::host_paths::PinnedDirectory,
            ),
        >,
        output: impl Write + 'static,
    ) -> Result<Self, hirsel_run_config::RunRefusal> {
        let mounts: Vec<_> = mounts.into_iter().collect();
        let access = mount_access(
            mounts
                .iter()
                .map(|(path, access, _)| (path.clone(), *access)),
        )?;
        let mut state = State::default();
        state.mounts = access;
        Ok(Self {
            state,
            disk: Disk {
                mounts: mounts
                    .into_iter()
                    .map(|(path, _, pin)| (path, pin))
                    .collect(),
                directories: BTreeMap::new(),
                active: String::new(),
                active_id: 0,
                files: BTreeMap::new(),
                last_open: None,
                partial: None,
                next_write: 1,
                prepared: BTreeMap::new(),
            },
            tape: vec![],
            fault: None,
            output: Box::new(output),
            recording: None,
            retain_history: true,
            net: Box::new(crate::net_host::HostNet::new()),
        })
    }
    /// Borrow the original admitted pins for every recorder check, without reopening paths.
    pub fn mount_pins(&self) -> Vec<(&str, &crate::host_paths::PinnedDirectory)> {
        self.disk
            .mounts
            .iter()
            .map(|(path, pin)| (path.as_str(), pin))
            .collect()
    }
    /// Pin and refuse tape/grant overlap before snapshotting or writing a header.
    /// Multi-mount HostAuthority adapters use RecorderDestination::open with all
    /// retained mount pins instead of this single-root convenience.
    pub fn prepare_recording(&self, path: &Path) -> Result<File, hirsel_run_config::RunRefusal> {
        self.prepare_recording_with_files(path, &[])
    }
    /// Include file-backed output grants acquired by the launcher/provider.
    pub fn prepare_recording_with_files(
        &self,
        path: &Path,
        files: &[(&str, &File)],
    ) -> Result<File, hirsel_run_config::RunRefusal> {
        crate::host_paths::RecorderDestination::pin(path)?
            .open_with_files(&self.mount_pins(), files)
    }
    /// Independently admitted clock providers; absent grants stay denied.
    pub fn clock_grants(&mut self, monotonic: bool, wall: bool) {
        self.state.monotonic_granted = monotonic;
        self.state.wall_granted = wall;
    }
    /// A Test grant is a typed launch input; no kernel reads ambient entropy.
    pub fn test_context(&mut self, seed: u64) {
        self.state.handles = Handles::test(seed);
    }
    pub fn grant_test(&mut self, seed: u64) -> u64 {
        self.state.grant_test(seed)
    }
    pub fn fake_trees(&self) -> &BTreeMap<u64, Tree> {
        &self.state.fake_trees
    }
    /// Explicit recorded fault injection for disposable-filesystem tests, never ambient input.
    pub fn inject(&mut self, fault: Fault) {
        self.fault = Some(fault);
    }
    /// Streams ordinary completions; this tape has an unknown suffix after termination.
    pub fn record_to(&mut self, writer: crate::launch::RecordingWriter<File>) {
        self.recording = Some(writer);
    }
    pub fn finish_recording(&mut self) -> std::io::Result<()> {
        if let Some(writer) = &mut self.recording {
            writer.finish()?;
        }
        Ok(())
    }
    /// The native launcher streams events and needs no duplicate in-memory history.
    pub(crate) fn discard_history(&mut self) {
        self.retain_history = false;
        self.tape.clear();
        self.state.stdout.clear();
    }
    fn record(&mut self, event: Event) -> Result<(), AdapterError> {
        if let Some(writer) = &mut self.recording {
            writer
                .append(&event)
                .map_err(|e| AdapterError::RecordingFailed(e.to_string()))?;
        }
        if self.retain_history {
            self.tape.push(event);
        }
        Ok(())
    }
    pub fn tape(&self) -> &[Event] {
        &self.tape
    }
    pub fn handles(&self) -> &Handles {
        &self.state.handles
    }
    pub fn stdout(&self) -> &[u8] {
        &self.state.stdout
    }
    pub fn snapshot(&mut self) -> Result<Tree, IoError> {
        let pins = self
            .disk
            .mounts
            .iter()
            .map(|(selector, pin)| {
                Ok((
                    selector.clone(),
                    pin.descriptor().try_clone().map_err(error)?,
                ))
            })
            .collect::<Result<Vec<_>, IoError>>()?;
        let mut snapshots = BTreeMap::new();
        for (selector, root) in pins {
            let meta = root.metadata().map_err(error)?;
            let root_identity = (meta.dev(), meta.ino());
            let mut identities = BTreeMap::from([(root_identity, String::new())]);
            let mut tree = Tree::new(BTreeMap::new())?;
            self.disk.active = selector.clone();
            self.disk.active_id = 0;
            // Snapshot physical entries, without following links or reopening a mount designator.
            let mut work = vec![(root, String::new(), vec![root_identity])];
            while let Some((fd, prefix, ancestry)) = work.pop() {
                for entry in
                    std::fs::read_dir(format!("/proc/self/fd/{}", fd.as_raw_fd())).map_err(error)?
                {
                    let entry = entry.map_err(error)?;
                    let Ok(name) = entry.file_name().into_string() else {
                        continue;
                    };
                    // Portable paths cannot name these entries. Their host
                    // walk/access faults are recorded at the operation boundary.
                    if name.contains(['\\', '\0']) {
                        continue;
                    }
                    let path = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}/{name}")
                    };
                    let pin = open_component(fd.as_raw_fd(), &name, libc::O_PATH, 0)?;
                    let meta = pin.metadata().map_err(error)?;
                    if meta.file_type().is_symlink() {
                        let mut bytes = vec![0; 4096];
                        // SAFETY: pin is a live O_PATH symlink and the buffer is writable.
                        let count = unsafe {
                            libc::readlinkat(
                                pin.as_raw_fd(),
                                c"".as_ptr(),
                                bytes.as_mut_ptr().cast(),
                                bytes.len(),
                            )
                        };
                        if count >= 0 && (count as usize) < bytes.len() {
                            bytes.truncate(count as usize);
                            if let Ok(target) = String::from_utf8(bytes) {
                                tree.links.insert(path, target);
                            }
                        }
                    } else if meta.is_dir() {
                        let identity = (meta.dev(), meta.ino());
                        // A bind view of an ancestor cannot create an unbounded snapshot.
                        if ancestry.contains(&identity) {
                            return Err(IoError::Fault(1));
                        }
                        identities.entry(identity).or_insert_with(|| path.clone());
                        let mut ancestry = ancestry.clone();
                        ancestry.push(identity);
                        tree.directories.insert(path.clone());
                        work.push((pin, path, ancestry));
                    } else if meta.is_file() {
                        let logical = relative(&selector, &path).expect("snapshot entry");
                        // Invalid entries remain operation faults, rather than admission errors.
                        if let Ok(bytes) = self.disk.read(&logical) {
                            tree.files.insert(path, bytes);
                        }
                    }
                }
            }
            snapshots.insert(selector, (root_identity, identities, tree));
        }
        // Prefer the widest configured physical root, then its first virtual selector.
        // Narrower/same-resource mounts become root aliases, so replay preserves shared
        // publications while resolving every issued Dir inside its own physical root.
        let widest: Vec<_> = snapshots
            .iter()
            .filter(|(selector, (identity, _, _))| {
                !snapshots
                    .iter()
                    .any(|(other, (other_identity, directories, _))| {
                        (other_identity != identity || other < selector)
                            && directories.contains_key(identity)
                    })
            })
            .map(|(selector, _)| selector.clone())
            .collect();
        let mut tree = Tree::new(BTreeMap::new())?;
        for selector in &widest {
            let (_, _, snapshot) = &snapshots[selector];
            tree.directories.extend(
                snapshot
                    .directories
                    .iter()
                    .map(|p| relative(selector, p).expect("snapshot directory")),
            );
            tree.files.extend(
                snapshot
                    .files
                    .iter()
                    .map(|(p, b)| (relative(selector, p).expect("snapshot file"), b.clone())),
            );
            tree.links.extend(snapshot.links.iter().map(|(p, target)| {
                (
                    relative(selector, p).expect("snapshot link"),
                    target.clone(),
                )
            }));
        }
        for (selector, (identity, _, _)) in &snapshots {
            let mut parent = selector.as_str();
            while let Some((p, _)) = parent.rsplit_once('/') {
                tree.directories.insert(p.into());
                parent = p;
            }
            if widest.contains(selector) {
                continue;
            }
            let (canonical, suffix) = widest
                .iter()
                .find_map(|top| snapshots[top].1.get(identity).map(|suffix| (top, suffix)))
                .ok_or(IoError::Fault(1))?;
            let target = relative(canonical, suffix).expect("snapshot mount alias");
            let parent = selector.rsplit_once('/').map_or("", |(p, _)| p);
            let prefix = "../".repeat(parent.split('/').filter(|p| !p.is_empty()).count());
            tree.links
                .insert(selector.clone(), format!("{prefix}{target}"));
        }
        Ok(tree)
    }
}
impl Adapter for RealIo {
    fn tls_roots(&mut self, pem: &[u8]) {
        self.net.tls_roots(pem);
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        let fake = self.state.is_fake_request(&request);
        let mut fault =
            if self.state.denied_projection(&request) || crate::net::is_net_request(&request) {
                None
            } else {
                self.fault.take()
            };
        self.disk.partial = None;
        let before = self.state.stdout.len();
        let result = match &request {
            Request::Monotonic { .. } | Request::Wall { .. } => {
                let input = clock_input(&request, fault.as_ref());
                self.state.sample(&request, input)
            }
            _ => self
                .state
                .execute(&mut self.disk, &mut *self.net, &request, fault.as_ref()),
        };
        if matches!(request, Request::Monotonic { .. } | Request::Wall { .. }) {
            fault = None;
        }
        let mut completion = match result {
            Ok(completion) => completion,
            Err(AdapterError::InvalidHandle(id)) => {
                self.record(Event {
                    request,
                    completion: Completion::Refused(id),
                    fault: None,
                })?;
                return Err(AdapterError::InvalidHandle(id));
            }
            Err(error) => return Err(error),
        };
        if matches!(
            request,
            Request::TestCheckCase {
                condition: false,
                ..
            }
        ) {
            self.output
                .write_all(&self.state.stdout[before..])
                .and_then(|()| self.output.flush())
                .map_err(|e| AdapterError::RecordingFailed(format!("test report: {e}")))?;
            if !self.retain_history {
                self.state.stdout.truncate(before);
            }
        }
        if matches!(request, Request::Print { .. }) {
            let bytes = &self.state.stdout[before..];
            let mut written = 0;
            while written < bytes.len() {
                match self.output.write(&bytes[written..]) {
                    Ok(0) => {
                        let e = IoError::Fault(1);
                        completion = Completion::Unit(Err(e.clone()));
                        fault = Some(Fault {
                            error: e,
                            partial: written,
                        });
                        break;
                    }
                    Ok(n) => written += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        let e = error(e);
                        completion = Completion::Unit(Err(e.clone()));
                        fault = Some(Fault {
                            error: e,
                            partial: written,
                        });
                        break;
                    }
                }
            }
            // Completion means acceptance at the provider boundary, including
            // flushing any private buffer before its success enters the tape.
            if let Err(e) = self.output.flush() {
                let e = error(e);
                completion = Completion::Unit(Err(e.clone()));
                fault = Some(Fault {
                    error: e,
                    partial: written,
                });
            }
            self.state.stdout.truncate(if self.retain_history {
                before + written
            } else {
                before
            });
        }
        if let Some((error, partial)) = self.disk.partial.take() {
            fault = Some(Fault { error, partial });
        }
        // A file may no longer denote the inode reachable through the target grant
        // after a replacement. Record that host observation just like an OS error;
        // ordered replay derives program publications; external host mutations remain inputs.
        if fault.is_none()
            && matches!(request, Request::NarrowFile { .. })
            && completion == Completion::OptionalFile(None)
        {
            fault = Some(Fault {
                error: IoError::Fault(1),
                partial: 0,
            });
        }
        if fault.is_none() {
            let error = match &completion {
                Completion::Handle(Err(e))
                | Completion::Unit(Err(e))
                | Completion::Size(Err(e))
                | Completion::Bytes(Err(e))
                | Completion::Paths(Err(e))
                    if *e != IoError::NotGranted =>
                {
                    Some(e.clone())
                }
                _ => None,
            };
            fault = error.map(|error| Fault { error, partial: 0 });
        }
        match (&request, &completion) {
            (Request::Open { .. }, Completion::Handle(Ok(id))) if !fake => {
                self.disk
                    .files
                    .insert(*id, self.disk.last_open.take().expect("opened descriptor"));
            }
            (Request::NarrowFile { file, .. }, Completion::OptionalFile(new)) => {
                if let Some(fd) = self.disk.files.remove(file)
                    && let Some(id) = new
                {
                    self.disk.files.insert(*id, fd);
                }
            }
            (Request::Close { handle }, _) => {
                self.disk.files.remove(handle);
            }
            _ => {}
        }
        self.record(Event {
            request,
            completion: completion.clone(),
            fault,
        })?;
        Ok(completion)
    }
}
#[derive(Debug)]
struct Disk {
    mounts: BTreeMap<String, crate::host_paths::PinnedDirectory>,
    directories: BTreeMap<u64, File>,
    active: String,
    active_id: u64,
    files: BTreeMap<u64, File>,
    last_open: Option<File>,
    partial: Option<(IoError, usize)>,
    next_write: u64,
    prepared: BTreeMap<(u64, String), File>,
}
fn error(e: std::io::Error) -> IoError {
    // Stable capability codes, never expose host errno as language data.
    IoError::Fault(match e.kind() {
        std::io::ErrorKind::NotFound => 2,
        std::io::ErrorKind::AlreadyExists => 3,
        _ => 1,
    })
}
impl Disk {
    fn base(&self) -> Result<&File, IoError> {
        if self.active_id == 0 {
            self.mounts.get(&self.active).map(|pin| pin.descriptor())
        } else {
            self.directories.get(&self.active_id)
        }
        .ok_or(IoError::NotGranted)
    }
    fn local<'a>(&self, path: &'a str) -> Result<&'a str, IoError> {
        if self.active.is_empty() {
            Ok(path)
        } else if path == self.active {
            Ok("")
        } else {
            path.strip_prefix(&format!("{}/", self.active))
                .ok_or(IoError::Fault(1))
        }
    }
    fn open(&self, path: &str, flags: i32, mode: u64) -> Result<File, IoError> {
        open_beneath(self.base()?.as_raw_fd(), self.local(path)?, flags, mode)
    }
    fn parent(&self, path: &str) -> Result<(File, CString), IoError> {
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        if name.is_empty() {
            return Err(IoError::Fault(1));
        }
        Ok((
            self.open(parent, libc::O_RDONLY | libc::O_DIRECTORY, 0)?,
            CString::new(name).map_err(|_| IoError::Fault(1))?,
        ))
    }
    fn entries(&self, root: &str) -> Result<(Vec<String>, Vec<String>), IoError> {
        let base = self.base()?.try_clone().map_err(error)?;
        let (fd, mut parents) = resolve_beneath(
            vec![Rc::new(base)],
            self.local(root)?,
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        )?;
        parents.push(Rc::new(fd));
        let meta = parents
            .last()
            .expect("walk root")
            .metadata()
            .map_err(error)?;
        let mut work = vec![(parents, String::new(), vec![(meta.dev(), meta.ino())])];
        let mut files = vec![];
        let mut dirs = vec![root.into()];
        while let Some((parents, prefix, visited)) = work.pop() {
            let fd = parents.last().expect("pinned directory");
            let host = format!("/proc/self/fd/{}", fd.as_raw_fd());
            for entry in std::fs::read_dir(host).map_err(error)? {
                let entry = entry.map_err(error)?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| IoError::Fault(1))?;
                let ancestry = parents.clone();
                // Classify first; O_PATH never invokes a device/FIFO's open hook.
                // The resolver also fstatat's every component before pinning it.
                let (child, mut ancestry) = resolve_beneath(ancestry, &name, libc::O_PATH, 0)?;
                let rel = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                let meta = child.metadata().map_err(error)?;
                if meta.is_dir() {
                    // Only active ancestors count: aliases to an ordinary subtree
                    // are followed independently, but links back to an ancestor fail.
                    let identity = (meta.dev(), meta.ino());
                    if visited.contains(&identity) {
                        return Err(IoError::Fault(1));
                    }
                    let mut visited = visited.clone();
                    visited.push(identity);
                    dirs.push(if root.is_empty() {
                        rel.clone()
                    } else {
                        format!("{root}/{rel}")
                    });
                    ancestry.push(Rc::new(child));
                    work.push((ancestry, rel, visited));
                } else if meta.is_file() {
                    files.push(rel);
                } else {
                    return Err(IoError::Fault(1));
                }
            }
        }
        files.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        dirs.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        Ok((files, dirs))
    }
}
impl Storage for Disk {
    fn destination(&mut self, path: &str) -> Result<(), IoError> {
        self.parent(path)?;
        match self.open(path, libc::O_PATH, 0) {
            Ok(fd) => {
                if fd.metadata().map_err(error)?.is_file() {
                    Ok(())
                } else {
                    Err(IoError::Fault(2))
                }
            }
            Err(IoError::Fault(2)) => Ok(()),
            Err(e) => Err(e),
        }
    }
    fn activate(&mut self, id: u64, root: &str) {
        self.active = root.into();
        self.active_id = id;
    }
    fn directory(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        let fd = self
            .open(path, libc::O_RDONLY | libc::O_DIRECTORY, 0)
            .map_err(|_| IoError::Fault(1))?;
        self.directories.insert(id, fd);
        Ok(())
    }
    fn file_fits(&mut self, id: u64, path: &str) -> bool {
        use std::os::unix::fs::MetadataExt;
        let Some(old) = self.files.get(&id) else {
            return false;
        };
        let (Ok(old), Ok(new)) = (
            old.metadata(),
            self.open(path, libc::O_RDONLY | libc::O_NONBLOCK, 0)
                .and_then(|f| f.metadata().map_err(error)),
        ) else {
            return false;
        };
        old.dev() == new.dev() && old.ino() == new.ino()
    }
    fn open_file(&mut self, path: &str, _id: u64) -> Result<(), IoError> {
        let fd = self.open(path, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        if !fd.metadata().map_err(error)?.is_file() {
            return Err(IoError::Fault(2));
        }
        self.last_open = Some(fd);
        Ok(())
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, IoError> {
        let mut fd = self.open(path, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        if !fd.metadata().map_err(error)?.is_file() {
            return Err(IoError::Fault(2));
        }
        let mut bytes = vec![];
        fd.read_to_end(&mut bytes).map_err(error)?;
        Ok(bytes)
    }
    fn write(
        &mut self,
        path: &str,
        bytes: &[u8],
        append: bool,
        exclusive: bool,
    ) -> Result<(), IoError> {
        // Prepare replacement separately: an OS error before the first byte must
        // not truncate an existing destination. Publish a committed prefix only
        // after writing it, so every recorded partial count describes the effect.
        let replacement = !append && !exclusive;
        if replacement {
            self.destination(path)?;
        }
        let temporary = if replacement {
            let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
            loop {
                let serial = self.next_write;
                self.next_write = self
                    .next_write
                    .checked_add(1)
                    .expect("write names exhausted");
                let name = if parent.is_empty() {
                    format!(".hirsel-write-{serial}")
                } else {
                    format!("{parent}/.hirsel-write-{serial}")
                };
                match self.open(&name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600) {
                    Ok(fd) => break Some((name, fd)),
                    Err(IoError::Fault(3)) => {}
                    Err(e) => return Err(e),
                }
            }
        } else {
            None
        };
        let (temporary, mut fd) = match temporary {
            Some((name, fd)) => (Some(name), fd),
            None => (
                None,
                self.open(
                    path,
                    libc::O_WRONLY
                        | libc::O_NONBLOCK
                        | if append {
                            libc::O_APPEND
                        } else {
                            libc::O_CREAT | libc::O_EXCL
                        },
                    if append { 0 } else { 0o600 },
                )?,
            ),
        };
        if !fd.metadata().map_err(error)?.is_file() {
            return Err(IoError::Fault(2));
        }
        if !append {
            let name = temporary.as_deref().unwrap_or(path);
            self.prepared.insert(
                (self.active_id, name.into()),
                fd.try_clone().map_err(error)?,
            );
        }
        let mut written = 0;
        let mut failure = None;
        while written < bytes.len() {
            match fd.write(&bytes[written..]) {
                Ok(0) => {
                    failure = Some(IoError::Fault(1));
                    break;
                }
                Ok(n) => written += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    failure = Some(error(e));
                    break;
                }
            }
        }
        if let Some(temporary) = temporary {
            if failure.is_some() && written == 0 {
                let _ = self.remove(&temporary);
            } else if let Err(e) = self.rename(&temporary, path) {
                let _ = self.remove(&temporary);
                self.partial = Some((e.clone(), 0));
                return Err(e);
            }
        }
        if let Some(e) = failure {
            self.partial = Some((e.clone(), written));
            Err(e)
        } else {
            Ok(())
        }
    }
    fn remove(&mut self, path: &str) -> Result<(), IoError> {
        self.prepared.remove(&(self.active_id, path.into()));
        let (dir, name) = self.parent(path)?;
        // SAFETY: live directory fd and NUL-terminated local leaf. unlink never follows links.
        if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            Err(error(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), IoError> {
        let (src, a) = self.parent(from)?;
        let (dst, b) = self.parent(to)?;
        let key = (self.active_id, from.to_owned());
        let prepared = self.prepared.get(&key).ok_or(IoError::Fault(1))?;
        let current = open_component(
            src.as_raw_fd(),
            a.to_str().map_err(|_| IoError::Fault(1))?,
            libc::O_PATH,
            0,
        )?;
        let old = prepared.metadata().map_err(error)?;
        let new = current.metadata().map_err(error)?;
        if !new.is_file() || old.dev() != new.dev() || old.ino() != new.ino() {
            return Err(IoError::Fault(1));
        }
        // SAFETY: both parents are pinned confined descriptors and both names are local leaves.
        if unsafe { libc::renameat(src.as_raw_fd(), a.as_ptr(), dst.as_raw_fd(), b.as_ptr()) } != 0
        {
            Err(error(std::io::Error::last_os_error()))
        } else {
            self.prepared.remove(&key);
            Ok(())
        }
    }
    fn walk(&mut self, root: &str) -> Result<Vec<String>, IoError> {
        self.entries(root).map(|(f, _)| f)
    }
}
fn open_beneath(fd: RawFd, path: &str, flags: i32, mode: u64) -> Result<File, IoError> {
    let root = open_component(fd, ".", libc::O_PATH | libc::O_DIRECTORY, 0)?;
    resolve_beneath(vec![Rc::new(root)], path, flags, mode).map(|(file, _)| file)
}

/// §22: "Directory paths are relative; absolute paths, traversal outside the
/// root (including resolved links) and inaccessible paths return FileFault
/// without granting authority." Parent steps pop only already pinned in-root
/// descriptors. An O_PATH descriptor pins each classified inode before use.
fn resolve_beneath(
    mut parents: Vec<Rc<File>>,
    path: &str,
    flags: i32,
    mode: u64,
) -> Result<(File, Vec<Rc<File>>), IoError> {
    if path.starts_with('/') || path.contains('\\') {
        return Err(IoError::Fault(1));
    }
    let mut pending: VecDeque<String> = path.split('/').map(str::to_owned).collect();
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        match part.as_str() {
            "" | "." => continue,
            ".." => {
                if parents.len() == 1 {
                    return Err(IoError::Fault(1));
                }
                parents.pop();
                continue;
            }
            _ => {}
        }
        let fd = parents.last().expect("pinned root").as_raw_fd();
        // Exclusive creation never expands the final link. open_component adds
        // O_NOFOLLOW: the kernel decides existence and creates atomically.
        if pending.is_empty() && flags & libc::O_CREAT != 0 && flags & libc::O_EXCL != 0 {
            let file = open_component(fd, &part, flags | libc::O_NONBLOCK, mode)?;
            return Ok((file, parents));
        }
        let name = CString::new(part.as_str()).map_err(|_| IoError::Fault(1))?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: live fd, terminated name and writable stat storage.
        let status = unsafe {
            libc::fstatat(
                fd,
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status != 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::NotFound
                && pending.is_empty()
                && flags & libc::O_CREAT != 0
            {
                let file = open_component(fd, &part, flags | libc::O_NONBLOCK, mode)?;
                return Ok((file, parents));
            }
            return Err(error(e));
        }
        // SAFETY: fstatat initialized stat on success.
        let kind = unsafe { stat.assume_init() }.st_mode & libc::S_IFMT;
        if !matches!(kind, libc::S_IFREG | libc::S_IFDIR | libc::S_IFLNK) {
            return Err(IoError::Fault(1));
        }
        let pin = open_component(fd, &part, libc::O_PATH, 0)?;
        let meta = pin.metadata().map_err(error)?;
        // Check the pinned inode too: a concurrent replacement after fstatat
        // must never turn a data open into a special-file open.
        if meta.file_type().is_symlink() {
            links += 1;
            if links > 40 {
                return Err(IoError::Fault(1));
            }
            let mut bytes = vec![0; 4096];
            // SAFETY: the empty name reads the pinned O_PATH symlink itself;
            // the buffer is writable for the provided length.
            let count = unsafe {
                libc::readlinkat(
                    pin.as_raw_fd(),
                    c"".as_ptr(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                )
            };
            if count < 0 {
                return Err(error(std::io::Error::last_os_error()));
            }
            if count as usize == bytes.len() {
                return Err(IoError::Fault(1));
            }
            bytes.truncate(count as usize);
            let target = String::from_utf8(bytes).map_err(|_| IoError::Fault(1))?;
            if target.starts_with('/') || target.contains('\\') {
                return Err(IoError::Fault(1));
            }
            for component in target.split('/').rev() {
                pending.push_front(component.into());
            }
        } else if !meta.is_file() && !meta.is_dir() {
            return Err(IoError::Fault(1));
        } else if pending.is_empty() {
            if flags & libc::O_EXCL != 0 && flags & libc::O_CREAT != 0 {
                return Err(IoError::Fault(3));
            }
            if flags & libc::O_DIRECTORY != 0 && !meta.is_dir() {
                return Err(IoError::Fault(1));
            }
            if flags & libc::O_PATH != 0 {
                return Ok((pin, parents));
            }
            // Reopen exactly the classified inode, never its mutable directory name.
            let host = CString::new(format!("/proc/self/fd/{}", pin.as_raw_fd())).unwrap();
            // SAFETY: terminated descriptor path and ordinary open flags. The
            // pinned object is already known to be a regular file or directory.
            let result = unsafe {
                libc::open(
                    host.as_ptr(),
                    (flags & !(libc::O_CREAT | libc::O_EXCL)) | libc::O_CLOEXEC | libc::O_NONBLOCK,
                    mode as libc::mode_t,
                )
            };
            if result < 0 {
                return Err(error(std::io::Error::last_os_error()));
            }
            // SAFETY: open returned a fresh owned descriptor.
            return Ok((unsafe { File::from_raw_fd(result) }, parents));
        } else if meta.is_dir() {
            parents.push(Rc::new(pin));
        } else {
            return Err(IoError::Fault(1));
        }
    }
    let current = parents.pop().expect("pinned root");
    Ok((
        open_component(current.as_raw_fd(), ".", flags | libc::O_NONBLOCK, mode)?,
        parents,
    ))
}

fn open_component(fd: RawFd, name: &str, flags: i32, mode: u64) -> Result<File, IoError> {
    let name = CString::new(name).map_err(|_| IoError::Fault(1))?;
    // SAFETY: a pinned parent and terminated local name. O_PATH|NOFOLLOW pins
    // links without resolving them; data opens never implicitly follow a link.
    let result = unsafe {
        libc::openat(
            fd,
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode as libc::mode_t,
        )
    };
    if result < 0 {
        Err(error(std::io::Error::last_os_error()))
    } else {
        Ok(unsafe { File::from_raw_fd(result) })
    }
}

fn clock_input(request: &Request, fault: Option<&Fault>) -> Completion {
    let monotonic = matches!(request, Request::Monotonic { .. });
    let value = if let Some(fault) = fault {
        Err(match fault.error {
            IoError::Fault(1) => ClockError::OutOfRange,
            _ => ClockError::Unavailable,
        })
    } else {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: time is writable; these fixed clock IDs retain no host handles.
        let status = unsafe {
            libc::clock_gettime(
                if monotonic {
                    libc::CLOCK_MONOTONIC
                } else {
                    libc::CLOCK_REALTIME
                },
                &mut time,
            )
        };
        if status != 0 {
            Err(ClockError::Unavailable)
        } else {
            Ok(i128::from(time.tv_sec) * 1_000_000_000 + i128::from(time.tv_nsec))
        }
    };
    if monotonic {
        Completion::Monotonic(
            value.and_then(|n| u64::try_from(n).map_err(|_| ClockError::OutOfRange)),
        )
    } else {
        Completion::Wall(value.and_then(|n| i64::try_from(n).map_err(|_| ClockError::OutOfRange)))
    }
}
