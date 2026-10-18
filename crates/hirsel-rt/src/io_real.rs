//! Linux directory-fd confinement, including component-wise relative link resolution.
//! Each Dir has its own pinned root; links cannot climb out of a narrowed grant.
use crate::handles::Handles;
use crate::io::*;
use std::collections::{BTreeMap, VecDeque};
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::Path;
use std::rc::Rc;
pub struct RealIo {
    state: State,
    disk: Disk,
    tape: Vec<Event>,
    fault: Option<Fault>,
    output: Box<dyn Write>,
    input: Input,
    stderr: Box<dyn Write>,
    recording: Option<crate::launch::RecordingWriter<File>>,
    retain_history: bool,
    net: Box<dyn NetProvider>,
    /// Cooperative admission: the readiness its providers park on and the waits it records.
    cooperative: Option<(crate::readiness::Readiness, crate::readiness::WaitLog)>,
    /// The host process provider's streaming children.
    children: crate::host_child::HostChildren,
}
enum Input {
    Blocking(Box<dyn Read>),
    Cooperative(crate::host_stdin::CooperativeStdin),
}
impl RealIo {
    /// Pin the explicitly admitted input stream before entry. Projections remain independent.
    pub fn set_stdin(&mut self, input: impl Read + 'static) {
        self.input = Input::Blocking(Box::new(input));
    }
    /// The interpreter's stdin: a read with no available byte is `AdapterError::Blocked`
    /// and leaves POLLIN interest under the entered Readiness token.
    pub fn set_cooperative_stdin(&mut self, input: crate::host_stdin::CooperativeStdin) {
        self.input = Input::Cooperative(input);
    }
    pub fn set_stderr(&mut self, output: impl Write + 'static) {
        self.stderr = Box::new(output);
    }
    /// Exact name grants and their admission snapshots; no later ambient lookup.
    pub fn set_env(&mut self, env: BTreeMap<String, Option<String>>) {
        self.state.env = env;
    }
    pub fn stderr(&self) -> &[u8] {
        &self.state.stderr
    }

    /// Configure the root Net's isolated scripted world before entry. Rules are
    /// supplied separately by set_net_rules; peer data never widens them.
    pub fn set_net_script(
        &mut self,
        peers: Vec<hirsel_run_config::NetPeer>,
        listeners: Vec<hirsel_run_config::NetListener>,
    ) -> Result<(), AdapterError> {
        self.state.set_net_script(peers, listeners)
    }
    /// The admitted `io.process` grant. Absent, the root Proc runs nothing.
    pub fn set_process_grant(&mut self, grant: &hirsel_run_config::ProcessGrant) {
        self.state.set_process_grant(grant);
    }
    /// The admitted RunConfig rule set of the root Net (`io.net.rules`). Absent, the
    /// root Net permits nothing.
    pub fn set_net_rules(&mut self, rules: std::collections::BTreeSet<NetRule>) {
        self.state.net_rules = rules;
    }
    /// Cooperative admission: a request whose provider parks on `readiness` answers
    /// `AdapterError::Blocked`, and the tape records it and each ready poll (DET-1).
    pub fn set_cooperative(&mut self, readiness: crate::readiness::Readiness) {
        self.cooperative = Some((readiness, Default::default()));
    }
    /// Semantic harness clock, with the ordinary cooperative park/ready tape protocol.
    pub(crate) fn virtual_clock(&mut self, ticks: u64) {
        self.clock_grants(true, false);
        self.set_cooperative(crate::readiness::Readiness::virtual_clock(ticks));
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
            input: Input::Blocking(Box::new(std::io::empty())),
            stderr: Box::new(std::io::sink()),
            recording: None,
            retain_history: true,
            net: Box::new(crate::net_host::HostNet::new()),
            cooperative: None,
            children: Default::default(),
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
    /// The admitted Random provider and the run's Random seed (used only by splitmix64).
    pub fn set_random(&mut self, provider: hirsel_run_config::ProviderContract, seed: u64) {
        self.state.random = match provider {
            hirsel_run_config::ProviderContract::RandomSystem => RandomSource::System,
            _ => RandomSource::SplitMix64(seed),
        };
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
        let mut file_identities = BTreeMap::new();
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
                        tree.metadata.insert(
                            path.clone(),
                            Metadata {
                                modified_unix_ns: None,
                                ..entry_metadata(&meta)
                            },
                        );
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
                                tree.metadata.remove(&path);
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
                        file_identities.insert(logical.clone(), (meta.dev(), meta.ino()));
                        if let Ok(bytes) = self.disk.read(&logical) {
                            tree.files.insert(path, bytes);
                        } else {
                            tree.metadata.insert(
                                path,
                                Metadata {
                                    modified_unix_ns: None,
                                    ..entry_metadata(&meta)
                                },
                            );
                        }
                    } else {
                        tree.metadata.insert(
                            path,
                            Metadata {
                                modified_unix_ns: None,
                                ..entry_metadata(&meta)
                            },
                        );
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
            tree.metadata.extend(
                snapshot
                    .metadata
                    .iter()
                    .map(|(p, m)| (relative(selector, p).expect("snapshot metadata"), m.clone())),
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
        let mut canonical = BTreeMap::<(u64, u64), String>::new();
        let paths: Vec<_> = tree.regular_paths().into_iter().cloned().collect();
        for path in paths {
            let identity = file_identities[&path];
            if let Some(first) = canonical.get(&identity) {
                tree.file_aliases.insert(path.clone(), first.clone());
            } else {
                canonical.insert(identity, path.clone());
            }
        }
        Ok(tree)
    }
}
impl Adapter for RealIo {
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        self.state.preflight(request)
    }

    fn tls_roots(&mut self, stream: u64, pem: &[u8]) {
        self.state.remember_tls_roots(stream, pem);
        self.net.tls_roots(pem);
    }
    fn tls_identity(&mut self, stream: u64, chain: &[u8], key: &[u8]) {
        self.state.remember_tls_identity(stream, chain, key);
    }
    fn tls_server(
        &mut self,
        stream: u64,
        identities: &[(&[u8], &[u8])],
        clients: crate::net::ClientAuth<&[u8]>,
    ) {
        self.state.remember_tls_server(stream, identities, clients);
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        if self.cooperative.is_none() || !crate::host_waits::may_park(&request) {
            return self.perform(request);
        }
        let retry = request.clone();
        match self.perform(request) {
            Err(AdapterError::Blocked) => {
                let (readiness, log) = self.cooperative.as_mut().unwrap();
                let committed = !readiness.cancellable(log.current());
                log.park(committed);
                self.record(Event {
                    request: retry,
                    completion: Completion::Blocked { committed },
                    fault: None,
                })?;
                Err(AdapterError::Blocked)
            }
            result => result,
        }
    }
    fn enter(&mut self, token: u64) {
        if let Some((readiness, log)) = &mut self.cooperative {
            readiness.enter(token);
            log.enter(token);
        }
    }
    fn finish(&mut self, token: u64) {
        if let Some((readiness, log)) = &mut self.cooperative {
            readiness.finish(token);
            log.finish(token);
        }
    }
    fn cancellable(&self, token: u64) -> bool {
        self.cooperative
            .as_ref()
            .is_none_or(|(readiness, _)| readiness.cancellable(token))
    }
    fn ready(&mut self, token: u64) -> Result<bool, AdapterError> {
        let Some((readiness, log)) = &mut self.cooperative else {
            return Ok(true);
        };
        let ready = readiness.ready(token)?;
        let Some(row) = log.row(token) else {
            return Ok(ready);
        };
        if !ready {
            row.polls += 1;
            return Ok(false);
        }
        let event = Event {
            request: Request::Ready {
                wait: row.ordinal,
                polls: std::mem::take(&mut row.polls),
            },
            completion: Completion::Ready,
            fault: None,
        };
        self.record(event)?;
        Ok(true)
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        match &self.cooperative {
            Some((readiness, _)) => readiness.wait(),
            None => Ok(false),
        }
    }
}
impl RealIo {
    fn perform(&mut self, request: Request) -> Result<Completion, AdapterError> {
        let fake = self.state.is_fake_request(&request);
        let mut fault = if self.state.preflight(&request)?.is_some()
            || self.state.denied_projection(&request)
            || crate::net::is_net_request(&request)
            || self.state.is_process(&request)
            || matches!(&request, Request::ReadStdin { stdin, .. } if self.state.expect(*stdin, crate::handles::Grant::Stdin).is_err())
            || matches!(
                request,
                Request::Stdin { .. }
                    | Request::Stderr { .. }
                    | Request::ReadStdin { max: 0, .. }
                    | Request::EnvGet { .. }
                    | Request::EnvNarrow { .. }
            ) {
            None
        } else {
            self.fault.take()
        };
        self.disk.partial = None;
        let before = self.state.stdout.len();
        let stderr_before = self.state.stderr.len();
        let result = match &request {
            Request::ReadStdin { stdin, max } => {
                // Validate authority before touching the provider, including injected faults.
                match self.state.expect(*stdin, crate::handles::Grant::Stdin) {
                    Err(error) => Err(error),
                    Ok(()) => {
                        let value = if *max == 0 {
                            Err(InputError::InvalidInput)
                        } else if let Some(fault) = &fault {
                            match fault.error {
                                IoError::Fault(code) => Err(InputError::Fault(code)),
                                IoError::Canceled(cause) => Err(InputError::Canceled(cause)),
                                _ => return Err(AdapterError::InvalidCompletion),
                            }
                        } else {
                            let len = usize::try_from(*max).unwrap_or(usize::MAX).min(64 * 1024);
                            let mut bytes = vec![0; len];
                            loop {
                                let read = match &mut self.input {
                                    Input::Blocking(input) => input.read(&mut bytes).map(Some),
                                    Input::Cooperative(input) => input.read(&mut bytes),
                                };
                                match read {
                                    Ok(Some(count)) => {
                                        bytes.truncate(count);
                                        break Ok(bytes);
                                    }
                                    // Nothing was consumed or recorded; the same request retries.
                                    Ok(None) => return Err(AdapterError::Blocked),
                                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                                        continue;
                                    }
                                    Err(e) => {
                                        let IoError::Fault(code) = stream_error(e) else {
                                            unreachable!()
                                        };
                                        break Err(InputError::Fault(code));
                                    }
                                }
                            }
                        };
                        fault = None;
                        self.state.sample(&request, Completion::Input(value))
                    }
                }
            }
            Request::Monotonic { .. } | Request::Wall { .. } => {
                let input = match (&request, fault.as_ref(), &self.cooperative) {
                    (Request::Monotonic { .. }, None, Some((readiness, _)))
                        if readiness.virtual_time().is_some() =>
                    {
                        Completion::Monotonic(Ok(readiness.virtual_time().unwrap()))
                    }
                    _ => clock_input(&request, fault.as_ref()),
                };
                self.state.sample(&request, input)
            }
            Request::RandomBytes { random, count } => {
                self.state.expect(*random, crate::handles::Grant::Random)?;
                let input = if fault.is_some() {
                    Err(RandomError::Unavailable)
                } else {
                    self.state.random.draw(*count)
                };
                self.state.sample(&request, Completion::Random(input))
            }
            Request::SleepUntil { clock, deadline } => {
                self.state
                    .expect(*clock, crate::handles::Grant::MonotonicClock)?;
                let input = match &fault {
                    Some(fault) => Err(WaitError::Clock(match fault.error {
                        IoError::Fault(1) => ClockError::OutOfRange,
                        _ => ClockError::Unavailable,
                    })),
                    // Under cooperative admission a wait not yet due is a deadline-only
                    // interest under the entered token; the request is reissued when due.
                    None => match &self.cooperative {
                        Some((readiness, _)) => match readiness.timer(*deadline)? {
                            true => Ok(()),
                            false => return Err(AdapterError::Blocked),
                        },
                        None => sleep_until(*deadline).map_err(WaitError::Clock),
                    },
                };
                self.state.sample(&request, Completion::Wait(input))
            }
            _ if self.state.is_process(&request) => {
                let directories = &self.disk.directories;
                let cooperative = self.cooperative.as_ref().map(|(readiness, _)| readiness);
                let children = &mut self.children;
                let cwd = |d: u64| directories.get(&d).map(|pin| pin.as_raw_fd());
                self.state
                    .execute_process(&mut |call| children.call(call, &cwd, cooperative), &request)
            }
            _ => self
                .state
                .execute(&mut self.disk, &mut *self.net, &request, fault.as_ref()),
        };
        if matches!(
            request,
            Request::Monotonic { .. }
                | Request::Wall { .. }
                | Request::RandomBytes { .. }
                | Request::SleepUntil { .. }
        ) {
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
        // A Test report (a failed check_case, a scripted client's unmet Expect) is
        // the only stdout a request other than Print appends.
        if !matches!(request, Request::Print { .. }) && self.state.stdout.len() > before {
            self.output
                .write_all(&self.state.stdout[before..])
                .and_then(|()| self.output.flush())
                .map_err(|e| AdapterError::RecordingFailed(format!("test report: {e}")))?;
            if !self.retain_history {
                self.state.stdout.truncate(before);
            }
        }
        if matches!(request, Request::Print { .. } | Request::PrintStderr { .. }) {
            let stderr = matches!(request, Request::PrintStderr { .. });
            let output = if stderr {
                &mut self.stderr
            } else {
                &mut self.output
            };
            let captured = if stderr {
                &mut self.state.stderr
            } else {
                &mut self.state.stdout
            };
            let before = if stderr { stderr_before } else { before };
            let bytes = &captured[before..];
            let mut written = 0;
            while written < bytes.len() {
                match output.write(&bytes[written..]) {
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
                        let e = stream_error(e);
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
            if let Err(e) = output.flush() {
                let e = stream_error(e);
                completion = Completion::Unit(Err(e.clone()));
                fault = Some(Fault {
                    error: e,
                    partial: written,
                });
            }
            captured.truncate(if self.retain_history {
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
                | Completion::Entries(Err(e))
                | Completion::Metadata(Err(e))
                    if *e != IoError::NotGranted =>
                {
                    Some(e.clone())
                }
                _ => None,
            };
            fault = error.map(|error| Fault { error, partial: 0 });
        }
        match (&request, &completion) {
            (Request::Open { .. } | Request::Create { .. }, Completion::Handle(Ok(id)))
                if !fake =>
            {
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
    // Portable conditions, never expose host errno as language data.
    match e.kind() {
        std::io::ErrorKind::NotFound => IoError::NotFound,
        std::io::ErrorKind::PermissionDenied => IoError::PermissionDenied,
        std::io::ErrorKind::AlreadyExists => IoError::AlreadyExists,
        std::io::ErrorKind::IsADirectory => IoError::IsDirectory,
        std::io::ErrorKind::NotADirectory => IoError::NotDirectory,
        std::io::ErrorKind::DirectoryNotEmpty => IoError::DirectoryNotEmpty,
        std::io::ErrorKind::CrossesDevices => IoError::CrossDevice,
        _ => IoError::Fault(1),
    }
}
fn stream_error(_: std::io::Error) -> IoError {
    IoError::Fault(1)
}

fn entry_metadata(meta: &std::fs::Metadata) -> Metadata {
    Metadata {
        kind: if meta.is_file() {
            FileKind::Regular
        } else if meta.is_dir() {
            FileKind::Directory
        } else if meta.file_type().is_symlink() {
            FileKind::Symlink
        } else {
            FileKind::Other
        },
        size: meta.len(),
        permissions: meta.mode() & 0o777,
        modified_unix_ns: Some(
            i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()),
        ),
    }
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
impl Disk {
    /// fsync every directory from the parent of tree path `path` up to its mount's root.
    fn sync_directories(&self, path: &str) -> Result<(), IoError> {
        let (selector, mount) = self
            .mounts
            .iter()
            .filter(|(selector, _)| {
                selector.is_empty()
                    || path == selector.as_str()
                    || path.starts_with(&format!("{selector}/"))
            })
            .max_by_key(|(selector, _)| selector.len())
            .ok_or(IoError::NotGranted)?;
        let mut directory = path.rsplit_once('/').map_or("", |(p, _)| p);
        loop {
            let local = if selector.is_empty() {
                directory
            } else if directory.len() <= selector.len() {
                ""
            } else {
                &directory[selector.len() + 1..]
            };
            open_beneath(
                mount.descriptor().as_raw_fd(),
                local,
                libc::O_RDONLY | libc::O_DIRECTORY,
                0,
            )?
            .sync_all()
            .map_err(error)?;
            if local.is_empty() {
                return Ok(());
            }
            directory = directory.rsplit_once('/').map_or("", |(p, _)| p);
        }
    }
    /// Whether `name` in `parent` is a regular file holding exactly `bytes`.
    fn holds(parent: &File, name: &str, bytes: &[u8]) -> Result<bool, IoError> {
        let mut existing = match open_component(
            parent.as_raw_fd(),
            name,
            libc::O_RDONLY | libc::O_NONBLOCK,
            0,
        ) {
            Ok(fd) => fd,
            // A final symlink (ELOOP) or a special entry is an existing non-equal entry.
            Err(IoError::NotFound) => return Err(IoError::NotFound),
            Err(_) => return Ok(false),
        };
        if !existing.metadata().map_err(error)?.is_file() {
            return Ok(false);
        }
        let mut current = vec![];
        existing.read_to_end(&mut current).map_err(error)?;
        Ok(current == bytes)
    }
}
impl Storage for Disk {
    fn sync_file(&mut self, id: u64, path: &str) -> Result<(), IoError> {
        self.files
            .get(&id)
            .ok_or(IoError::Fault(1))?
            .sync_all()
            .map_err(error)?;
        self.sync_directories(path)
    }
    fn put_new(&mut self, path: &str, bytes: &[u8], durable: bool) -> Result<(), IoError> {
        // The Dir's own root always exists.
        if self.local(path)?.is_empty() {
            return Err(IoError::AlreadyExists);
        }
        let (parent, name) = self.parent(path)?;
        let name = name.into_string().map_err(|_| IoError::Fault(1))?;
        // A hidden temporary in the destination's directory, whole before it is linked.
        let (temporary, mut fd) = loop {
            let serial = self.next_write;
            self.next_write = serial.checked_add(1).expect("write names exhausted");
            let temporary = format!(".hirsel-put-{serial}");
            match open_component(
                parent.as_raw_fd(),
                &temporary,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            ) {
                Ok(fd) => break (CString::new(temporary).expect("NUL-free name"), fd),
                Err(IoError::AlreadyExists) => {}
                Err(e) => return Err(e),
            }
        };
        let unlink = |temporary: &CString| {
            // SAFETY: a live directory fd and a NUL-terminated local name.
            unsafe { libc::unlinkat(parent.as_raw_fd(), temporary.as_ptr(), 0) };
        };
        let written = fd
            .write_all(bytes)
            .and_then(|()| if durable { fd.sync_all() } else { Ok(()) });
        drop(fd);
        if let Err(e) = written {
            unlink(&temporary);
            return Err(error(e));
        }
        let target = CString::new(name.as_str()).map_err(|_| IoError::Fault(1))?;
        // SAFETY: live directory fds and NUL-terminated local names; linkat never replaces
        // an existing entry and, without AT_SYMLINK_FOLLOW, never follows the source.
        let linked = unsafe {
            libc::linkat(
                parent.as_raw_fd(),
                temporary.as_ptr(),
                parent.as_raw_fd(),
                target.as_ptr(),
                0,
            )
        };
        let link_error = (linked != 0).then(std::io::Error::last_os_error);
        unlink(&temporary);
        match link_error {
            None if durable => self.sync_directories(path),
            None => Ok(()),
            Some(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                match Self::holds(&parent, &name, bytes)? {
                    true if durable => self.sync_directories(path),
                    true => Ok(()),
                    false => Err(IoError::AlreadyExists),
                }
            }
            Some(e) => Err(error(e)),
        }
    }
    fn destination(&mut self, path: &str) -> Result<(), IoError> {
        if self.local(path)?.is_empty() {
            return Err(IoError::IsDirectory);
        }
        self.parent(path)?;
        match self.open(path, libc::O_PATH, 0) {
            Ok(fd) => {
                if fd.metadata().map_err(error)?.is_file() {
                    Ok(())
                } else {
                    Err(IoError::IsDirectory)
                }
            }
            Err(IoError::NotFound) => Ok(()),
            Err(e) => Err(e),
        }
    }
    fn activate(&mut self, id: u64, root: &str) {
        self.active = root.into();
        self.active_id = id;
    }
    fn directory(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        let fd = self.open(path, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
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
            return Err(IoError::IsDirectory);
        }
        self.last_open = Some(fd);
        Ok(())
    }
    fn create_file(&mut self, path: &str, _id: u64) -> Result<(), IoError> {
        // Open and check before truncating: special files never receive writes.
        let fd = self.open(
            path,
            libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND | libc::O_NONBLOCK,
            0o600,
        )?;
        if !fd.metadata().map_err(error)?.is_file() {
            return Err(IoError::IsDirectory);
        }
        fd.set_len(0).map_err(error)?;
        self.last_open = Some(fd);
        Ok(())
    }
    fn read_file(&mut self, id: u64, offset: u64, max: u64) -> Result<Vec<u8>, IoError> {
        let file = self.files.get(&id).ok_or(IoError::Fault(1))?;
        let mut bytes = vec![0; max.min(65536) as usize];
        let n = loop {
            match file.read_at(&mut bytes, offset) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                other => break other.map_err(error)?,
            }
        };
        bytes.truncate(n);
        Ok(bytes)
    }
    fn write_file(&mut self, id: u64, bytes: &[u8], all: bool) -> Result<u64, IoError> {
        let file = self.files.get_mut(&id).ok_or(IoError::Fault(1))?;
        let mut written = 0;
        while written < bytes.len() {
            match file.write(&bytes[written..]) {
                Ok(0) => {
                    let e = IoError::Fault(1);
                    self.partial = Some((e.clone(), written));
                    return Err(e);
                }
                Ok(n) => {
                    written += n;
                    if !all {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    let e = error(e);
                    self.partial = Some((e.clone(), written));
                    return Err(e);
                }
            }
        }
        Ok(written as u64)
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, IoError> {
        let mut fd = self.open(path, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        if !fd.metadata().map_err(error)?.is_file() {
            return Err(IoError::IsDirectory);
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
                    Err(IoError::AlreadyExists) => {}
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
            return Err(IoError::IsDirectory);
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
        if self.local(path)?.is_empty() {
            return Err(IoError::IsDirectory);
        }
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
    fn move_entry(&mut self, from: &str, to: &str) -> Result<(), IoError> {
        let (src, a) = self.parent(from)?;
        let (dst, b) = self.parent(to)?;
        // SAFETY: pinned confined parents; renameat never follows either leaf.
        if unsafe { libc::renameat(src.as_raw_fd(), a.as_ptr(), dst.as_raw_fd(), b.as_ptr()) } != 0
        {
            let failure = std::io::Error::last_os_error();
            if failure.kind() == std::io::ErrorKind::AlreadyExists
                && open_component(
                    dst.as_raw_fd(),
                    b.to_str().map_err(|_| IoError::Fault(1))?,
                    libc::O_PATH,
                    0,
                )
                .and_then(|pin| pin.metadata().map_err(error))
                .is_ok_and(|m| m.is_dir())
            {
                Err(IoError::DirectoryNotEmpty)
            } else {
                Err(error(failure))
            }
        } else {
            self.prepared.remove(&(self.active_id, from.into()));
            self.prepared.remove(&(self.active_id, to.into()));
            Ok(())
        }
    }
    fn mkdir(&mut self, path: &str) -> Result<(), IoError> {
        let (parent, name) = self.parent(path)?;
        // SAFETY: live pinned directory and terminated leaf; mkdirat does not follow the leaf.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o777) } != 0 {
            Err(error(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
    fn remove_dir(&mut self, path: &str) -> Result<(), IoError> {
        let (parent, name) = self.parent(path)?;
        // SAFETY: pinned confined directory; AT_REMOVEDIR refuses symlinks and nonempty dirs.
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            Err(error(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
    fn set_permissions(&mut self, path: &str, permissions: u32) -> Result<(), IoError> {
        let pin = if self.local(path)?.is_empty() {
            self.base()?.try_clone().map_err(error)?
        } else {
            let (parent, name) = self.parent(path)?;
            open_component(
                parent.as_raw_fd(),
                name.to_str().map_err(|_| IoError::Fault(1))?,
                libc::O_PATH,
                0,
            )?
        };
        let meta = pin.metadata().map_err(error)?;
        if !meta.is_file() && !meta.is_dir() {
            return Err(IoError::Fault(1));
        }
        // Like the resolver's data reopen, this trusted magic path names the pinned
        // inode, not a mutable directory entry. O_PATH needs no file read permission.
        let pinned = CString::new(format!("/proc/self/fd/{}", pin.as_raw_fd())).unwrap();
        // SAFETY: pinned remains live and pinned is a terminated descriptor path.
        if unsafe { libc::chmod(pinned.as_ptr(), permissions as libc::mode_t) } != 0 {
            Err(error(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
    fn remove_all(&mut self, path: &str, limit: Option<usize>) -> Result<(), IoError> {
        let (parent, name) = self.parent(path)?;
        let pin = open_component(
            parent.as_raw_fd(),
            name.to_str().map_err(|_| IoError::Fault(1))?,
            libc::O_PATH,
            0,
        )?;
        let meta = pin.metadata().map_err(error)?;
        if !meta.is_dir() && !meta.file_type().is_symlink() {
            return Err(IoError::NotDirectory);
        }
        // Adapted from Rust's openat/unlinkat remove_dir_all. An explicit work
        // stack bounds the Rust call stack; every descent opens with O_NOFOLLOW.
        enum Work {
            Enter(Rc<File>, CString),
            Leave(Rc<File>, CString),
        }
        let mut work = vec![Work::Enter(Rc::new(parent), name)];
        let mut removed = 0;
        let result = (|| {
            while let Some(item) = work.pop() {
                if limit == Some(removed) {
                    return Ok(());
                }
                let (parent, name, flags) = match item {
                    Work::Leave(parent, name) => (parent, name, libc::AT_REMOVEDIR),
                    Work::Enter(parent, name) => {
                        // SAFETY: parent is pinned, name is a terminated local leaf.
                        let fd = unsafe {
                            libc::openat(
                                parent.as_raw_fd(),
                                name.as_ptr(),
                                libc::O_RDONLY
                                    | libc::O_DIRECTORY
                                    | libc::O_NOFOLLOW
                                    | libc::O_CLOEXEC,
                            )
                        };
                        if fd >= 0 {
                            // SAFETY: openat returned a fresh descriptor.
                            let directory = Rc::new(unsafe { File::from_raw_fd(fd) });
                            let children = std::fs::read_dir(format!(
                                "/proc/self/fd/{}",
                                directory.as_raw_fd()
                            ))
                            .map_err(error)?;
                            work.push(Work::Leave(parent, name));
                            let mut names = vec![];
                            for entry in children {
                                use std::os::unix::ffi::OsStrExt;
                                let entry = entry.map_err(error)?;
                                names.push(
                                    CString::new(entry.file_name().as_bytes())
                                        .map_err(|_| IoError::Fault(1))?,
                                );
                            }
                            names.sort();
                            for name in names.into_iter().rev() {
                                work.push(Work::Enter(directory.clone(), name));
                            }
                            continue;
                        }
                        let e = std::io::Error::last_os_error();
                        if !matches!(e.raw_os_error(), Some(libc::ENOTDIR | libc::ELOOP)) {
                            return Err(error(e));
                        }
                        (parent, name, 0)
                    }
                };
                // SAFETY: only the pinned parent and local leaf are used. unlinkat
                // cannot follow a symlink even if the entry was concurrently replaced.
                if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) } != 0 {
                    return Err(error(std::io::Error::last_os_error()));
                }
                removed += 1;
            }
            Ok(())
        })();
        if let Err(ref failure) = result {
            self.partial = Some((failure.clone(), removed));
        }
        result
    }

    fn metadata(&mut self, path: &str) -> Result<Metadata, IoError> {
        let pin = if self.local(path)?.is_empty() {
            self.base()?.try_clone().map_err(error)?
        } else {
            let (parent, name) = self.parent(path)?;
            open_component(
                parent.as_raw_fd(),
                name.to_str().map_err(|_| IoError::Fault(1))?,
                libc::O_PATH,
                0,
            )?
        };
        Ok(entry_metadata(&pin.metadata().map_err(error)?))
    }
    fn read_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, IoError> {
        let fd = self.open(path, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
        let mut entries = vec![];
        for entry in
            std::fs::read_dir(format!("/proc/self/fd/{}", fd.as_raw_fd())).map_err(error)?
        {
            let name = entry
                .map_err(error)?
                .file_name()
                .into_string()
                .map_err(|_| IoError::Fault(1))?;
            if name.contains(['\\', '\0']) {
                return Err(IoError::Fault(1));
            }
            let pin = open_component(fd.as_raw_fd(), &name, libc::O_PATH, 0)?;
            let kind = entry_metadata(&pin.metadata().map_err(error)?).kind;
            entries.push(DirEntry { path: name, kind });
        }
        entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
        Ok(entries)
    }
    fn walk(&mut self, root: &str) -> Result<Vec<String>, IoError> {
        self.entries(root).map(|(f, _)| f)
    }
}
fn open_beneath(fd: RawFd, path: &str, flags: i32, mode: u64) -> Result<File, IoError> {
    let root = open_component(fd, ".", libc::O_PATH | libc::O_DIRECTORY, 0)?;
    resolve_beneath(vec![Rc::new(root)], path, flags, mode).map(|(file, _)| file)
}

/// docs/spec/core-library.md: "Directory paths are relative; absolute paths, traversal outside the
/// root (including resolved links) return FileFault
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
                return Err(IoError::AlreadyExists);
            }
            if flags & libc::O_DIRECTORY != 0 && !meta.is_dir() {
                return Err(IoError::NotDirectory);
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
            return Err(IoError::NotDirectory);
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

/// Block the thread until CLOCK_MONOTONIC reaches `deadline` nanoseconds; an
/// absolute wait cannot drift when a signal restarts it. Adapted from Zig
/// lib/std/Io/Threaded.zig sleepPosix (third-party/NOTICES.md).
fn sleep_until(deadline: u64) -> Result<(), ClockError> {
    let time = libc::timespec {
        tv_sec: (deadline / 1_000_000_000) as libc::time_t,
        tv_nsec: (deadline % 1_000_000_000) as libc::c_long,
    };
    loop {
        // SAFETY: time is a valid timespec; no remainder is requested for TIMER_ABSTIME.
        match unsafe {
            libc::clock_nanosleep(
                libc::CLOCK_MONOTONIC,
                libc::TIMER_ABSTIME,
                &time,
                std::ptr::null_mut(),
            )
        } {
            0 => return Ok(()),
            libc::EINTR => continue,
            _ => return Err(ClockError::Unavailable),
        }
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
