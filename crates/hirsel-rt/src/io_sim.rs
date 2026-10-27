//! Exact tape replay: validation precedes every visible state change, even handle consumption.
use crate::handles::Handles;
use crate::io::*;
use crate::watch::WatchEvent;
pub use hirsel_run_schema::FileReadScript;

/// Fixture readiness for snapshot files. Existing Blocked/Ready events replay it.
#[derive(Default)]
pub(crate) struct ScriptedFileReads(hirsel_run_schema::FileReadiness);
impl ScriptedFileReads {
    pub(crate) fn new(scripts: std::collections::BTreeMap<String, FileReadScript>) -> Self {
        Self(hirsel_run_schema::FileReadiness::new(scripts))
    }
    pub(crate) fn blocked(&self, state: &State, request: &Request) -> bool {
        let Request::FileRead {
            directory, file, ..
        } = request
        else {
            return false;
        };
        let Some(tree) = state.fake_directories.get(directory) else {
            return false;
        };
        if state.fake_file_stale(*directory, *file) {
            return false;
        }
        let Some(crate::handles::Grant::File { path, .. }) = state.handles.rows.get(file) else {
            return false;
        };
        self.0.blocked(*tree, path)
    }
    pub(crate) fn completed(
        &mut self,
        state: &State,
        request: &Request,
        completion: &Completion,
    ) -> bool {
        let (
            Request::Write {
                directory, path, ..
            },
            Completion::Unit(Ok(())),
        ) = (request, completion)
        else {
            return false;
        };
        let Some(tree) = state.fake_directories.get(directory) else {
            return false;
        };
        let Ok(root) = state.directory(*directory) else {
            return false;
        };
        let Ok(path) = state.fake_trees[tree].resolve(&root, path, false) else {
            return false;
        };
        self.0.wrote(*tree, &path)
    }
}
/// Snapshot-only operation queues. Production requests cannot consume an entry.
#[derive(Default)]
pub(crate) struct ScriptedFileFaults(hirsel_run_schema::FileFaults);
impl ScriptedFileFaults {
    pub(crate) fn new(scripts: hirsel_run_schema::FileFaults) -> Result<Self, String> {
        hirsel_run_schema::validate_file_faults(&scripts)?;
        Ok(Self(scripts))
    }
    pub(crate) fn take(
        &mut self,
        state: &State,
        request: &Request,
    ) -> Result<Option<Fault>, AdapterError> {
        use hirsel_run_schema::{FileFaultError as E, FileOperation as O};
        if !state.is_fake_request(request) || state.preflight(request)?.is_some() {
            return Ok(None);
        }
        let (op, count) = match request {
            Request::Open { .. } => (O::Open, None),
            Request::Create { .. } => (O::Create, None),
            Request::OpenAppend { .. } => (O::OpenAppend, None),
            Request::FileRead { max, .. } => (O::Read, Some(*max)),
            Request::Read { .. } => (O::ReadBytes, None),
            Request::FileWrite { bytes, .. } => (O::Write, Some(bytes.len() as u64)),
            Request::FileAppend { bytes, .. } => (O::Append, Some(bytes.len() as u64)),
            Request::Write { bytes, .. } => (O::WriteBytes, Some(bytes.len() as u64)),
            Request::CreateAtomic { .. } => (O::CreateAtomic, None),
            Request::WriteAll { bytes, .. } => (O::WriteAll, Some(bytes.len() as u64)),
            Request::Commit { .. } => (O::Commit, None),
            Request::Sync { .. } => (O::Sync, None),
            Request::PutNew { bytes, .. } => (O::PutNew, Some(bytes.len() as u64)),
            _ => return Ok(None),
        };
        let step = hirsel_run_schema::take_file_fault(&mut self.0, op, count)
            .map_err(AdapterError::Unsupported)?;
        Ok(step.and_then(|step| {
            step.error.map(|error| Fault {
                partial: step.partial as usize,
                error: match error {
                    E::Fault(code) => IoError::Fault(code),
                    E::Canceled(cause) => IoError::Canceled(cause),
                    E::NotFound => IoError::NotFound,
                    E::PermissionDenied => IoError::PermissionDenied,
                    E::AlreadyExists => IoError::AlreadyExists,
                    E::IsDirectory => IoError::IsDirectory,
                    E::NotDirectory => IoError::NotDirectory,
                    E::DirectoryNotEmpty => IoError::DirectoryNotEmpty,
                    E::CrossDevice => IoError::CrossDevice,
                },
            })
        }))
    }
}
/// Binary replay keeps one shared buffer and only the current/previous decoded
/// event. Advancing releases payload allocations instead of retaining a second
/// owned copy of every event in the tape.
#[derive(Clone, Debug)]
enum ReplayTape {
    Owned(Vec<Event>),
    Binary {
        tape: crate::tape_binary::Tape,
        current: Option<(usize, Event)>,
        previous: Option<(usize, Event)>,
    },
}
impl ReplayTape {
    fn len(&self) -> usize {
        match self {
            Self::Owned(events) => events.len(),
            Self::Binary { tape, .. } => tape.len(),
        }
    }
    fn get(&mut self, index: usize) -> Result<Option<&Event>, AdapterError> {
        match self {
            Self::Owned(events) => Ok(events.get(index)),
            Self::Binary {
                tape,
                current,
                previous,
            } => {
                if index >= tape.len() {
                    return Ok(None);
                }
                if current.as_ref().is_none_or(|(at, _)| *at != index) {
                    *previous = current.take();
                    *current = Some((
                        index,
                        tape.event(index)
                            .map_err(|_| AdapterError::InvalidCompletion)?,
                    ));
                }
                Ok(current.as_ref().map(|(_, event)| event))
            }
        }
    }
    /// An event without moving the decoded window: metadata queries never
    /// advance or reorder replay.
    fn peek(&self, index: usize) -> Option<std::borrow::Cow<'_, Event>> {
        if let Some(event) = self.cached(index) {
            return Some(std::borrow::Cow::Borrowed(event));
        }
        match self {
            Self::Owned(_) => None,
            Self::Binary { tape, .. } => tape.event(index).ok().map(std::borrow::Cow::Owned),
        }
    }
    fn cached(&self, index: usize) -> Option<&Event> {
        match self {
            Self::Owned(events) => events.get(index),
            Self::Binary {
                current, previous, ..
            } => current
                .iter()
                .chain(previous)
                .find(|(at, _)| *at == index)
                .map(|(_, event)| event),
        }
    }
}
/// The resource graph a keyed recording declared (determinism.md, Resource
/// footprints). Replay serves it instead of discovering host storage, and
/// refuses a footprint the graph so far contradicts.
#[derive(Clone, Debug, Default)]
struct Resources {
    /// Whether request events carry footprints, fixed by the first one.
    declared: Option<bool>,
    identities: std::collections::BTreeMap<u64, (crate::io_keys::Key, crate::io_keys::Kind)>,
    keys: std::collections::BTreeSet<crate::io_keys::Key>,
}
impl Resources {
    fn known(&self, key: crate::io_keys::Key) -> bool {
        key.0 < crate::io_keys::FIRST_RESOURCE || self.keys.contains(&key)
    }
    /// Check one request event against the graph without changing it: no handle
    /// changes identity, a split half names a known stream, every domain owner and
    /// result handle is identified, and every key is a system key, an established
    /// resource or one of this request's domains.
    fn check(&self, event: &Event) -> Result<(), &'static str> {
        use crate::io_keys::Kind;
        if self
            .declared
            .is_some_and(|declared| declared != event.resources.is_some())
        {
            return Err("request events disagree on recorded footprints");
        }
        let Some(footprint) = &event.resources else {
            return Ok(());
        };
        let mut identities = std::collections::BTreeMap::new();
        let mut keys = std::collections::BTreeSet::new();
        for &(handle, key, kind) in &footprint.identities {
            if key.0 < crate::io_keys::FIRST_RESOURCE && key != crate::io_keys::COUPLED
                || self
                    .identities
                    .get(&handle)
                    .is_some_and(|prior| *prior != (key, kind))
            {
                return Err("recorded handle identity changed");
            }
            if let Kind::Half { stream } = kind
                && !self.known(stream)
                && !keys.contains(&stream)
            {
                return Err("recorded split half names an unknown stream");
            }
            identities.insert(handle, (key, kind));
            keys.insert(key);
        }
        let identified =
            |handle: &u64| identities.contains_key(handle) || self.identities.contains_key(handle);
        for (key, owner, label) in &footprint.domains {
            if label.is_empty() || !identified(owner) {
                return Err("recorded domain has no identified owner");
            }
            keys.insert(*key);
        }
        if !matches!(
            event.completion,
            Completion::Blocked { .. } | Completion::Held
        ) {
            let mut result = Ok(event.completion.clone());
            if let Ok(fields) = crate::io_observation::fields(&mut result)
                && !fields.into_iter().flatten().all(|word| identified(word))
            {
                return Err("recorded result handle has no identity");
            }
        }
        if footprint
            .keys
            .keys()
            .iter()
            .any(|(key, _)| !self.known(*key) && !keys.contains(key))
        {
            return Err("recorded key names no established resource");
        }
        Ok(())
    }
    /// Install a checked request event's footprint.
    fn apply(&mut self, footprint: Option<&crate::io_keys::Footprint>) {
        self.declared = Some(footprint.is_some());
        let Some(footprint) = footprint else {
            return;
        };
        for &(handle, key, kind) in &footprint.identities {
            self.identities.insert(handle, (key, kind));
            self.keys.insert(key);
        }
        self.keys
            .extend(footprint.domains.iter().map(|(key, _, _)| *key));
    }
}
#[derive(Clone, Debug)]
pub struct SimIo {
    /// Bound native Free phases, consumed by the native production admission.
    /// Ordinary completion replay must never serve this tape as empty Io.
    pub(crate) free_tape: Option<Vec<u8>>,
    state: State,
    tree: Tree,
    tape: ReplayTape,
    position: usize,
    /// A complete recording's confirmed stop, never inferred from a missing event.
    ending: Option<hirsel_run_schema::RunOutcome>,
    ending_observed: bool,
    roots: std::collections::BTreeMap<u64, String>,
    identities: FileIdentities,
    detached_roots: std::collections::BTreeSet<u64>,
    mount_pins: MountPins,
    /// The requests the tape records as `blocked`, parked again in replay.
    waits: crate::readiness::WaitLog,
    /// The parked waits that are sent runner calls: only a callee's end delivers them.
    runner_waits: std::collections::BTreeSet<u64>,
    /// Scripted regular-file waits also have no source when their release never occurs.
    file_waits: std::collections::BTreeSet<u64>,
    /// Old durable tapes cannot distinguish barrier holds from issued waits.
    legacy_barrier_waits: bool,
    /// Scripted unanswered attempts consumed by a blocked request, until its token ends.
    unanswered_connects: std::collections::BTreeSet<u64>,
    /// The recorded run instance (RUN-11).
    pub(crate) instance: Option<[u8; 16]>,
    resources: Resources,
}
impl SimIo {
    pub fn set_env(&mut self, env: std::collections::BTreeMap<String, Option<String>>) {
        self.state.env = env;
    }
    pub fn stderr(&self) -> &[u8] {
        &self.state.stderr
    }

    /// Configure the root Net's isolated scripted world before entry. Rules are
    /// supplied separately by set_net_rules; peer data never widens them.
    pub fn set_net_script(
        &mut self,
        peers: Vec<hirsel_run_schema::NetPeer>,
        listeners: Vec<hirsel_run_schema::NetListener>,
    ) -> Result<(), AdapterError> {
        self.state.set_net_script(peers, listeners)
    }
    /// The kinds the admitted `io.signals` grant observes, as bits; replay re-derives each
    /// wait's checks from them.
    pub fn set_signal_kinds(&mut self, kinds: u8) {
        self.state.signal_kinds = kinds;
        self.state.projections.insert("signals".into());
    }
    /// Replay uses the admitted projection set, independently of tape completions.
    pub fn set_projections(&mut self, projections: std::collections::BTreeSet<String>) {
        self.state.projections = projections;
    }
    /// The harness's denied-output fixture only removes authority.
    pub(crate) fn deny_output(&mut self) {
        self.state.projections.remove("stdout");
        self.state.projections.remove("stderr");
    }
    pub fn new(tree: Tree, tape: Vec<Event>) -> Self {
        Self {
            free_tape: None,
            state: State::default(),
            detached_roots: Default::default(),
            mount_pins: MountPins {
                roots: std::collections::BTreeMap::from([(String::new(), String::new())]),
                detached: Default::default(),
            },
            identities: FileIdentities::new(&tree),
            tree,
            tape: ReplayTape::Owned(tape),
            position: 0,
            ending: None,
            ending_observed: false,
            roots: Default::default(),
            waits: Default::default(),
            runner_waits: Default::default(),
            file_waits: Default::default(),
            unanswered_connects: Default::default(),
            legacy_barrier_waits: false,
            instance: None,
            resources: Resources::default(),
        }
    }
    pub fn from_binary(tree: Tree, tape: crate::tape_binary::Tape) -> Self {
        let mut io = Self::new(tree, vec![]);
        io.set_ending(tape.outcome.clone());
        io.tape = ReplayTape::Binary {
            tape,
            current: None,
            previous: None,
        };
        io
    }
    /// A trailer or recovered durable journal supplies the confirmed run ending.
    pub(crate) fn set_ending(&mut self, outcome: Option<hirsel_run_schema::RunOutcome>) {
        self.ending = outcome;
    }
    /// Check the terminal class too. Source coordinates in diagnostic metadata can
    /// change after a formatting-only source edit, independently of program identity.
    pub fn finish_outcome(
        &self,
        actual: &hirsel_run_schema::RunOutcome,
    ) -> Result<(), AdapterError> {
        self.finish()?;
        if let Some(expected) = &self.ending {
            use hirsel_run_schema::RunOutcome as O;
            let matches = match (expected, actual) {
                (O::Returned(a), O::Returned(b)) => a == b,
                (O::Signaled(a), O::Signaled(b)) => a == b,
                (O::BudgetExceeded(a), O::BudgetExceeded(b)) => a == b,
                _ => std::mem::discriminant(expected) == std::mem::discriminant(actual),
            };
            if !matches {
                return Err(AdapterError::ReplayMismatch(self.position));
            }
        }
        Ok(())
    }
    /// Refuse ambiguous old durable scripted parks rather than guessing issue order.
    pub(crate) fn legacy_barrier_waits(&mut self, legacy: bool) {
        self.legacy_barrier_waits = legacy;
    }
    /// The run instance the recording's header names, for a run that calls runners.
    pub fn instance(&self) -> Option<[u8; 16]> {
        self.instance
    }
    /// Configure only the virtual selectors and rights; replay acquires no host authority.
    pub fn set_mounts(
        &mut self,
        mounts: impl IntoIterator<Item = (String, hirsel_run_schema::Access)>,
    ) -> Result<(), hirsel_run_schema::RunRefusal> {
        let mounts = mount_access(mounts)?;
        let mut directories = self.tree.directories.clone();
        for selector in mounts.keys() {
            let mut path = selector.as_str();
            loop {
                if self.tree.files.contains_key(path) {
                    return Err(hirsel_run_schema::RunRefusal::Invalid {
                        field: "io.dirs".into(),
                        detail: "mount directory collides with a snapshot file".into(),
                    });
                }
                // A root alias identifies shared snapshot storage; it must not
                // be replaced by an independent empty virtual directory.
                if !self.tree.links.contains_key(path) {
                    directories.insert(path.into());
                }
                let Some((parent, _)) = path.rsplit_once('/') else {
                    break;
                };
                path = parent;
            }
        }
        self.tree.directories = directories;
        self.mount_pins.roots = mounts
            .keys()
            .filter_map(|selector| {
                self.tree
                    .resolve("", selector, true)
                    .ok()
                    .map(|root| (selector.clone(), root))
            })
            .collect();
        self.mount_pins.detached.clear();
        self.state.mounts = mounts;
        Ok(())
    }
    /// The admitted `io.process` grant. Absent, the root Proc runs nothing.
    pub fn set_process_grant(&mut self, grant: &hirsel_run_schema::ProcessGrant) {
        self.state.set_process_grant(grant);
    }
    pub fn set_process_time(&mut self, initial: u64) {
        self.state.last_monotonic = Some(initial);
    }
    /// The bound RunConfig rule set; replay re-derives every grant decision from it.
    pub fn set_net_rules(&mut self, rules: std::collections::BTreeSet<NetRule>) {
        self.state.net_rules = rules;
    }
    pub fn clock_grants(&mut self, monotonic: bool, wall: bool) {
        self.state.monotonic_granted = monotonic;
        self.state.wall_granted = wall;
    }
    pub fn test_context(&mut self, seed: u64) {
        self.state.handles = Handles::test(seed);
    }
    pub fn grant_test(&mut self, seed: u64) -> u64 {
        self.state.grant_test(seed)
    }
    pub fn fake_trees(&self) -> &std::collections::BTreeMap<u64, Tree> {
        &self.state.fake_trees
    }
    pub fn finish(&self) -> Result<(), AdapterError> {
        if self.free_tape.is_some() {
            return Err(AdapterError::Unsupported(
                "Free tape requires phase replay admission".into(),
            ));
        }
        if self.position == self.tape.len() {
            if matches!(
                self.ending,
                Some(
                    hirsel_run_schema::RunOutcome::Blocked(_)
                        | hirsel_run_schema::RunOutcome::Signaled(_)
                )
            ) && !self.ending_observed
            {
                return Err(AdapterError::MissingInput(self.position));
            }
            Ok(())
        } else {
            Err(AdapterError::UnconsumedInput(self.position))
        }
    }
    pub fn tree(&self) -> &Tree {
        &self.tree
    }
    pub fn handles(&self) -> &Handles {
        &self.state.handles
    }
    pub fn stdout(&self) -> &[u8] {
        &self.state.stdout
    }
    pub fn position(&self) -> usize {
        self.position
    }
    /// The last consumed event; its fault states a committed prefix.
    pub fn last_event(&self) -> Option<&Event> {
        self.position
            .checked_sub(1)
            .and_then(|index| self.tape.cached(index))
    }
}
impl SimIo {
    /// The recorded footprint of `request`: its next event before the call, or
    /// the event it just consumed after it.
    fn footprint(&self, request: &Request) -> Option<crate::io_keys::Footprint> {
        let at = |index: usize| {
            self.tape
                .peek(index)
                .filter(|event| event.request == *request)
                .map(|event| event.resources.as_deref().cloned())
        };
        at(self.position)
            .or_else(|| self.position.checked_sub(1).and_then(at))
            .flatten()
    }
    fn ready_tokens(&mut self) -> Result<Ready, AdapterError> {
        let mut tokens = Vec::new();
        while let Some(Request::Ready { wait, points }) =
            self.tape.get(self.position)?.map(|event| &event.request)
        {
            let Some(token) = self
                .waits
                .token_of(*wait)
                .filter(|&t| self.waits.rows_of(t).is_some_and(|row| row.parked))
            else {
                break;
            };
            if self.state.timed_process && self.waits.next_points(token) == Some(0) {
                return Err(AdapterError::InvalidCompletion);
            }
            if self.state.timed_process {
                if self
                    .state
                    .timed_requests
                    .get(&token)
                    .is_some_and(|r| !self.state.process_ready(r))
                    || self
                        .state
                        .timed_sleeps
                        .get(&token)
                        .is_some_and(|d| *d > self.state.last_monotonic.unwrap_or(0))
                {
                    return Err(AdapterError::InvalidCompletion);
                }
            }
            let waited = self.waits.points(token).unwrap();
            if waited < *points {
                break;
            }
            if waited > *points {
                return Err(AdapterError::ReplayMismatch(self.position));
            }
            if self.state.timed_process && tokens.contains(&token) {
                return Err(AdapterError::InvalidCompletion);
            }
            self.waits.ready(token);
            self.position += 1;
            tokens.push(token);
        }
        tokens.sort_unstable();
        Ok(Ready::Tokens(tokens))
    }
}
impl Adapter for SimIo {
    fn print_observed(
        &mut self,
        handle: u64,
        stderr: bool,
        bytes: &[u8],
    ) -> (Result<Completion, AdapterError>, Option<u64>) {
        let before = if stderr {
            self.state.stderr.len()
        } else {
            self.state.stdout.len()
        };
        let result = self.print(handle, stderr, bytes);
        let after = if stderr {
            self.state.stderr.len()
        } else {
            self.state.stdout.len()
        };
        (result, Some((after - before) as u64))
    }
    fn tls(&mut self, engine: &'static dyn crate::net_tls::TlsEngine) {
        self.state.tls = crate::net_tls::Engine(Some(engine));
    }
    fn tls_roots(&mut self, stream: u64, pem: &[u8]) {
        self.state.remember_tls_roots(stream, pem);
    }
    fn tls_identity(&mut self, stream: u64, chain: &[u8], key: &[u8]) {
        self.state.remember_tls_identity(stream, chain, key);
    }
    fn tls_server(
        &mut self,
        stream: u64,
        identities: &[(&[u8], &[u8])],
        clients: crate::net::ClientAuth<&[u8]>,
        alpn: &[String],
    ) {
        self.state
            .remember_tls_server(stream, identities, clients, alpn);
    }
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        self.state.preflight(request)
    }
    fn keys(&self, request: &Request) -> Option<crate::io_keys::KeySet> {
        self.footprint(request).map(|footprint| footprint.keys)
    }
    fn domains(&self, request: &Request) -> Vec<(crate::io_keys::Key, u64, String)> {
        self.footprint(request)
            .map(|footprint| footprint.domains)
            .unwrap_or_default()
    }
    fn identity(&self, handle: u64) -> Option<(crate::io_keys::Key, crate::io_keys::Kind)> {
        self.resources.identities.get(&handle).copied()
    }

    fn enter(&mut self, token: u64) {
        self.waits.enter(token);
    }
    fn finish(&mut self, token: u64) {
        self.state.timed_requests.remove(&token);
        self.state.timed_sleeps.remove(&token);
        self.waits.finish(token);
        self.runner_waits.remove(&token);
        self.file_waits.remove(&token);
        self.unanswered_connects.remove(&token);
    }
    fn parked(&mut self, token: u64) {
        self.waits.parked(token);
    }
    fn cancellable(&self, token: u64) -> bool {
        self.waits.rows_of(token).is_none_or(|row| !row.committed)
    }
    fn cpu_control(
        &mut self,
        width: u32,
        action: u32,
        count: u64,
        value: Option<u32>,
    ) -> Result<Option<u32>, AdapterError> {
        if value.is_some() {
            return Err(AdapterError::InvalidCompletion);
        }
        match self.tape.get(self.position)? {
            Some(Event {
                request:
                    Request::CpuControl {
                        width: w,
                        action: a,
                        count: n,
                        outcome,
                    },
                completion: Completion::Ready,
                fault: None,
                resources: None,
            }) if (*w, *a, *n) == (width, action, count) => {
                let outcome = *outcome;
                self.position += 1;
                Ok(Some(outcome))
            }
            Some(_) => Err(AdapterError::ReplayMismatch(self.position)),
            None => Err(AdapterError::MissingInput(self.position)),
        }
    }
    /// The recorded choice among `count` runnable tasks: the next event is a `schedule` of
    /// as many candidates, whatever the policy chose (the trace is authoritative, DET-1).
    fn schedule(&mut self, count: u64, _choice: u64) -> Result<u64, AdapterError> {
        match self.tape.get(self.position)?.map(|event| &event.request) {
            Some(Request::Schedule {
                count: recorded,
                choice,
            }) if *recorded == count && *choice < count => {
                self.position += 1;
                Ok(*choice)
            }
            Some(_) => Err(AdapterError::ReplayMismatch(self.position)),
            None => Err(AdapterError::MissingInput(self.position)),
        }
    }
    fn injected_wait(&self, request: &Request) -> bool {
        self.state.injected_wait(request)
    }
    fn telemetry_context(&self, telemetry: u64, span: u64) -> Option<Vec<u8>> {
        self.state.telemetry_context(telemetry, span).ok()
    }
    /// The waits the recording re-issued at this scheduling point: the `ready` events at
    /// the tape's head whose wait has waited through exactly the recorded points.
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        self.waits.point();
        self.ready_tokens()
    }
    fn isolated_clock(&self) -> bool {
        self.state.timed_process
    }
    fn process_site(&mut self, child: u64, site: u64, operation_site: u64, path: &[u32]) {
        if self.state.timed_process {
            self.state
                .process_sites
                .insert(child, (site, operation_site, path.to_vec()));
        }
    }
    fn process_sources(&self) -> bool {
        self.state.next_child_gate().is_some() || self.state.process_batch_pending
    }
    fn process_batch(&mut self, canceled: &[u64]) -> Result<Ready, AdapterError> {
        if !self.state.process_batch_pending {
            return Ok(Ready::Tokens(Vec::new()));
        }
        let event = Event {
            request: Request::ProcessSettle {
                canceled: canceled.to_vec(),
            },
            completion: Completion::Ready,
            fault: None,
            resources: None,
        };
        if self.tape.get(self.position)? != Some(&event) {
            return Err(AdapterError::ReplayMismatch(self.position));
        }
        self.state.claim_children(canceled)?;
        self.position += 1;
        if let Some((child, report)) = self
            .state
            .settle_children(self.state.last_monotonic.unwrap_or(0))
        {
            return Err(self.state.process_failed(child, &report));
        }
        self.ready_tokens()
    }
    /// No host wait: idle scheduling points continue only toward the next recorded `ready`,
    /// which must belong to a parked request that has not waited past it.
    fn scripted_input(&self) -> bool {
        self.state.next_child_gate().is_some()
            || self.state.process_batch_pending
            // A confirmed terminal input must reach wait even when only
            // language objects are parked and no host request owns a token.
            || (self.position == self.tape.len()
                && !self.ending_observed
                && matches!(
                    self.ending,
                    Some(hirsel_run_schema::RunOutcome::Blocked(_)
                        | hirsel_run_schema::RunOutcome::Signaled(_))
                ))
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        if self.position == self.tape.len() {
            use hirsel_run_schema::RunOutcome as O;
            match self.ending {
                Some(O::Blocked(_)) => {
                    self.ending_observed = true;
                    return Ok(false);
                }
                Some(O::Signaled(kind)) => {
                    self.ending_observed = true;
                    return Err(AdapterError::Signaled(kind));
                }
                _ => {}
            }
        }
        if let Some(Event {
            request: Request::ProcessTime { from, to },
            completion: Completion::Ready,
            fault: None,
            resources: None,
        }) = self.tape.get(self.position)?.cloned()
        {
            self.state.apply_process_time(from, to)?;
            self.position += 1;
            return Ok(true);
        }
        if self.waits.is_empty() {
            return Ok(false);
        }

        let Some(Request::Ready { wait, points }) =
            self.tape.get(self.position)?.map(|event| &event.request)
        else {
            // Sent runner calls and scripted snapshot reads can end a recording still
            // parked: their source never arrived, so replay blocks as the recording did.
            if self.position == self.tape.len()
                && self.waits.tokens().all(|t| {
                    self.runner_waits.contains(&t)
                        || self.file_waits.contains(&t)
                        || self.state.timed_process && self.state.next_process_time().is_none()
                })
            {
                return Ok(false);
            }
            return Err(AdapterError::MissingInput(self.position));
        };
        match self
            .waits
            .token_of(*wait)
            .and_then(|t| self.waits.next_points(t))
        {
            Some(waited) if waited <= *points => Ok(true),
            _ => Err(AdapterError::ReplayMismatch(self.position)),
        }
    }
    fn replay_stderr(&self) -> Vec<u8> {
        self.state.stderr.clone()
    }
    fn replay_stdout(&self) -> Vec<u8> {
        self.state.stdout.clone()
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        if self.free_tape.is_some() {
            return Err(AdapterError::Unsupported(
                "Free tape requires phase replay admission".into(),
            ));
        }
        if self.state.awaits_injection(&request) {
            return Err(AdapterError::Blocked);
        }
        let event = self
            .tape
            .get(self.position)?
            .ok_or(AdapterError::MissingInput(self.position))?;
        // The recorded run's place stopped it before this request: so does replay.
        if event.request == Request::Stopped && event.completion == Completion::Ready {
            self.position += 1;
            return Err(AdapterError::Stopped);
        }
        if event.request != request {
            return Err(AdapterError::ReplayMismatch(self.position));
        }
        if self.resources.check(event).is_err() {
            return Err(AdapterError::ReplayMismatch(self.position));
        }
        let footprint = event.resources.clone();
        if self.state.timed_process {
            match (&request, &event.completion) {
                (Request::Monotonic { clock }, Completion::Monotonic(Ok(ticks)))
                    if !self.state.test_clocks.contains(clock)
                        && *ticks != self.state.last_monotonic.unwrap_or(0) =>
                {
                    return Err(AdapterError::InvalidCompletion);
                }
                (Request::SleepUntil { clock, deadline }, Completion::Wait(Ok(())))
                    if !self.state.test_clocks.contains(clock)
                        && *deadline > self.state.last_monotonic.unwrap_or(0) =>
                {
                    return Err(AdapterError::InvalidCompletion);
                }
                _ => {}
            }
        }
        // Only explicitly tagged host file observations replace the storage input.
        // Old tapes retain their snapshot-derived checks; test trees never accept a tag.
        let input = match &event.completion {
            Completion::FileInput(input) => {
                if event.fault.is_some() || self.state.is_fake_request(&request) {
                    return Err(AdapterError::InvalidCompletion);
                }
                Some(input)
            }
            _ => None,
        };
        let unwrapped;
        let event = if let Some(input) = input {
            unwrapped = Event {
                request: event.request.clone(),
                completion: input.completion(&request)?,
                fault: None,
                resources: None,
            };
            &unwrapped
        } else {
            event
        };
        if event.completion == Completion::Held
            && (event.fault.is_some()
                || !crate::host_waits::may_park(&request)
                || self.unanswered_connects.contains(&self.waits.current()))
        {
            return Err(AdapterError::InvalidCompletion);
        }
        // The recorded run parked this request: its task parks again (DET-1).
        if matches!(
            event.completion,
            Completion::Blocked { .. } | Completion::Held
        ) {
            let committed = matches!(event.completion, Completion::Blocked { committed: true });
            if self.state.timed_process && event.completion != Completion::Held {
                if self.state.is_process(&request) {
                    let mut state = self.state.clone();
                    if state
                        .execute_process(&mut |_| Err(AdapterError::InvalidCompletion), &request)
                        != Err(AdapterError::Blocked)
                    {
                        return Err(AdapterError::ReplayMismatch(self.position));
                    }
                    self.state = state;
                    if matches!(
                        request,
                        Request::ChildRead { .. }
                            | Request::ChildWait { .. }
                            | Request::GroupWait { .. }
                    ) {
                        self.state
                            .timed_requests
                            .insert(self.waits.current(), request.clone());
                    }
                } else if let Request::SleepUntil { clock, deadline } = &request {
                    self.state
                        .expect(*clock, crate::handles::Grant::MonotonicClock)?;
                    if *deadline <= self.state.last_monotonic.unwrap_or(0) {
                        return Err(AdapterError::InvalidCompletion);
                    }
                    if !self.state.test_clocks.contains(clock) {
                        self.state
                            .timed_sleeps
                            .insert(self.waits.current(), *deadline);
                    }
                }
            }
            // An unanswered scripted connect consumes one peer attempt before it
            // parks. Reconstruct that transition once per request token; retries
            // of the same wait must not consume another connection (DET-1).
            let token = self.waits.current();
            if event.completion != Completion::Held
                && self.state.scripted(&request)
                && matches!(
                    request,
                    Request::Connect { .. }
                        | Request::ConnectAddr { .. }
                        | Request::UnixConnect { .. }
                )
                && !self.unanswered_connects.contains(&token)
            {
                if self.legacy_barrier_waits {
                    return Err(AdapterError::Unsupported("legacy durable scripted connect has an ambiguous Blocked completion (barrier hold or issued wait); re-record with Held completions".into()));
                }
                let mut state = self.state.clone();
                if state.execute_net(&mut crate::net::NoNet, &request) != Err(AdapterError::Blocked)
                {
                    return Err(AdapterError::ReplayMismatch(self.position));
                }
                self.state = state;
                self.unanswered_connects.insert(token);
            }
            self.position += 1;
            self.resources.apply(footprint.as_deref());
            self.waits.park(committed);
            if matches!(request, Request::RunnerCall(_)) {
                self.runner_waits.insert(self.waits.current());
            }
            if matches!(request, Request::FileRead { .. }) && self.state.is_fake_request(&request) {
                self.file_waits.insert(self.waits.current());
            }
            return Err(AdapterError::Blocked);
        }
        // An unanswered attempt cannot later acquire the next scripted connection.
        if matches!(
            request,
            Request::Connect { .. } | Request::ConnectAddr { .. } | Request::UnixConnect { .. }
        ) && self.unanswered_connects.contains(&self.waits.current())
        {
            return Err(AdapterError::InvalidCompletion);
        }
        // A runner call's recorded outcome stands in for its place; no callee runs.
        match (&request, &event.completion) {
            (
                Request::RunnerCall(_) | Request::RunnerPoll { .. },
                Completion::Runner(_) | Completion::Ready,
            )
            | (Request::RunnerCancel { .. } | Request::CallReturn(_), Completion::Ready)
                if event.fault.is_none() =>
            {
                let completion = event.completion.clone();
                self.position += 1;
                self.resources.apply(footprint.as_deref());
                return Ok(completion);
            }
            (
                Request::RunnerCall(_)
                | Request::RunnerPoll { .. }
                | Request::RunnerCancel { .. }
                | Request::CallReturn(_),
                _,
            ) => {
                return Err(AdapterError::InvalidCompletion);
            }
            _ => {}
        }
        let mut state = self.state.clone();
        let mut tree = self.tree.clone();
        let mut roots = self.roots.clone();
        let mut identities = self.identities.clone();
        let mut detached = self.detached_roots.clone();
        let mut mount_pins = self.mount_pins.clone();
        let mut storage = LinkedTree {
            mount_pins: Some(&mut mount_pins),
            active_id: 0,
            detached: &mut detached,
            tree: &mut tree,
            roots: &mut roots,
            active: Default::default(),
            identities: &mut identities,
            file_input: input,
            metadata_input: match &event.completion {
                Completion::Metadata(Ok(meta)) => Some(meta.clone()),
                _ => None,
            },
            view_input: match &event.completion {
                Completion::View(view) => view.bytes.clone(),
                _ => None,
            },
            watch_input: match &event.completion {
                Completion::Watch(Ok(event)) => Some(event.clone()),
                _ => None,
            },
            read_limit: match (&request, &event.completion) {
                (Request::FileRead { .. }, Completion::Bytes(Ok(b))) => Some(b.len() as u64),
                _ => None,
            },
            write_limit: match (&request, &event.completion) {
                (Request::FileWrite { .. }, Completion::Size(Ok(n))) => Some(*n),
                _ => None,
            },
        };
        let result = if state.is_process(&request) {
            if event.fault.is_some() {
                return Err(AdapterError::InvalidCompletion);
            }
            // The recorded completion stands in for the host; the checks are re-derived.
            state.execute_process(&mut |_| Ok(event.completion.clone()), &request)
        } else if matches!(
            request,
            Request::Monotonic { .. }
                | Request::Wall { .. }
                | Request::ReadStdin { .. }
                | Request::RandomBytes { .. }
                | Request::TelemetryBegin { .. }
                | Request::SleepUntil { .. }
                | Request::SignalWait { .. }
        ) {
            if event.fault.is_some() {
                return Err(AdapterError::InvalidCompletion);
            }
            state.sample(&request, event.completion.clone())
        } else {
            let mut recorded = Recorded(&event.completion, false);
            state.execute(&mut storage, &mut recorded, &request, event.fault.as_ref())
        };
        let completion = match &result {
            Ok(completion) => completion.clone(),
            Err(AdapterError::InvalidHandle(id)) => Completion::Refused(*id),
            Err(error) => return Err(error.clone()),
        };
        if event.completion != completion {
            return Err(AdapterError::ReplayMismatch(self.position));
        }
        self.state = state;
        self.tree = tree;
        self.roots = roots;
        self.identities = identities;
        self.detached_roots = detached;
        self.mount_pins = mount_pins;
        self.position += 1;
        self.resources.apply(footprint.as_deref());
        result
    }
}
/// Replay reconstructs local inode numbers from initial entries and ordered
/// publication, independently of logical handles and host inode values.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct FileIdentities {
    serial: u64,
    reachable: std::collections::BTreeMap<String, u64>,
    opened: std::collections::BTreeMap<u64, u64>,
    prepared: std::collections::BTreeMap<String, u64>,
    contents: std::collections::BTreeMap<u64, Vec<u8>>,
    /// What a crash of this tree keeps (core-library.md, Durability).
    durable: hirsel_kernels::disk::Durable,
    /// File and AtomicFile handles opened before the tree's last crash.
    stale: std::collections::BTreeSet<u64>,
}
impl FileIdentities {
    pub(crate) fn is_stale(&self, file: u64) -> bool {
        self.stale.contains(&file)
    }
    pub(crate) fn new(tree: &Tree) -> Self {
        let mut files = Self::default();
        for path in tree.regular_paths() {
            files.publish(path);
            files.contents.insert(
                files.serial,
                tree.files.get(path).cloned().unwrap_or_default(),
            );
        }
        for (alias, canonical) in &tree.file_aliases {
            if let Some(identity) = files.reachable.get(canonical).copied() {
                files.reachable.insert(alias.clone(), identity);
            }
        }
        files
    }
    /// A `test.dir` tree starts durable as created.
    pub(crate) fn durable_start(&mut self, tree: &Tree) {
        let files = self
            .reachable
            .iter()
            .map(|(path, id)| (path.clone(), (*id, self.contents[id].clone())))
            .collect();
        self.durable = hirsel_kernels::disk::Durable::of(&tree.directories, &files);
    }
    /// The tree becomes its durable state. Every handle opened before is stale.
    pub(crate) fn crash(&mut self, tree: &mut Tree, seed: u64, stale: impl Iterator<Item = u64>) {
        let crashed = self.durable.crashed(seed);
        let surviving =
            |p: &String| crashed.directories.contains(p) || crashed.files.contains_key(p);
        tree.metadata.retain(|p, _| surviving(p));
        tree.links.clear();
        tree.file_aliases.clear();
        tree.directories = crashed.directories.clone();
        tree.files = crashed
            .files
            .iter()
            .map(|(p, (_, bytes))| (p.clone(), bytes.clone()))
            .collect();
        self.reachable = crashed
            .files
            .iter()
            .map(|(p, (id, _))| (p.clone(), *id))
            .collect();
        let mut canonical = std::collections::BTreeMap::<u64, &String>::new();
        for (path, (id, bytes)) in &crashed.files {
            self.contents.insert(*id, bytes.clone());
            match canonical.get(id) {
                Some(first) => {
                    tree.file_aliases.insert(path.clone(), (*first).clone());
                }
                None => {
                    canonical.insert(*id, path);
                }
            }
        }
        self.opened.clear();
        self.prepared.clear();
        self.stale.extend(stale);
    }
    fn publish(&mut self, path: &str) {
        self.serial = self
            .serial
            .checked_add(1)
            .expect("file identities exhausted");
        self.reachable.insert(path.into(), self.serial);
    }
    pub(crate) fn close(&mut self, id: u64) {
        self.opened.remove(&id);
    }
}
pub(crate) struct MemoryTree<'a> {
    pub(crate) tree: &'a mut Tree,
    pub(crate) identities: &'a mut FileIdentities,
}
impl MemoryTree<'_> {
    fn invalidate_metadata(&mut self, path: &str) {
        if let Some(meta) = self.tree.metadata.get_mut(path) {
            meta.modified_unix_ns = None;
            meta.size = self
                .tree
                .files
                .get(path)
                .map_or(0, |bytes| bytes.len() as u64);
        }
    }
    fn sync_aliases(&mut self) {
        self.tree.file_aliases.clear();
        let mut canonical = std::collections::BTreeMap::<u64, String>::new();
        for (path, identity) in &self.identities.reachable {
            if let Some(first) = canonical.get(identity) {
                self.tree.file_aliases.insert(path.clone(), first.clone());
            } else {
                canonical.insert(*identity, path.clone());
            }
        }
    }
    /// Make file `identity` and the directory chain above `path` durable.
    fn sync_durable(&mut self, identity: u64, path: &str) {
        self.identities.durable.sync(
            identity,
            &self.identities.contents[&identity],
            path,
            &self.tree.directories,
            &self.identities.reachable,
        );
    }
    fn sync_contents(&mut self, identity: u64) {
        for (path, _) in self
            .identities
            .reachable
            .iter()
            .filter(|(_, id)| **id == identity)
        {
            self.tree
                .files
                .insert(path.clone(), self.identities.contents[&identity].clone());
            if let Some(meta) = self.tree.metadata.get_mut(path) {
                meta.modified_unix_ns = None;
                meta.size = self.identities.contents[&identity].len() as u64;
            }
        }
    }
}
impl Storage for MemoryTree<'_> {
    fn stale(&self, id: u64) -> bool {
        self.identities.stale.contains(&id)
    }
    fn sync_file(&mut self, id: u64, path: &str) -> Result<(), IoError> {
        let identity = *self.identities.opened.get(&id).ok_or(IoError::Fault(1))?;
        self.sync_durable(identity, path);
        Ok(())
    }
    fn put_new(&mut self, path: &str, bytes: &[u8], durable: bool) -> Result<(), IoError> {
        if self.tree.directories.contains(path)
            || self.tree.links.contains_key(path)
            || (self.tree.metadata.contains_key(path) && !self.tree.files.contains_key(path))
        {
            self.tree.resolve("", path, false)?;
            return Err(IoError::AlreadyExists);
        }
        self.destination(path)?;
        if let Some(existing) = self.tree.files.get(path) {
            if existing != bytes {
                return Err(IoError::AlreadyExists);
            }
            if durable {
                self.sync_durable(self.identities.reachable[path], path);
            }
            return Ok(());
        }
        self.tree.files.insert(path.into(), bytes.to_vec());
        self.identities.publish(path);
        let identity = self.identities.serial;
        self.identities.contents.insert(identity, bytes.to_vec());
        self.invalidate_metadata(path.rsplit_once('/').map_or("", |(p, _)| p));
        if durable {
            self.sync_durable(identity, path);
        } else {
            self.identities.durable.wrote(identity, 0, bytes);
        }
        Ok(())
    }
    fn map_file(&mut self, path: &str, id: u64) -> Result<u64, IoError> {
        self.open_file(path, id)?;
        Ok(self.identities.contents[&self.identities.opened[&id]].len() as u64)
    }
    fn map_view(&mut self, id: u64, length: u64, _capture: bool) -> MapView {
        let contents = &self.identities.contents[&self.identities.opened[&id]];
        let mut bytes = contents[..contents.len().min(length as usize)].to_vec();
        bytes.resize(length as usize, 0);
        MapView::of(bytes)
    }
    fn map_truncated(&mut self, id: u64, length: u64) -> bool {
        (self.identities.contents[&self.identities.opened[&id]].len() as u64) < length
    }
    fn unmap(&mut self, id: u64) {
        self.identities.opened.remove(&id);
    }
    /// A tree has no change source of its own: its watchers read injected events.
    fn watch(&mut self, path: &str, _id: u64, _recursive: bool) -> Result<(), IoError> {
        self.metadata(path).map(|_| ())
    }
    fn watch_next(&mut self, _id: u64, _root: &str) -> Result<Option<WatchEvent>, IoError> {
        Ok(None)
    }
    fn unwatch(&mut self, _id: u64) {}
    fn set_permissions(&mut self, path: &str, permissions: u32) -> Result<(), IoError> {
        let mut meta = self.metadata(path)?;
        if !matches!(meta.kind, FileKind::Regular | FileKind::Directory) {
            return Err(IoError::Fault(1));
        }
        meta.permissions = permissions;
        self.tree.metadata.insert(path.into(), meta);
        if let Some(identity) = self.identities.reachable.get(path) {
            let aliases: Vec<_> = self
                .identities
                .reachable
                .iter()
                .filter_map(|(p, id)| (id == identity).then_some(p.clone()))
                .collect();
            for alias in aliases {
                let mut meta = self.metadata(&alias)?;
                meta.permissions = permissions;
                self.tree.metadata.insert(alias, meta);
            }
        }
        Ok(())
    }
    fn remove_all(&mut self, path: &str, limit: Option<usize>) -> Result<(), IoError> {
        let kind = self.metadata(path)?.kind;
        if !matches!(kind, FileKind::Directory | FileKind::Symlink) {
            return Err(IoError::NotDirectory);
        }
        let mut pending = vec![(path.to_owned(), false)];
        let mut removed = 0;
        while let Some((path, visited)) = pending.pop() {
            if limit == Some(removed) {
                return Ok(());
            }
            if self.metadata(&path)?.kind == FileKind::Directory {
                if visited {
                    self.remove_dir(&path)?;
                    removed += 1;
                } else {
                    let children = self.read_dir(&path)?;
                    pending.push((path.clone(), true));
                    for entry in children.into_iter().rev() {
                        pending.push((format!("{path}/{}", entry.path), false));
                    }
                }
            } else if self.tree.links.remove(&path).is_some() {
                self.tree.metadata.remove(&path);
                removed += 1;
            } else {
                self.remove(&path)?;
                removed += 1;
            }
        }
        Ok(())
    }
    fn metadata(&mut self, path: &str) -> Result<Metadata, IoError> {
        self.tree.resolve("", path, false)?;
        if let Some(meta) = self.tree.metadata.get(path) {
            return Ok(meta.clone());
        }
        let (kind, size) = if let Some(bytes) = self.tree.files.get(path) {
            (FileKind::Regular, bytes.len() as u64)
        } else if self.tree.directories.contains(path) {
            (FileKind::Directory, 0)
        } else if let Some(target) = self.tree.links.get(path) {
            (FileKind::Symlink, target.len() as u64)
        } else {
            return Err(IoError::NotFound);
        };
        Ok(Metadata {
            kind,
            size,
            modified_unix_ns: None,
            permissions: if kind == FileKind::Regular {
                0o666
            } else {
                0o777
            },
        })
    }

    fn read_dir_page(
        &mut self,
        path: &str,
        after: &str,
        recursive: bool,
    ) -> Result<Vec<DirEntry>, IoError> {
        self.directory(path, 0)?;
        let mut page = hirsel_kernels::walk::PageSelector::default();
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        let mut offer = |p: String| {
            let name = p.strip_prefix(&prefix).expect("direct child");
            let kind = self
                .tree
                .metadata
                .get(&p)
                .map(|m| m.kind)
                .unwrap_or_else(|| {
                    if self.tree.directories.contains(&p) {
                        FileKind::Directory
                    } else if self.tree.files.contains_key(&p) {
                        FileKind::Regular
                    } else {
                        FileKind::Symlink
                    }
                });
            page.offer(
                hirsel_kernels::walk::Entry {
                    path: name.into(),
                    kind: kind as u32,
                },
                after,
                recursive,
            );
        };
        hirsel_kernels::walk::visit_children(
            path,
            |bound| {
                self.tree
                    .directories
                    .range(bound.to_owned()..)
                    .next()
                    .cloned()
            },
            &mut offer,
        );
        hirsel_kernels::walk::visit_children(
            path,
            |bound| {
                self.tree
                    .files
                    .range(bound.to_owned()..)
                    .next()
                    .map(|(p, _)| p.clone())
            },
            &mut offer,
        );
        hirsel_kernels::walk::visit_children(
            path,
            |bound| {
                self.tree
                    .links
                    .range(bound.to_owned()..)
                    .next()
                    .map(|(p, _)| p.clone())
            },
            &mut offer,
        );
        hirsel_kernels::walk::visit_children(
            path,
            |bound| {
                self.tree
                    .metadata
                    .range(bound.to_owned()..)
                    .next()
                    .map(|(p, _)| p.clone())
            },
            &mut offer,
        );
        Ok(page
            .finish()
            .into_iter()
            .map(|e| DirEntry {
                path: e.path,
                kind: match e.kind {
                    0 => FileKind::Regular,
                    1 => FileKind::Directory,
                    2 => FileKind::Symlink,
                    _ => FileKind::Other,
                },
            })
            .collect())
    }
    fn read_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, IoError> {
        self.directory(path, 0)?;
        let mut children: std::collections::BTreeSet<_> =
            hirsel_kernels::walk::direct_children(path, |bound| {
                self.tree
                    .directories
                    .range(bound.to_owned()..)
                    .next()
                    .cloned()
            })
            .into_iter()
            .collect();
        children.extend(hirsel_kernels::walk::direct_children(path, |bound| {
            self.tree
                .files
                .range(bound.to_owned()..)
                .next()
                .map(|(p, _)| p.clone())
        }));
        children.extend(hirsel_kernels::walk::direct_children(path, |bound| {
            self.tree
                .links
                .range(bound.to_owned()..)
                .next()
                .map(|(p, _)| p.clone())
        }));
        children.extend(hirsel_kernels::walk::direct_children(path, |bound| {
            self.tree
                .metadata
                .range(bound.to_owned()..)
                .next()
                .map(|(p, _)| p.clone())
        }));
        children
            .into_iter()
            .map(|path| {
                let kind = self.metadata(&path)?.kind;
                Ok(DirEntry {
                    path: path
                        .rsplit_once('/')
                        .map_or(path.as_str(), |(_, n)| n)
                        .into(),
                    kind,
                })
            })
            .collect()
    }
    fn mkdir(&mut self, path: &str) -> Result<(), IoError> {
        if self.metadata(path).is_ok() {
            return Err(IoError::AlreadyExists);
        }
        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
        self.directory(parent, 0)?;
        self.tree.directories.insert(path.into());
        self.invalidate_metadata(parent);
        Ok(())
    }
    fn remove_dir(&mut self, path: &str) -> Result<(), IoError> {
        if self.metadata(path)?.kind != FileKind::Directory {
            return Err(IoError::NotDirectory);
        }
        if !self.read_dir(path)?.is_empty() {
            return Err(IoError::DirectoryNotEmpty);
        }
        self.tree.directories.remove(path);
        self.tree.metadata.remove(path);
        self.invalidate_metadata(path.rsplit_once('/').map_or("", |(p, _)| p));
        Ok(())
    }
    fn move_entry(&mut self, from: &str, to: &str) -> Result<(), IoError> {
        let source = self.metadata(from)?.kind;
        if source == FileKind::Regular
            && self.identities.reachable.contains_key(from)
            && self.identities.reachable.get(from) == self.identities.reachable.get(to)
        {
            return Ok(());
        }
        let parent = to.rsplit_once('/').map_or("", |(p, _)| p);
        self.directory(parent, 0)?;
        if from == to {
            return Ok(());
        }
        if source == FileKind::Directory && to.starts_with(&format!("{from}/")) {
            return Err(IoError::Fault(1));
        }
        match self.metadata(to) {
            Ok(meta) => {
                if source == FileKind::Directory && meta.kind != FileKind::Directory {
                    return Err(IoError::NotDirectory);
                }
                if source != FileKind::Directory && meta.kind == FileKind::Directory {
                    return Err(IoError::IsDirectory);
                }
                if meta.kind == FileKind::Directory && !self.read_dir(to)?.is_empty() {
                    return Err(IoError::DirectoryNotEmpty);
                }
            }
            Err(IoError::NotFound) => {}
            Err(e) => return Err(e),
        }
        self.tree.files.remove(to);
        self.tree.links.remove(to);
        self.tree.directories.remove(to);
        self.tree.metadata.remove(to);
        self.identities.reachable.remove(to);
        self.identities.prepared.remove(to);
        let moved = |p: &str| p == from || p.starts_with(&format!("{from}/"));
        let destination = |p: &str| format!("{to}{}", &p[from.len()..]);
        let dirs: Vec<_> = self
            .tree
            .directories
            .iter()
            .filter(|p| moved(p))
            .cloned()
            .collect();
        for path in dirs {
            self.tree.directories.remove(&path);
            self.tree.directories.insert(destination(&path));
        }
        macro_rules! relocate {
            ($map:expr) => {{
                let paths: Vec<_> = $map.keys().filter(|p| moved(p)).cloned().collect();
                for path in paths {
                    let value = $map.remove(&path).unwrap();
                    $map.insert(destination(&path), value);
                }
            }};
        }
        relocate!(self.tree.files);
        relocate!(self.tree.links);
        relocate!(self.tree.metadata);
        relocate!(self.identities.reachable);
        relocate!(self.identities.prepared);
        self.invalidate_metadata(parent);
        self.invalidate_metadata(from.rsplit_once('/').map_or("", |(p, _)| p));
        Ok(())
    }

    fn open_file(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        self.destination(path)?;
        let identity = self
            .identities
            .reachable
            .get(path)
            .copied()
            .ok_or(IoError::NotFound)?;
        self.identities.opened.insert(id, identity);
        Ok(())
    }
    fn open_append_file(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        self.destination(path)?;
        if self.identities.reachable.contains_key(path) {
            self.open_file(path, id)
        } else {
            self.create_file(path, id)
        }
    }
    fn create_file(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        self.destination(path)?;
        let identity = if let Some(identity) = self.identities.reachable.get(path) {
            *identity
        } else {
            self.identities.publish(path);
            self.identities.serial
        };
        self.identities.contents.insert(identity, vec![]);
        self.tree.files.insert(path.into(), vec![]);
        self.invalidate_metadata(path);
        self.identities.opened.insert(id, identity);
        self.sync_contents(identity);
        self.sync_aliases();
        Ok(())
    }
    fn read_file(&mut self, id: u64, offset: u64, max: u64) -> Result<Vec<u8>, IoError> {
        let identity = self.identities.opened.get(&id).ok_or(IoError::Fault(1))?;
        let bytes = &self.identities.contents[identity];
        let start = offset.min(bytes.len() as u64) as usize;
        let n = max.min(65536).min((bytes.len() - start) as u64) as usize;
        Ok(bytes[start..start + n].to_vec())
    }
    fn write_file(&mut self, id: u64, bytes: &[u8], _all: bool) -> Result<u64, IoError> {
        let identity = *self.identities.opened.get(&id).ok_or(IoError::Fault(1))?;
        let contents = self.identities.contents.get_mut(&identity).unwrap();
        if !bytes.is_empty() {
            self.identities
                .durable
                .wrote(identity, contents.len(), bytes);
        }
        contents.extend_from_slice(bytes);
        self.sync_contents(identity);
        Ok(bytes.len() as u64)
    }
    fn file_fits(&mut self, id: u64, path: &str) -> bool {
        self.identities
            .opened
            .get(&id)
            .is_some_and(|identity| self.identities.reachable.get(path) == Some(identity))
    }
    fn transfer_file(&mut self, from: u64, to: Option<u64>) {
        if let Some(identity) = self.identities.opened.remove(&from)
            && let Some(to) = to
        {
            self.identities.opened.insert(to, identity);
        }
    }
    fn destination(&mut self, path: &str) -> Result<(), IoError> {
        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
        self.tree.resolve("", path, false)?;
        if self.tree.directories.contains(path) {
            Err(IoError::IsDirectory)
        } else if !self.tree.directories.contains(parent) {
            Err(if self.tree.files.contains_key(parent) {
                IoError::NotDirectory
            } else {
                IoError::NotFound
            })
        } else {
            Ok(())
        }
    }
    fn directory(&mut self, path: &str, _id: u64) -> Result<(), IoError> {
        if self.tree.directories.contains(path) {
            Ok(())
        } else {
            self.tree.resolve("", path, true)?;
            Err(if self.tree.files.contains_key(path) {
                IoError::NotDirectory
            } else {
                IoError::NotFound
            })
        }
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, IoError> {
        self.destination(path)?;
        self.tree.files.get(path).cloned().ok_or(IoError::NotFound)
    }
    fn write(
        &mut self,
        path: &str,
        bytes: &[u8],
        append: bool,
        exclusive: bool,
    ) -> Result<(), IoError> {
        if exclusive && (self.tree.files.contains_key(path) || self.tree.directories.contains(path))
        {
            return Err(IoError::AlreadyExists);
        }
        self.destination(path)?;
        let start = if append {
            self.tree.files.get(path).map_or(0, Vec::len)
        } else {
            0
        };
        if append {
            self.tree
                .files
                .get_mut(path)
                .ok_or(IoError::NotFound)?
                .extend_from_slice(bytes);
        } else {
            self.tree.files.insert(path.into(), bytes.to_vec());
            self.identities.publish(path);
            if exclusive {
                self.identities
                    .prepared
                    .insert(path.into(), self.identities.serial);
            }
        }
        if append {
            self.invalidate_metadata(path);
        } else {
            self.tree.metadata.remove(path);
        }
        let identity = self.identities.reachable[path];
        self.identities
            .contents
            .insert(identity, self.tree.files[path].clone());
        if !bytes.is_empty() {
            self.identities.durable.wrote(identity, start, bytes);
        }
        self.sync_aliases();
        Ok(())
    }
    fn remove(&mut self, path: &str) -> Result<(), IoError> {
        self.destination(path)?;
        if self.tree.files.remove(path).is_none() && !self.tree.metadata.contains_key(path) {
            return Err(IoError::NotFound);
        }
        self.tree.metadata.remove(path);
        self.identities.reachable.remove(path);
        self.identities.prepared.remove(path);
        self.sync_aliases();
        Ok(())
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), IoError> {
        if self.tree.directories.contains(to) {
            return Err(IoError::IsDirectory);
        }
        if self.identities.prepared.get(from).is_none()
            || self.identities.prepared.get(from) != self.identities.reachable.get(from)
        {
            return Err(IoError::Fault(1));
        }
        let bytes = self.tree.files.remove(from).ok_or(IoError::NotFound)?;
        self.tree.files.insert(to.into(), bytes);
        self.tree.metadata.remove(to);
        if let Some(meta) = self.tree.metadata.remove(from) {
            self.tree.metadata.insert(to.into(), meta);
        }
        let identity = self
            .identities
            .reachable
            .remove(from)
            .expect("published identity");
        self.identities.reachable.insert(to.into(), identity);
        self.identities.prepared.remove(from);
        self.sync_aliases();
        Ok(())
    }
}

/// A replay storage view keeps each issued directory's resolved root. Link
/// expansion is scoped to that pinned root rather than the launch tree.
#[derive(Clone, Debug, Default)]
struct MountPins {
    roots: std::collections::BTreeMap<String, String>,
    detached: std::collections::BTreeSet<String>,
}
impl MountPins {
    fn update(&mut self, tree: &mut Tree, from: &str, to: Option<&str>) {
        for (selector, root) in &mut self.roots {
            if self.detached.contains(selector) {
                continue;
            }
            if (to.is_none() && root == from) || to.is_some_and(|to| to != from && root == to) {
                self.detached.insert(selector.clone());
                tree.links.remove(selector);
                tree.directories.insert(selector.clone());
                *root = selector.clone();
            } else if let Some(to) = to
                && (*root == from || root.starts_with(&format!("{from}/")))
            {
                *root = format!("{to}{}", &root[from.len()..]);
            }
            if tree.links.contains_key(selector) {
                let parent = selector.rsplit_once('/').map_or("", |(p, _)| p);
                let prefix = "../".repeat(parent.split('/').filter(|p| !p.is_empty()).count());
                tree.links
                    .insert(selector.clone(), format!("{prefix}{root}"));
            }
        }
    }
}
pub(crate) struct LinkedTree<'a> {
    mount_pins: Option<&'a mut MountPins>,
    active_id: u64,
    detached: &'a mut std::collections::BTreeSet<u64>,
    file_input: Option<&'a FileInput>,
    metadata_input: Option<Metadata>,
    /// A recorded `mapped.with` view: a host input, like recorded metadata.
    view_input: Option<Vec<u8>>,
    /// A recorded `watcher.next` event of a host watcher.
    watch_input: Option<WatchEvent>,
    read_limit: Option<u64>,
    write_limit: Option<u64>,
    tree: &'a mut Tree,
    roots: &'a mut std::collections::BTreeMap<u64, String>,
    active: (String, String),
    identities: &'a mut FileIdentities,
}
impl<'a> LinkedTree<'a> {
    pub(crate) fn new(
        tree: &'a mut Tree,
        roots: &'a mut std::collections::BTreeMap<u64, String>,
        identities: &'a mut FileIdentities,
        detached: &'a mut std::collections::BTreeSet<u64>,
    ) -> Self {
        Self {
            mount_pins: None,
            active_id: 0,
            detached,
            tree,
            roots,
            identities,
            active: Default::default(),
            file_input: None,
            metadata_input: None,
            view_input: None,
            watch_input: None,
            read_limit: None,
            write_limit: None,
        }
    }

    fn detached(&self) -> bool {
        self.detached.contains(&self.active_id)
            || (self.active_id == 0
                && self
                    .mount_pins
                    .as_ref()
                    .is_some_and(|p| p.detached.contains(&self.active.0)))
    }
    fn memory(&mut self) -> MemoryTree<'_> {
        MemoryTree {
            tree: self.tree,
            identities: self.identities,
        }
    }
    fn path(&self, path: &str, follow_leaf: bool) -> Result<String, IoError> {
        let local = if self.active.0.is_empty() {
            path
        } else if path == self.active.0 {
            ""
        } else {
            path.strip_prefix(&format!("{}/", self.active.0))
                .ok_or(IoError::Fault(1))?
        };
        if self.detached() {
            return if hirsel_run_schema::confined_relative("", local).as_deref() == Some("") {
                Ok(self.active.1.clone())
            } else {
                Err(IoError::NotFound)
            };
        }
        self.tree.resolve(&self.active.1, local, follow_leaf)
    }
}
impl Storage for LinkedTree<'_> {
    fn stale(&self, id: u64) -> bool {
        self.identities.stale.contains(&id)
    }
    fn sync_file(&mut self, id: u64, path: &str) -> Result<(), IoError> {
        // A detached or unresolvable open path still syncs the file's bytes.
        let path = self.path(path, true).unwrap_or_default();
        self.memory().sync_file(id, &path)
    }
    fn put_new(&mut self, path: &str, bytes: &[u8], durable: bool) -> Result<(), IoError> {
        let path = self.path(path, false)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        self.memory().put_new(&path, bytes, durable)
    }
    fn set_permissions(&mut self, path: &str, permissions: u32) -> Result<(), IoError> {
        let path = self.path(path, false)?;
        self.memory().set_permissions(&path, permissions)
    }
    fn remove_all(&mut self, path: &str, limit: Option<usize>) -> Result<(), IoError> {
        let path = self.path(path, false)?;
        self.memory().remove_all(&path, limit)?;
        // Every removed issued descendant remains an empty detached directory.
        let removed = |root: &str| root == path || root.starts_with(&format!("{path}/"));
        if let Some(pins) = &mut self.mount_pins {
            let roots: Vec<_> = pins
                .roots
                .values()
                .filter(|p| removed(p) && !self.tree.directories.contains(*p))
                .cloned()
                .collect();
            for root in roots {
                pins.update(self.tree, &root, None);
            }
        }
        self.detached
            .extend(self.roots.iter().filter_map(|(id, root)| {
                (removed(root) && !self.tree.directories.contains(root)).then_some(*id)
            }));
        Ok(())
    }
    fn metadata(&mut self, path: &str) -> Result<Metadata, IoError> {
        let path = self.path(path, false)?;
        let derived = if self.detached() {
            Metadata {
                kind: FileKind::Directory,
                size: 0,
                modified_unix_ns: None,
                permissions: 0o777,
            }
        } else {
            self.memory().metadata(&path)?
        };
        match &self.metadata_input {
            Some(input)
                if input.kind == derived.kind
                    && (!matches!(input.kind, FileKind::Regular | FileKind::Symlink)
                        || input.size == derived.size) =>
            {
                Ok(input.clone())
            }
            Some(_) => Err(IoError::Fault(1)),
            None => Ok(derived),
        }
    }

    fn read_dir_page(
        &mut self,
        path: &str,
        after: &str,
        recursive: bool,
    ) -> Result<Vec<DirEntry>, IoError> {
        let physical = self.path(path, true)?;
        if self.detached() {
            return Ok(vec![]);
        }
        self.memory().read_dir_page(&physical, after, recursive)
    }
    fn read_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, IoError> {
        let physical = self.path(path, true)?;
        if self.detached() {
            return Ok(vec![]);
        }
        self.memory().read_dir(&physical)
    }
    fn mkdir(&mut self, path: &str) -> Result<(), IoError> {
        let path = self.path(path, false)?;
        self.memory().mkdir(&path)
    }
    fn remove_dir(&mut self, path: &str) -> Result<(), IoError> {
        let path = self.path(path, false)?;
        self.memory().remove_dir(&path)?;
        if let Some(pins) = &mut self.mount_pins {
            pins.update(self.tree, &path, None);
        }
        self.detached.extend(
            self.roots
                .iter()
                .filter_map(|(id, root)| (*root == path).then_some(*id)),
        );
        Ok(())
    }
    fn move_entry(&mut self, from: &str, to: &str) -> Result<(), IoError> {
        let from = self.path(from, false)?;
        let to = self.path(to, false)?;
        self.memory().move_entry(&from, &to)?;
        if let Some(pins) = &mut self.mount_pins {
            pins.update(self.tree, &from, Some(&to));
        }
        if from != to {
            self.detached.extend(
                self.roots
                    .iter()
                    .filter_map(|(id, root)| (*root == to).then_some(*id)),
            );
        }
        for (id, root) in self.roots.iter_mut() {
            if self.detached.contains(id) {
                continue;
            }
            if *root == from || root.starts_with(&format!("{from}/")) {
                *root = format!("{to}{}", &root[from.len()..]);
            }
        }
        Ok(())
    }

    fn open_file(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        if matches!(self.file_input, Some(FileInput::Opened(_)))
            && !self.identities.reachable.contains_key(&path)
        {
            // A confirmed host open can discover a file absent at admission.
            // Its bytes are supplied by each subsequent recorded read.
            self.memory().create_file(&path, id)
        } else {
            self.memory().open_file(&path, id)
        }
    }
    fn map_file(&mut self, path: &str, id: u64) -> Result<u64, IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        self.memory().map_file(&path, id)
    }
    fn map_view(&mut self, id: u64, length: u64, capture: bool) -> MapView {
        match self.view_input.take() {
            Some(bytes) if bytes.len() as u64 == length => MapView::of(bytes),
            _ => self.memory().map_view(id, length, capture),
        }
    }
    fn map_truncated(&mut self, id: u64, length: u64) -> bool {
        self.memory().map_truncated(id, length)
    }
    fn unmap(&mut self, id: u64) {
        self.memory().unmap(id);
    }
    fn watch(&mut self, path: &str, id: u64, recursive: bool) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        self.memory().watch(&path, id, recursive)
    }
    /// A host watcher's recorded event: an input, like a recorded view.
    fn watch_next(&mut self, _id: u64, _root: &str) -> Result<Option<WatchEvent>, IoError> {
        Ok(self.watch_input.take())
    }
    fn unwatch(&mut self, _id: u64) {}
    fn open_append_file(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        self.memory().open_append_file(&path, id)
    }
    fn create_file(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        self.memory().create_file(&path, id)
    }
    fn read_file(&mut self, id: u64, offset: u64, max: u64) -> Result<Vec<u8>, IoError> {
        if let Some(FileInput::Bytes(bytes)) = self.file_input {
            return Ok(bytes.clone());
        }
        let max = self.read_limit.unwrap_or(max).min(max);
        self.memory().read_file(id, offset, max)
    }
    fn write_file(&mut self, id: u64, bytes: &[u8], all: bool) -> Result<u64, IoError> {
        let n = self
            .write_limit
            .unwrap_or(bytes.len() as u64)
            .min(bytes.len() as u64) as usize;
        self.memory().write_file(id, &bytes[..n], all)
    }
    fn file_fits(&mut self, id: u64, path: &str) -> bool {
        self.path(path, true)
            .ok()
            .is_some_and(|path| self.memory().file_fits(id, &path))
    }
    fn transfer_file(&mut self, from: u64, to: Option<u64>) {
        self.memory().transfer_file(from, to);
    }
    fn activate(&mut self, id: u64, root: &str) {
        self.active_id = id;
        self.active = (
            root.into(),
            self.roots
                .get(&id)
                .cloned()
                .or_else(|| {
                    (id == 0)
                        .then(|| self.mount_pins.as_ref()?.roots.get(root).cloned())
                        .flatten()
                })
                .unwrap_or_else(|| {
                    self.tree
                        .resolve("", root, true)
                        .unwrap_or_else(|_| root.into())
                }),
        );
    }
    fn directory(&mut self, path: &str, id: u64) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            self.detached.insert(id);
        } else {
            self.memory().directory(&path, id)?;
        }
        self.roots.insert(id, path);
        Ok(())
    }
    fn destination(&mut self, path: &str) -> Result<(), IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        self.memory().destination(&path)
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        if let Some(FileInput::Bytes(bytes)) = self.file_input {
            return Ok(bytes.clone());
        }
        self.memory().read(&path)
    }
    fn size(&mut self, path: &str) -> Result<u64, IoError> {
        let path = self.path(path, true)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        match self.file_input {
            Some(FileInput::Size(size)) => Ok(*size),
            _ => self.memory().size(&path),
        }
    }
    fn write(
        &mut self,
        path: &str,
        bytes: &[u8],
        append: bool,
        exclusive: bool,
    ) -> Result<(), IoError> {
        let path = self.path(path, append)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        if exclusive && self.tree.links.contains_key(&path) {
            return Err(IoError::AlreadyExists);
        }
        self.memory().write(&path, bytes, append, exclusive)?;
        if !append {
            self.tree.links.remove(&path);
        }
        Ok(())
    }
    fn remove(&mut self, path: &str) -> Result<(), IoError> {
        let path = self.path(path, false)?;
        if self.detached() {
            return Err(IoError::IsDirectory);
        }
        if self.tree.links.remove(&path).is_some() {
            self.tree.metadata.remove(&path);
            Ok(())
        } else {
            self.memory().remove(&path)
        }
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), IoError> {
        let from = self.path(from, false)?;
        let to = self.path(to, false)?;
        self.memory().rename(&from, &to)?;
        self.tree.links.remove(&to);
        Ok(())
    }
}

/// Host network replay: the recorded completion is the provider's only input. No socket,
/// resolver or TLS runs; policy was already re-derived, and the completion it builds must
/// equal the recorded one (handles included) or replay refuses.
struct Recorded<'a>(&'a Completion, bool);
impl Recorded<'_> {
    fn unit(&self) -> crate::net::Provided<()> {
        match self.0 {
            Completion::NetUnit(value) => Ok(*value),
            Completion::NetHandle(value) => Ok(value.map(|_| ())),
            Completion::Listened(value) => Ok(value.map(|_| ())),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
    fn protocol(&self) -> crate::net::Provided<Option<String>> {
        match self.0 {
            Completion::Protocol(value) => Ok(value.clone()),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
    fn address(&self) -> crate::net::Provided<SocketAddr> {
        match self.0 {
            Completion::Address(value) => Ok(*value),
            Completion::Listened(value) => Ok(value.map(|(_, local)| local)),
            Completion::Accepted(value) => Ok(value.map(|(_, peer)| peer)),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
}
impl NetProvider for Recorded<'_> {
    fn unix_connect(&mut self, _: u64, _: &str) -> crate::net::Provided<()> {
        self.unit()
    }
    fn unix_listen(&mut self, _: u64, _: &str, _: bool) -> crate::net::Provided<()> {
        self.unit()
    }
    fn udp_bind(&mut self, _: u64, _: SocketAddr) -> crate::net::Provided<()> {
        self.unit()
    }
    fn udp_send(&mut self, _: u64, bytes: &[u8], _: SocketAddr) -> crate::net::Provided<u64> {
        self.write(0, bytes)
    }
    fn udp_recv(&mut self, _: u64) -> crate::net::Provided<(Vec<u8>, SocketAddr)> {
        if std::mem::replace(&mut self.1, true) {
            return Err(AdapterError::InvalidCompletion);
        }
        match self.0 {
            Completion::Datagram(value) => Ok(value.clone().map(|(mut bytes, peer, truncated)| {
                if truncated {
                    bytes.push(0);
                }
                (bytes, peer)
            })),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }

    fn resolve(&mut self, _: &str, _: u16) -> crate::net::Provided<Vec<SocketAddr>> {
        match self.0 {
            Completion::Addresses(value) => Ok(value.clone()),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
    fn connect(&mut self, _: u64, _: &str, _: u16, _: NetTimeout) -> crate::net::Provided<()> {
        self.unit()
    }
    fn connect_addr(&mut self, _: u64, _: SocketAddr, _: NetTimeout) -> crate::net::Provided<()> {
        self.unit()
    }
    fn read(&mut self, _: u64, _: u64) -> crate::net::Provided<Vec<u8>> {
        match self.0 {
            Completion::NetBytes(value) => Ok(value.clone()),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
    fn write(&mut self, _: u64, bytes: &[u8]) -> crate::net::Provided<u64> {
        match self.0 {
            Completion::Count(Ok(n))
                if *n > bytes.len() as u64 || (*n == 0 && !bytes.is_empty()) =>
            {
                Err(AdapterError::InvalidCompletion)
            }
            Completion::Count(value) => Ok(*value),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
    fn write_all(&mut self, _: u64, _: &[u8]) -> crate::net::Provided<()> {
        self.unit()
    }
    fn shutdown(&mut self, _: u64, _: ShutdownHow) -> crate::net::Provided<()> {
        self.unit()
    }
    fn set_timeout(&mut self, _: u64, _: TimeoutKind, _: NetTimeout) -> crate::net::Provided<()> {
        self.unit()
    }
    fn peer_addr(&mut self, _: u64) -> crate::net::Provided<SocketAddr> {
        self.address()
    }
    fn tls_connect(
        &mut self,
        _: &dyn crate::net_tls::TlsEngine,
        _: u64,
        _: &str,
        _: &RootsDigest,
        _: Option<&std::sync::Arc<hirsel_run_schema::tls::Identity>>,
        _: &[String],
    ) -> crate::net::Provided<Option<String>> {
        self.protocol()
    }
    fn tls_accept(
        &mut self,
        _: &dyn crate::net_tls::TlsEngine,
        _: u64,
        _: &hirsel_run_schema::tls::Server,
    ) -> crate::net::Provided<Option<String>> {
        self.protocol()
    }
    fn tls_peer(&mut self, _: u64) -> crate::net::Provided<Option<Vec<u8>>> {
        match self.0 {
            Completion::Certificates(value) => Ok(value.clone()),
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
    fn tls_close(&mut self, _: u64) -> crate::net::Provided<()> {
        self.unit()
    }
    fn listen(&mut self, _: u64, _: SocketAddr) -> crate::net::Provided<()> {
        self.unit()
    }
    fn accept(&mut self, _: u64, _: u64) -> crate::net::Provided<SocketAddr> {
        self.address()
    }
    fn local_addr(&mut self, _: u64) -> crate::net::Provided<SocketAddr> {
        self.address()
    }
    fn close(&mut self, _: u64) {}
}

#[cfg(test)]
mod scripted_connect_tests {
    use super::*;
    use crate::net::{NetOperation, NetRule, NetTimeout};
    #[test]
    fn legacy_durable_blocked_connect_refuses_ambiguous_issue_order() {
        let connect = Request::Connect {
            net: 2,
            host: "example.com".into(),
            port: 80,
            timeout: NetTimeout::Unbounded,
        };
        let mut io = SimIo::new(
            Tree::default(),
            vec![
                Event {
                    request: Request::Net { io: 1 },
                    completion: Completion::Handle(Ok(2)),
                    fault: None,
                    resources: None,
                },
                Event {
                    request: connect.clone(),
                    completion: Completion::Blocked { committed: false },
                    fault: None,
                    resources: None,
                },
            ],
        );
        io.set_projections(["net".into()].into());
        io.set_net_rules(
            [NetRule {
                address: "example.com".into(),
                port: 80,
                operation: NetOperation::Connect,
            }]
            .into(),
        );
        io.set_net_script(
            vec![hirsel_run_schema::NetPeer {
                address: "example.com".into(),
                port: 80,
                resolves_to: vec![],
                tls: hirsel_run_schema::PeerTls::Plain,
                connections: vec![
                    hirsel_run_schema::NetConnection::Unanswered,
                    hirsel_run_schema::NetConnection::Refused,
                ],
            }],
            vec![],
        )
        .unwrap();
        io.legacy_barrier_waits(true);
        assert_eq!(
            io.call(Request::Net { io: 1 }),
            Ok(Completion::Handle(Ok(2)))
        );
        io.enter(10);
        assert!(
            matches!(io.call(connect), Err(AdapterError::Unsupported(detail)) if detail.contains("ambiguous Blocked") && detail.contains("re-record with Held"))
        );
        assert_eq!(io.position, 1);
    }
}
#[cfg(test)]
mod snapshot_wait_tests {
    use super::*;
    fn replay(held: bool, host: bool) -> (SimIo, Request) {
        let directory = Request::Directory {
            io: 1,
            path: "data".into(),
        };
        let open = Request::Open {
            directory: 2,
            path: "input".into(),
        };
        let read = Request::FileRead {
            directory: 2,
            file: 3,
            max: 1,
        };
        let mut io = SimIo::new(
            Tree::new([("data/input".into(), b"x".to_vec())].into()).unwrap(),
            vec![
                Event {
                    request: directory.clone(),
                    completion: Completion::Handle(Ok(2)),
                    fault: None,
                    resources: None,
                },
                Event {
                    request: open.clone(),
                    completion: if host {
                        Completion::FileInput(FileInput::Opened(3))
                    } else {
                        Completion::Handle(Ok(3))
                    },
                    fault: None,
                    resources: None,
                },
                Event {
                    request: read.clone(),
                    completion: if held {
                        Completion::Held
                    } else {
                        Completion::Blocked { committed: false }
                    },
                    fault: None,
                    resources: None,
                },
            ],
        );
        io.set_mounts([("data".into(), hirsel_run_schema::Access::Read)])
            .unwrap();
        assert_eq!(io.call(directory), Ok(Completion::Handle(Ok(2))));
        assert_eq!(io.call(open), Ok(Completion::Handle(Ok(3))));
        io.enter(10);
        assert_eq!(io.call(read.clone()), Err(AdapterError::Blocked));
        io.parked(10);
        (io, read)
    }
    #[test]
    fn root_snapshot_read_requires_confirmed_blocked_evidence() {
        let (mut io, _) = replay(false, false);
        assert_eq!(io.ready(), Ok(Ready::Tokens(vec![])));
        assert_eq!(io.wait(), Err(AdapterError::MissingInput(3)));
        io.set_ending(Some(hirsel_run_schema::RunOutcome::Blocked(vec![])));
        assert_eq!(io.wait(), Ok(false));
        assert_eq!(io.finish(), Ok(()));
    }
    #[test]
    fn held_file_read_still_requires_the_journal_release() {
        let (mut io, _) = replay(true, false);
        assert_eq!(io.wait(), Err(AdapterError::MissingInput(3)));
    }
    #[test]
    fn host_file_read_still_requires_its_completion() {
        let (mut io, _) = replay(false, true);
        assert_eq!(io.wait(), Err(AdapterError::MissingInput(3)));
    }
}

#[cfg(test)]
mod terminal_outcome_tests {
    use super::*;
    use crate::launch::{Recording, RecordingWriter};
    use hirsel_run_schema::{RunOutcome as O, SignalKind};

    fn binary(outcome: &O) -> Vec<u8> {
        let header = Recording {
            arguments: vec![],
            run_identity: [0; 32],
            hash_seed: 0,
            random_seed: 0,
            test_seed: 0,
            binding: None,
            tree: Tree::new(Default::default()).unwrap(),
            events: vec![],
        };
        let mut bytes = vec![];
        let mut writer = RecordingWriter::new(&mut bytes, &header).unwrap();
        writer.finish_with_outcome(outcome).unwrap();
        drop(writer);
        bytes
    }
    fn replay(bytes: Vec<u8>) -> SimIo {
        let (header, tape) = crate::tape_binary::replay(bytes).unwrap();
        SimIo::from_binary(header.tree, tape.unwrap())
    }
    #[test]
    fn confirmed_blocked_stop_is_required_and_observed_without_host_waits() {
        let ending = O::Blocked(vec![]);
        let mut io = replay(binary(&ending));
        assert_eq!(io.finish(), Err(AdapterError::MissingInput(0)));
        assert!(io.waits.is_empty());
        assert!(
            io.scripted_input(),
            "idle scheduler must observe the trailer"
        );
        assert_eq!(io.wait(), Ok(false));
        assert!(!io.scripted_input());
        assert_eq!(io.finish_outcome(&ending), Ok(()));
        assert_eq!(
            io.finish_outcome(&O::Returned(hirsel_run_schema::Returned::Ok)),
            Err(AdapterError::ReplayMismatch(0))
        );
    }
    #[test]
    fn scripted_default_signal_is_a_recorded_stop() {
        let mut io = replay(binary(&O::Signaled(SignalKind::Interrupt)));
        assert_eq!(io.finish(), Err(AdapterError::MissingInput(0)));
        assert!(io.waits.is_empty());
        assert!(io.scripted_input());
        assert_eq!(
            io.wait(),
            Err(AdapterError::Signaled(SignalKind::Interrupt))
        );
        assert_eq!(
            io.finish_outcome(&O::Signaled(SignalKind::Interrupt)),
            Ok(())
        );
        assert_eq!(
            io.finish_outcome(&O::Signaled(SignalKind::Terminate)),
            Err(AdapterError::ReplayMismatch(0))
        );
    }
    #[test]
    fn terminal_frame_requires_full_envelope_and_no_suffix() {
        let bytes = binary(&O::Blocked(vec![]));
        assert!(crate::tape_binary::parse(&bytes[..bytes.len() - 1]).is_err());
        let mut suffix = bytes.clone();
        suffix.push(0);
        assert!(crate::tape_binary::parse(&suffix).is_err());
        let mut corrupt = bytes;
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(crate::tape_binary::parse(&corrupt).is_err());
    }
    #[test]
    fn returned_and_budget_outcomes_still_require_the_exact_status() {
        for (a, b) in [
            (
                O::Returned(hirsel_run_schema::Returned::Ok),
                O::Returned(hirsel_run_schema::Returned::Error),
            ),
            (
                O::BudgetExceeded(hirsel_run_schema::BudgetKind::Fuel),
                O::BudgetExceeded(hirsel_run_schema::BudgetKind::Memory),
            ),
        ] {
            let io = replay(binary(&a));
            assert_eq!(io.finish_outcome(&a), Ok(()));
            assert_eq!(io.finish_outcome(&b), Err(AdapterError::ReplayMismatch(0)));
        }
    }
}
