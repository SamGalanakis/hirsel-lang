//! Real worker I/O. Tokio ce685ee6 tokio/src/fs/mod.rs:3-23 supplies the
//! blocking-call boundary; Go ebe2feef src/internal/poll/fd_mutex.go:9-55
//! supplies per-resource lifetime/serialization. Hirsel uses logical key epochs
//! rather than physical fd locks, and one namespace domain for cross-mount aliases.
use super::*;
use crate::io_concurrent::ConcurrentAdapter;
use crate::io_keys::{Key, KeySet, KeyTable, Kind, Registry};
use std::sync::Mutex;

/// The bit a live Ordered run's home binding sets on its tokens; service
/// request tokens stay below 2^48 and readiness run tags occupy bits 48 through 61.
/// Bit 62 belongs to Free scope timers, which the service must receive intact.
/// The scheduler strips its separate concurrent-wait bit before adapter calls.
pub(crate) const HOME_TOKEN: u64 = 1 << 63;

/// Real host I/O callable on any worker. Only bounded calls hold resource positions;
/// providers register readiness and return before a process/socket/timer wait.
pub struct ConcurrentIo {
    core: Mutex<RealCore<SendStreams>>,
    registry: Mutex<Registry>,
    calls: KeyTable,
    // TLS roots belong to a stream until its first bounded handshake attempt.
    // A provider's single staging slot must never couple concurrent callers.
    tls_pem: Mutex<std::collections::BTreeMap<u64, Vec<u8>>>,
    /// Ready tokens one poll found for the other side: [service, home binding].
    /// Readiness hands each finding out once, and both sides share it.
    deferred: Mutex<[Vec<u64>; 2]>,
    /// Whether events are recorded, each request event with its footprint.
    records: bool,
    #[cfg(test)]
    before_file: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    before_path: Option<Arc<dyn Fn(&Request) + Send + Sync>>,
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
        let records = core.records_events();
        let registry = core.take_registry();
        Self {
            core: Mutex::new(core),
            registry: Mutex::new(registry),
            calls: KeyTable::default(),
            tls_pem: Default::default(),
            deferred: Default::default(),
            records,
            #[cfg(test)]
            before_file: None,
            #[cfg(test)]
            before_path: None,
        }
    }
    pub(crate) fn cpu_control(
        &self,
        width: u32,
        action: u32,
        count: u64,
        value: Option<u32>,
    ) -> Result<Option<u32>, AdapterError> {
        self.core
            .lock()
            .unwrap()
            .cpu_control(width, action, count, value)
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
    /// The ready parked tokens of one side: the run's service, or (`home`) the
    /// home binding of a live Ordered run, whose tokens carry [`HOME_TOKEN`].
    /// The other side's findings wait for its own next poll.
    pub(crate) fn ready_for(&self, home: bool) -> Result<Ready, AdapterError> {
        let Ready::Tokens(tokens) = self.core.lock().unwrap().ready()? else {
            return Ok(Ready::Every);
        };
        let mut deferred = self.deferred.lock().unwrap();
        for token in tokens {
            deferred[usize::from(token & HOME_TOKEN != 0)].push(token);
        }
        Ok(Self::take(&mut deferred, home))
    }
    /// The service's share of the poll the home binding made at this same
    /// scheduling point. A point polls the core once: a second poll would
    /// count a wait-log point that replay, with a single adapter, never takes.
    pub(crate) fn ready_after_home(&self, home: &Ready) -> Ready {
        if matches!(home, Ready::Every) {
            return Ready::Every;
        }
        Self::take(&mut self.deferred.lock().unwrap(), false)
    }
    fn take(deferred: &mut [Vec<u64>; 2], home: bool) -> Ready {
        let mut mine: Vec<u64> = std::mem::take(&mut deferred[usize::from(home)])
            .into_iter()
            .map(|token| token & !HOME_TOKEN)
            .collect();
        mine.sort_unstable();
        mine.dedup();
        Ready::Tokens(mine)
    }
    /// The admitted mounts' pins, for destinations checked after the run.
    pub fn with_mount_pins<T>(
        &self,
        f: impl FnOnce(&[(&str, &crate::host_paths::PinnedDirectory)]) -> T,
    ) -> T {
        f(&self.core.lock().unwrap().mount_pins())
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
                | Request::CpuControl { .. }
                | Request::ProcessTime { .. }
                | Request::ProcessSettle { .. }
        ) {
            return Err(AdapterError::InvalidCompletion);
        }
        let keys = self.keys(&request);
        let domains = if self.records {
            self.domains(&request)
        } else {
            Vec::new()
        };
        let positions = self.calls.take(&keys);
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
        // Path syscalls likewise run outside the core lock: the resource and
        // namespace positions pin the directory grant and path through them.
        if file.is_none()
            && preflight.is_none()
            && core.fault.is_none()
            && !core.journal.durable()
            && !core.state.is_fake_request(&request)
            && let Some((directory, root, path)) = core.state.path_target(&request)
            && let Ok((base, active)) = core.disk.detach(directory, &root)
        {
            drop(core);
            #[cfg(test)]
            if let Some(hook) = &self.before_path {
                hook(&request);
            }
            let result = super::At {
                base: &base,
                active: &active,
            }
            .call(&request, &path);
            core = self.core.lock().unwrap();
            core.enter(token);
            core.disk.prefetched_path = Some(result);
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
        let publication = super::discovery::Publication::of(&core, &request);
        core.disk.observed_output = None;
        if let Request::TlsConnect { stream, .. } = &request
            && let Some(pem) = self.tls_pem.lock().unwrap().remove(stream)
        {
            core.tls_roots(*stream, &pem);
        }
        if let Request::Close { handle } = &request {
            self.tls_pem.lock().unwrap().remove(handle);
        }
        core.stage();
        let result = core.call_once(request);
        *observed = core.disk.observed_output.take();
        debug_assert!(
            core.disk.prefetched.is_none()
                && core.disk.prefetched_output.is_none()
                && core.disk.prefetched_path.is_none()
        );
        let mut registry = self.registry.lock().unwrap();
        publication.publish(&core, &mut registry, &result);
        // Closed physical words never get reused. Retain resource metadata for the
        // owner that is publishing a consuming operation's result.
        core.commit_with(&mut registry, keys, domains)?;
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
        let core = self.core.lock().unwrap();
        super::discovery::domains(&core, &mut self.registry.lock().unwrap(), request)
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
    fn net(&self, host: fn(Option<&crate::readiness::Readiness>) -> Box<dyn NetProvider + Send>) {
        let mut core = self.core.lock().unwrap();
        if core.net.is_none() {
            core.net = Some(host(core.readiness().as_ref()));
        }
    }
    fn process(&self, host: fn() -> Box<dyn crate::process::ProcessHost + Send>) {
        self.core.lock().unwrap().children.get_or_insert_with(host);
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
                | Request::CpuControl { .. }
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
        self.ready_for(false)
    }
    fn ready_after_home(&self, home: &Ready) -> Result<Ready, AdapterError> {
        Ok(ConcurrentIo::ready_after_home(self, home))
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
mod footprint_tests;
#[cfg(test)]
mod tests;
