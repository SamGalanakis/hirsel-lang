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
use std::sync::Arc;
mod concurrent;
mod discovery;
pub use concurrent::ConcurrentIo;
pub(crate) use concurrent::HOME_TOKEN;
/// The home adapter, accepting custom streams with no `Send` bound.
pub type RealIo = RealCore<LocalStreams>;

/// Real host authority, providers and recording state, parameterized by stream ownership.
/// `RealCore<SendStreams>` is `Send`; non-Send custom streams remain home-affine.
pub struct RealCore<S: Streams> {
    file_reads: crate::io_sim::ScriptedFileReads,
    file_faults: crate::io_sim::ScriptedFileFaults,
    state: Box<State>,
    disk: Disk,
    tape: Vec<Event>,
    fault: Option<Fault>,
    streams: S,
    /// The recording level's gate: every request and event passes through it.
    journal: crate::journal::Journaled<S::Journal>,
    retain_history: bool,
    /// Last validated read/write stream. Only ordinary calls mutate stream
    /// authority; call clears both slots before executing one of them.
    live_streams: [Option<(u64, u64, u64)>; 2],
    /// The host network provider, once a net entry handed its constructor over
    /// (`Adapter::net`); without it a host network request is a tool refusal.
    net: Option<Box<dyn NetProvider + Send>>,
    /// Cooperative admission: the readiness its providers park on and the waits it records.
    cooperative: Option<(crate::readiness::Readiness, crate::readiness::WaitLog)>,
    /// Whether the host network provider parks on that readiness (`set_cooperative_net`);
    /// otherwise it blocks, as under the harness virtual clock.
    cooperative_net: bool,
    /// Test clocks use the same timer heap, with an independent virtual origin.
    test_clock: Option<crate::readiness::Readiness>,
    /// The host process provider, once a process entry handed its constructor over
    /// (`Adapter::process`); without it a host process call is a tool refusal.
    children: Option<Box<dyn crate::process::ProcessHost + Send>>,
    /// Runner call outcomes delivered and not yet taken, by wait token (RUN-11).
    delivered: BTreeMap<u64, hirsel_run_schema::RunnerCompletion>,
    /// The admitted `io.signals` source, and the parked signal waits by token: their kinds
    /// as bits and whether their task parked.
    signals: Option<crate::signals::SignalSource>,
    signal_waits: BTreeMap<u64, (u8, bool)>,
    /// The entered wait token, with or without cooperative admission.
    current: u64,
    /// A script delivered an arrival at the last idle point (no cooperative readiness).
    signal_delivered: bool,
    /// A keyed caller's events, recorded once it supplies their resource footprint.
    staged: Option<Vec<Event>>,
    /// The home adapter's resource discovery, admitted at its first use; the
    /// concurrent service takes it over (`take_registry`).
    registry: std::cell::RefCell<Option<crate::io_keys::Registry>>,
}
/// Stream access used by the real adapter. Implementations are sealed to this module.
#[doc(hidden)]
pub trait Streams: sealed::Sealed {
    type Journal: crate::journal::Journal;
    fn send_journal(journal: Box<dyn crate::journal::Journal + Send>) -> Self::Journal;
    fn net(&mut self) -> Option<&mut (dyn NetProvider + 'static)> {
        None
    }
    fn install_net(&mut self, _net: Box<dyn NetProvider>) {}
    fn children(&mut self) -> Option<&mut (dyn crate::process::ProcessHost + 'static)> {
        None
    }
    fn install_children(&mut self, _host: fn() -> Box<dyn crate::process::ProcessHost>) {}

    fn output(&mut self, stderr: bool) -> &mut dyn Write;
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<Option<usize>>;
    /// The admitted storage of stdout, stderr and stdin, for resource discovery.
    fn storages(&self) -> [crate::io_keys::Storage; 3];
}
mod sealed {
    pub trait Sealed {}
}

/// Home-affine streams; explicitly Send streams may be converted at admission.
pub struct LocalStreams {
    output: Output,
    input: Input,
    stderr: Output,
    net: Option<Box<dyn NetProvider>>,
    children: Option<Box<dyn crate::process::ProcessHost>>,
}
enum Output {
    Local(Box<dyn Write>),
    Send(crate::host::SendSink),
}
enum Input {
    Blocking(Box<dyn Read>),
    Send(Box<dyn Read + Send>, crate::io_keys::Storage),
    Cooperative(crate::host_stdin::CooperativeStdin),
}
/// Streams that can move to the shared real adapter.
pub struct SendStreams {
    output: crate::host::SendSink,
    input: SendInput,
    stderr: crate::host::SendSink,
}
enum SendInput {
    Blocking(Box<dyn Read + Send>, crate::io_keys::Storage),
    Cooperative(crate::host_stdin::CooperativeStdin),
}
impl sealed::Sealed for LocalStreams {}
impl sealed::Sealed for SendStreams {}
impl Streams for LocalStreams {
    type Journal = HomeJournal;
    fn send_journal(journal: Box<dyn crate::journal::Journal + Send>) -> HomeJournal {
        HomeJournal::Send(journal)
    }
    fn net(&mut self) -> Option<&mut (dyn NetProvider + 'static)> {
        self.net.as_deref_mut()
    }
    fn install_net(&mut self, net: Box<dyn NetProvider>) {
        self.net = Some(net);
    }
    fn children(&mut self) -> Option<&mut (dyn crate::process::ProcessHost + 'static)> {
        self.children.as_deref_mut()
    }
    fn install_children(&mut self, host: fn() -> Box<dyn crate::process::ProcessHost>) {
        self.children.get_or_insert_with(host);
    }

    fn output(&mut self, stderr: bool) -> &mut dyn Write {
        match if stderr {
            &mut self.stderr
        } else {
            &mut self.output
        } {
            Output::Local(output) => output.as_mut(),
            Output::Send(output) => output,
        }
    }
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<Option<usize>> {
        match &mut self.input {
            Input::Blocking(input) => input.read(bytes).map(Some),
            Input::Send(input, _) => input.read(bytes).map(Some),
            Input::Cooperative(input) => input.read(bytes),
        }
    }
    fn storages(&self) -> [crate::io_keys::Storage; 3] {
        use crate::io_keys::Storage;
        let output = |output: &Output| match output {
            Output::Local(_) => Storage::Unknown,
            Output::Send(output) => output.resource_storage(),
        };
        let input = match &self.input {
            Input::Blocking(_) => Storage::Unknown,
            Input::Send(_, storage) => *storage,
            Input::Cooperative(input) => input.storage(),
        };
        [output(&self.output), output(&self.stderr), input]
    }
}
impl Streams for SendStreams {
    type Journal = Box<dyn crate::journal::Journal + Send>;
    fn send_journal(journal: Self::Journal) -> Self::Journal {
        journal
    }

    fn output(&mut self, stderr: bool) -> &mut dyn Write {
        if stderr {
            &mut self.stderr
        } else {
            &mut self.output
        }
    }
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<Option<usize>> {
        match &mut self.input {
            SendInput::Blocking(input, _) => input.read(bytes).map(Some),
            SendInput::Cooperative(input) => input.read(bytes),
        }
    }
    fn storages(&self) -> [crate::io_keys::Storage; 3] {
        let input = match &self.input {
            SendInput::Blocking(_, storage) => *storage,
            SendInput::Cooperative(input) => input.storage(),
        };
        [
            self.output.resource_storage(),
            self.stderr.resource_storage(),
            input,
        ]
    }
}
/// The home journal keeps its original non-Send extension point.
#[doc(hidden)]
pub enum HomeJournal {
    Local(Box<dyn crate::journal::Journal>),
    Send(Box<dyn crate::journal::Journal + Send>),
}
impl HomeJournal {
    fn as_journal(&self) -> &dyn crate::journal::Journal {
        match self {
            Self::Local(j) => j.as_ref(),
            Self::Send(j) => j.as_ref(),
        }
    }
    fn as_journal_mut(&mut self) -> &mut dyn crate::journal::Journal {
        match self {
            Self::Local(j) => j.as_mut(),
            Self::Send(j) => j.as_mut(),
        }
    }
}
impl crate::journal::Journal for HomeJournal {
    fn records(&self) -> bool {
        self.as_journal().records()
    }
    fn durable(&self) -> bool {
        self.as_journal().durable()
    }
    fn prepare(&mut self, p: &crate::journal::Prepared) -> std::io::Result<()> {
        self.as_journal_mut().prepare(p)
    }
    fn event(&mut self, id: Option<u64>, event: &Event) -> std::io::Result<()> {
        self.as_journal_mut().event(id, event)
    }
    fn withdraw(&mut self, id: u64) -> std::io::Result<()> {
        self.as_journal_mut().withdraw(id)
    }
    fn barrier(&mut self) -> std::io::Result<()> {
        self.as_journal_mut().barrier()
    }
    fn pending(&self) -> usize {
        self.as_journal().pending()
    }
    fn end(&mut self, outcome: &hirsel_run_schema::RunOutcome) -> std::io::Result<()> {
        self.as_journal_mut().end(outcome)
    }
}
impl RealIo {
    pub(crate) fn free_process_keys(&self, handle: u64) -> Option<crate::io_keys::KeySet> {
        if !matches!(
            self.state.handles.rows.get(&handle),
            Some(crate::handles::Grant::Process { .. })
        ) {
            return None;
        }
        self.with_registry(|registry| registry.process_keys(handle))
    }
    pub(crate) fn free_net_ports(&self, handle: u64) -> Option<crate::io_keys::Key> {
        if !matches!(
            self.state.handles.rows.get(&handle),
            Some(crate::handles::Grant::Net { .. })
        ) {
            return None;
        }
        self.with_registry(|registry| registry.net_ports(handle))
    }
    pub(crate) fn namespace_coupled(&self) -> bool {
        self.streams
            .storages()
            .contains(&crate::io_keys::Storage::Unknown)
    }
    /// Preserve custom home-affine journals; explicit Send journals use send_journal_to.
    pub fn journal_to(&mut self, journal: Box<dyn crate::journal::Journal>) {
        self.journal = crate::journal::Journaled::with_destination(HomeJournal::Local(journal));
        self.state.capture_views = self.journal.records() || self.retain_history;
    }
    /// A custom home provider is never silently moved to another worker.
    pub fn set_net_provider(&mut self, provider: impl NetProvider + 'static) {
        self.streams.net = Some(Box::new(provider));
    }
    /// Explicitly transportable provider; admission must configure cooperative readiness first.
    pub fn set_send_net_provider(&mut self, provider: impl NetProvider + Send + 'static) {
        self.net = Some(Box::new(provider));
    }
    /// Pin the explicitly admitted input stream before entry.
    pub fn set_stdin(&mut self, input: impl Read + 'static) {
        self.streams.input = Input::Blocking(Box::new(input));
    }
    pub fn set_send_stdin(&mut self, input: impl Read + Send + 'static) {
        self.streams.input = Input::Send(Box::new(input), crate::io_keys::Storage::Unknown);
    }
    pub fn set_cooperative_stdin(&mut self, input: crate::host_stdin::CooperativeStdin) {
        self.streams.input = Input::Cooperative(input);
    }
    pub fn set_stderr(&mut self, output: impl Write + 'static) {
        self.streams.stderr = Output::Local(Box::new(output));
    }
    pub fn set_send_output(&mut self, output: crate::host::SendSink) {
        self.streams.output = Output::Send(output);
    }
    pub fn set_send_stderr(&mut self, output: crate::host::SendSink) {
        self.streams.stderr = Output::Send(output);
    }
}
impl<S: Streams> RealCore<S> {
    /// Fixture-only regular-file readiness. Host files are never gated.
    pub fn set_file_read_script(
        &mut self,
        scripts: BTreeMap<String, hirsel_run_schema::FileReadScript>,
    ) {
        self.file_reads = crate::io_sim::ScriptedFileReads::new(scripts);
        if self.cooperative.is_none() {
            self.set_cooperative(Default::default());
        }
    }
    /// Script errors only for snapshot/test.dir requests; production storage ignores them.
    pub(crate) fn set_file_fault_script(
        &mut self,
        scripts: hirsel_run_schema::FileFaults,
    ) -> Result<(), String> {
        self.file_faults = crate::io_sim::ScriptedFileFaults::new(scripts)?;
        Ok(())
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
        peers: Vec<hirsel_run_schema::NetPeer>,
        listeners: Vec<hirsel_run_schema::NetListener>,
    ) -> Result<(), AdapterError> {
        self.state.set_net_script(peers, listeners)
    }
    /// The admitted `io.signals` grant: its script, or the process's signal hold, which a
    /// second holder is refused.
    pub fn set_signal_grant(
        &mut self,
        grant: &hirsel_run_schema::SignalsGrant,
    ) -> Result<(), hirsel_run_schema::RunRefusal> {
        let source = crate::host_launch::signal_source(grant)?;
        self.set_signals(grant.bits(), source);
        Ok(())
    }
    /// The admitted `io.signals` grant's observed kinds and its source.
    pub(crate) fn set_signals(&mut self, observed: u8, source: crate::signals::SignalSource) {
        self.state.signal_kinds = observed;
        self.state.projections.insert("signals".into());
        self.signals = Some(source);
    }
    /// The admitted `io.process` grant. Absent, the root Proc runs nothing.
    pub fn set_process_grant(&mut self, grant: &hirsel_run_schema::ProcessGrant) {
        self.state.set_process_grant(grant);
        if grant.timed.is_some() {
            if let Some((r, _)) = &self.cooperative {
                r.isolate_process_clock();
            }
            self.state.last_monotonic = self
                .cooperative
                .as_ref()
                .and_then(|(r, _)| r.virtual_time());
        }
    }
    /// The admitted RunConfig rule set of the root Net (`io.net.rules`). Absent, the
    /// root Net permits nothing.
    pub fn set_net_rules(&mut self, rules: std::collections::BTreeSet<NetRule>) {
        self.state.net_rules = rules;
    }
    /// Cooperative admission: a request whose provider parks on `readiness` answers
    /// `AdapterError::Blocked`, and the tape records it and each ready poll (DET-1).
    pub fn set_cooperative(&mut self, readiness: crate::readiness::Readiness) {
        if self.state.timed_process {
            readiness.isolate_process_clock();
        }
        self.disk.readiness = Some(readiness.clone());
        self.cooperative = Some((readiness, Default::default()));
    }
    /// Cooperative admission whose host network provider is cooperative too: the net
    /// entries build `net_host::CooperativeNet` on `readiness` (`Adapter::net`).
    pub fn set_cooperative_net(&mut self, readiness: crate::readiness::Readiness) {
        self.set_cooperative(readiness);
        self.cooperative_net = true;
    }
    /// Semantic harness clock, with the ordinary cooperative park/ready tape protocol.
    pub(crate) fn virtual_clock(&mut self, ticks: u64) {
        self.clock_grants(true, false);
        self.set_cooperative(crate::readiness::Readiness::virtual_clock(ticks));
    }
    /// Admission supplies the configured projections; recordings cannot widen them.
    pub fn set_projections(&mut self, projections: std::collections::BTreeSet<String>) {
        self.state.projections = projections;
    }
    /// The harness's denied-output fixture only removes authority.
    pub(crate) fn deny_output(&mut self) {
        self.state.projections.remove("stdout");
        self.state.projections.remove("stderr");
    }
}
impl RealIo {
    /// The caller explicitly grants this directory. No ambient directory is consulted later.
    pub fn grant(root: &Path) -> std::io::Result<Self> {
        {
            let mut io = Self::grant_with_output(root, std::io::sink())?;
            io.set_send_output(crate::host::SendSink::shared(Arc::new(
                std::sync::Mutex::new(std::io::sink()),
            )));
            Ok(io)
        }
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
            [(String::new(), hirsel_run_schema::Access::ReadWrite, root)],
            output,
        )
        .expect("one explicit root mount")
    }
    /// Test launcher only: use snapshot storage, never host files, for root mounts.
    pub(crate) fn fixture(
        files: BTreeMap<String, Vec<u8>>,
        mounts: BTreeMap<String, hirsel_run_schema::Access>,
        stdin: Vec<u8>,
        output: impl Write + 'static,
    ) -> Result<Self, hirsel_run_schema::RunRefusal> {
        let tree = Tree::fixture(files, mounts.clone())?;
        let mut io = Self::from_pinned_mounts([], output)?;
        io.set_send_stdin(std::io::Cursor::new(stdin));
        io.state.fixture(tree, mount_access(mounts)?);
        Ok(io)
    }
    /// Retain every admitted descriptor and its rights. Empty Io has no directory pin.
    pub fn from_pinned_mounts(
        mounts: impl IntoIterator<
            Item = (
                String,
                hirsel_run_schema::Access,
                crate::host_paths::PinnedDirectory,
            ),
        >,
        output: impl Write + 'static,
    ) -> Result<Self, hirsel_run_schema::RunRefusal> {
        let mounts: Vec<_> = mounts.into_iter().collect();
        let access = mount_access(
            mounts
                .iter()
                .map(|(path, access, _)| (path.clone(), *access)),
        )?;
        let mut state = Box::new(State::default());
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
                prefetched: None,
                prefetched_output: None,
                prefetched_path: None,
                observed_output: None,
                last_open: None,
                partial: None,
                next_write: 1,
                prepared: BTreeMap::new(),
                maps: BTreeMap::new(),
                watches: BTreeMap::new(),
                readiness: None,
            },
            tape: vec![],
            fault: None,
            streams: LocalStreams {
                output: Output::Local(Box::new(output)),
                input: Input::Send(
                    Box::new(std::io::empty()),
                    crate::io_keys::Storage::Shared(0),
                ),
                net: None,
                children: None,
                stderr: Output::Send(crate::host::SendSink::shared(Arc::new(
                    std::sync::Mutex::new(std::io::sink()),
                ))),
            },
            journal: crate::journal::Journaled::unrecorded(HomeJournal::Send(Box::new(
                crate::journal::NoJournal,
            ))),
            retain_history: true,
            live_streams: [None; 2],
            net: None,
            cooperative: None,
            cooperative_net: false,
            test_clock: None,
            children: None,
            delivered: BTreeMap::new(),
            signals: None,
            signal_waits: BTreeMap::new(),
            current: 0,
            signal_delivered: false,
            staged: None,
            registry: Default::default(),
            file_reads: Default::default(),
            file_faults: Default::default(),
        })
    }
}
impl<S: Streams> RealCore<S> {
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
    ) -> Result<File, hirsel_run_schema::RunRefusal> {
        crate::host_paths::RecorderDestination::pin(path)?
            .open_with_files(&self.mount_pins(), files)
    }
    /// Independently admitted clock providers; absent grants stay denied.
    pub fn clock_grants(&mut self, monotonic: bool, wall: bool) {
        self.state.monotonic_granted = monotonic;
        self.state.wall_granted = wall;
    }
    /// The admitted Random provider and the run's Random seed (used only by splitmix64).
    pub fn set_random(&mut self, provider: hirsel_run_schema::ProviderContract, seed: u64) {
        self.state.random = match provider {
            hirsel_run_schema::ProviderContract::RandomSystem => RandomSource::System,
            _ => RandomSource::SplitMix64(seed),
        };
    }
    /// The admitted Telemetry provider.
    pub fn set_telemetry(&mut self, capture: crate::telemetry::Capture) {
        self.state.telemetry = capture;
    }
    /// The run's Telemetry provider state: its simulated exporter's queue and drops.
    pub fn telemetry(&self) -> &crate::telemetry::Capture {
        &self.state.telemetry
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
        self.send_journal_to(Box::new(crate::journal::TapeJournal(writer)));
    }
    /// The cooperative readiness this run parks on, which its callee runs share (RUN-12).
    pub fn readiness(&self) -> Option<crate::readiness::Readiness> {
        self.cooperative
            .as_ref()
            .map(|(readiness, _)| readiness.clone())
    }
    /// The outcome of the runner call parked under `token`: re-issuing the call completes
    /// it, and the wait is ready at the next scheduling point.
    pub fn deliver(&mut self, token: u64, completion: hirsel_run_schema::RunnerCompletion) {
        self.delivered.insert(token, completion);
        if let Some((readiness, _)) = &self.cooperative {
            readiness.deliver(token);
        }
    }
    /// Record through `journal`, the recording level the run selected.
    pub fn send_journal_to(&mut self, journal: Box<dyn crate::journal::Journal + Send>) {
        self.journal = crate::journal::Journaled::with_destination(S::send_journal(journal));
        self.state.capture_views = self.journal.records() || self.retain_history;
    }
    /// The run ended with `outcome`: the recording's trailer.
    pub fn finish_recording(
        &mut self,
        outcome: &hirsel_run_schema::RunOutcome,
    ) -> std::io::Result<()> {
        self.journal.end(outcome)
    }
    /// The native launcher streams events and needs no duplicate in-memory history.
    pub(crate) fn discard_history(&mut self) {
        self.retain_history = false;
        self.state.capture_views = self.journal.records();
        self.tape.clear();
        self.state.stdout.clear();
    }
    fn record(&mut self, event: Event) -> Result<(), AdapterError> {
        if let Some(staged) = &mut self.staged {
            staged.push(event);
            return Ok(());
        }
        if !self.journal.absent() {
            self.journal.record(&event).map_err(recording_failed)?;
        }
        if self.retain_history {
            self.tape.push(event);
        }
        Ok(())
    }
    /// Whether the run's events reach a journal or the retained history.
    pub(crate) fn records_events(&self) -> bool {
        !self.journal.absent() || self.retain_history
    }
    /// Hold the next call's events until [`Self::commit`] supplies their footprint.
    pub(crate) fn stage(&mut self) {
        debug_assert!(self.staged.is_none());
        if self.records_events() {
            self.staged = Some(Vec::new());
        }
    }
    /// Record the staged events. Every request event carries the call's
    /// footprint; the first also carries the identities established since the
    /// previous request event. Scheduling inputs carry none. `footprint` runs
    /// only when a request event was staged.
    pub(crate) fn commit(
        &mut self,
        footprint: impl FnOnce() -> crate::io_keys::Footprint,
    ) -> Result<(), AdapterError> {
        let Some(events) = self.staged.take() else {
            return Ok(());
        };
        let mut footprint = Some(footprint);
        let mut later = None;
        for mut event in events {
            if !matches!(
                event.request,
                Request::Ready { .. }
                    | Request::Schedule { .. }
                    | Request::CpuControl { .. }
                    | Request::CpuSummary(_)
                    | Request::ProcessTime { .. }
                    | Request::ProcessSettle { .. }
            ) {
                let resources = match footprint.take() {
                    Some(footprint) => {
                        let first = footprint();
                        later = Some(crate::io_keys::Footprint {
                            identities: Vec::new(),
                            ..first.clone()
                        });
                        first
                    }
                    None => later.clone().expect("the first request event's footprint"),
                };
                event.resources = Some(Box::new(resources));
            }
            self.record(event)?;
        }
        Ok(())
    }
    /// Record a keyed call's staged events with its footprint. Identities wait
    /// in the registry's log for the next recorded request event.
    pub(crate) fn commit_with(
        &mut self,
        registry: &mut crate::io_keys::Registry,
        keys: crate::io_keys::KeySet,
        domains: Vec<(crate::io_keys::Key, u64, String)>,
    ) -> Result<(), AdapterError> {
        if !self.records_events() {
            registry.established();
        }
        self.commit(|| crate::io_keys::Footprint {
            keys,
            domains,
            identities: registry.established(),
        })
    }
    /// One unkeyed call: the concurrent service supplies its own footprint.
    pub(crate) fn call_once(&mut self, request: Request) -> Result<Completion, AdapterError> {
        if self.state.timed_process {
            let virtual_clock = self
                .cooperative
                .as_ref()
                .and_then(|(r, _)| r.virtual_time());
            if virtual_clock.is_none() {
                return Err(AdapterError::Unsupported(
                    "timed process requires an isolated virtual clock".into(),
                ));
            }
        }
        let _ = self.preflight(&request)?;
        // Close, split/join, TLS and new grants use this protocol. Invalidate
        // before any authority mutation, including failed or canceled operations.
        self.live_streams = [None; 2];
        if self.state.awaits_injection(&request) {
            return Err(AdapterError::Blocked);
        }
        let parks = self.cooperative.is_some() && crate::host_waits::may_park(&request);
        // A durable journal prepares the request first; one that may park waits for the
        // barrier that makes its record durable, parked as for the host (group commit).
        let token = parks.then(|| self.cooperative.as_ref().unwrap().1.current());
        if !self.journal.absent()
            && self
                .journal
                .before(&request, token)
                .map_err(recording_failed)?
                == crate::journal::Gate::Hold
        {
            let (_, log) = self.cooperative.as_mut().unwrap();
            log.park(false);
            self.record(Event {
                request,
                completion: Completion::Held,
                fault: None,
                resources: None,
            })?;
            return Err(AdapterError::Blocked);
        }
        let network = crate::net::is_net_request(&request)
            || matches!(request, Request::Close { .. })
            || self.state.timed_process
                && matches!(
                    request,
                    Request::ChildWrite { .. }
                        | Request::ChildCloseStdin { .. }
                        | Request::ChildKill { .. }
                );
        let scripted =
            self.state.scripted(&request) || self.file_reads.blocked(&self.state, &request);
        if !parks {
            let result = self.perform(request);
            if self.state.timed_process {
                if let Some((r, _)) = &self.cooperative {
                    r.provider_deadline(self.state.next_process_time());
                }
            }
            if network && let Some((readiness, _)) = &self.cooperative {
                readiness.progressed();
            }
            return result;
        }
        let retry = request.clone();
        let connect = scripted
            && matches!(
                request,
                Request::Connect { .. } | Request::ConnectAddr { .. } | Request::UnixConnect { .. }
            );
        // Retrying an unanswered connect must not consume the next scripted attempt.
        let pending_connect = connect
            && self
                .cooperative
                .as_ref()
                .unwrap()
                .0
                .pending_scripted_connect();
        let result = if pending_connect {
            Err(AdapterError::Blocked)
        } else {
            self.perform(request)
        };
        if self.state.timed_process {
            if let Request::SleepUntil { clock, deadline } = &retry
                && !self.state.test_clocks.contains(clock)
            {
                if matches!(result, Err(AdapterError::Blocked)) {
                    self.state.timed_sleeps.insert(self.current, *deadline);
                } else {
                    self.state.timed_sleeps.remove(&self.current);
                }
            }
            if let Some((r, _)) = &self.cooperative {
                r.provider_deadline(self.state.next_process_time());
            }
        }
        match result {
            Err(AdapterError::Blocked) => {
                let (readiness, log) = self.cooperative.as_mut().unwrap();
                let token = log.current();
                if connect {
                    readiness.scripted_connect();
                } else if scripted
                    || self.state.timed_process
                        && matches!(
                            retry,
                            Request::ChildRead { .. }
                                | Request::ChildWait { .. }
                                | Request::GroupWait { .. }
                        )
                {
                    readiness.scripted();
                    if self.state.timed_process {
                        self.state.timed_requests.insert(token, retry.clone());
                    }
                }
                let committed = !readiness.cancellable(token);
                // Re-issued while its task stays parked (a held request a journal barrier
                // released first registers its interest here): Readiness reports it from now.
                if log.rows_of(token).is_some_and(|row| row.parked) {
                    readiness.parked(token);
                    if let Some(clock) = &self.test_clock {
                        clock.parked(token);
                    }
                }
                log.park(committed);
                self.record(Event {
                    request: retry,
                    completion: Completion::Blocked { committed },
                    fault: None,
                    resources: None,
                })?;
                Err(AdapterError::Blocked)
            }
            result => {
                if network && let Some((readiness, _)) = &self.cooperative {
                    readiness.progressed();
                }
                result
            }
        }
    }
    fn with_registry<T>(&self, f: impl FnOnce(&mut crate::io_keys::Registry) -> T) -> T {
        let mut registry = self.registry.borrow_mut();
        f(registry.get_or_insert_with(|| discovery::admit(self)))
    }
    /// The home discovery so far, for the concurrent service that takes over.
    pub(crate) fn take_registry(&mut self) -> crate::io_keys::Registry {
        match self.registry.get_mut().take() {
            Some(registry) => registry,
            None => discovery::admit(self),
        }
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
        if let Some(tree) = self.state.fake_trees.get(&0) {
            return Ok(tree.clone());
        }
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
                        let logical = hirsel_run_schema::confined_relative(&selector, &path)
                            .expect("snapshot entry");
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
            tree.directories
                .extend(snapshot.directories.iter().map(|p| {
                    hirsel_run_schema::confined_relative(selector, p).expect("snapshot directory")
                }));
            tree.files.extend(snapshot.files.iter().map(|(p, b)| {
                (
                    hirsel_run_schema::confined_relative(selector, p).expect("snapshot file"),
                    b.clone(),
                )
            }));
            tree.metadata.extend(snapshot.metadata.iter().map(|(p, m)| {
                (
                    hirsel_run_schema::confined_relative(selector, p).expect("snapshot metadata"),
                    m.clone(),
                )
            }));
            tree.links.extend(snapshot.links.iter().map(|(p, target)| {
                (
                    hirsel_run_schema::confined_relative(selector, p).expect("snapshot link"),
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
            let target = hirsel_run_schema::confined_relative(canonical, suffix)
                .expect("snapshot mount alias");
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
impl<S: Streams> Adapter for RealCore<S> {
    fn net_io(
        &mut self,
        token: u64,
        net: u64,
        stream: u64,
        inline: bool,
        buffer: &mut NetBuffer<'_>,
    ) -> Option<Result<crate::io::NetResult, AdapterError>> {
        if self.retain_history || self.journal.records() || self.fault.is_some() {
            return None;
        }
        let (write, empty) = match buffer {
            NetBuffer::Read(bytes) => (false, bytes.is_empty()),
            _ => (true, false),
        };
        let slot = usize::from(write);
        let id = match self.live_streams[slot] {
            Some((owner, handle, id)) if owner == net && handle == stream && !empty => id,
            _ => match self.state.host_stream(net, stream, write, empty) {
                Ok(Some(Ok(id))) => {
                    self.live_streams[slot] = Some((net, stream, id));
                    id
                }
                Ok(Some(Err(error))) => return Some(Ok(Err(error))),
                Ok(None) => return None,
                Err(error) => return Some(Err(error)),
            },
        };
        if inline
            && let Some(provider) = self.streams.net().or_else(|| {
                self.net
                    .as_deref_mut()
                    .map(|net| net as &mut dyn NetProvider)
            })
            && let Some(result) = provider.net_io_inline(id, buffer)
        {
            return Some(result);
        }
        if let Some((readiness, _)) = &self.cooperative {
            readiness.enter(token);
        }
        Some((|| {
            let provider = self
                .streams
                .net()
                .or_else(|| {
                    self.net
                        .as_deref_mut()
                        .map(|net| net as &mut dyn NetProvider)
                })
                .ok_or_else(|| AdapterError::Unsupported("network provider missing".into()))?;
            loop {
                let result = match buffer {
                    NetBuffer::Read(bytes) => provider
                        .read_uninit(id, bytes)
                        .map(|value| value.map(|n| n as u64)),
                    NetBuffer::Write(bytes) => provider.write(id, bytes),
                    NetBuffer::WriteAll(bytes) => {
                        provider.write_all(id, bytes).map(|value| value.map(|()| 0))
                    }
                };
                if let Some((readiness, _)) = &self.cooperative {
                    if matches!(result, Err(AdapterError::Blocked)) {
                        // Authority and arguments cannot change while this sole root waits.
                        // Retry the provider directly, without rebuilding the host protocol.
                        if inline && readiness.wait_current()? {
                            continue;
                        }
                    } else {
                        readiness.progressed();
                        readiness.finish(token);
                    }
                }
                return result;
            }
        })())
    }

    fn print_observed(
        &mut self,
        handle: u64,
        stderr: bool,
        bytes: &[u8],
    ) -> (Result<Completion, AdapterError>, Option<u64>) {
        if !self.retain_history && !self.journal.records() && self.fault.is_none() {
            if let Err(error) = self.state.expect(
                handle,
                if stderr {
                    crate::handles::Grant::Stderr
                } else {
                    crate::handles::Grant::Stdout
                },
            ) {
                return (Err(error), None);
            }
            let (value, written) = write_output(self.streams.output(stderr), bytes);
            return (Ok(Completion::Unit(value)), Some(written as u64));
        }
        self.disk.observed_output = None;
        let result = self.print(handle, stderr, bytes);
        (result, self.disk.observed_output.take())
    }

    fn print(
        &mut self,
        handle: u64,
        stderr: bool,
        bytes: &[u8],
    ) -> Result<Completion, AdapterError> {
        if !self.retain_history && !self.journal.records() && self.fault.is_none() {
            self.state.expect(
                handle,
                if stderr {
                    crate::handles::Grant::Stderr
                } else {
                    crate::handles::Grant::Stdout
                },
            )?;
            let output = self.streams.output(stderr);
            let (value, _) = write_output(output, bytes);
            return Ok(Completion::Unit(value));
        }
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

    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        let checked = self.state.preflight(request)?;
        if checked.is_some() {
            return Ok(checked);
        }
        if self.state.timed_process {
            if matches!(request, Request::Wall { .. }) {
                return Err(AdapterError::Unsupported(
                    "host wall clock in timed process clock domain".into(),
                ));
            }
            if crate::net::is_net_request(request) && !self.state.scripted(request) {
                return Err(AdapterError::Unsupported(
                    "live network in timed process clock domain".into(),
                ));
            }
            if (self.state.file_directory(request).is_some()
                || matches!(request, Request::Directory { .. }))
                && !self.state.is_fake_request(request)
                && !matches!(request, Request::Close { .. })
            {
                return Err(AdapterError::Unsupported(
                    "live file operation in timed process clock domain".into(),
                ));
            }
        }
        Ok(None)
    }

    fn tls(&mut self, engine: &'static dyn crate::net_tls::TlsEngine) {
        self.state.tls = crate::net_tls::Engine(Some(engine));
    }
    fn net(&mut self, host: crate::net_host::Provider) {
        if self.net.is_none() && self.streams.net().is_none() {
            let readiness = self.cooperative.as_ref().filter(|_| self.cooperative_net);
            self.streams
                .install_net(host(readiness.map(|(readiness, _)| readiness)));
        }
    }
    fn process(&mut self, host: fn() -> Box<dyn crate::process::ProcessHost>) {
        if self.children.is_none() {
            self.streams.install_children(host);
        }
    }
    fn tls_roots(&mut self, stream: u64, pem: &[u8]) {
        self.state.remember_tls_roots(stream, pem);
        if let Some(net) = self.streams.net().or_else(|| {
            self.net
                .as_deref_mut()
                .map(|net| net as &mut dyn NetProvider)
        }) {
            net.tls_roots(pem);
        }
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
    fn wants_issuer(&self) -> bool {
        self.journal.durable()
    }
    fn issuer(&mut self, task: &[u32]) {
        self.journal.issuer(task);
    }
    /// The home adapter is a keyed recorder: its keys, domains and identities
    /// are the concurrent service's, from the same discovery.
    fn keys(&self, request: &Request) -> Option<crate::io_keys::KeySet> {
        Some(self.with_registry(|registry| registry.keys(request)))
    }
    fn domains(&self, request: &Request) -> Vec<(crate::io_keys::Key, u64, String)> {
        self.with_registry(|registry| discovery::domains(self, registry, request))
    }
    fn identity(&self, handle: u64) -> Option<(crate::io_keys::Key, crate::io_keys::Kind)> {
        self.with_registry(|registry| registry.identity(handle))
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        let keys = self.with_registry(|registry| registry.keys(&request));
        let domains = if self.records_events() {
            self.domains(&request)
        } else {
            Vec::new()
        };
        let publication = discovery::Publication::of(self, &request);
        self.stage();
        let result = self.call_once(request);
        let mut registry = self.take_registry();
        publication.publish(self, &mut registry, &result);
        let committed = self.commit_with(&mut registry, keys, domains);
        *self.registry.get_mut() = Some(registry);
        committed?;
        result
    }
    fn enter(&mut self, token: u64) {
        self.current = token;
        if let Some(clock) = &self.test_clock {
            clock.enter(token);
        }
        if let Some((readiness, log)) = &mut self.cooperative {
            readiness.enter(token);
            log.enter(token);
        }
    }
    fn finish(&mut self, token: u64) {
        self.state.timed_requests.remove(&token);
        self.state.timed_sleeps.remove(&token);
        if self.state.timed_process {
            if let Some((r, _)) = &self.cooperative {
                r.provider_deadline(self.state.next_process_time());
            }
        }
        self.signal_waits.remove(&token);
        if let Some(clock) = &self.test_clock {
            clock.finish(token);
        }
        if let Some((readiness, log)) = &mut self.cooperative {
            readiness.finish(token);
            log.finish(token);
        }
        // A withdrawal need not be durable: a crash before it leaves the request in doubt.
        if let Err(error) = self.journal.finish(token) {
            self.journal.fail(error);
        }
    }
    fn parked(&mut self, token: u64) {
        if let Some(clock) = &self.test_clock {
            clock.parked(token);
        }
        if let Some((_, parked)) = self.signal_waits.get_mut(&token) {
            *parked = true;
        }
        if let Some((readiness, log)) = &mut self.cooperative {
            readiness.parked(token);
            log.parked(token);
        }
        self.journal.parked(token);
    }
    fn cancellable(&self, token: u64) -> bool {
        self.cooperative
            .as_ref()
            .is_none_or(|(readiness, _)| readiness.cancellable(token))
    }
    fn cpu_summary(&mut self, summary: &CpuSummary) -> Result<(), AdapterError> {
        summary
            .validate()
            .map_err(|_| AdapterError::InvalidCompletion)?;
        self.record(Event {
            request: Request::CpuSummary(summary.clone()),
            completion: Completion::Ready,
            fault: None,
            resources: None,
        })
    }
    fn cpu_control(
        &mut self,
        width: u32,
        action: u32,
        count: u64,
        value: Option<u32>,
    ) -> Result<Option<u32>, AdapterError> {
        if let Some(outcome) = value {
            self.record(Event {
                request: Request::CpuControl {
                    width,
                    action,
                    count,
                    outcome,
                },
                completion: Completion::Ready,
                fault: None,
                resources: None,
            })?;
        }
        Ok(value)
    }
    fn schedule(&mut self, count: u64, choice: u64) -> Result<u64, AdapterError> {
        self.record(Event {
            request: Request::Schedule { count, choice },
            completion: Completion::Ready,
            fault: None,
            resources: None,
        })?;
        Ok(choice)
    }
    fn injected_wait(&self, request: &Request) -> bool {
        self.state.injected_wait(request)
    }
    fn telemetry_context(&self, telemetry: u64, span: u64) -> Option<Vec<u8>> {
        (crate::runtime_roots::telemetry()?.context)(&self.state, telemetry, span).ok()
    }
    /// The tokens the Readiness finds ready, with the held requests a journal barrier
    /// released (which wait on no host interest), each recorded as its wait's `ready` event,
    /// in token (so ordinal) order, before any of them is re-issued.
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        if self.state.timed_process {
            if let Some(to) = self
                .cooperative
                .as_ref()
                .and_then(|(r, _)| r.virtual_time())
            {
                let from = self.state.last_monotonic.unwrap_or(0);
                if to > from {
                    self.state.apply_process_time(from, to)?;
                    self.record(Event {
                        request: Request::ProcessTime { from, to },
                        completion: Completion::Ready,
                        fault: None,
                        resources: None,
                    })?;
                    if let Some((r, _)) = &self.cooperative {
                        r.provider_deadline(self.state.next_process_time());
                    }
                }
            }
        }
        self.ready_tokens(true)
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
        self.state.claim_children(canceled)?;
        self.record(Event {
            request: Request::ProcessSettle {
                canceled: canceled.to_vec(),
            },
            completion: Completion::Ready,
            fault: None,
            resources: None,
        })?;
        if let Some((child, report)) = self
            .state
            .settle_children(self.state.last_monotonic.unwrap_or(0))
        {
            let before = self.state.stdout.len();
            let error = self.state.process_failed(child, &report);
            self.publish_test_report(before)?;
            return Err(error);
        }
        if let Some((r, _)) = &self.cooperative {
            r.provider_deadline(self.state.next_process_time());
            r.progressed();
        }
        self.ready_tokens(false)
    }
    fn ready_deadline(&self) -> Option<std::time::Instant> {
        // Durable barriers, signal delivery, test time and runner completions
        // have readiness semantics beyond an exclusive real timer driver.
        if self.journal.durable()
            || self.test_clock.is_some()
            || !self.signal_waits.is_empty()
            || !self.delivered.is_empty()
        {
            return None;
        }
        self.cooperative.as_ref()?.0.ready_deadline()
    }
    fn advance_ready_points(&mut self, points: u64) {
        if (self.retain_history || self.journal.records())
            && let Some((_, log)) = &mut self.cooperative
        {
            log.advance(points);
        }
    }
    fn runner_readiness(&mut self) -> Option<crate::readiness::Readiness> {
        if self.cooperative.is_none() {
            // A harness run's in-process callees keep virtual time from 0, as its own
            // virtual clock would.
            self.set_cooperative(crate::readiness::Readiness::virtual_clock(0));
        }
        self.readiness()
    }
    fn deliver(&mut self, token: u64, completion: hirsel_run_schema::RunnerCompletion) {
        RealCore::deliver(self, token, completion);
    }
    fn has_ready(&mut self) -> Result<bool, AdapterError> {
        if self.signal_delivered && !self.signal_waits.is_empty() {
            return Ok(true);
        }
        match &self.cooperative {
            Some((readiness, _)) => readiness.has_ready(),
            None => Ok(false),
        }
    }
    /// An idle point delivers the signal script's next arrival before anything waits.
    fn wait_sources(&mut self) -> Result<Option<crate::io::WaitSources>, AdapterError> {
        // The run would wait for the host: first close the journal's batch.
        if self.journal.idle().map_err(recording_failed)? {
            return Ok(Some(crate::io::WaitSources::Ready));
        }
        if let Some(source) = &mut self.signals {
            match source.deliver() {
                crate::signals::Delivery::Pending => {
                    self.signal_delivered = true;
                    return Ok(Some(crate::io::WaitSources::Ready));
                }
                crate::signals::Delivery::Default(kind) => {
                    return Err(AdapterError::Signaled(kind));
                }
                crate::signals::Delivery::Exhausted => {}
            }
        }
        if let Some(clock) = &self.test_clock
            && clock.wait()?
        {
            return Ok(Some(crate::io::WaitSources::Ready));
        }
        Ok(Some(crate::io::WaitSources::Drivers(
            self.readiness().into_iter().collect(),
        )))
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        self.wait_sources()?
            .expect("real adapter source preparation")
            .wait()
    }
    fn scripted_input(&self) -> bool {
        self.signals.as_ref().is_some_and(|s| s.scripted())
            || self.state.next_child_gate().is_some()
            || self.state.process_batch_pending
    }
    fn wake_on(&mut self, fd: std::os::fd::RawFd) -> bool {
        match &self.cooperative {
            Some((readiness, _)) => readiness.wake_on(fd).is_ok(),
            None => false,
        }
    }
}
fn recording_failed(error: std::io::Error) -> AdapterError {
    AdapterError::RecordingFailed(error.to_string())
}
impl<S: Streams> RealCore<S> {
    fn publish_test_report(&mut self, before: usize) -> Result<(), AdapterError> {
        self.streams
            .output(false)
            .write_all(&self.state.stdout[before..])
            .and_then(|()| self.streams.output(false).flush())
            .map_err(|e| AdapterError::RecordingFailed(format!("test report: {e}")))?;
        if !self.retain_history {
            self.state.stdout.truncate(before);
        }
        Ok(())
    }
    fn ready_tokens(&mut self, point: bool) -> Result<Ready, AdapterError> {
        let released = self.journal.released().map_err(recording_failed)?;
        let Some((readiness, log)) = &mut self.cooperative else {
            return Ok(Ready::Tokens(
                if std::mem::take(&mut self.signal_delivered) {
                    self.signals_ready(Vec::new())
                } else {
                    Vec::new()
                },
            ));
        };
        // With no journal or retained history, no Ready events or wait-ordinal
        // bookkeeping are constructed. The readiness tokens are sufficient.
        let recording = self.retain_history || self.journal.records();
        if recording && point {
            log.point();
        }
        let mut tokens = readiness.ready()?;
        if self.state.timed_process {
            tokens.retain(|token| {
                let ready = self
                    .state
                    .timed_requests
                    .get(token)
                    .is_none_or(|request| self.state.process_ready(request));
                if !ready {
                    readiness.acknowledge_scripted(*token);
                }
                ready
            });
        }
        if let Some(clock) = &self.test_clock {
            tokens.extend(clock.ready()?);
        }
        if !released.is_empty() || self.test_clock.is_some() {
            tokens.extend(released);
            tokens.sort_unstable();
            tokens.dedup();
        }
        let mut tokens = if self.signal_waits.is_empty() {
            tokens
        } else {
            self.signals_ready(tokens)
        };
        if recording && !point {
            // A due taking-wait timer may already have published its token in phase A.
            // The same point's gate batch must not publish or record it a second time.
            let log = &self.cooperative.as_ref().unwrap().1;
            tokens.retain(|token| log.next_points(*token) != Some(0));
        }
        if !recording {
            return Ok(Ready::Tokens(tokens));
        }
        let Some((_, log)) = &mut self.cooperative else {
            unreachable!()
        };
        let events: Vec<Event> = tokens
            .iter()
            .filter_map(|&token| log.ready(token))
            .map(|(wait, points)| Event {
                request: Request::Ready { wait, points },
                completion: Completion::Ready,
                fault: None,
                resources: None,
            })
            .collect();
        for event in events {
            self.record(event)?;
        }
        Ok(Ready::Tokens(tokens))
    }

    /// A scheduling point's ready tokens with the parked signal waits decided by what is
    /// pending, not by the wake pipe: a wait is ready exactly when an arrival among its
    /// kinds is pending. The pipe is drained here, so an arrival no wait wants never keeps
    /// the readiness set awake.
    fn signals_ready(&mut self, mut tokens: Vec<u64>) -> Vec<u64> {
        let Some(source) = &mut self.signals else {
            return tokens;
        };
        source.drain();
        let pending = source.pending();
        tokens.retain(|token| !self.signal_waits.contains_key(token));
        tokens.extend(
            self.signal_waits
                .iter()
                .filter(|(_, (bits, parked))| *parked && bits & pending != 0)
                .map(|(token, _)| *token),
        );
        tokens.sort_unstable();
        tokens
    }
    /// `signal.wait` after its handle check: its program checks, a pending arrival, or a
    /// wait (the readiness interest of the source's wake pipe under cooperative admission).
    fn signal_wait(&mut self, kinds: u8) -> Result<Completion, AdapterError> {
        if let Some(error) = self.state.signal_check(kinds) {
            return Ok(Completion::Signal(Err(error)));
        }
        let source = self
            .signals
            .as_mut()
            .ok_or(AdapterError::InvalidCompletion)?;
        if let Some(kind) = source.take(kinds) {
            return Ok(Completion::Signal(Ok(kind)));
        }
        if let Some((readiness, _)) = &self.cooperative {
            readiness.pending().interest = Some((source.interest(), libc::POLLIN));
        } else if source.is_host() {
            return Ok(Completion::Signal(Ok(source.block(kinds))));
        }
        self.signal_waits.insert(self.current, (kinds, false));
        Err(AdapterError::Blocked)
    }
    fn perform(&mut self, request: Request) -> Result<Completion, AdapterError> {
        match &request {
            // A runner call completes with its delivered outcome, else waits for it.
            Request::RunnerPoll { .. } => {
                return Err(AdapterError::Unsupported(
                    "runner polls require qualified runner transport".into(),
                ));
            }
            Request::RunnerCall(_) => {
                let (readiness, log) = self.cooperative.as_ref().ok_or_else(|| {
                    AdapterError::Unsupported("a runner call without cooperative tasks".into())
                })?;
                let Some(completion) = self.delivered.remove(&log.current()) else {
                    readiness.await_delivery();
                    return Err(AdapterError::Blocked);
                };
                let completion = Completion::Runner(completion);
                self.record(Event {
                    request,
                    completion: completion.clone(),
                    fault: None,
                    resources: None,
                })?;
                return Ok(completion);
            }
            // The in-process place acknowledges a forwarded cancellation at once; a
            // callee's outcome is recorded as it ended.
            Request::RunnerCancel { .. } | Request::CallReturn(_) | Request::Stopped => {
                self.record(Event {
                    request,
                    completion: Completion::Ready,
                    fault: None,
                    resources: None,
                })?;
                return Ok(Completion::Ready);
            }
            _ => {}
        }
        if matches!(request, Request::TestClock { .. }) {
            if self.cooperative.is_none() {
                self.set_cooperative(Default::default());
            }
            self.test_clock
                .get_or_insert_with(|| crate::readiness::Readiness::virtual_clock(0));
        }
        let fake = self.state.is_fake_request(&request);
        if !self.journal.absent() {
            self.state.capture_views = self.journal.records() || self.retain_history;
        }
        let mut fault = if self.state.preflight(&request)?.is_some()
            || self.state.denied_projection(&request)
            || crate::net::is_net_request(&request)
            || self.state.is_process(&request)
            || matches!(&request, Request::ReadStdin { stdin, .. } if self.state.expect(*stdin, crate::handles::Grant::Stdin).is_err())
            || matches!(
                request,
                Request::Stdin { .. }
                    | Request::Stderr { .. }
                    | Request::Signals { .. }
                    | Request::SignalWait { .. }
                    | Request::ReadStdin { max: 0, .. }
                    | Request::EnvGet { .. }
                    | Request::EnvNarrow { .. }
            ) {
            None
        } else {
            self.fault.take()
        };
        if fault.is_none()
            && self.state.preflight(&request)?.is_none()
            && self.file_reads.blocked(&self.state, &request)
        {
            return Err(AdapterError::Blocked);
        }
        // Pending is checked first; neither parks nor task cancellation consume a script.
        if fault.is_none() {
            fault = self.file_faults.take(&self.state, &request)?;
        }
        self.disk.partial = None;
        let before = self.state.stdout.len();
        let stderr_before = self.state.stderr.len();
        let result = match &request {
            Request::SignalWait { signals, kinds } => {
                self.state
                    .expect(*signals, crate::handles::Grant::Signals)?;
                let completion = self.signal_wait(*kinds)?;
                self.state.sample(&request, completion)
            }
            Request::Signals { .. } => {
                let net: &mut dyn NetProvider = match (self.streams.net(), &mut self.net) {
                    (Some(net), _) => net,
                    (_, Some(net)) => &mut **net,
                    _ => &mut crate::net::NoNet,
                };
                let result = self.state.execute(&mut self.disk, net, &request, None);
                if let Ok(Completion::Handle(Ok(_))) = &result
                    && let Some(source) = &mut self.signals
                {
                    source.install();
                }
                result
            }
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
                                let read = self.streams.read(&mut bytes);
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
            Request::Monotonic { clock } if self.state.test_clocks.contains(clock) => {
                let ticks = self.test_clock.as_ref().unwrap().virtual_time().unwrap();
                self.state
                    .sample(&request, Completion::Monotonic(Ok(ticks)))
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
            Request::TelemetryBegin { .. } => {
                if fault.is_some() {
                    return Err(AdapterError::InvalidCompletion);
                }
                let input = (crate::runtime_roots::telemetry()
                    .ok_or(AdapterError::InvalidCompletion)?
                    .draw)(&mut self.state, &request)?;
                self.state.sample(&request, input)
            }
            Request::SleepUntil { clock, deadline } if self.state.test_clocks.contains(clock) => {
                if !self.test_clock.as_ref().unwrap().timer(*deadline)? {
                    return Err(AdapterError::Blocked);
                }
                self.state.sample(&request, Completion::Wait(Ok(())))
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
                let mut children = self.streams.children().or_else(|| {
                    self.children
                        .as_deref_mut()
                        .map(|host| host as &mut dyn crate::process::ProcessHost)
                });
                let cwd = |d: u64| directories.get(&d).map(|pin| pin.as_raw_fd());
                self.state.execute_process(
                    &mut |call| match &mut children {
                        Some(host) => host.call(call, &cwd, cooperative),
                        None => Err(AdapterError::InvalidCompletion),
                    },
                    &request,
                )
            }
            _ => {
                let net: &mut dyn NetProvider = match (self.streams.net(), &mut self.net) {
                    (Some(net), _) => net,
                    (_, Some(net)) => &mut **net,
                    _ => &mut crate::net::NoNet,
                };
                self.state
                    .execute(&mut self.disk, net, &request, fault.as_ref())
            }
        };
        if matches!(
            request,
            Request::Monotonic { .. }
                | Request::Wall { .. }
                | Request::RandomBytes { .. }
                | Request::TelemetryBegin { .. }
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
                    resources: None,
                })?;
                return Err(AdapterError::InvalidHandle(id));
            }
            Err(error) => return Err(error),
        };
        // A Test report (a failed check_case, a scripted client's unmet Expect) is
        // the only stdout a request other than Print appends.
        if !matches!(request, Request::Print { .. }) && self.state.stdout.len() > before {
            self.publish_test_report(before)?;
        }
        if matches!(request, Request::Print { .. } | Request::PrintStderr { .. }) {
            let stderr = matches!(request, Request::PrintStderr { .. });
            let output = self.streams.output(stderr);
            let captured = if stderr {
                &mut self.state.stderr
            } else {
                &mut self.state.stdout
            };
            let before = if stderr { stderr_before } else { before };
            let bytes = &captured[before..];
            let (value, written) = self
                .disk
                .prefetched_output
                .take()
                .unwrap_or_else(|| write_output(output, bytes));
            self.disk.observed_output = Some(written as u64);
            if let Err(error) = value {
                completion = Completion::Unit(Err(error.clone()));
                fault = Some(Fault {
                    error,
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
                | Completion::Entries(Err(e))
                | Completion::Metadata(Err(e))
                | Completion::Watch(Err(e))
                    if *e != IoError::NotGranted =>
                {
                    Some(e.clone())
                }
                _ => None,
            };
            fault = error.map(|error| Fault { error, partial: 0 });
        }
        match (&request, &completion) {
            (
                Request::Open { .. } | Request::Create { .. } | Request::OpenAppend { .. },
                Completion::Handle(Ok(id)),
            ) if !fake => {
                self.disk.files.insert(
                    *id,
                    Arc::new(self.disk.last_open.take().expect("opened descriptor")),
                );
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
        if self
            .file_reads
            .completed(&self.state, &request, &completion)
            && let Some((readiness, _)) = &self.cooperative
        {
            readiness.progressed();
        }
        if self.retain_history || !self.journal.absent() {
            self.record(Event {
                completion: if !fake && fault.is_none() {
                    FileInput::capture(&request, &completion)
                        .map(Completion::FileInput)
                        .unwrap_or_else(|| completion.clone())
                } else {
                    completion.clone()
                },
                request,
                fault,
                resources: None,
            })?;
        }
        Ok(completion)
    }
}
// Success includes flushing the provider's private buffer before returning.
fn write_output(output: &mut dyn Write, bytes: &[u8]) -> (Result<(), IoError>, usize) {
    let mut written = 0;
    let mut result = Ok(());
    while written < bytes.len() {
        match output.write(&bytes[written..]) {
            Ok(0) => {
                result = Err(IoError::Fault(1));
                break;
            }
            Ok(n) => written += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                result = Err(stream_error(e));
                break;
            }
        }
    }
    if let Err(e) = output.flush() {
        result = Err(stream_error(e));
    }
    (result, written)
}

#[derive(Debug)]
struct Disk {
    mounts: BTreeMap<String, crate::host_paths::PinnedDirectory>,
    directories: BTreeMap<u64, File>,
    active: String,
    active_id: u64,
    files: BTreeMap<u64, Arc<File>>,
    prefetched: Option<concurrent::FileResult>,
    prefetched_output: Option<(Result<(), IoError>, usize)>,
    prefetched_path: Option<PathResult>,
    observed_output: Option<u64>,
    last_open: Option<File>,
    partial: Option<(IoError, usize)>,
    next_write: u64,
    prepared: BTreeMap<(u64, String), File>,
    /// Live `file.map` mappings by Mapped handle.
    maps: BTreeMap<u64, crate::mapping::Mapping>,
    /// Live `file.watch` watchers by Watcher handle.
    watches: BTreeMap<u64, crate::watch::HostWatch>,
    /// Cooperative admission's readiness: a watcher with no event records its interest.
    readiness: Option<crate::readiness::Readiness>,
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
/// One path syscall's outcome, performed by a concurrent caller on a detached
/// directory view before the locked dispatch consumes it in place of its own.
#[derive(Debug)]
pub(super) enum PathResult {
    File(Result<File, IoError>),
    Metadata(Result<Metadata, IoError>),
    Bytes(Result<Vec<u8>, IoError>),
    Unit(Result<(), IoError>),
}

/// An activated directory: its descriptor and tree prefix. Disk resolves
/// every confined path through this view, locked or detached.
pub(super) struct At<'a> {
    pub(super) base: &'a File,
    pub(super) active: &'a str,
}

impl At<'_> {
    fn local<'p>(&self, path: &'p str) -> Result<&'p str, IoError> {
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
        open_beneath(self.base.as_raw_fd(), self.local(path)?, flags, mode)
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
    fn open_file(&self, path: &str) -> Result<File, IoError> {
        let fd = self.open(path, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        if !fd.metadata().map_err(error)?.is_file() {
            return Err(IoError::IsDirectory);
        }
        Ok(fd)
    }
    fn open_append_file(&self, path: &str) -> Result<File, IoError> {
        // Go os.OpenFile(O_APPEND|O_CREATE|O_WRONLY): retain confined pinned paths.
        // Open and check before writing: special files never receive writes.
        let fd = self.open(
            path,
            libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND | libc::O_NONBLOCK,
            0o600,
        )?;
        if !fd.metadata().map_err(error)?.is_file() {
            return Err(IoError::IsDirectory);
        }
        Ok(fd)
    }
    fn create_file(&self, path: &str) -> Result<File, IoError> {
        let fd = self.open_append_file(path)?;
        fd.set_len(0).map_err(error)?;
        Ok(fd)
    }
    fn read(&self, path: &str) -> Result<Vec<u8>, IoError> {
        let mut fd = self.open_file(path)?;
        let mut bytes = vec![];
        fd.read_to_end(&mut bytes).map_err(error)?;
        Ok(bytes)
    }
    fn metadata(&self, path: &str) -> Result<Metadata, IoError> {
        let pin = if self.local(path)?.is_empty() {
            self.base.try_clone().map_err(error)?
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
    fn unlink(&self, path: &str, flags: i32) -> Result<(), IoError> {
        let (dir, name) = self.parent(path)?;
        // SAFETY: live directory fd and NUL-terminated local leaf; unlinkat never
        // follows the leaf, and AT_REMOVEDIR refuses symlinks and nonempty dirs.
        if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), flags) } != 0 {
            Err(error(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
    fn mkdir(&self, path: &str) -> Result<(), IoError> {
        let (parent, name) = self.parent(path)?;
        // SAFETY: live pinned directory and terminated leaf; mkdirat does not follow the leaf.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o777) } != 0 {
            Err(error(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
    /// The syscall execute_storage would make for `request` at `path`.
    pub(super) fn call(&self, request: &Request, path: &str) -> PathResult {
        use Request as R;
        match request {
            R::Open { .. } => PathResult::File(self.open_file(path)),
            R::OpenAppend { .. } => PathResult::File(self.open_append_file(path)),
            R::Create { .. } => PathResult::File(self.create_file(path)),
            R::Read { .. } | R::Size { .. } => PathResult::Bytes(self.read(path)),
            R::Metadata { .. } => PathResult::Metadata(self.metadata(path)),
            R::Mkdir { .. } => PathResult::Unit(self.mkdir(path)),
            R::RemoveDir { .. } => PathResult::Unit(self.unlink(path, libc::AT_REMOVEDIR)),
            R::Remove { .. } => PathResult::Unit(if self.local(path).is_ok_and(str::is_empty) {
                Err(IoError::IsDirectory)
            } else {
                self.local(path).and_then(|_| self.unlink(path, 0))
            }),
            _ => unreachable!("not a detached path syscall"),
        }
    }
}

impl Disk {
    fn at(&self) -> Result<At<'_>, IoError> {
        Ok(At {
            base: self.base()?,
            active: &self.active,
        })
    }
    /// Activate `id` and detach its view for an unlocked path syscall.
    pub(super) fn detach(&mut self, id: u64, root: &str) -> Result<(File, String), IoError> {
        self.activate(id, root);
        Ok((
            self.base()?.try_clone().map_err(error)?,
            self.active.clone(),
        ))
    }
    fn prefetched_path(&mut self) -> Option<PathResult> {
        self.prefetched_path.take()
    }
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
        self.at()?.open(path, flags, mode)
    }
    fn parent(&self, path: &str) -> Result<(File, CString), IoError> {
        self.at()?.parent(path)
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
        let fd = match self.prefetched_path() {
            Some(PathResult::File(fd)) => fd,
            None => self.at()?.open_file(path),
            other => unreachable!("prefetched {other:?} for open"),
        }?;
        self.last_open = Some(fd);
        Ok(())
    }
    fn create_file(&mut self, path: &str, _id: u64) -> Result<(), IoError> {
        let fd = match self.prefetched_path() {
            Some(PathResult::File(fd)) => fd,
            None => self.at()?.create_file(path),
            other => unreachable!("prefetched {other:?} for create"),
        }?;
        self.last_open = Some(fd);
        Ok(())
    }
    fn open_append_file(&mut self, path: &str, _id: u64) -> Result<(), IoError> {
        let fd = match self.prefetched_path() {
            Some(PathResult::File(fd)) => fd,
            None => self.at()?.open_append_file(path),
            other => unreachable!("prefetched {other:?} for append"),
        }?;
        self.last_open = Some(fd);
        Ok(())
    }
    fn read_file(&mut self, id: u64, offset: u64, max: u64) -> Result<Vec<u8>, IoError> {
        if let Some(concurrent::FileResult::Read(result)) = self.prefetched.take() {
            return result;
        }
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
        if let Some(concurrent::FileResult::Write(result, partial)) = self.prefetched.take() {
            self.partial = partial;
            return result;
        }
        let mut file = self.files.get(&id).ok_or(IoError::Fault(1))?.as_ref();
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
        match self.prefetched_path() {
            Some(PathResult::Bytes(bytes)) => bytes,
            None => self.at()?.read(path),
            other => unreachable!("prefetched {other:?} for read"),
        }
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
        let prefetched = self.prefetched_path();
        if self.local(path)?.is_empty() {
            return Err(IoError::IsDirectory);
        }
        self.prepared.remove(&(self.active_id, path.into()));
        match prefetched {
            Some(PathResult::Unit(result)) => result,
            None => self.at()?.unlink(path, 0),
            other => unreachable!("prefetched {other:?} for remove"),
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
        match self.prefetched_path() {
            Some(PathResult::Unit(result)) => result,
            None => self.at()?.mkdir(path),
            other => unreachable!("prefetched {other:?} for mkdir"),
        }
    }
    fn remove_dir(&mut self, path: &str) -> Result<(), IoError> {
        match self.prefetched_path() {
            Some(PathResult::Unit(result)) => result,
            None => self.at()?.unlink(path, libc::AT_REMOVEDIR),
            other => unreachable!("prefetched {other:?} for remove_dir"),
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
    fn map_file(&mut self, path: &str, id: u64) -> Result<u64, IoError> {
        let fd = self.open(path, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        let meta = fd.metadata().map_err(error)?;
        if !meta.is_file() {
            return Err(IoError::IsDirectory);
        }
        let mapping = crate::mapping::Mapping::new(fd, meta.len())?;
        self.maps.insert(id, mapping);
        Ok(meta.len())
    }
    fn map_view(&mut self, id: u64, length: u64, capture: bool) -> MapView {
        let block = self.maps[&id].block();
        MapView {
            length,
            bytes: capture.then(|| crate::mapping::contents(block, length)),
            block,
        }
    }
    fn map_truncated(&mut self, id: u64, _length: u64) -> bool {
        self.maps[&id].truncated()
    }
    fn unmap(&mut self, id: u64) {
        self.maps.remove(&id);
    }
    fn watch(&mut self, path: &str, id: u64, recursive: bool) -> Result<(), IoError> {
        let target = self.open(path, libc::O_PATH, 0)?;
        let directories = if recursive && target.metadata().map_err(error)?.is_dir() {
            subdirectories(&target, path)?
        } else {
            vec![]
        };
        let watch = crate::watch::HostWatch::new(&target, path, recursive, directories)?;
        self.watches.insert(id, watch);
        Ok(())
    }
    fn watch_next(
        &mut self,
        id: u64,
        root: &str,
    ) -> Result<Option<crate::watch::WatchEvent>, IoError> {
        let mut watch = self.watches.remove(&id).expect("live watcher");
        let disk = &*self;
        let mut open = |path: &str| disk.open(path, libc::O_PATH | libc::O_DIRECTORY, 0).ok();
        let event = loop {
            match watch.next(&mut open) {
                Ok(None) if disk.readiness.is_none() => {
                    if let Err(e) = watch.block() {
                        break Err(e);
                    }
                }
                Ok(None) => {
                    let readiness = disk.readiness.as_ref().unwrap();
                    readiness.pending().interest = Some((watch.descriptor(), libc::POLLIN));
                    break Ok(None);
                }
                other => break other,
            }
        };
        self.watches.insert(id, watch);
        event.map(|event| event.map(|event| crate::watch::relative_to(root, event)))
    }
    fn unwatch(&mut self, id: u64) {
        self.watches.remove(&id);
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
        match self.prefetched_path() {
            Some(PathResult::Metadata(metadata)) => metadata,
            None => self.at()?.metadata(path),
            other => unreachable!("prefetched {other:?} for metadata"),
        }
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
    fn read_dir_page(
        &mut self,
        path: &str,
        after: &str,
        recursive: bool,
    ) -> Result<Vec<DirEntry>, IoError> {
        let fd = self.open(path, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
        let mut entries = hirsel_kernels::walk::PageSelector::default();
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
            entries.offer(
                hirsel_kernels::walk::Entry {
                    path: name,
                    kind: kind as u32,
                },
                after,
                recursive,
            );
        }
        Ok(entries
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
}
/// Every directory below pinned directory `root` (tree path `path`), each pinned, without
/// following links.
fn subdirectories(root: &File, path: &str) -> Result<Vec<(File, String)>, IoError> {
    let mut found = vec![];
    let mut work = vec![(root.try_clone().map_err(error)?, path.to_owned())];
    while let Some((directory, prefix)) = work.pop() {
        let listing = open_component(
            directory.as_raw_fd(),
            ".",
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        )?;
        for entry in
            std::fs::read_dir(format!("/proc/self/fd/{}", listing.as_raw_fd())).map_err(error)?
        {
            let name = entry
                .map_err(error)?
                .file_name()
                .into_string()
                .map_err(|_| IoError::Fault(1))?;
            let pin = open_component(directory.as_raw_fd(), &name, libc::O_PATH, 0)?;
            if pin.metadata().map_err(error)?.is_dir() {
                let child = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                found.push((pin.try_clone().map_err(error)?, child.clone()));
                work.push((pin, child));
            }
        }
    }
    Ok(found)
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

#[cfg(test)]
mod output_tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Written {
        bytes: Vec<u8>,
        flushes: usize,
        interrupted: bool,
    }
    struct Sink {
        written: Rc<RefCell<Written>>,
        limit: usize,
        fail_flush: bool,
    }
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let mut written = self.written.borrow_mut();
            if !written.interrupted {
                written.interrupted = true;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let count = bytes.len().min(2).min(self.limit - written.bytes.len());
            written.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.written.borrow_mut().flushes += 1;
            if self.fail_flush {
                Err(std::io::ErrorKind::BrokenPipe.into())
            } else {
                Ok(())
            }
        }
    }

    // Direct output must preserve the recorded protocol's handle checks, interrupted
    // and partial writes, flush failures and accepted byte prefix, without capture.
    #[test]
    fn borrowed_output_matches_retained_output_without_capturing() {
        for stderr in [false, true] {
            for (limit, fail_flush) in [(usize::MAX, false), (3, false), (usize::MAX, true)] {
                let mut observations = Vec::new();
                for retain in [true, false] {
                    let written = Rc::new(RefCell::new(Written::default()));
                    let sink = Sink {
                        written: written.clone(),
                        limit,
                        fail_flush,
                    };
                    let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
                    if stderr {
                        io.set_stderr(sink);
                    } else {
                        io.streams.output = Output::Local(Box::new(sink));
                    }
                    if !retain {
                        io.discard_history();
                    }
                    let projection = if stderr {
                        Request::Stderr { io: 1 }
                    } else {
                        Request::Stdout { io: 1 }
                    };
                    let Completion::Handle(Ok(handle)) = io.call(projection).unwrap() else {
                        panic!("output grant");
                    };
                    assert!(matches!(
                        io.print(9999, stderr, b"denied"),
                        Err(AdapterError::InvalidHandle(9999))
                    ));
                    assert!(written.borrow().bytes.is_empty());
                    let completion = io.print(handle, stderr, "é😀\n".as_bytes()).unwrap();
                    let written = written.borrow();
                    observations.push((completion, written.bytes.clone(), written.flushes));
                    if retain {
                        let captured = if stderr {
                            &io.state.stderr
                        } else {
                            &io.state.stdout
                        };
                        assert_eq!(*captured, written.bytes);
                        assert_eq!(io.tape.len(), 2);
                    } else {
                        assert!(
                            io.state.stdout.is_empty()
                                && io.state.stderr.is_empty()
                                && io.tape.is_empty()
                        );
                    }
                }
                assert_eq!(observations[0], observations[1]);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod borrowed_net_tests {
    use super::*;
    use crate::net::{NetOperation, NetWorld};
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::mem::MaybeUninit;
    use std::net::TcpListener;

    /// The test binary's allocator: counts this thread's allocation calls, and traps
    /// allocations and frees made while a Free recorder section is open on the thread.
    pub(crate) struct Count;
    thread_local! {
        static CALLS: Cell<u64> = const { Cell::new(0) };
        static WATCHED_FREE: Cell<usize> = const { Cell::new(0) };
        static MATCHING_FREES: Cell<u64> = const { Cell::new(0) };
        static IN_SECTION: Cell<bool> = const { Cell::new(false) };
    }
    static SECTIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static TRAPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub(crate) fn allocation_calls() -> u64 {
        CALLS.with(Cell::get)
    }
    /// Observe destruction of one owned allocation, without counting unrelated frees.
    pub(crate) fn count_deallocation(pointer: usize, drop_owner: impl FnOnce()) -> u64 {
        assert_eq!(WATCHED_FREE.replace(pointer), 0);
        MATCHING_FREES.set(0);
        drop_owner();
        WATCHED_FREE.set(0);
        MATCHING_FREES.get()
    }
    /// A test recorder's section window on this thread (`Metrics::with_section_hook`).
    pub(crate) fn section_hook(open: bool) {
        IN_SECTION.set(open);
        if open {
            SECTIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    /// Sections opened and allocations or frees inside them, process-wide.
    pub(crate) fn section_traps() -> (u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        (SECTIONS.load(Relaxed), TRAPPED.load(Relaxed))
    }
    fn trap() {
        if IN_SECTION.try_with(Cell::get).unwrap_or(false) {
            TRAPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    unsafe impl GlobalAlloc for Count {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = CALLS.try_with(|n| n.set(n.get() + 1));
            trap();
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            if WATCHED_FREE.try_with(Cell::get).unwrap_or(0) == pointer.addr() {
                let _ = MATCHING_FREES.try_with(|n| n.set(n.get() + 1));
            }
            trap();
            unsafe { System.dealloc(pointer, layout) }
        }
    }

    fn connection() -> (RealIo, u64, u64, std::net::TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        io.net(crate::net_host::provider);
        let net = io.state.handles.insert(crate::handles::Grant::Net {
            rules: Default::default(),
            world: NetWorld::Host,
        });
        let stream = io.state.handles.insert(crate::handles::Grant::Stream {
            net,
            address: "127.0.0.1".into(),
            port: address.port(),
            operation: NetOperation::Connect,
            tls: false,
            protocol: None,
        });
        assert_eq!(
            io.streams.net.as_mut().unwrap().connect_addr(
                stream,
                SocketAddr {
                    ip: address.ip(),
                    port: address.port()
                },
                NetTimeout::Unbounded
            ),
            Ok(Ok(()))
        );
        let (peer, _) = listener.accept().unwrap();
        (io, net, stream, peer)
    }

    #[test]
    fn ready_native_read_write_allocates_zero_after_warmup() {
        let (mut io, net, stream, mut peer) = connection();
        io.discard_history();
        let mut binding = Box::new(Binding::new(io));
        let mut ctx = std::ptr::null_mut();
        assert_eq!(
            unsafe { crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx) },
            0
        );
        unsafe { binding.bind(&mut *ctx) };
        let output = unsafe { crate::text::allocate_bytes(ctx, b"x") };
        let input = [42u8; 1024];
        peer.write_all(&input).unwrap();
        let mut read = [0u8; 1];
        let mut result = crate::net_abi::NetWordResult::default();
        let mut stream = stream;
        let before = CALLS.with(Cell::get);
        for _ in 0..1024 {
            assert_eq!(
                unsafe {
                    crate::net_abi::hirsel_rt_v2_net_read_into(
                        ctx,
                        std::ptr::null_mut(),
                        &mut result,
                        &net,
                        &mut stream,
                        read.as_mut_ptr(),
                        1,
                    )
                },
                0
            );
            assert_eq!((result.tag, result.value, read[0]), (0, 1, 42));
            let mut written = crate::net_abi::NetUnitResult::default();
            assert_eq!(
                unsafe {
                    crate::net_abi::hirsel_rt_v2_net_write_all(
                        ctx,
                        std::ptr::null_mut(),
                        &mut written,
                        &net,
                        &mut stream,
                        output.owner,
                        output.start,
                        output.length,
                    )
                },
                0
            );
            assert_eq!(written.tag, 0);
        }
        let allocations = CALLS.with(Cell::get) - before;
        assert_eq!(
            allocations, 0,
            "allocations over 1024 ready read/write pairs"
        );
        let mut received = [0; 1024];
        peer.read_exact(&mut received).unwrap();
        assert_eq!(received, [b'x'; 1024]);
        unsafe {
            (*ctx).bind_io_data(std::ptr::null_mut());
            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
        }
    }

    #[test]
    fn borrowed_network_preserves_tail_and_checks_brands_and_half_direction() {
        let (mut io, net, stream, mut peer) = connection();
        let mut bytes = [MaybeUninit::new(0xa5); 16];
        assert!(
            io.net_io(1, net, stream, false, &mut NetBuffer::Read(&mut bytes))
                .is_none()
        );
        io.discard_history();
        peer.write_all(b"abc").unwrap();
        assert_eq!(
            io.net_io(1, net, stream, false, &mut NetBuffer::Read(&mut bytes)),
            Some(Ok(Ok(3)))
        );
        let initialized = bytes.map(|b| unsafe { b.assume_init() });
        assert_eq!(&initialized[..3], b"abc");
        assert_eq!(&initialized[3..], &[0xa5; 13]);
        assert!(matches!(
            io.net_io(1, 999, stream, false, &mut NetBuffer::Read(&mut bytes)),
            Some(Err(AdapterError::InvalidHandle(999)))
        ));
        let half = io.state.handles.insert(crate::handles::Grant::Half {
            net,
            stream,
            write: true,
        });
        assert!(matches!(
            io.net_io(1, net, half, false, &mut NetBuffer::Read(&mut bytes)),
            Some(Err(AdapterError::InvalidHandle(_)))
        ));
        assert!(io.tape.is_empty());
        assert_eq!(
            io.net_io(1, net, stream, false, &mut NetBuffer::WriteAll(b"response")),
            Some(Ok(Ok(0)))
        );
        let mut response = [0; 8];
        peer.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"response");
    }

    #[test]
    fn a_closed_stream_does_not_reuse_a_cached_live_grant() {
        let (mut io, net, stream, mut peer) = connection();
        io.discard_history();
        peer.write_all(b"ab").unwrap();
        let mut byte = [MaybeUninit::uninit(); 1];
        assert_eq!(
            io.net_io(1, net, stream, false, &mut NetBuffer::Read(&mut byte)),
            Some(Ok(Ok(1)))
        );
        assert!(io.live_streams[0].is_some());
        io.call(Request::Close { handle: stream }).unwrap();
        assert_eq!(io.live_streams, [None; 2]);
        assert!(matches!(
            io.net_io(2, net, stream, false, &mut NetBuffer::Read(&mut byte)),
            Some(Err(AdapterError::InvalidHandle(_)))
        ));
    }

    #[test]
    fn borrowed_network_observes_tls_only_rules_and_invalid_capacity() {
        let (mut io, net, stream, _peer) = connection();
        io.discard_history();
        if let Some(crate::handles::Grant::Stream { operation, .. }) =
            io.state.handles.rows.get_mut(&stream)
        {
            *operation = NetOperation::ConnectTls;
        }
        let mut bytes = [MaybeUninit::uninit(); 8];
        assert_eq!(
            io.net_io(1, net, stream, false, &mut NetBuffer::Read(&mut bytes)),
            Some(Ok(Err(NetError::NotGranted)))
        );
        assert_eq!(
            io.net_io(1, net, stream, false, &mut NetBuffer::Read(&mut [])),
            Some(Ok(Err(NetError::InvalidInput)))
        );
    }
}

impl RealIo {
    /// Convert explicitly transportable streams, retaining the untouched home adapter on refusal.
    #[allow(clippy::result_large_err)]
    pub fn into_concurrent(self) -> Result<ConcurrentIo, Self> {
        if !matches!(self.streams.output, Output::Send(_))
            || !matches!(self.streams.stderr, Output::Send(_))
            || matches!(self.streams.input, Input::Blocking(_))
            || (self.net.is_some() && !self.cooperative_net)
            || self.streams.net.is_some()
            || self.streams.children.is_some()
            || matches!(self.journal.destination(), HomeJournal::Local(_))
        {
            return Err(self);
        }
        let Self {
            file_reads,
            file_faults,
            streams,
            state,
            disk,
            tape,
            fault,
            journal,
            retain_history,
            live_streams,
            net,
            cooperative,
            cooperative_net,
            test_clock,
            children,
            delivered,
            signals,
            signal_waits,
            current,
            signal_delivered,
            staged,
            registry,
        } = self;
        let Output::Send(output) = streams.output else {
            unreachable!()
        };
        let Output::Send(stderr) = streams.stderr else {
            unreachable!()
        };
        let input = match streams.input {
            Input::Send(input, storage) => SendInput::Blocking(input, storage),
            Input::Cooperative(input) => SendInput::Cooperative(input),
            Input::Blocking(_) => unreachable!(),
        };
        let journal = journal.map_destination(|journal| match journal {
            HomeJournal::Send(journal) => journal,
            HomeJournal::Local(_) => unreachable!(),
        });
        let core = RealCore {
            file_reads,
            file_faults,
            streams: SendStreams {
                output,
                input,
                stderr,
            },
            state,
            disk,
            tape,
            fault,
            journal,
            retain_history,
            live_streams,
            net,
            cooperative,
            cooperative_net,
            test_clock,
            children,
            delivered,
            signals,
            signal_waits,
            current,
            signal_delivered,
            staged,
            registry,
        };
        Ok(ConcurrentIo::new(core))
    }
}
