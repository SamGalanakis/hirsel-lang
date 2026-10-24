//! Real worker I/O. Tokio ce685ee6 tokio/src/fs/mod.rs:3-23 supplies the
//! blocking-call boundary; Go ebe2feef src/internal/poll/fd_mutex.go:9-55
//! supplies per-resource lifetime/serialization. Hirsel uses logical key epochs
//! rather than physical fd locks, and one namespace domain for cross-mount aliases.
use super::*;
use crate::io_concurrent::ConcurrentAdapter;
use crate::io_keys::{Key, KeySet, KeyTable, Kind, Registry};
use std::sync::Mutex;

/// Real host I/O callable on any worker. Only bounded calls hold resource positions;
/// providers register readiness and return before a process/socket/timer wait.
pub struct ConcurrentIo {
    core: Mutex<RealCore<SendStreams>>,
    registry: Mutex<Registry>,
    calls: KeyTable,
    // TLS roots belong to a stream until its first bounded handshake attempt.
    // A provider's single staging slot must never couple concurrent callers.
    tls_pem: Mutex<std::collections::BTreeMap<u64, Vec<u8>>>,
    #[cfg(test)]
    before_file: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[derive(Debug)]
pub(super) enum FileResult {
    Read(Result<Vec<u8>, IoError>),
    Write(Result<u64, IoError>, Option<(IoError, usize)>),
}

impl ConcurrentIo {
    pub(super) fn new(mut core: RealCore<SendStreams>) -> Self {
        // Every potentially blocking process/network request must register an event source.
        let readiness = core.readiness().unwrap_or_default();
        readiness.mark_shared();
        core.set_cooperative_net(readiness);
        if core.net.is_none() {
            core.net = Some(crate::net_host::send_provider(core.readiness().as_ref()));
        }
        core.children.get_or_insert_with(crate::process::send_host);
        let mut registry = Registry::default();
        // Discover all admitted storage before the first request, even before its
        // language grant is projected. Unknown writers may alias any filesystem.
        registry.sink(u64::MAX - 1, core.streams.output.resource_storage());
        registry.sink(u64::MAX - 2, core.streams.stderr.resource_storage());
        registry.sink(u64::MAX - 3, input_storage(&core.streams.input));
        for reserved in [u64::MAX - 1, u64::MAX - 2, u64::MAX - 3] {
            registry.forget(reserved);
        }
        for (handle, grant) in core.state.handles.rows.sorted() {
            discover(&core, &mut registry, handle, grant);
        }
        Self {
            core: Mutex::new(core),
            registry: Mutex::new(registry),
            calls: KeyTable::default(),
            tls_pem: Default::default(),
            #[cfg(test)]
            before_file: None,
        }
    }
    /// Snapshot history for diagnostics; free-mode recording owns its creator-qualified tape.
    pub fn tape(&self) -> Vec<Event> {
        self.core.lock().unwrap().tape.clone()
    }
    pub fn stdout(&self) -> Vec<u8> {
        self.core.lock().unwrap().state.stdout.clone()
    }
    pub fn stderr(&self) -> Vec<u8> {
        self.core.lock().unwrap().state.stderr.clone()
    }
    pub fn finish_recording(&self, outcome: &hirsel_run_schema::RunOutcome) -> std::io::Result<()> {
        self.core.lock().unwrap().finish_recording(outcome)
    }
    pub fn deliver(&self, token: u64, completion: hirsel_run_schema::RunnerCompletion) {
        self.core.lock().unwrap().deliver(token, completion);
    }
    /// Idle driving never holds the core lock: workers must be able to register interests.
    pub fn wait(&self) -> Result<bool, AdapterError> {
        self.wait_sources()?.wait()
    }
    fn attempt(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        self.attempt_observed(token, request, &mut None)
    }
    fn attempt_observed(
        &self,
        token: u64,
        request: Request,
        observed: &mut Option<u64>,
    ) -> Result<Completion, AdapterError> {
        // Scheduling and timed-process recorder notifications are never host call payloads.
        // Reject them explicitly instead of reaching State's filesystem dispatch.
        if matches!(
            request,
            Request::Ready { .. }
                | Request::Schedule { .. }
                | Request::ProcessTime { .. }
                | Request::ProcessSettle { .. }
        ) {
            return Err(AdapterError::InvalidCompletion);
        }
        let positions = self.calls.take(&self.keys(&request));
        self.calls.wait(&positions);
        let mut core = self.core.lock().unwrap();
        core.enter(token);
        // Do every authority/argument check before touching an fd. Fault injection and
        // durable journals retain the ordinary gate (Prepared must precede the syscall).
        let preflight = core.preflight(&request)?;
        let file = match &request {
            Request::FileRead { file, .. }
            | Request::FileWrite { file, .. }
            | Request::FileAppend { file, .. }
                if preflight.is_none()
                    && core.fault.is_none()
                    && !core.journal.durable()
                    && !core.state.is_fake_request(&request) =>
            {
                Some(*file)
            }
            _ => None,
        };
        if let Some(id) = file {
            let file = core
                .disk
                .files
                .get(&id)
                .cloned()
                .ok_or(AdapterError::InvalidHandle(id))?;
            let offset = match core.state.handles.rows.get(&id) {
                Some(crate::handles::Grant::File { position, .. }) => *position,
                _ => return Err(AdapterError::InvalidHandle(id)),
            };
            drop(core);
            #[cfg(test)]
            if let Some(hook) = &self.before_file {
                hook();
            }
            let result = file_call(&file, offset, &request);
            core = self.core.lock().unwrap();
            core.enter(token);
            // Resource positions pin both authority and cursor through validation,
            // syscall and publication; distinct files only share the namespace epoch.
            core.disk.prefetched = Some(result);
        }
        if preflight.is_none()
            && core.fault.is_none()
            && !core.journal.durable()
            && let Request::Print { bytes, .. } | Request::PrintStderr { bytes, .. } = &request
        {
            let mut sink = if matches!(request, Request::PrintStderr { .. }) {
                core.streams.stderr.clone()
            } else {
                core.streams.output.clone()
            };
            drop(core);
            let output = write_output(&mut sink, bytes);
            core = self.core.lock().unwrap();
            core.enter(token);
            core.disk.prefetched_output = Some(output);
        }
        let tls_split = matches!(&request, Request::NetSplit { stream, .. }
            if matches!(core.state.handles.rows.get(stream), Some(crate::handles::Grant::Stream { tls: true, .. })));
        let first_new = core.state.handles.next();
        core.disk.observed_output = None;
        if let Request::TlsConnect { stream, .. } = &request
            && let Some(pem) = self.tls_pem.lock().unwrap().remove(stream)
        {
            core.tls_roots(*stream, &pem);
        }
        if let Request::Close { handle } = &request {
            self.tls_pem.lock().unwrap().remove(handle);
        }
        let result = core.call(request.clone());
        *observed = core.disk.observed_output.take();
        debug_assert!(core.disk.prefetched.is_none() && core.disk.prefetched_output.is_none());
        let mut registry = self.registry.lock().unwrap();
        if let Request::NetSplit { stream, .. } = &request
            && let Ok(Completion::Halves(Ok((read, write)))) = &result
        {
            if tls_split {
                let (parent, _) = registry.identity(*stream).expect("registered TLS stream");
                registry.bind(*read, parent, Kind::Other);
                registry.bind(*write, parent, Kind::Other);
            } else {
                registry.split(*stream, *read, *write);
            }
        }
        if let Request::NarrowFile { file, .. }
        | Request::ReadOnlyDir { directory: file }
        | Request::NarrowDir {
            directory: file, ..
        } = &request
        {
            if let Ok(Completion::OptionalFile(Some(to)) | Completion::Handle(Ok(to))) = &result {
                registry.alias(*file, *to);
            }
        }
        if let Request::NetJoin { read, .. } = &request
            && let Some((key, kind)) = registry.identity(*read)
            && let Ok(Completion::NetHandle(Ok(joined))) = &result
        {
            let parent = match kind {
                Kind::Half { stream } => stream,
                _ => key,
            };
            registry.bind(*joined, parent, Kind::Other);
        }
        for handle in first_new..core.state.handles.next() {
            if registry.identity(handle).is_none()
                && let Some(grant) = core.state.handles.rows.get(&handle)
            {
                discover(&core, &mut registry, handle, grant);
            }
        }
        // Closed physical words never get reused. Retain resource metadata for the
        // owner that is publishing a consuming operation's result.
        result
    }
}

impl ConcurrentAdapter for ConcurrentIo {
    fn namespace_coupled(&self) -> bool {
        self.registry.lock().unwrap().namespace_coupled()
    }
    fn keys(&self, request: &Request) -> KeySet {
        self.registry.lock().unwrap().keys(request)
    }
    fn domains(&self, request: &Request) -> Vec<(Key, u64, String)> {
        use Request as R;
        let core = self.core.lock().unwrap();
        let mut registry = self.registry.lock().unwrap();
        match request {
            R::Connect { net, .. }
            | R::ConnectAddr { net, .. }
            | R::Listen { net, .. }
            | R::UdpBind { net, .. }
            | R::UnixConnect { net, .. }
            | R::UnixListen { net, .. }
                if matches!(
                    core.state.handles.rows.get(net),
                    Some(crate::handles::Grant::Net { .. })
                ) =>
            {
                registry
                    .net_ports(*net)
                    .map(|key| vec![(key, *net, "ports".into())])
                    .unwrap_or_default()
            }
            R::ProcessRun { process, .. }
            | R::ProcessSpawn { process, .. }
            | R::ProcessPipeline { process, .. }
                if matches!(
                    core.state.handles.rows.get(process),
                    Some(crate::handles::Grant::Process { .. })
                ) =>
            {
                registry
                    .inherited_domains()
                    .into_iter()
                    .enumerate()
                    .map(|(index, key)| (key, *process, format!("inherited-{index}")))
                    .collect()
            }
            _ => Vec::new(),
        }
    }
    fn identity(&self, handle: u64) -> Option<(Key, Kind)> {
        self.registry.lock().unwrap().identity(handle)
    }
    fn process_keys(&self, process: u64) -> Option<KeySet> {
        let core = self.core.lock().unwrap();
        if !matches!(
            core.state.handles.rows.get(&process),
            Some(crate::handles::Grant::Process { .. })
        ) {
            return None;
        }
        self.registry.lock().unwrap().process_keys(process)
    }
    fn net_ports(&self, net: u64) -> Option<Key> {
        let core = self.core.lock().unwrap();
        if !matches!(
            core.state.handles.rows.get(&net),
            Some(crate::handles::Grant::Net { .. })
        ) {
            return None;
        }
        self.registry.lock().unwrap().net_ports(net)
    }
    fn tls(&self, engine: &'static dyn crate::net_tls::TlsEngine) {
        self.core.lock().unwrap().tls(engine);
    }
    fn tls_roots(&self, stream: u64, pem: &[u8]) {
        let mut core = self.core.lock().unwrap();
        core.state.remember_tls_roots(stream, pem);
        self.tls_pem.lock().unwrap().insert(stream, pem.to_vec());
    }
    fn tls_identity(&self, stream: u64, chain: &[u8], key: &[u8]) {
        self.core.lock().unwrap().tls_identity(stream, chain, key);
    }
    fn tls_server(
        &self,
        stream: u64,
        identities: &[(&[u8], &[u8])],
        clients: crate::net::ClientAuth<&[u8]>,
        alpn: &[String],
    ) {
        self.core
            .lock()
            .unwrap()
            .tls_server(stream, identities, clients, alpn);
    }
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        if matches!(
            request,
            Request::Ready { .. }
                | Request::Schedule { .. }
                | Request::ProcessTime { .. }
                | Request::ProcessSettle { .. }
        ) {
            return Err(AdapterError::InvalidCompletion);
        }
        self.core.lock().unwrap().preflight(request)
    }
    fn call(&self, request: Request) -> Result<Completion, AdapterError> {
        self.attempt(0, request)
    }
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        self.attempt(token, request)
    }
    fn call_observed_at(
        &self,
        token: u64,
        request: Request,
    ) -> (Result<Completion, AdapterError>, Option<u64>) {
        let mut output = None;
        let result = self.attempt_observed(token, request, &mut output);
        (result, output)
    }
    fn blocking(&self, request: &Request) -> bool {
        use Request as R;
        matches!(
            request,
            R::Open { .. }
                | R::OpenAppend { .. }
                | R::Create { .. }
                | R::FileRead { .. }
                | R::FileWrite { .. }
                | R::FileAppend { .. }
                | R::ReadDirPage { .. }
                | R::Metadata { .. }
                | R::Mkdir { .. }
                | R::Rename { .. }
                | R::RemoveDir { .. }
                | R::SetPermissions { .. }
                | R::RemoveAll { .. }
                | R::Size { .. }
                | R::Read { .. }
                | R::Write { .. }
                | R::Remove { .. }
                | R::CreateAtomic { .. }
                | R::WriteAll { .. }
                | R::Commit { .. }
                | R::Sync { .. }
                | R::PutNew { .. }
        ) && !self.core.lock().unwrap().state.is_fake_request(request)
    }
    fn finish(&self, token: u64) {
        self.core.lock().unwrap().finish(token);
    }
    fn parked(&self, token: u64) {
        self.core.lock().unwrap().parked(token);
    }
    fn cancellable(&self, token: u64) -> bool {
        self.core.lock().unwrap().cancellable(token)
    }
    fn ready(&self) -> Result<Ready, AdapterError> {
        self.core.lock().unwrap().ready()
    }
    fn wait_sources(&self) -> Result<crate::io::WaitSources, AdapterError> {
        Ok(self
            .core
            .lock()
            .unwrap()
            .wait_sources()?
            .expect("real concurrent source preparation"))
    }
    fn readiness(&self) -> Option<crate::readiness::Readiness> {
        self.core.lock().unwrap().readiness()
    }
}

fn input_storage(input: &SendInput) -> crate::io_keys::Storage {
    match input {
        SendInput::Blocking(_, storage) => *storage,
        SendInput::Cooperative(input) => input.storage(),
    }
}

fn discover(
    core: &RealCore<SendStreams>,
    registry: &mut Registry,
    handle: u64,
    grant: &crate::handles::Grant,
) {
    use crate::handles::Grant as G;
    // Repeated and narrowed grants of the same provider share its draw/observation
    // sequence. In particular random's one generator must not get independent keys.
    let alias = core
        .state
        .handles
        .rows
        .sorted()
        .into_iter()
        .find(|(other, candidate)| {
            *other != handle
                && registry.identity(*other).is_some()
                && match (grant, *candidate) {
                    (G::Random, G::Random)
                    | (G::Stdin, G::Stdin)
                    | (G::MonotonicClock, G::MonotonicClock)
                    | (G::WallClock, G::WallClock)
                    | (G::Signals, G::Signals)
                    | (G::Telemetry, G::Telemetry) => true,
                    (G::Net { world: a, .. }, G::Net { world: b, .. }) => a == b,
                    (G::Process { world: a, .. }, G::Process { world: b, .. }) => a == b,
                    _ => false,
                }
        })
        .map(|(other, _)| other);
    if let Some(other) = alias {
        registry.alias(other, handle);
        return;
    }
    match grant {
        G::Stdout => {
            registry.sink(handle, core.streams.output.resource_storage());
        }
        G::Stderr => {
            registry.sink(handle, core.streams.stderr.resource_storage());
        }
        G::Stdin => {
            registry.sink(handle, input_storage(&core.streams.input));
        }
        G::File { .. } | G::AtomicFile { .. } | G::Mapped { .. } => {
            let storage = match grant {
                G::File { .. } => core
                    .disk
                    .files
                    .get(&handle)
                    .map(|file| crate::io_keys::Storage::of_descriptor(file.as_raw_fd())),
                G::AtomicFile {
                    temporary,
                    directory,
                    ..
                } => core
                    .disk
                    .prepared
                    .get(&(*directory, temporary.clone()))
                    .map(|file| crate::io_keys::Storage::of_descriptor(file.as_raw_fd())),
                G::Mapped { .. } => core
                    .disk
                    .maps
                    .get(&handle)
                    .map(crate::mapping::Mapping::storage),
                _ => unreachable!(),
            };
            match storage {
                Some(crate::io_keys::Storage::Descriptor { dev, ino, .. }) => {
                    registry.inode(handle, Kind::File, dev, ino);
                }
                // Scripted files have no host descriptor and cannot alias the host filesystem.
                None => {
                    registry.resource(handle, Kind::File);
                }
                _ => {
                    registry.sink(handle, crate::io_keys::Storage::Unknown);
                }
            }
        }
        G::Watcher { .. } | G::Directory { .. } => {
            registry.resource(handle, Kind::File);
        }
        G::Span { telemetry, .. } => registry.alias(*telemetry, handle),
        G::Half { .. } => {} // registered with their parent before result publication
        _ => {
            registry.resource(handle, Kind::Other);
        }
    }
}

fn file_call(file: &File, offset: u64, request: &Request) -> FileResult {
    if let Request::FileRead { max, .. } = request {
        let mut bytes = vec![0; (*max).min(65536) as usize];
        let result = loop {
            match file.read_at(&mut bytes, offset) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Ok(n) => {
                    bytes.truncate(n);
                    break Ok(bytes);
                }
                Err(e) => break Err(error(e)),
            }
        };
        return FileResult::Read(result);
    }
    let (bytes, all) = match request {
        Request::FileWrite { bytes, .. } => (bytes, false),
        Request::FileAppend { bytes, .. } => (bytes, true),
        _ => unreachable!(),
    };
    let mut file = file;
    let mut written = 0;
    while written < bytes.len() {
        match file.write(&bytes[written..]) {
            Ok(0) => break,
            Ok(n) => {
                written += n;
                if !all {
                    return FileResult::Write(Ok(written as u64), None);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let e = error(e);
                return FileResult::Write(Err(e.clone()), Some((e, written)));
            }
        }
    }
    if written == bytes.len() {
        FileResult::Write(Ok(written as u64), None)
    } else {
        FileResult::Write(Err(IoError::Fault(1)), Some((IoError::Fault(1), written)))
    }
}

#[cfg(test)]
mod tests;
