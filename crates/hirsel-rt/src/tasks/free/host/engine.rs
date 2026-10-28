//! Engine-facing Free host recording and replay. The typed codec, authority mapping and
//! footprint allow-list are the native executor's own implementations. Engines
//! own phase scheduling through `hirsel_free::session::Session`.
use super::*;
use hirsel_free::session::Session;

pub use crate::trace_events::free::canonical_events;

/// Typed host state for one production recording or strict replay.
pub struct Codec {
    handles: handles::Handles,
    authorities: requests::Authorities,
    initial_env: bool,
    namespace_coupled: bool,
    spans: native::SpanState,
    live: Option<Live>,
}

#[derive(Default)]
struct Live {
    words: std::collections::BTreeMap<u64, Created>,
    ordinals: std::collections::BTreeMap<Vec<u32>, u64>,
    ready: std::collections::BTreeSet<u64>,
    waiting: std::collections::BTreeSet<u64>,
    ready_all: bool,
}

fn error(stop: Stop) -> String {
    match stop {
        Stop::Diverged(message) | Stop::Refused(message) => message,
        stop => format!("Free host replay: {stop:?}"),
    }
}

impl Codec {
    /// Restore the public terminal only from a strictly finished phase session.
    /// A truncated tape has no terminal to restore. Root Done, when reached,
    /// must still pass check_outcome against the evaluator's actual result.
    pub fn finished_outcome(
        projection: &hirsel_free::projection::Projection,
    ) -> Result<Option<hirsel_run_schema::RunOutcome>, String> {
        if projection.outcome == "truncated" {
            return Ok(None);
        }
        use crate::free_launch::Terminal;
        Ok(Some(
            match crate::free_launch::terminal(&projection.outcome)? {
                Terminal::Stop(Stop::Failed) => {
                    return Err("internal Free failure has no canonical failure outcome".into());
                }
                Terminal::Stop(stop) => stop.outcome(),
                Terminal::Outcome(outcome) | Terminal::Failed(outcome) => outcome,
            },
        ))
    }

    /// Admit a bound Free tape using production header, terminal and receipt
    /// validation. A completion-only replay has no phase session.
    pub fn open(io: &crate::io_sim::SimIo) -> Result<Option<(Self, Session)>, String> {
        let Some(bytes) = &io.free_tape else {
            return Ok(None);
        };
        crate::free_launch::Session::replay(bytes, false)?;
        let tape = hirsel_free::tape::parse(bytes)?;
        let (header, namespace_coupled) = crate::free_launch::binding(&tape.binding)?;
        let initial_env = header
            .bound_run()
            .map_err(|e| e.to_string())?
            .record()
            .env
            .is_some();
        let authorities = requests::Authorities::default();
        authorities.root(initial_env);
        Ok(Some((
            Self {
                handles: Default::default(),
                authorities,
                initial_env,
                namespace_coupled,
                spans: Default::default(),
                live: None,
            },
            Session::replay(bytes).map_err(|e| e.to_string())?,
        )))
    }

    /// Admit a source recording with the native header, destination protection,
    /// file sink and durable barriers. The home adapter retains its providers.
    pub fn record(
        io: &mut crate::io_real::RealIo,
        admitted: &hirsel_run_config::AdmittedRun,
        path: Option<&std::path::Path>,
    ) -> Result<Option<(Self, Session, Option<Report>)>, String> {
        use hirsel_run_schema::RecordingLevel;
        let level = crate::host_launch::recording_level(admitted.run(), path.is_some())?;
        if level == RecordingLevel::None {
            return Ok(None);
        }
        let path = path.expect("validated recording destination");
        let (file, header, path) = crate::host_launch::header(io, admitted, path, &[])?;
        let report = Report::open(&io.mount_pins(), &[("recording", &file)])?;
        let durable = level == RecordingLevel::Durable;
        let namespace_coupled = io.namespace_coupled();
        let mut binding = b"F1B4".to_vec();
        crate::ABI_VERSION.encode(&mut binding);
        header.encode(&mut binding);
        namespace_coupled.encode(&mut binding);
        let sink = super::super::record::Sink::file(file, durable);
        let session = Session::record_to(
            &binding,
            hirsel_free::session::Recording {
                durable,
                ..Default::default()
            },
            Box::new(sink),
        )
        .map_err(|e| format!("Free recording: {e}"))?;
        if durable {
            std::fs::File::open(path.parent().unwrap_or(std::path::Path::new("/")))
                .and_then(|directory| directory.sync_all())
                .map_err(|e| e.to_string())?;
        }
        let initial_env = admitted.run().record().env.is_some();
        let authorities = requests::Authorities::default();
        authorities.root(initial_env);
        io.discard_history();
        Ok(Some((
            Self {
                handles: Default::default(),
                authorities,
                initial_env,
                namespace_coupled,
                spans: Default::default(),
                live: Some(Live::default()),
            },
            session,
            report,
        )))
    }

    /// Derived from checked begin observations, with the native owner checks.
    pub fn span_context(&self, telemetry: u64, span: u64) -> Option<Vec<u8>> {
        self.spans.context(telemetry, span)
    }
    /// Exact native key footprint, including bound namespace and Net domains.
    pub fn keys(&self, request: &Request) -> Result<Vec<(String, bool)>, String> {
        if let Request::TelemetryEvent {
            telemetry, span, ..
        }
        | Request::TelemetryFinish {
            telemetry, span, ..
        } = request
            && !self.spans.valid(*telemetry, *span)
        {
            return Err("unknown native telemetry span owner".into());
        }
        let keys = native::request_keys_with_domains(
            request,
            |word| match &self.live {
                Some(live) => live
                    .words
                    .get(&word)
                    .map(|created| (crate::io_keys::Key(created.resource), created.kind))
                    .ok_or_else(|| Stop::Refused("unmapped live host resource".into())),
                None => self.handles.identity(word),
            },
            |word| match &self.live {
                Some(live) => live
                    .words
                    .get(&word)
                    .and_then(|created| created.process_keys.as_ref())
                    .map(|keys| {
                        KeySet::new(
                            keys.iter()
                                .map(|&key| (crate::io_keys::Key(key), Mode::Exclusive))
                                .collect(),
                        )
                    })
                    .ok_or_else(|| Stop::Refused("missing process issue domains".into())),
                None => self.handles.process_keys(word),
            },
            |word| match &self.live {
                Some(live) => live
                    .words
                    .get(&word)
                    .and_then(|created| created.net_ports)
                    .map(crate::io_keys::Key)
                    .ok_or_else(|| Stop::Refused("missing Net port domain".into())),
                None => self.handles.net_ports(word),
            },
            self.initial_env,
        )
        .map_err(error)?;
        Ok(native::namespace_policy(keys, self.namespace_coupled)
            .keys()
            .iter()
            .map(|(key, mode)| (format!("host-resource/{}", key.0), *mode == Mode::Shared))
            .collect())
    }

    /// Canonical request bytes bind every authority and input before claim.
    pub fn request(&self, request: &Request) -> Result<Vec<u8>, String> {
        self.authorities.project(request).map_err(error)
    }

    /// Validate a bounded transition, substitute creator-qualified handles and
    /// return its completion plus the exact externally written prefix length.
    pub fn completion(
        &mut self,
        path: &[u32],
        op: u64,
        request: &Request,
        bytes: &[u8],
    ) -> Result<(Result<Completion, AdapterError>, Option<u64>), String> {
        let envelope: Envelope = crate::tape_wire::decode(bytes)?;
        let count =
            checked_output(request, &envelope.result, envelope.output_count).map_err(error)?;
        let result = restore(
            path,
            op,
            request,
            envelope,
            &mut self.handles,
            &self.authorities,
        )
        .map_err(error)?;
        Ok((result, count))
    }

    /// Distribute the home driver's single readiness snapshot to pending owners.
    pub fn observe_ready(&mut self, ready: &crate::io::Ready) {
        let Some(live) = &mut self.live else {
            return;
        };
        match ready {
            crate::io::Ready::Every => live.ready_all = true,
            crate::io::Ready::Tokens(tokens) => live.ready.extend(
                tokens
                    .iter()
                    .filter(|token| live.waiting.contains(token))
                    .copied(),
            ),
        }
    }

    pub fn has_ready(&self) -> bool {
        self.live
            .as_ref()
            .is_some_and(|live| live.ready_all || !live.ready.is_empty())
    }

    fn record_completion(
        &mut self,
        io: &crate::io_real::RealIo,
        path: &[u32],
        op: u64,
        request: &Request,
        result: &mut Result<Completion, AdapterError>,
        output_count: &mut Option<u64>,
    ) -> Result<Vec<u8>, String> {
        use crate::io::Adapter;
        let resources = super::discover(
            result,
            |word| io.identity(word),
            |word| io.free_process_keys(word),
            |word| io.free_net_ports(word),
        )
        .map_err(error)?;
        let live = self
            .live
            .as_mut()
            .ok_or("host result requires live codec")?;
        let mut words_result = result.clone();
        let words = if handles::refusal_word(result).is_some() {
            Vec::new()
        } else {
            handles::fields(&mut words_result)
                .map_err(str::to_string)?
                .into_iter()
                .flatten()
                .map(|word| *word)
                .collect::<Vec<_>>()
        };
        let created = words
            .into_iter()
            .zip(resources)
            .map(|(word, (resource, kind, process_keys, net_ports))| {
                let ordinal = live.ordinals.entry(path.to_vec()).or_default();
                *ordinal = ordinal.checked_add(1).expect("host handle ordinal");
                let created = Created {
                    path: path.to_vec(),
                    ordinal: *ordinal,
                    resource: resource.0,
                    kind,
                    process_keys: process_keys
                        .map(|keys| keys.keys().iter().map(|(key, _)| key.0).collect()),
                    net_ports: net_ports.map(|key| key.0),
                };
                live.words.insert(word, created.clone());
                created
            })
            .collect::<Vec<_>>();
        let bytes = handles::encode_result(
            request,
            result,
            &created,
            output_count,
            output_bytes(request).is_some(),
            &self.authorities,
        )
        .map_err(error)?;
        self.authorities.observe(path, op, result).map_err(error)?;
        self.spans.observe(request, result).map_err(error)?;
        Ok(bytes)
    }

    /// Persist the public root outcome in Done before closing the native terminal.
    pub fn finish_recording(
        session: Session,
        outcome: &hirsel_run_schema::RunOutcome,
        root_pending: bool,
    ) -> Result<hirsel_free::projection::Projection, String> {
        let mut session = session;
        let progress = session.progress(&[]);
        if root_pending {
            if progress.phase != Phase::Entered {
                return Err("Free root end is not Entered".into());
            }
            let mut bytes = Vec::new();
            outcome.encode(&mut bytes);
            session
                .record_host_observation(&[], &bytes)
                .map_err(|e| e.to_string())?;
            session
                .publish(&[], Phase::Done)
                .map_err(|e| e.to_string())?;
        }
        if matches!(outcome, hirsel_run_schema::RunOutcome::Returned(_)) && !root_pending {
            return Err("Free returned outcome has no root end".into());
        }
        let kind =
            if matches!(outcome, hirsel_run_schema::RunOutcome::Failed { .. }) && !root_pending {
                "native-failure-v2"
            } else {
                "native-outcome-v2"
            };
        session
            .finish_recording(|sweep| crate::free_launch::terminal_bytes(kind, outcome, sweep))
            .map_err(|e| e.to_string())
    }

    /// Root Done carries the canonical public outcome, never an engine-specific
    /// return representation. Its bytes are checked before session finish.
    pub fn check_outcome(
        expected: &[u8],
        actual: &hirsel_run_schema::RunOutcome,
    ) -> Result<(), String> {
        let bound: hirsel_run_schema::RunOutcome = crate::tape_wire::decode(expected)?;
        if bound != *actual {
            return Err(format!(
                "Free root outcome differs from recording: expected {bound:?}, actual {actual:?}"
            ));
        }
        Ok(())
    }
}

pub(in crate::tasks::free) fn restore(
    path: &[u32],
    op: u64,
    request: &Request,
    mut envelope: Envelope,
    handles: &mut handles::Handles,
    authorities: &requests::Authorities,
) -> Stopped<Result<Completion, AdapterError>> {
    if handles::restore_refusal(&mut envelope, authorities)? {
        return Ok(envelope.result);
    }
    if envelope.created.iter().any(|created| created.path != path) {
        return Err(Stop::Diverged(
            "host creation belongs to a different task".into(),
        ));
    }
    let result = handles.substitute_for(request, envelope)?;
    authorities.observe(path, op, &result)?;
    Ok(result)
}

/// An owned host request paused at a key predecessor or a terminal cut.
pub struct Call {
    request: Request,
    keys: Vec<(String, bool)>,
    positions: Vec<hirsel_free::recorder::Position>,
    next: Phase,
    attempt: u32,
    observed: Option<(Result<Completion, AdapterError>, Option<u64>)>,
    live: Option<hirsel_free::session::HostWrite>,
    encoded: Vec<u8>,
    waiting: bool,
}

/// One bounded advance never waits on a host resource.
pub enum Poll {
    Pending,
    Cut,
    Complete(Result<Completion, AdapterError>),
}

impl Call {
    /// The engine has begun this operation and applied its pending roots.
    pub fn new(codec: &Codec, request: Request) -> Result<Self, String> {
        Ok(Self {
            keys: codec.keys(&request)?,
            request,
            positions: Vec::new(),
            next: Phase::Entered,
            attempt: 0,
            observed: None,
            live: None,
            encoded: Vec::new(),
            waiting: false,
        })
    }

    pub fn request(&self) -> &Request {
        &self.request
    }

    /// Drive the native request ownership and phase order with the home adapter.
    /// Pending keeps all buffers in Call. No adapter loan crosses a saved wait.
    pub fn advance_live(
        &mut self,
        codec: &mut Codec,
        session: &mut Session,
        path: &[u32],
        cancellation: Option<Option<u32>>,
        io: &mut crate::io_real::RealIo,
    ) -> Result<Poll, String> {
        use crate::io::Adapter;
        use hirsel_free::session::Claim;
        loop {
            if !session.admits(path, self.next) {
                return Ok(Poll::Cut);
            }
            match self.next {
                Phase::Entered | Phase::Attempt(_) => {
                    let expected = self
                        .keys
                        .iter()
                        .map(|(key, shared)| {
                            let key = key
                                .strip_prefix("host-resource/")
                                .ok_or("host key prefix")?
                                .parse::<u32>()
                                .map_err(|e| e.to_string())?;
                            Ok((
                                crate::io_keys::Key(key),
                                if *shared {
                                    Mode::Shared
                                } else {
                                    Mode::Exclusive
                                },
                            ))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    super::concurrent::check_keys(
                        KeySet::new(expected),
                        io.keys(&self.request),
                        codec.namespace_coupled,
                        io.namespace_coupled(),
                    )
                    .map_err(error)?;
                    let keys = self
                        .keys
                        .iter()
                        .map(|(key, shared)| (key.as_str(), *shared))
                        .collect::<Vec<_>>();
                    if self.attempt == 0 {
                        self.live = Some(
                            session
                                .record_host_prepare(path, &keys)
                                .map_err(|e| e.to_string())?,
                        );
                    } else {
                        let token = Session::record_host_id(self.live.as_ref().unwrap());
                        if self.waiting && cancellation.flatten().is_none() {
                            let live = codec.live.as_mut().unwrap();
                            if !live.ready.remove(&token) && !live.ready_all {
                                return Ok(Poll::Pending);
                            }
                        }
                        codec.live.as_mut().unwrap().waiting.remove(&token);
                        self.waiting = false;
                        session
                            .record_host_positions(
                                path,
                                self.live.as_mut().unwrap(),
                                self.attempt,
                                &keys,
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    if crate::io_checks::cancellation_result(&self.request, 0).is_some()
                        && let Some(live) = cancellation
                    {
                        session
                            .cancel_check(path, live)
                            .map_err(|e| e.to_string())?;
                    }
                    if self.attempt == 0 {
                        session
                            .record_host_observation(path, &codec.request(&self.request)?)
                            .map_err(|e| e.to_string())?;
                        session
                            .record_host_entered(path)
                            .map_err(|e| e.to_string())?;
                        self.next = Phase::Claimed;
                    } else {
                        session
                            .publish(path, self.next)
                            .map_err(|e| e.to_string())?;
                        self.next = Phase::Ended(self.attempt);
                    }
                }
                Phase::Claimed => {
                    match session
                        .host_claim(path, self.live.as_mut().unwrap())
                        .map_err(|e| e.to_string())?
                    {
                        Claim::Waiting => return Ok(Poll::Pending),
                        Claim::Final => return Ok(Poll::Cut),
                        Claim::Call => self.next = Phase::Ended(0),
                    }
                }
                Phase::Ended(index) => {
                    let request = self.live.as_ref().unwrap();
                    if !session.record_host_ready(request) {
                        return Ok(Poll::Pending);
                    }
                    let token = Session::record_host_id(request);
                    io.enter(token);
                    let cause = cancellation.flatten().filter(|_| io.cancellable(token));
                    let canceled = match cause {
                        Some(cause) => crate::io_checks::cancel(io, &self.request, cause),
                        None => Ok(None),
                    };
                    let (mut result, mut count) = match canceled {
                        Ok(Some(result)) => (Ok(result), None),
                        Err(error) => (Err(error), None),
                        Ok(None) => match &self.request {
                            Request::Print { stdout, bytes } => {
                                io.print_observed(*stdout, false, bytes)
                            }
                            Request::PrintStderr { stderr, bytes } => {
                                io.print_observed(*stderr, true, bytes)
                            }
                            _ => (io.call(self.request.clone()), None),
                        },
                    };
                    self.encoded = codec.record_completion(
                        io,
                        path,
                        session.op(path),
                        &self.request,
                        &mut result,
                        &mut count,
                    )?;
                    let output = match count {
                        Some(count) => Some((
                            self.keys
                                .iter()
                                .find(|(_, shared)| !*shared)
                                .ok_or("output has no sink")?
                                .0
                                .as_str(),
                            &output_bytes(&self.request).unwrap()[..count as usize],
                        )),
                        None => None,
                    };
                    let blocked = matches!(result, Err(AdapterError::Blocked));
                    session
                        .record_host_ended(path, request, index, &self.encoded, output, blocked)
                        .map_err(|e| e.to_string())?;
                    self.observed = Some((result, count));
                    if blocked {
                        io.parked(token);
                        codec.live.as_mut().unwrap().waiting.insert(token);
                        self.attempt = index.checked_add(1).ok_or("host attempt overflow")?;
                        self.next = Phase::Attempt(self.attempt);
                        self.waiting = true;
                        return Ok(Poll::Pending);
                    }
                    io.finish(token);
                    self.next = Phase::Done;
                }
                Phase::Done => {
                    session
                        .record_host_complete(path, self.live.as_ref().unwrap(), &self.encoded)
                        .map_err(|e| e.to_string())?;
                    return Ok(Poll::Complete(self.observed.take().unwrap().0));
                }
                _ => return Err("invalid live host phase".into()),
            }
        }
    }

    /// Follow the same phases as the native executor. The caller retries
    /// Pending after another task publishes; Cut is a permanent saved wait.
    pub fn advance(
        &mut self,
        codec: &mut Codec,
        session: &mut Session,
        path: &[u32],
        cancellation: Option<Option<u32>>,
    ) -> Result<Poll, String> {
        loop {
            if !session.admits(path, self.next) {
                return Ok(Poll::Cut);
            }
            match self.next {
                Phase::Entered | Phase::Attempt(_) => {
                    if self.attempt != 0 {
                        session
                            .replay_host_readiness(path, true)
                            .map_err(|e| e.to_string())?;
                    }
                    let keys: Vec<_> = self
                        .keys
                        .iter()
                        .map(|(key, shared)| (key.as_str(), *shared))
                        .collect();
                    self.positions = session
                        .replay_host_positions(path, self.attempt, &keys)
                        .map_err(|e| e.to_string())?;
                    if crate::io_checks::cancellation_result(&self.request, 0).is_some()
                        && let Some(live) = cancellation
                    {
                        session
                            .cancel_check(path, live)
                            .map_err(|e| e.to_string())?;
                    }
                    if self.attempt == 0 {
                        session
                            .replay_host_request(path, &codec.request(&self.request)?)
                            .map_err(|e| e.to_string())?;
                    }
                    session
                        .publish(path, self.next)
                        .map_err(|e| e.to_string())?;
                    self.next = if self.attempt == 0 {
                        Phase::Claimed
                    } else {
                        Phase::Ended(self.attempt)
                    };
                }
                Phase::Claimed => {
                    if !session.replay_host_ready(&self.positions) {
                        return Ok(Poll::Pending);
                    }
                    session
                        .publish(path, Phase::Claimed)
                        .map_err(|e| e.to_string())?;
                    self.next = Phase::Ended(0);
                }
                Phase::Ended(index) => {
                    if !session.replay_host_ready(&self.positions) {
                        return Ok(Poll::Pending);
                    }
                    let bytes = session
                        .replay_host_observation(path, &self.positions)
                        .map_err(|e| e.to_string())?;
                    let observed =
                        codec.completion(path, session.op(path), &self.request, &bytes)?;
                    if let Some(count) = observed.1 {
                        let (sink, _) = self
                            .keys
                            .iter()
                            .find(|(_, shared)| !*shared)
                            .ok_or("output has no exclusive sink")?;
                        session
                            .replay_host_output(
                                path,
                                &self.positions,
                                sink,
                                &output_bytes(&self.request).unwrap()[..count as usize],
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    codec
                        .spans
                        .observe(&self.request, &observed.0)
                        .map_err(error)?;
                    let blocked = matches!(observed.0, Err(AdapterError::Blocked));
                    if blocked {
                        session
                            .replay_host_readiness(path, false)
                            .map_err(|e| e.to_string())?;
                    }
                    self.observed = Some(observed);
                    session
                        .publish(path, Phase::Ended(index))
                        .map_err(|e| e.to_string())?;
                    if blocked {
                        self.attempt = index.checked_add(1).ok_or("host attempt overflow")?;
                        self.next = Phase::Attempt(self.attempt);
                    } else {
                        self.next = Phase::Done;
                    }
                }
                Phase::Done => {
                    let bytes = session
                        .replay_host_completion(path)
                        .map_err(|e| e.to_string())?;
                    let done = codec.completion(path, session.op(path), &self.request, &bytes)?;
                    if self.observed.as_ref() != Some(&done) {
                        return Err("host completion differs from final observation".into());
                    }
                    session
                        .publish(path, Phase::Done)
                        .map_err(|e| e.to_string())?;
                    return Ok(Poll::Complete(done.0));
                }
                _ => return Err("invalid host replay phase".into()),
            }
        }
    }
}

/// Optional host-test diagnostic, alongside HIRSEL_RT_OBSERVATION. This channel
/// writes only a normalized projection: it never enables admission or changes
/// recording contents, scheduling inputs or replay decisions.
pub struct Report(std::fs::File);
impl Report {
    pub const VAR: &'static str = "HIRSEL_RT_FREE_PROJECTION";

    pub fn check_input(path: &std::path::Path) -> Result<(), String> {
        if let Some(report) = std::env::var_os(Self::VAR) {
            let output = crate::host_paths::RecorderDestination::pin(std::path::Path::new(&report))
                .map_err(|e| e.to_string())?;
            let input =
                crate::host_paths::RecorderDestination::pin(path).map_err(|e| e.to_string())?;
            output.check_distinct(&input).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn open(
        grants: &[(&str, &crate::host_paths::PinnedDirectory)],
        outputs: &[(&str, &std::fs::File)],
    ) -> Result<Option<Self>, String> {
        let Some(path) = std::env::var_os(Self::VAR) else {
            return Ok(None);
        };
        let destination = crate::host_paths::RecorderDestination::pin(std::path::Path::new(&path))
            .map_err(|e| e.to_string())?;
        destination
            .open_with_files(grants, outputs)
            .map(Self)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    pub fn write(
        &mut self,
        projection: &hirsel_free::projection::Projection,
    ) -> Result<(), String> {
        use std::io::Write;
        writeln!(self.0, "{projection:?}")
            .and_then(|()| self.0.flush())
            .map_err(|e| format!("Free projection report: {e}"))
    }
}

/// Inspect only runner links in a production Free tape. This does not execute
/// phases or authorize continuation; joined checks compare canonical data.
pub fn linked_recording(bytes: &[u8]) -> Result<crate::launch::Recording, String> {
    use super::requests::Authority;
    use hirsel_free::record::Record;
    let mut header = crate::free_launch::header(bytes)?;
    let _validated = crate::free_launch::Session::replay(bytes, false)?;
    let tape = hirsel_free::tape::parse(bytes)?;
    let mut events = Vec::new();
    for (path, log) in &tape.logs {
        let cut = tape.cut.get(path).ok_or("host log without a cut")?;
        let mut requests = std::collections::BTreeMap::new();
        let mut prepared = std::collections::BTreeSet::new();
        let mut entered = std::collections::BTreeSet::new();
        let mut completed = std::collections::BTreeSet::new();
        for record in hirsel_free::record::decode(log)? {
            match record {
                Record::Prepared { op, .. } => {
                    if !cut.admits(hirsel_free::phase::Progress::new(
                        op,
                        hirsel_free::phase::Phase::Entered,
                    )) {
                        return Err("host request lies beyond its cut".into());
                    }
                    if !entered.insert(op) {
                        return Err("duplicate prepared host request".into());
                    }
                    prepared.insert(op);
                }
                Record::Observation { op, bytes } if prepared.remove(&op) => {
                    let (payload, authorities): (Vec<u8>, Vec<Authority>) =
                        crate::tape_wire::decode(&bytes)?;
                    let request: Request = crate::tape_wire::decode(&payload)?;
                    if matches!(
                        request,
                        Request::RunnerCall(_)
                            | Request::RunnerPoll { .. }
                            | Request::CallReturn(_)
                    ) {
                        if !authorities.is_empty() {
                            return Err("runner request has physical authorities".into());
                        }
                        requests.insert(op, request);
                    }
                }
                Record::Completion { op, bytes } => {
                    if !cut.admits(hirsel_free::phase::Progress::new(
                        op,
                        hirsel_free::phase::Phase::Done,
                    )) {
                        return Err("host completion lies beyond its cut".into());
                    }
                    if !entered.contains(&op) || prepared.contains(&op) || !completed.insert(op) {
                        return Err(
                            "host completion without one prepared request observation".into()
                        );
                    }
                    if let Some(request) = requests.remove(&op) {
                        let envelope: Envelope = crate::tape_wire::decode(&bytes)?;
                        envelope.validate_request(&request).map_err(error)?;
                        if envelope.refusal.is_some()
                            || (matches!(request, Request::CallReturn(_))
                                && (!envelope.created.is_empty()
                                    || envelope.output_count.is_some()
                                    || envelope.result != Ok(Completion::Ready)))
                        {
                            return Err("invalid runner link completion metadata".into());
                        }
                        let completion = envelope
                            .result
                            .map_err(|e| format!("runner link completion: {e:?}"))?;
                        events.push(crate::io::Event {
                            request,
                            completion,
                            fault: None,
                            resources: None,
                        });
                    }
                }
                _ => {}
            }
        }
        // A partial operation still establishes its issued request, but no
        // callee outcome can be claimed until its completion was recorded.
        for (_, request) in requests {
            if matches!(request, Request::CallReturn(_)) {
                continue;
            }
            events.push(crate::io::Event {
                request,
                completion: Completion::Ready,
                fault: None,
                resources: None,
            });
        }
    }
    header.events = events;
    Ok(header)
}

#[cfg(test)]
#[path = "linked_tests.rs"]
mod linked_tests;
