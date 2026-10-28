//! Host effects for interpreted launches. The runtime owns CAP-1/2 policy;
//! the evaluator continues to execute only explicit operation results.
use hirsel_interp::library::{Library, SimulationConfig, error, ok, u64_value};
use hirsel_interp::{
    Error, Integer, OperationCall, OperationContext, OperationResult, Operations, Value,
};
use hirsel_rt::process::{PipelineOutcome, ProcessFailure, ProcessOutcome};
use hirsel_rt::{
    io::{Adapter, Completion as C, Ready, Request as R},
    io_real::RealIo,
    io_sim::SimIo,
    net::*,
};
use hirsel_syntax::DefPath;
use hirsel_types::{BuiltinType, Primitive};
use std::collections::BTreeMap;
mod free;
mod remote;
mod runners;
mod walk;
pub(crate) use remote::{CancelHandle, ExternalCancel, RemoteRunners, wire as remote_wire};
pub(crate) use runners::RunnerContext;

fn invalid() -> Error {
    Error::InvalidTir("host network operation schema".into())
}
fn word(v: &Value) -> Result<u64, Error> {
    if let Value::Integer(n) = v {
        u64::try_from(n.bits()).map_err(|_| invalid())
    } else {
        Err(invalid())
    }
}
fn fields(v: &Value) -> Result<Vec<&Value>, Error> {
    let f: &[Option<Value>] = match v {
        Value::Record { fields, .. } | Value::Variant { fields, .. } | Value::Tuple(fields) => {
            fields
        }
        Value::Sequence { elements, .. } => elements,
        _ => return Err(invalid()),
    };
    f.iter().map(|v| v.as_ref().ok_or_else(invalid)).collect()
}
fn bytes(v: &Value) -> Result<Vec<u8>, Error> {
    match v {
        Value::Bytes(b) => Ok(b.clone()),
        Value::String(s) => Ok(s.as_bytes().to_vec()),
        _ => fields(v)?
            .into_iter()
            .map(|v| u8::try_from(word(v)?).map_err(|_| invalid()))
            .collect(),
    }
}
fn text(v: &Value) -> Result<String, Error> {
    String::from_utf8(bytes(v)?).map_err(|_| invalid())
}
fn tag(v: &Value) -> Result<u32, Error> {
    if let Value::Variant { variant, .. } = v {
        Ok(*variant)
    } else {
        Err(invalid())
    }
}
fn variant(n: u32, f: Vec<Value>) -> Value {
    Value::Variant {
        variant: n,
        fields: f.into_iter().map(Some).collect(),
    }
}
fn int(p: Primitive, n: impl Into<u128>) -> Value {
    Value::Integer(Integer::new(p, n.into()).unwrap())
}
fn ip(v: &Value) -> Result<std::net::IpAddr, Error> {
    let f = fields(v)?;
    let b = bytes(f[0])?;
    match tag(v)? {
        0 => Ok(std::net::Ipv4Addr::from(<[u8; 4]>::try_from(b).map_err(|_| invalid())?).into()),
        1 => Ok(std::net::Ipv6Addr::from(<[u8; 16]>::try_from(b).map_err(|_| invalid())?).into()),
        _ => Err(invalid()),
    }
}
fn addr(v: &Value) -> Result<SocketAddr, Error> {
    let f = fields(v)?;
    Ok(SocketAddr {
        ip: ip(f[0])?,
        port: u16::try_from(word(f[1])?).map_err(|_| invalid())?,
    })
}
fn timeout(v: &Value) -> Result<NetTimeout, Error> {
    match tag(v)? {
        0 => Ok(NetTimeout::Unbounded),
        1 => Ok(NetTimeout::Nanos(word(fields(v)?[0])?)),
        _ => Err(invalid()),
    }
}
fn sequence(kind: BuiltinType, items: impl IntoIterator<Item = Value>) -> Value {
    Value::Sequence {
        kind,
        elements: items.into_iter().map(Some).collect(),
        arena: None,
    }
}
fn address(a: SocketAddr) -> Value {
    let (t, b) = match a.ip {
        std::net::IpAddr::V4(ip) => (0, ip.octets().to_vec()),
        std::net::IpAddr::V6(ip) => (1, ip.octets().to_vec()),
    };
    Value::Record {
        definition: (&DefPath::core("SocketAddr")).into(),
        fields: vec![
            Some(variant(
                t,
                vec![sequence(
                    BuiltinType::Array,
                    b.into_iter().map(|b| int(Primitive::U8, b)),
                )],
            )),
            Some(int(Primitive::U16, a.port)),
        ],
    }
}
fn network_name(name: &str) -> &str {
    match name {
        "read_half.read" => "net.read",
        "read_half.read_into" => "net.read_into",
        "read_half.set_timeout" => "net.set_read_timeout",
        "write_half.write" => "net.write",
        "write_half.write_all" => "net.write_all",
        "write_half.set_timeout" => "net.set_write_timeout",
        "read_half.close" | "write_half.close" => "net.close",
        name => name,
    }
}
fn net_error(e: NetError) -> Value {
    use NetError::*;
    let (t, f) = match e {
        NotGranted => (0, vec![]),
        Canceled(c) => (1, vec![variant(c, vec![])]),
        InvalidInput => (2, vec![]),
        UnknownHost => (3, vec![]),
        ResolverFailure => (4, vec![]),
        ConnectionRefused => (5, vec![]),
        ConnectionReset => (6, vec![]),
        ConnectionAborted => (7, vec![]),
        NotConnected => (8, vec![]),
        AddrInUse => (9, vec![]),
        AddrNotAvailable => (10, vec![]),
        HostUnreachable => (11, vec![]),
        NetworkUnreachable => (12, vec![]),
        NetworkDown => (13, vec![]),
        BrokenPipe => (14, vec![]),
        TimedOut => (15, vec![]),
        Fault(c) => (17, vec![hirsel_interp::library::u32_value(c)]),
        Tls(e) => {
            let (t, f) = match e {
                TlsFailure::UnknownIssuer => (0, vec![]),
                TlsFailure::Expired => (1, vec![]),
                TlsFailure::NotYetValid => (2, vec![]),
                TlsFailure::NameMismatch => (3, vec![]),
                TlsFailure::Revoked => (4, vec![]),
                TlsFailure::BadCertificate => (5, vec![]),
                TlsFailure::AlertReceived(c) => (6, vec![int(Primitive::U8, c)]),
                TlsFailure::HandshakeFailure => (7, vec![]),
                TlsFailure::PeerMisbehaved => (8, vec![]),
                TlsFailure::Truncated => (9, vec![]),
                TlsFailure::CertificateRequired => (10, vec![]),
            };
            (16, vec![variant(t, f)])
        }
    };
    error(variant(t, f))
}
fn file_error(e: hirsel_rt::io::IoError) -> Value {
    use hirsel_rt::io::IoError;
    error(match e {
        IoError::Fault(c) => variant(0, vec![hirsel_interp::library::u32_value(c)]),
        IoError::Canceled(c) => variant(1, vec![variant(c, vec![])]),
        IoError::NotGranted => variant(2, vec![]),
        IoError::NotFound => variant(3, vec![]),
        IoError::PermissionDenied => variant(4, vec![]),
        IoError::AlreadyExists => variant(5, vec![]),
        IoError::IsDirectory => variant(6, vec![]),
        IoError::NotDirectory => variant(7, vec![]),
        IoError::DirectoryNotEmpty => variant(8, vec![]),
        IoError::CrossDevice => variant(9, vec![]),
        IoError::Truncated => variant(10, vec![]),
    })
}
fn clock_error(e: hirsel_rt::io::ClockError) -> Value {
    error(variant(
        match e {
            hirsel_rt::io::ClockError::Unavailable => 0,
            hirsel_rt::io::ClockError::OutOfRange => 1,
        },
        vec![],
    ))
}
fn result<T>(r: Result<T, NetError>, f: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => ok(f(v)),
        Err(e) => net_error(e),
    }
}

enum TlsSetup {
    Client {
        stream: u64,
        roots: Option<Vec<u8>>,
        identity: Option<(Vec<u8>, Vec<u8>)>,
    },
    Server {
        stream: u64,
        identities: Vec<(Vec<u8>, Vec<u8>)>,
        clients: ClientAuth<Vec<u8>>,
        alpn: Vec<String>,
    },
}
impl TlsSetup {
    fn install(self, io: &mut HostIo) {
        io.tls();
        match self {
            Self::Client {
                stream,
                roots,
                identity,
            } => {
                if let Some(pem) = roots {
                    io.tls_roots(stream, &pem);
                }
                if let Some((chain, key)) = identity {
                    io.tls_identity(stream, &chain, &key);
                }
            }
            Self::Server {
                stream,
                identities,
                clients,
                alpn,
            } => {
                let identities: Vec<_> = identities.iter().map(|(c, k)| (&c[..], &k[..])).collect();
                io.tls_server(stream, &identities, clients.as_ref().map(|v| &v[..]), &alpn);
            }
        }
    }
}

fn mapped_checked(result: OperationResult, completion: C) -> Result<OperationResult, Error> {
    let OperationResult::Complete {
        value,
        mut released,
        ..
    } = result
    else {
        return Err(invalid());
    };
    // A truncated mapping's body result is dropped after the check.
    let value = match completion {
        C::Unit(Ok(())) => ok(value),
        C::Unit(Err(e)) => {
            released.push(value);
            file_error(e)
        }
        _ => return Err(invalid()),
    };
    Ok(OperationResult::Complete {
        value,
        arguments: vec![],
        consumed: vec![],
        released,
    })
}

/// The interpreter's host Io: the admitted real adapter, which writes the process's
/// standard streams itself and may stream a tape, or a recording's replay.
pub(crate) enum HostIo {
    Live(RealIo),
    Replay(SimIo),
}
impl HostIo {
    fn call(&mut self, request: R) -> Result<C, hirsel_rt::io::AdapterError> {
        match self {
            Self::Live(io) => io.call(request),
            Self::Replay(io) => io.call(request),
        }
    }
    fn tls(&mut self) {
        let engine = hirsel_rt::net_tls::engine();
        match self {
            Self::Live(io) => io.tls(engine),
            Self::Replay(io) => io.tls(engine),
        }
    }
    fn tls_roots(&mut self, stream: u64, pem: &[u8]) {
        match self {
            Self::Live(io) => io.tls_roots(stream, pem),
            Self::Replay(io) => io.tls_roots(stream, pem),
        }
    }
    fn tls_identity(&mut self, stream: u64, chain: &[u8], key: &[u8]) {
        match self {
            Self::Live(io) => io.tls_identity(stream, chain, key),
            Self::Replay(io) => io.tls_identity(stream, chain, key),
        }
    }
    fn tls_server(
        &mut self,
        stream: u64,
        identities: &[(&[u8], &[u8])],
        clients: ClientAuth<&[u8]>,
        alpn: &[String],
    ) {
        match self {
            Self::Live(io) => io.tls_server(stream, identities, clients, alpn),
            Self::Replay(io) => io.tls_server(stream, identities, clients, alpn),
        }
    }
    fn stdout(&self) -> &[u8] {
        match self {
            Self::Live(io) => io.stdout(),
            Self::Replay(io) => io.stdout(),
        }
    }
    /// The adapter's cooperative waits (`Adapter::enter` and the rest): live readiness
    /// polls, or the recording's `blocked` and `ready` events in replay.
    fn adapter(&mut self) -> &mut dyn Adapter {
        match self {
            Self::Live(io) => io,
            Self::Replay(io) => io,
        }
    }
    fn cancellable(&self, token: u64) -> bool {
        match self {
            Self::Live(io) => io.cancellable(token),
            Self::Replay(io) => io.cancellable(token),
        }
    }
    /// The committed prefix of the last completion's fault.
    fn last_partial(&self) -> usize {
        match self {
            Self::Live(io) => io.tape().last(),
            Self::Replay(io) => io.last_event(),
        }
        .and_then(|e| e.fault.as_ref())
        .map_or(0, |f| f.partial)
    }
}

/// `Result[ProcessOutput, ProcessError]` for a run's outcome.
fn process_value(outcome: ProcessOutcome) -> Value {
    hirsel_interp::library::process::outcome_value(&outcome)
}
fn pipeline_value(outcome: PipelineOutcome) -> Value {
    hirsel_interp::library::process::pipeline_value(&outcome)
}
/// `WaitError::Canceled(cause)`.
fn wait_canceled(cause: u32) -> Value {
    error(variant(1, vec![variant(cause, vec![])]))
}
pub(crate) struct HostLibrary {
    pub library: Library,
    free: Option<hirsel_rt::free_replay::Codec>,
    free_report: Option<hirsel_rt::free_replay::Report>,
    free_waits: BTreeMap<u64, free::Wait>,
    // A pending host poll can publish phases that enable an earlier-polled sibling.
    free_progressed: bool,
    free_drops: BTreeMap<u64, free::Drop>,
    free_checks: BTreeMap<u64, free::Check>,
    free_runners: BTreeMap<u64, free::Runner>,
    pending_free_drop: Option<OperationResult>,
    free_ready: Option<(R, Result<C, hirsel_rt::io::AdapterError>)>,
    pub io: HostIo,
    /// The first adapter refusal: a replay divergence or a live provider error.
    pub refused: Option<String>,
    /// Replay output already copied to the process's streams.
    echoed: (usize, usize),
    /// Offline semantic observation must not echo recorded streams or exit reports.
    emit_output: bool,
    handles: BTreeMap<u64, u64>,
    next: u64,
    next_wait: u64,
    process_sites: BTreeMap<u64, (hirsel_interp::observation::Anchor, Vec<u32>)>,
    waits: BTreeMap<u64, HostWait>,
    /// Armed scope deadlines (CONC-3), in entry order: token, scope and its timer request.
    armed: Vec<(u64, u64, R)>,
    /// The host requests the current scheduling point found ready.
    ready: Ready,
    /// The library's cancellation count at the last scheduling point.
    cancellations: u64,
    yield_now: bool,
    runners: runners::HostRunners,
    /// The host `mapped.with` calls whose bodies run, by continuation: Dir and Mapped.
    mapped: BTreeMap<u64, (u64, u64)>,
    next_continuation: u64,
}
struct HostWait {
    task: u64,
    name: String,
    arguments: Vec<Value>,
}
impl HostLibrary {
    pub fn new(
        run: &hirsel_tir::RunConfiguration,
        admitted: &hirsel_run_config::ValidatedRun,
        io: HostIo,
    ) -> Result<Self, Error> {
        let record = admitted.record();
        let mut grants = std::collections::BTreeSet::new();
        for (name, enabled) in [
            ("Io", record.io.is_some()),
            ("Args", record.args.is_some()),
            ("Env", record.env.is_some()),
            ("Unsafe", record.unsafe_root),
        ] {
            if enabled {
                grants.insert(name.into());
            }
        }
        let (monotonic, wall) = admitted.clock_grants();
        for (name, enabled) in [("MonotonicClock", monotonic), ("WallClock", wall)] {
            if enabled {
                grants.insert(name.into());
            }
        }
        let config = SimulationConfig {
            grants,
            cpu: record.io.as_ref().and_then(|io| io.cpu.clone()),
            tasks_free: record.io.as_ref().and_then(|io| io.tasks.as_deref())
                == Some("tasks.free@1"),
            // The host's source delivers every arrival; the library keeps the checks.
            signals: admitted
                .signals_grant()
                .map(|grant| hirsel_run_schema::SignalsGrant {
                    script: None,
                    ..grant.clone()
                }),
            projections: admitted.io_projections(),
            mounts: Default::default(),
            args: record.args.clone().unwrap_or_default(),
            env: record.env.clone().unwrap_or_default(),
            random_seed: record.inputs.random_seed,
            telemetry_seed: admitted.telemetry_provider().map_or(0, |(_, seed, _)| seed),
            test_seed: record.inputs.test_seed,
            scheduler: record.inputs.scheduler.clone(),
            runners: record
                .io
                .as_ref()
                .map(|io| io.runners.clone())
                .unwrap_or_default(),
            fault_seed: record.inputs.fault_seed.unwrap_or(0),
            ..Default::default()
        };
        let mut library = Library::new(run, config)?;
        library.host_deadlines();
        let free = match &io {
            HostIo::Replay(io) => {
                hirsel_rt::free_replay::Codec::open(io).map_err(Error::InvalidTir)?
            }
            _ => None,
        };
        let free = free.map(|(codec, session)| {
            library.enable_native_free(session, record.inputs.test_seed);
            codec
        });
        let free_report = if free.is_some() {
            hirsel_rt::free_replay::Report::open(&[], &[]).map_err(Error::InvalidTir)?
        } else {
            None
        };
        Ok(Self {
            free_report,
            free,
            free_waits: BTreeMap::new(),
            free_progressed: false,
            free_drops: BTreeMap::new(),
            free_checks: BTreeMap::new(),
            free_runners: BTreeMap::new(),
            pending_free_drop: None,
            free_ready: None,
            next_wait: 1 << 62,
            process_sites: BTreeMap::new(),
            waits: BTreeMap::new(),
            armed: Vec::new(),
            ready: Ready::Tokens(Vec::new()),
            cancellations: 0,
            yield_now: false,
            runners: Default::default(),
            library,
            io,
            refused: None,
            echoed: (0, 0),
            emit_output: true,
            handles: BTreeMap::new(),
            next: 1 << 63,
            mapped: BTreeMap::new(),
            next_continuation: 1 << 61,
        })
    }
    pub fn silence_replay_output(&mut self) {
        if matches!(self.io, HostIo::Replay(_)) {
            self.emit_output = false;
        }
    }
    /// `mapped.with` over a host mapping: the view, then the body, then (`resume`) the check.
    fn host_mapped_with(&mut self, a: &[Value]) -> Result<OperationResult, Error> {
        self.yield_now = true;
        let (directory, mapped) = (self.id(&a[0])?, self.id(&a[1])?);
        let C::View(view) = self.host(R::MapView { directory, mapped })? else {
            return Err(invalid());
        };
        let continuation = self.next_continuation;
        self.next_continuation += 1;
        self.mapped.insert(continuation, (directory, mapped));
        Ok(OperationResult::Call {
            callee: a[2].clone(),
            arguments: vec![Value::Bytes(view.contents())],
            mut_arguments: vec![],
            writebacks: vec![],
            updates: vec![],
            owned: vec![],
            consumed: vec![],
            continuation,
        })
    }
    /// `watcher.next` on a host watcher: the next inotify event, or, with none yet, a park
    /// on the watcher's readiness, as a stdin read parks at once.
    fn host_watch_next(
        &mut self,
        a: &[Value],
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        if self.free_ready.is_some() {
            self.yield_now = true;
            return self.watch_request(a, 0);
        }
        let token = self.next_wait;
        self.next_wait += 1;
        match self.watch_request(a, token) {
            Err(Error::Blocked(_)) => {}
            result => {
                self.yield_now = true;
                return result;
            }
        }
        self.waits.insert(
            token,
            HostWait {
                task: self.library.task_id(),
                name: "watcher.next".into(),
                arguments: a.to_vec(),
            },
        );
        Ok(OperationResult::Park {
            token,
            wait: hirsel_interp::BlockedWait {
                spawn_path: self.library.task_path(),
                anchor: cx.anchor().cloned().ok_or_else(invalid)?,
                kind: "watcher.next".into(),
                targets: vec![],
            },
        })
    }
    fn watch_host_request(&self, a: &[Value]) -> Result<R, Error> {
        Ok(R::WatchNext {
            directory: self.id(&a[0])?,
            watcher: self.id(&a[1])?,
        })
    }
    /// Issue `watcher.next` under `token`: its event, or Blocked while none has arrived.
    fn watch_request(&mut self, a: &[Value], token: u64) -> Result<OperationResult, Error> {
        let request = self.watch_host_request(a)?;
        let checked = self.free_ready.is_some();
        if !checked {
            self.io.adapter().enter(token);
        }
        let completion = self.host(request);
        if !checked && !matches!(completion, Err(Error::Blocked(_))) {
            self.io.adapter().finish(token);
        }
        let C::Watch(result) = completion? else {
            return Err(invalid());
        };
        use hirsel_rt::watch::WatchEvent::*;
        let path = hirsel_interp::library::path;
        Ok(OperationResult::returned(match result {
            Ok(Created(p)) => ok(variant(0, vec![path(&p)])),
            Ok(Modified(p)) => ok(variant(1, vec![path(&p)])),
            Ok(Removed(p)) => ok(variant(2, vec![path(&p)])),
            Ok(Renamed(a, b)) => ok(variant(3, vec![path(&a), path(&b)])),
            Ok(Overflow) => ok(variant(4, vec![])),
            Err(e) => file_error(e),
        }))
    }
    /// Payload checks precede host phases. A refused finish still closes its span.
    fn telemetry_request(&self, name: &str, a: &[Value]) -> Result<Option<R>, Error> {
        use hirsel_rt::telemetry::kernel;
        if name == "io.telemetry" {
            return Ok(Some(R::Telemetry { io: 1 }));
        }
        let telemetry = self.id(&a[0])?;
        if name == "capture.context" {
            return Ok(None);
        }
        if name == "capture.begin" {
            let (parent, spec) = (bytes(&a[1])?, bytes(&a[2])?);
            return Ok(kernel::bounded(&[&spec])
                .and_then(|()| kernel::parent(&parent).map(|_| ()))
                .ok()
                .map(|()| R::TelemetryBegin {
                    telemetry,
                    parent,
                    spec,
                }));
        }
        let span = if matches!(name, "capture.event" | "capture.finish") {
            Some(self.id(&a[1])?)
        } else {
            None
        };
        let payload = bytes(&a[if span.is_some() { 2 } else { 1 }])?;
        if kernel::bounded(&[&payload]).is_err() {
            return Ok(if name == "capture.finish" {
                let span = span.unwrap();
                if self.free_ready.is_none()
                    && self
                        .free
                        .as_ref()
                        .is_some_and(|codec| codec.span_context(telemetry, span).is_none())
                {
                    return Err(invalid());
                }
                Some(R::Close { handle: span })
            } else {
                None
            });
        }
        Ok(Some(match name {
            "capture.event" => R::TelemetryEvent {
                telemetry,
                span: span.unwrap(),
                payload,
            },
            "capture.finish" => R::TelemetryFinish {
                telemetry,
                span: span.unwrap(),
                status: payload,
            },
            "capture.log" => R::TelemetryLog { telemetry, payload },
            "capture.measure" => R::TelemetryMeasure { telemetry, payload },
            _ => return Err(invalid()),
        }))
    }
    fn telemetry_context(&mut self, telemetry: u64, span: u64) -> Option<Vec<u8>> {
        match &self.free {
            Some(codec) => codec.span_context(telemetry, span),
            None => self.io.adapter().telemetry_context(telemetry, span),
        }
    }
    /// Telemetry on the run's host provider: the runtime checks, draws or replays span
    /// identities and records every accepted observation. A refused payload issues nothing
    /// and is no scheduling point; neither is reading a span's context.
    fn host_telemetry(&mut self, name: &str, a: &[Value]) -> Result<OperationResult, Error> {
        use hirsel_rt::telemetry::kernel;
        let refused = |refusal: kernel::Refusal| error(variant(refusal.ordinal(), vec![]));
        if name == "io.telemetry" {
            if !matches!(a.first(), Some(Value::Handle { kind, .. }) if kind == "Io") {
                return Err(invalid());
            }
            self.yield_now = true;
            return Ok(OperationResult::returned(
                match self.host(R::Telemetry { io: 1 })? {
                    C::Handle(Ok(handle)) => ok(self.handle(handle, "Telemetry")),
                    C::Handle(Err(hirsel_rt::io::IoError::NotGranted)) => error(variant(0, vec![])),
                    _ => return Err(invalid()),
                },
            ));
        }
        let telemetry = self.id(&a[0])?;
        let value = match name {
            "capture.context" => {
                let span = self.id(&a[1])?;
                Value::Bytes(
                    self.telemetry_context(telemetry, span)
                        .ok_or_else(invalid)?,
                )
            }
            "capture.begin" => {
                let (parent, spec) = (bytes(&a[1])?, bytes(&a[2])?);
                if let Err(refusal) =
                    kernel::bounded(&[&spec]).and_then(|()| kernel::parent(&parent).map(|_| ()))
                {
                    return Ok(OperationResult::returned(refused(refusal)));
                }
                self.yield_now = true;
                let request = self.telemetry_request(name, a)?.ok_or_else(invalid)?;
                let C::Span { handle, .. } = self.host(request)? else {
                    return Err(invalid());
                };
                ok(self.handle(handle, "Span"))
            }
            _ => {
                let span = match name {
                    "capture.event" | "capture.finish" => Some(self.id(&a[1])?),
                    _ => None,
                };
                let payload = bytes(&a[if span.is_some() { 2 } else { 1 }])?;
                if let Err(refusal) = kernel::bounded(&[&payload]) {
                    if name == "capture.finish" {
                        // Free validated the owner before its checked Close. The
                        // Ended observation has already removed the derived context.
                        if self.free_ready.is_none()
                            && self.telemetry_context(telemetry, span.unwrap()).is_none()
                        {
                            return Err(invalid());
                        }
                        let request = self.telemetry_request(name, a)?.ok_or_else(invalid)?;
                        self.forget_handle(&a[1]);
                        if self.host(request)? != C::Unit(Ok(())) {
                            return Err(invalid());
                        }
                    }
                    return Ok(OperationResult::returned(refused(refusal)));
                }
                self.yield_now = true;
                let request = self.telemetry_request(name, a)?.ok_or_else(invalid)?;
                if self.host(request)? != C::Unit(Ok(())) {
                    return Err(invalid());
                }
                if name == "capture.finish" {
                    self.forget_handle(&a[1]);
                }
                ok(Value::Unit)
            }
        };
        Ok(OperationResult::returned(value))
    }
    /// A taken host handle's mapping, once its runtime row is gone.
    fn forget_handle(&mut self, v: &Value) {
        if let Value::Handle { id, .. } = v {
            self.handles.remove(id);
        }
    }
    fn cpu_receipt(&mut self, request: hirsel_interp::library::CpuControl) -> Result<(), Error> {
        if self.free.is_some() {
            return Ok(());
        }
        let hirsel_interp::library::CpuControl {
            width,
            action,
            count,
        } = request;
        if self.io.adapter().wants_issuer() {
            let task = self.library.task_path();
            self.io.adapter().issuer(&task);
        }
        let result = (|| match self.io.adapter().cpu_control(width, action, count, None)? {
            Some(outcome) => Ok(outcome),
            None => {
                let outcome = self
                    .library
                    .cpu_live_outcome(request)
                    .map_err(|_| hirsel_rt::io::AdapterError::InvalidCompletion)?;
                self.io
                    .adapter()
                    .cpu_control(width, action, count, Some(outcome))?;
                Ok(outcome)
            }
        })();
        let outcome = result.map_err(|error: hirsel_rt::io::AdapterError| {
            let message = format!("{error:?}");
            self.refused.get_or_insert(message.clone());
            Error::Unavailable(format!("host CPU receipt: {message}"))
        })?;
        self.library.supply_cpu_outcome(request, outcome)
    }

    /// Every host request goes through here. A provider that would block parks the
    /// task; any other adapter error is a refusal (a replay divergence or host fault).
    fn host(&mut self, request: R) -> Result<C, Error> {
        self.host_cancelable(request, self.library.cancellation())
    }
    fn host_cancelable(&mut self, request: R, cancellation: Option<u32>) -> Result<C, Error> {
        if let Some((expected, result)) = self.free_ready.take() {
            if expected != request {
                return Err(invalid());
            }
            return result.map_err(|e| Error::Unavailable(format!("Free completion: {e:?}")));
        }
        if self.free.is_some() {
            return Err(Error::Unavailable(
                "Free host request has no evaluator continuation".into(),
            ));
        }
        self.crash_point()?;
        if self.io.adapter().wants_issuer() {
            let task = self.library.task_path();
            self.io.adapter().issuer(&task);
        }
        let result = match cancellation {
            Some(cause) => hirsel_rt::io_checks::cancel(self.io.adapter(), &request, cause)
                .and_then(|completion| match completion {
                    Some(completion) => Ok(completion),
                    None => self.io.call(request),
                }),
            None => self.io.call(request),
        };
        if !matches!(result, Err(hirsel_rt::io::AdapterError::Blocked)) {
            self.runners.requests += 1;
        }
        result.map_err(|e| match e {
            hirsel_rt::io::AdapterError::Blocked => Error::Blocked(vec![]),
            hirsel_rt::io::AdapterError::Stopped => {
                Error::Environmental(hirsel_run_schema::Environmental::Stopped)
            }
            e => {
                let message = format!("{e:?}");
                self.refused.get_or_insert(message.clone());
                Error::Unavailable(format!("host Io: {message}"))
            }
        })
    }
    /// Replay performs no host effect, so the program's recorded output is copied
    /// to the process's streams as it is produced.
    fn echo(&mut self, stdout: &[u8]) -> Result<(), Error> {
        use std::io::Write;
        if self.emit_output
            && let HostIo::Replay(_) = self.io
        {
            let stderr = &self.library.simulation.stderr;
            std::io::stdout()
                .write_all(&stdout[self.echoed.0..])
                .and_then(|()| std::io::stdout().flush())
                .and_then(|()| std::io::stderr().write_all(&stderr[self.echoed.1..]))
                .map_err(|e| Error::Unavailable(format!("replay output: {e}")))?;
            self.echoed = (stdout.len(), stderr.len());
        }
        Ok(())
    }
    fn id(&self, v: &Value) -> Result<u64, Error> {
        if let Value::Handle { id, .. } = v {
            self.handles.get(id).copied().ok_or_else(invalid)
        } else {
            Err(invalid())
        }
    }
    fn handle(&mut self, id: u64, kind: &str) -> Value {
        let h = self.next;
        self.next += 1;
        self.handles.insert(h, id);
        Value::Handle {
            kind: kind.into(),
            id: h,
        }
    }
    fn input_request(&self, name: &str, a: &[Value]) -> Result<R, Error> {
        if name == "stdin.read" {
            return Ok(R::ReadStdin {
                stdin: self.id(&a[0])?,
                max: word(&a[2])?,
            });
        }
        // The launch Env is immutable and has no projected host row. Narrowed
        // Env values retain the exact host authority returned by their creator.
        let env = match &a[0] {
            Value::Handle { id, .. } => self
                .handles
                .get(id)
                .copied()
                .unwrap_or(hirsel_rt::io::ENV_HANDLE),
            _ => hirsel_rt::io::ENV_HANDLE,
        };
        Ok(match name {
            "env.get" => R::EnvGet {
                env,
                name: text(&a[1])?,
            },
            "env.narrow" => R::EnvNarrow {
                env,
                names: fields(&a[1])?
                    .into_iter()
                    .map(text)
                    .collect::<Result<_, _>>()?,
            },
            _ => return Err(invalid()),
        })
    }
    fn host_input(
        &mut self,
        name: &str,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        use hirsel_rt::io::IoError;
        let a = &*call.arguments;
        if matches!(
            name,
            "io.stdin" | "io.stderr" | "io.stdout" | "io.random" | "io.tasks" | "io.signals"
        ) {
            // The library validates the root and records the semantic projection once.
            let result = self.library.call(call, cx)?;
            let completion = self.host(match name {
                "io.stdin" => R::Stdin { io: 1 },
                "io.stderr" => R::Stderr { io: 1 },
                "io.stdout" => R::Stdout { io: 1 },
                "io.random" => R::Random { io: 1 },
                "io.signals" => R::Signals { io: 1 },
                _ => R::Tasks { io: 1 },
            })?;
            match (&result, completion) {
                (
                    Some(OperationResult::Complete {
                        value: Value::Variant { variant: 0, fields },
                        ..
                    }),
                    C::Handle(Ok(host)),
                ) => {
                    let Some(Some(Value::Handle { id, .. })) = fields.first() else {
                        return Err(invalid());
                    };
                    self.handles.insert(*id, host);
                }
                (
                    Some(OperationResult::Complete {
                        value: Value::Variant { variant: 1, .. },
                        ..
                    }),
                    C::Handle(Err(IoError::NotGranted)),
                ) => {}
                _ => return Err(invalid()),
            }
            return Ok(result);
        }
        if name == "env.narrow" {
            let request = self.input_request(name, a)?;
            let result = self.library.call(call, cx)?;
            match (&result, self.host(request)?) {
                (
                    Some(OperationResult::Complete {
                        value: Value::Variant { variant: 0, fields },
                        ..
                    }),
                    C::Handle(Ok(host)),
                ) => {
                    let Some(Some(Value::Handle { id, .. })) = fields.first() else {
                        return Err(invalid());
                    };
                    self.handles.insert(*id, host);
                }
                (
                    Some(OperationResult::Complete {
                        value: Value::Variant { variant: 1, .. },
                        ..
                    }),
                    C::Handle(Err(IoError::NotGranted)),
                ) => {}
                _ => return Err(invalid()),
            }
            return Ok(result);
        }
        if name == "env.get" {
            let request = self.input_request(name, a)?;
            let result = self.library.call(call, cx)?;
            let completion = self.host(request)?;
            let C::Env(value) = completion else {
                return Err(invalid());
            };
            let expected = match value {
                Ok(Some(value)) => ok(variant(1, vec![Value::String(value)])),
                Ok(None) => ok(variant(0, vec![])),
                Err(hirsel_rt::io::EnvError::NotGranted) => error(variant(0, vec![])),
                Err(hirsel_rt::io::EnvError::InvalidUtf8) => error(variant(1, vec![])),
            };
            if !matches!(&result, Some(OperationResult::Complete { value, .. }) if value == &expected)
            {
                return Err(invalid());
            }
            return Ok(result);
        }
        // Free mode: the write is a host request whose host call runs between its claim and
        // its end, and only live.
        if matches!(name, "err.print" | "out.print" | "err.write" | "out.write") {
            use hirsel_interp::library::FreePrint;
            match self.library.free_print_begin(name, a, cx)? {
                None => {}
                Some(FreePrint::Done(result)) => return Ok(Some(result)),
                Some(FreePrint::Write(write)) => {
                    let request = if name.starts_with("err.") {
                        R::PrintStderr {
                            stderr: self.id(&a[0])?,
                            bytes: bytes(&a[1])?,
                        }
                    } else {
                        R::Print {
                            stdout: self.id(&a[0])?,
                            bytes: bytes(&a[1])?,
                        }
                    };
                    let completion = self.host_cancelable(request, None)?;
                    self.supply(name, completion)?;
                    return self.library.free_print_end(write, name, a, cx).map(Some);
                }
            }
        }
        // The library validates the handle/schema and returns Canceled without
        // consuming input or publishing output. Destruction's entered task is shielded.
        if matches!(
            name,
            "stdin.read" | "err.print" | "out.print" | "err.write" | "out.write"
        ) && self.free_ready.is_none()
            && self.library.cancellation().is_some()
        {
            return self.library.call(call, cx);
        }
        if name == "stdin.read" {
            // A pipe or terminal with no byte yet parks only this task (CONC-1).
            let token = self.next_wait;
            self.next_wait += 1;
            if self.host_stdin(a, token)? {
                return self.library.call(call, cx);
            }
            self.waits.insert(
                token,
                HostWait {
                    task: self.library.task_id(),
                    name: name.into(),
                    arguments: a.to_vec(),
                },
            );
            return Ok(Some(OperationResult::Park {
                token,
                wait: hirsel_interp::BlockedWait {
                    spawn_path: self.library.task_path(),
                    anchor: cx.anchor().cloned().ok_or_else(invalid)?,
                    kind: name.into(),
                    targets: vec![],
                },
            }));
        }
        let request = if name.starts_with("err.") {
            R::PrintStderr {
                stderr: self.id(&a[0])?,
                bytes: bytes(&a[1])?,
            }
        } else {
            R::Print {
                stdout: self.id(&a[0])?,
                bytes: bytes(&a[1])?,
            }
        };
        let completion = self.host(request)?;
        self.supply(name, completion)?;
        self.library.call(call, cx)
    }
    /// The provider draws (splitmix64 or the host's secure generator) and the library
    /// records the supplied bytes as the semantic event. A zero count reaches no provider.
    fn host_random(
        &mut self,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let count = word(&call.arguments[1])?;
        if count > 0 {
            let completion = self.host(R::RandomBytes {
                random: self.id(&call.arguments[0])?,
                count,
            })?;
            let C::Random(draw) = completion else {
                return Err(invalid());
            };
            self.library
                .simulation
                .supply_random(draw.map_err(|hirsel_rt::io::RandomError::Unavailable| ()));
        }
        self.library.call(call, cx)
    }
    /// A due or elapsed deadline completes at once; otherwise the task parks on the
    /// timer's Readiness token until it is due or cancelled (CONC-3).
    fn host_sleep(
        &mut self,
        a: &[Value],
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        // Cancellation suppresses the provider call, not this blocking entry's
        // scheduling point. Native capability_call and simulated sleeps yield too.
        self.yield_now = true;
        if self.free_ready.is_none()
            && let Some(cause) = self.library.cancellation()
        {
            return Ok(OperationResult::returned(wait_canceled(cause)));
        }
        let token = self.next_wait;
        self.next_wait += 1;
        match self.sleep_request(a, token) {
            Err(Error::Blocked(_)) => {}
            result => return result,
        }
        self.waits.insert(
            token,
            HostWait {
                task: self.library.task_id(),
                name: "clock.sleep_until".into(),
                arguments: a.to_vec(),
            },
        );
        Ok(OperationResult::Park {
            token,
            wait: hirsel_interp::BlockedWait {
                spawn_path: self.library.task_path(),
                anchor: cx.anchor().cloned().ok_or_else(invalid)?,
                kind: "clock.sleep_until".into(),
                targets: vec![],
            },
        })
    }
    fn sleep_request(&mut self, a: &[Value], token: u64) -> Result<OperationResult, Error> {
        let Value::Handle { kind, id: deadline } = &a[1] else {
            return Err(invalid());
        };
        if kind != "Instant" {
            return Err(invalid());
        }
        let clock = self.id(&a[0])?;
        self.io.adapter().enter(token);
        let completion = self.host(R::SleepUntil {
            clock,
            deadline: *deadline,
        });
        if !matches!(completion, Err(Error::Blocked(_))) {
            self.io.adapter().finish(token);
        }
        let completion = completion?;
        let C::Wait(outcome) = completion else {
            return Err(invalid());
        };
        Ok(OperationResult::returned(match outcome {
            Ok(()) => ok(Value::Unit),
            Err(hirsel_rt::io::WaitError::Clock(e)) => error(variant(
                0,
                vec![variant(
                    match e {
                        hirsel_rt::io::ClockError::Unavailable => 0,
                        hirsel_rt::io::ClockError::OutOfRange => 1,
                    },
                    vec![],
                )],
            )),
            Err(hirsel_rt::io::WaitError::Canceled(cause)) => wait_canceled(cause),
        }))
    }
    /// Arm `scope`'s deadline as a timer wait on its clock under a token of its own; no
    /// cancellation applies to it. A reached deadline, or a clock the host cannot read,
    /// expires the scope at once (CONC-3).
    // Scope timers are control interests, as in the native Free host service.
    // Their expiry is recorded by cancellation checks and the scope exit.
    fn scope_timer(&mut self, request: R) -> Result<C, Error> {
        if self.free.is_some() && !self.library.free_replay() {
            return self.io.call(request).map_err(|error| match error {
                hirsel_rt::io::AdapterError::Blocked => Error::Blocked(Vec::new()),
                error => {
                    let message = format!("{error:?}");
                    self.refused.get_or_insert(message.clone());
                    Error::Unavailable(format!("host scope timer: {message}"))
                }
            });
        }
        self.host_cancelable(request, None)
    }

    fn arm_deadline(
        &mut self,
        scope: u64,
        clock: &Value,
        at: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        let request = R::SleepUntil {
            clock: self.id(clock)?,
            deadline: at,
        };
        let token = self.next_wait;
        self.next_wait += 1;
        self.io.adapter().enter(token);
        match self.scope_timer(request.clone()) {
            Err(Error::Blocked(_)) => {
                self.io.adapter().parked(token);
                self.armed.push((token, scope, request));
                Ok(())
            }
            completion => {
                self.io.adapter().finish(token);
                let C::Wait(_) = completion? else {
                    return Err(invalid());
                };
                self.library.expire_deadline(scope, cx)
            }
        }
    }
    /// `signal.wait` through the host's signal source; the library then consumes the kind
    /// it supplied. A cancelled wait reaches no provider: the library makes the checks and
    /// returns Canceled.
    fn host_signal(
        &mut self,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        if self.free_ready.is_none() && self.library.cancellation().is_some() {
            return self.library.call(call, cx);
        }
        let a = &*call.arguments;
        // A pending wait has not entered Library::wait_signal, which normally
        // publishes this yield. Preserve native's yield-before-park boundary.
        self.yield_now = true;
        let token = self.next_wait;
        self.next_wait += 1;
        if self.host_signal_wait(a, token)? {
            return self.library.call(call, cx);
        }
        self.waits.insert(
            token,
            HostWait {
                task: self.library.task_id(),
                name: "signal.wait".into(),
                arguments: a.to_vec(),
            },
        );
        Ok(Some(OperationResult::Park {
            token,
            wait: hirsel_interp::BlockedWait {
                spawn_path: self.library.task_path(),
                anchor: cx.anchor().cloned().ok_or_else(invalid)?,
                kind: "signal.wait".into(),
                targets: vec![],
            },
        }))
    }
    fn signal_request(&self, a: &[Value]) -> Result<R, Error> {
        let kinds = fields(&a[1])?.into_iter().try_fold(0u8, |bits, kind| {
            hirsel_run_schema::SignalKind::from_ordinal(tag(kind)?)
                .map(|kind| bits | kind.bit())
                .ok_or_else(invalid)
        })?;
        Ok(R::SignalWait {
            signals: self.id(&a[0])?,
            kinds,
        })
    }
    /// Issue `signal.wait` under `token`, supplying an arrival to the library. False while
    /// none among its kinds is pending: the token keeps the source's readiness interest.
    fn host_signal_wait(&mut self, a: &[Value], token: u64) -> Result<bool, Error> {
        let request = self.signal_request(a)?;
        self.io.adapter().enter(token);
        let completion = match self.host(request) {
            Err(Error::Blocked(_)) => return Ok(false),
            completion => {
                self.io.adapter().finish(token);
                completion?
            }
        };
        match completion {
            C::Signal(Ok(kind)) => self.library.supply_signal(kind),
            // The library derives the same program error.
            C::Signal(Err(_)) => {}
            _ => return Err(invalid()),
        }
        Ok(true)
    }
    /// Read host stdin into the simulation the library then consumes. False when no
    /// byte is available yet: the token keeps the descriptor's readiness interest.
    fn host_stdin(&mut self, a: &[Value], token: u64) -> Result<bool, Error> {
        let request = self.input_request("stdin.read", a)?;
        self.io.adapter().enter(token);
        let completion = match self.host(request) {
            Err(Error::Blocked(_)) => return Ok(false),
            completion => {
                self.io.adapter().finish(token);
                completion?
            }
        };
        self.supply("stdin.read", completion)?;
        Ok(true)
    }
    /// Hand a host stdin/stderr completion to the library as configured input or fault.
    fn supply(&mut self, name: &str, completion: C) -> Result<(), Error> {
        use hirsel_interp::library::simulation::Fault;
        use hirsel_rt::io::{InputError, IoError};
        let fault = match completion {
            C::Input(Ok(bytes)) => {
                self.library.simulation.supply_stdin(bytes);
                None
            }
            C::Input(Err(InputError::Fault(code))) => Some(Fault::Fault { code, partial: 0 }),
            C::Input(Err(InputError::Canceled(cause))) => Some(Fault::Canceled(cause)),
            C::Input(Err(InputError::InvalidInput)) => None,
            C::Unit(Ok(())) => None,
            C::Unit(Err(IoError::Fault(code))) => Some(Fault::Fault {
                code,
                partial: self.io.last_partial(),
            }),
            C::Unit(Err(IoError::Canceled(cause))) => Some(Fault::Canceled(cause)),
            _ => return Err(invalid()),
        };
        if let Some(fault) = fault {
            self.library
                .simulation
                .config
                .faults
                .entry(name.into())
                .or_default()
                .push_back(fault);
        }
        Ok(())
    }
    /// The root Proc and its runs go to the runtime's host provider, which checks and
    /// records them; a scripted `test.process` Proc stays with the library. A run parks
    /// its task on the provider's readiness token until its child exits (CONC-1), and a
    /// cancelled scope returns Canceled after program checks (CONC-3).
    fn host_process(
        &mut self,
        name: &str,
        a: &[Value],
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        if name == "io.process" {
            return Ok(OperationResult::returned(
                match self.host(self.process_projection_request(name, a)?)? {
                    C::Handle(Ok(id)) => ok(self.handle(id, "Proc")),
                    C::Handle(Err(hirsel_rt::io::IoError::NotGranted)) => error(variant(0, vec![])),
                    _ => return Err(invalid()),
                },
            ));
        }
        if name == "proc.narrow" {
            let request = self.process_projection_request(name, a)?;
            return Ok(OperationResult::returned(match self.host(request)? {
                C::Spawned(Ok(id)) => ok(self.handle(id, "Proc")),
                C::Spawned(Err(f)) => error(hirsel_interp::library::process::failure_value(f)),
                _ => return Err(invalid()),
            }));
        }
        self.yield_now = true;
        if name != "proc.run" && name != "proc.pipeline" {
            return self.host_child(name, a, cx);
        }
        let token = self.next_wait;
        self.next_wait += 1;
        match self.process_request(name, a, token) {
            Err(Error::Blocked(_)) => {}
            result => return result,
        }
        self.waits.insert(
            token,
            HostWait {
                task: self.library.task_id(),
                name: name.into(),
                arguments: a.to_vec(),
            },
        );
        Ok(OperationResult::Park {
            token,
            wait: hirsel_interp::BlockedWait {
                spawn_path: self.library.task_path(),
                anchor: cx.anchor().cloned().ok_or_else(invalid)?,
                kind: name.into(),
                targets: vec![],
            },
        })
    }
    fn process_projection_request(&self, name: &str, a: &[Value]) -> Result<R, Error> {
        if name == "io.process" {
            return Ok(R::Process { io: 1 });
        }
        let rules = fields(&a[1])?
            .into_iter()
            .map(|rule| {
                let [program, env] = fields(rule)?[..] else {
                    return Err(invalid());
                };
                Ok(hirsel_run_schema::ProcessRule {
                    program: text(program)?,
                    env: fields(env)?
                        .into_iter()
                        .map(text)
                        .collect::<Result<_, _>>()?,
                })
            })
            .collect::<Result<_, _>>()?;
        let process = self.id(&a[0])?;
        Ok(R::ProcessNarrow { process, rules })
    }
    /// `proc.spawn` or a child operation of a host Proc (`hirsel-rt/src/host_waits.rs`): a
    /// cancellation point completes Canceled without a request, and a write, read or wait
    /// parks its task until the child can proceed.
    fn host_child(
        &mut self,
        name: &str,
        a: &[Value],
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        if !matches!(
            name,
            "child.write"
                | "child.read_stdout"
                | "child.read_stderr"
                | "child.wait"
                | "group.write"
                | "group.read_stdout"
                | "proc.wait_group"
        ) {
            return self.child_request(name, a, None, cx);
        }
        let token = self.next_wait;
        self.next_wait += 1;
        match self.child_request(name, a, Some(token), cx) {
            Err(Error::Blocked(_)) => {}
            result => return result,
        }
        self.waits.insert(
            token,
            HostWait {
                task: self.library.task_id(),
                name: name.into(),
                arguments: a.to_vec(),
            },
        );
        Ok(OperationResult::Park {
            token,
            wait: hirsel_interp::BlockedWait {
                spawn_path: self.library.task_path(),
                anchor: cx.anchor().cloned().ok_or_else(invalid)?,
                kind: name.into(),
                targets: vec![],
            },
        })
    }
    fn child_host_request(&self, name: &str, a: &[Value]) -> Result<Option<R>, Error> {
        let process = self.id(&a[0])?;
        Ok(Some(match name {
            "proc.spawn" => match self.command(&a[1])? {
                Some(command) => R::ProcessSpawn { process, command },
                None => {
                    return Ok(None);
                }
            },
            "proc.spawn_pipeline" => {
                let mut stages = vec![];
                for stage in fields(&a[1])? {
                    match self.command(stage)? {
                        Some(command) => stages.push(command),
                        None => {
                            return Ok(None);
                        }
                    }
                }
                R::ProcessSpawnPipeline {
                    process,
                    stages,
                    max_stderr: word(&a[2])?,
                }
            }
            "proc.wait_group" => R::GroupWait {
                process,
                group: self.id(&a[1])?,
                deadline: self.deadline(&a[2])?,
            },
            "child.write" | "group.write" => R::ChildWrite {
                process,
                child: self.id(&a[1])?,
                bytes: bytes(&a[2])?,
            },
            "child.close_stdin" | "group.close_stdin" => R::ChildCloseStdin {
                process,
                child: self.id(&a[1])?,
            },
            "child.read_stdout" | "child.read_stderr" | "group.read_stdout" => R::ChildRead {
                process,
                child: self.id(&a[1])?,
                stream: if name == "child.read_stderr" {
                    hirsel_rt::process::ChildStream::Stderr
                } else {
                    hirsel_rt::process::ChildStream::Stdout
                },
                max: word(&a[3])?,
            },
            "child.wait" => R::ChildWait {
                process,
                child: self.id(&a[1])?,
                deadline: self.deadline(&a[2])?,
            },
            "child.kill" | "group.kill" => R::ChildKill {
                process,
                child: self.id(&a[1])?,
                signal: hirsel_rt::process::Signal::of_ordinal(tag(&a[2])?).ok_or_else(invalid)?,
            },
            _ => return Err(Error::Unavailable(format!("host process {name}"))),
        }))
    }
    /// Issue a child request, under `token` when it can park.
    fn child_request(
        &mut self,
        name: &str,
        a: &[Value],
        token: Option<u64>,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        use hirsel_interp::library::process::{failure_value, script_failure, status_value};
        let Some(request) = self.child_host_request(name, a)? else {
            return Ok(OperationResult::returned(error(failure_value(
                ProcessFailure::InvalidInput,
            ))));
        };
        if let Some(token) = token {
            self.io.adapter().enter(token);
        }
        let cancellation = self
            .library
            .cancellation()
            .filter(|_| token.is_none_or(|t| self.io.cancellable(t)));
        if let R::ChildWrite { child, .. }
        | R::ChildCloseStdin { child, .. }
        | R::ChildWait { child, .. }
        | R::GroupWait { group: child, .. } = &request
        {
            if let Some(anchor) = cx.anchor() {
                self.process_sites
                    .insert(*child, (anchor.clone(), self.library.task_path()));
            }
        }
        let printed = self.io.stdout().len();
        let completion = self.host_cancelable(request, cancellation);
        if matches!(completion, Ok(C::Checked(false))) {
            cx.stdout.extend_from_slice(&self.io.stdout()[printed..]);
        }
        if let Some(token) = token
            && !matches!(completion, Err(Error::Blocked(_)))
        {
            self.io.adapter().finish(token);
        }
        let completion = completion?;
        if let C::Spawned(Ok(child)) = &completion {
            if let Some(anchor) = cx.anchor() {
                self.process_sites
                    .insert(*child, (anchor.clone(), self.library.task_path()));
            }
        }
        // The wait consumes its Child on every outcome.
        if matches!(name, "child.wait" | "proc.wait_group")
            && let Value::Handle { id, .. } = a[1]
        {
            self.handles.remove(&id);
        }
        let mut updates = vec![];
        let value = match completion {
            C::Spawned(Ok(child)) if name == "proc.spawn_pipeline" => {
                ok(self.handle(child, "ChildGroup"))
            }
            C::Spawned(Ok(child)) => ok(self.handle(child, "Child")),
            C::ChildUnit(Ok(())) if name == "child.write" || name == "group.write" => {
                ok(Value::Unit)
            }
            C::GroupExit(outcome) => hirsel_interp::library::process::group_value(&outcome),
            C::ChildUnit(Ok(())) => Value::Unit,
            C::ChildBytes(Ok(read)) => {
                let n = read.len() as u64;
                let mut buffer = bytes(&a[2])?;
                buffer.extend_from_slice(&read);
                updates.push((
                    2,
                    sequence(
                        BuiltinType::List,
                        buffer.into_iter().map(|b| int(Primitive::U8, b)),
                    ),
                ));
                ok(u64_value(n))
            }
            C::ChildExit(Ok(status)) => ok(status_value(status)),
            C::Spawned(Err(f))
            | C::ChildUnit(Err(f))
            | C::ChildBytes(Err(f))
            | C::ChildExit(Err(f)) => error(failure_value(f)),
            C::Checked(false) => return script_failure(cx, "scripted child unmet"),
            _ => return Err(invalid()),
        };
        Ok(OperationResult::Complete {
            value,
            arguments: updates,
            consumed: if matches!(name, "child.wait" | "proc.wait_group") {
                vec![1]
            } else {
                vec![]
            },
            released: vec![],
        })
    }
    /// Issue `proc.run` under `token`: its completion, or Blocked while the child runs.
    /// A `Command` value with its host Dir handle; `None` for a Dir with no host directory
    /// to start in (one from test.dir).
    fn command(&self, v: &Value) -> Result<Option<hirsel_rt::process::Command>, Error> {
        let f = fields(v)?;
        let directory = match tag(f[2])? {
            0 => None,
            _ => match self.id(fields(f[2])?[0]) {
                Ok(directory) => Some(directory),
                Err(_) => return Ok(None),
            },
        };
        let env = match tag(f[3])? {
            0 => None,
            _ => Some(
                fields(fields(f[3])?[0])?
                    .into_iter()
                    .map(|var| match fields(var)?.as_slice() {
                        [name, value] => Ok((text(name)?, text(value)?)),
                        _ => Err(invalid()),
                    })
                    .collect::<Result<_, Error>>()?,
            ),
        };
        Ok(Some(hirsel_rt::process::Command {
            program: text(fields(f[0])?[0])?,
            args: fields(f[1])?
                .into_iter()
                .map(text)
                .collect::<Result<_, _>>()?,
            directory,
            env,
            own_group: match f.get(4) {
                Some(Value::Bool(own_group)) => *own_group,
                _ => return Err(invalid()),
            },
        }))
    }
    /// An `Option[Deadline]` value: its clock handle and the Instant's ticks.
    fn deadline(&self, v: &Value) -> Result<Option<hirsel_rt::process::Deadline>, Error> {
        Ok(match tag(v)? {
            0 => None,
            _ => {
                let deadline = fields(fields(v)?[0])?;
                let Value::Handle { kind, id: ticks } = deadline[1] else {
                    return Err(invalid());
                };
                if kind != "Instant" {
                    return Err(invalid());
                }
                Some(hirsel_rt::process::Deadline {
                    clock: self.id(deadline[0])?,
                    ticks: *ticks,
                })
            }
        })
    }
    fn process_run_request(&self, name: &str, a: &[Value]) -> Result<Option<R>, Error> {
        let (process, stdin, max_output) = (self.id(&a[0])?, bytes(&a[2])?, word(&a[3])?);
        let deadline = self.deadline(&a[4])?;
        Ok(Some(if name == "proc.run" {
            match self.command(&a[1])? {
                Some(command) => R::ProcessRun {
                    process,
                    command,
                    stdin,
                    max_output,
                    deadline,
                },
                None => return Ok(None),
            }
        } else {
            match fields(&a[1])?
                .into_iter()
                .map(|stage| self.command(stage))
                .collect::<Result<Option<Vec<_>>, _>>()?
            {
                Some(stages) => R::ProcessPipeline {
                    process,
                    stages,
                    stdin,
                    max_output,
                    deadline,
                },
                None => return Ok(None),
            }
        }))
    }
    /// Issue `proc.run` or `proc.pipeline` under `token`: its completion, or Blocked while
    /// the children run.
    fn process_request(
        &mut self,
        name: &str,
        a: &[Value],
        token: u64,
    ) -> Result<OperationResult, Error> {
        let Some(request) = self.process_run_request(name, a)? else {
            return Ok(OperationResult::returned(if name == "proc.run" {
                process_value(ProcessOutcome::Error(ProcessFailure::InvalidInput))
            } else {
                pipeline_value(PipelineOutcome::Error(ProcessFailure::InvalidInput))
            }));
        };
        self.io.adapter().enter(token);
        let cancellation = self
            .library
            .cancellation()
            .filter(|_| self.io.cancellable(token));
        let completion = self.host_cancelable(request, cancellation);
        if !matches!(completion, Err(Error::Blocked(_))) {
            self.io.adapter().finish(token);
        }
        match completion? {
            C::Process(outcome) => Ok(OperationResult::returned(process_value(outcome))),
            C::Pipeline(outcome) => Ok(OperationResult::returned(pipeline_value(outcome))),
            _ => Err(invalid()),
        }
    }
    fn storage_request(&self, name: &str, a: &[Value]) -> Result<R, Error> {
        let path = |v: &Value| text(fields(v)?[0]);
        Ok(match name {
            "io.dir" => R::Directory {
                io: 1,
                path: path(&a[1])?,
            },
            "dir.narrow" => R::NarrowDir {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "dir.read_only" => R::ReadOnlyDir {
                directory: self.id(&a[0])?,
            },
            "file.open" => R::Open {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "file.open_append" => R::OpenAppend {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "file.create" => R::Create {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "file.read" => R::FileRead {
                directory: self.id(&a[0])?,
                file: self.id(&a[1])?,
                max: word(&a[3])?,
            },
            "file.seek" => R::FileSeek {
                directory: self.id(&a[0])?,
                file: self.id(&a[1])?,
                offset: word(&a[2])?,
            },
            "file.write" => R::FileWrite {
                directory: self.id(&a[0])?,
                file: self.id(&a[1])?,
                bytes: bytes(&a[2])?,
            },
            "file.append" => R::FileAppend {
                directory: self.id(&a[0])?,
                file: self.id(&a[1])?,
                bytes: bytes(&a[2])?,
            },
            "file.narrow" => R::NarrowFile {
                directory: self.id(&a[0])?,
                file: self.id(&a[1])?,
            },
            "file.walk" => R::ReadDirPage {
                directory: self.id(&a[0])?,
                path: String::new(),
                after: String::new(),
                recursive: true,
            },
            "file.read_dir" => R::ReadDirPage {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
                after: String::new(),
                recursive: false,
            },
            "file.metadata" => R::Metadata {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "file.mkdir" => R::Mkdir {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "file.set_permissions" => R::SetPermissions {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
                permissions: word(&a[2])? as u32,
            },
            "dir.remove_all" => R::RemoveAll {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "file.remove_dir" => R::RemoveDir {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "file.rename" => R::Rename {
                directory: self.id(&a[0])?,
                from: path(&a[1])?,
                to: path(&a[2])?,
            },
            "file.size" => R::Size {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "file.read_bytes" => R::Read {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "file.write_bytes" => R::Write {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
                bytes: bytes(&a[2])?,
            },
            "file.remove" => R::Remove {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "file.create_atomic" => R::CreateAtomic {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "file.map" => R::Map {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
            },
            "file.watch" => R::Watch {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
                recursive: matches!(a[2], Value::Bool(true)),
            },
            "file.write_all" => R::WriteAll {
                directory: self.id(&a[0])?,
                file: self.id(&a[1])?,
                bytes: bytes(&a[2])?,
            },
            "file.commit" => R::Commit {
                directory: self.id(&a[0])?,
                file: self.id(&a[1])?,
            },
            "file.sync" => R::Sync {
                directory: self.id(&a[0])?,
                file: self.id(&a[1])?,
            },
            "file.put_new" => R::PutNew {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
                bytes: bytes(&a[2])?,
                durable: matches!(a[3], Value::Variant { variant: 0, .. }),
            },
            "io.monotonic_clock" => R::MonotonicClock { io: 1 },
            "io.wall_clock" => R::WallClock { io: 1 },
            "clock.monotonic" => R::Monotonic {
                clock: self.id(&a[0])?,
            },
            "clock.wall" => R::Wall {
                clock: self.id(&a[0])?,
            },
            _ => return Err(invalid()),
        })
    }
    /// A snapshot read can park just like a network read. Keep its continuation alive
    /// so siblings can release it or cancel it; Blocked is not a terminal run outcome.
    fn host_file_read(
        &mut self,
        a: &mut [Value],
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.yield_now = true;
        if self.free_ready.is_some() {
            return self.storage("file.read", a);
        }
        let token = self.next_wait;
        self.next_wait += 1;
        match self.file_read_request(a, token) {
            Err(Error::Blocked(_)) => {
                self.waits.insert(
                    token,
                    HostWait {
                        task: self.library.task_id(),
                        name: "file.read".into(),
                        arguments: a.to_vec(),
                    },
                );
                Ok(OperationResult::Park {
                    token,
                    wait: hirsel_interp::BlockedWait {
                        spawn_path: self.library.task_path(),
                        anchor: cx.anchor().cloned().ok_or_else(invalid)?,
                        kind: "file.read".into(),
                        targets: vec![],
                    },
                })
            }
            result => result,
        }
    }
    fn file_read_request(&mut self, a: &mut [Value], token: u64) -> Result<OperationResult, Error> {
        self.io.adapter().enter(token);
        let result = self.storage("file.read", a);
        if !matches!(result, Err(Error::Blocked(_))) {
            self.io.adapter().finish(token);
        }
        result
    }
    fn storage(&mut self, name: &str, a: &mut [Value]) -> Result<OperationResult, Error> {
        let iterator_path = if name == "file.read_dir" {
            text(fields(&a[1])?[0])?
        } else {
            String::new()
        };
        // A parked file read already spent its suspension at the initial call.
        // Readiness retries can remain blocked without adding a scheduling point.
        if name != "file.read" {
            self.yield_now = true;
        }
        let request = self.storage_request(name, a)?;
        let completion = self.host(request)?;
        let mut updates = vec![];
        let value = match completion {
            C::Handle(r) => match r {
                Ok(id) => ok(self.handle(
                    id,
                    match name {
                        "file.open" | "file.create" | "file.open_append" => "File",
                        "file.create_atomic" => "AtomicFile",
                        "file.map" => "Mapped",
                        "file.watch" => "Watcher",
                        _ => "Dir",
                    },
                )),
                Err(e) => file_error(e),
            },
            C::OptionalFile(id) => match id {
                Some(id) => hirsel_interp::library::some(self.handle(id, "File")),
                None => hirsel_interp::library::none(),
            },
            C::Unit(r) => match r {
                Ok(()) => ok(Value::Unit),
                Err(e) => file_error(e),
            },
            C::Size(r) => match r {
                Ok(n) => ok(u64_value(n)),
                Err(e) => file_error(e),
            },
            C::Bytes(r) => match r {
                Ok(b) if name == "file.read" => {
                    let n = b.len() as u64;
                    let mut buffer = std::mem::replace(&mut a[2], Value::Unit);
                    let Value::Sequence { elements, .. } = &mut buffer else {
                        return Err(invalid());
                    };
                    elements.extend(b.into_iter().map(|b| Some(int(Primitive::U8, b))));
                    updates.push((2, buffer));
                    ok(u64_value(n))
                }
                Ok(b) => ok(Value::Bytes(b)),
                Err(e) => file_error(e),
            },
            C::Metadata(r) => match r {
                Ok(meta) => ok(Value::Record {
                    definition: (&DefPath::core("Metadata")).into(),
                    fields: vec![
                        Some(variant(meta.kind as u32, vec![])),
                        Some(u64_value(meta.size)),
                        Some(meta.modified_unix_ns.map_or_else(
                            || variant(0, vec![]),
                            |n| variant(1, vec![int(Primitive::I128, n as u128)]),
                        )),
                        Some(int(Primitive::U32, meta.permissions)),
                    ],
                }),
                Err(e) => file_error(e),
            },
            C::Entries(r) => match r {
                Ok(entries) if name == "file.walk" || name == "file.read_dir" => {
                    ok(hirsel_interp::library::walk::iterator(
                        a[0].clone(),
                        &iterator_path,
                        entries
                            .into_iter()
                            .map(|e| hirsel_interp::library::walk::Entry {
                                path: e.path,
                                kind: e.kind as u32,
                            })
                            .collect(),
                        name == "file.walk",
                    ))
                }
                Ok(entries) => ok(sequence(
                    BuiltinType::List,
                    entries.into_iter().map(|entry| Value::Record {
                        definition: (&DefPath::core("DirEntry")).into(),
                        fields: vec![
                            Some(hirsel_interp::library::path(&entry.path)),
                            Some(variant(entry.kind as u32, vec![])),
                        ],
                    }),
                )),
                Err(e) => file_error(e),
            },
            C::ClockGrant(r) => match r {
                Ok(id) => ok(self.handle(
                    id,
                    if name == "io.monotonic_clock" {
                        "MonotonicClock"
                    } else {
                        "WallClock"
                    },
                )),
                Err(()) => error(variant(0, vec![])),
            },
            C::Monotonic(r) => match r {
                Ok(ticks) => ok(Value::Handle {
                    kind: "Instant".into(),
                    id: ticks,
                }),
                Err(e) => clock_error(e),
            },
            C::Wall(r) => match r {
                Ok(ticks) => ok(int(Primitive::I64, ticks as u64)),
                Err(e) => clock_error(e),
            },
            _ => return Err(invalid()),
        };
        let consumed = if matches!(name, "file.narrow" | "file.commit") {
            if let Value::Handle { id, .. } = a[1] {
                self.handles.remove(&id);
            }
            vec![1]
        } else {
            vec![]
        };
        Ok(OperationResult::Complete {
            value,
            arguments: updates,
            consumed,
            released: vec![],
        })
    }
    fn host_network(
        &mut self,
        name: &str,
        a: &[Value],
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.yield_now = true;
        if self.free_ready.is_some() {
            return self.network(name, a, 0, cx);
        }
        let token = self.next_wait;
        self.next_wait += 1;
        match self.try_network(name, a, token, cx) {
            Err(Error::Blocked(_)) => {
                self.waits.insert(
                    token,
                    HostWait {
                        task: self.library.task_id(),
                        name: name.into(),
                        arguments: a.to_vec(),
                    },
                );
                Ok(OperationResult::Park {
                    token,
                    wait: hirsel_interp::BlockedWait {
                        spawn_path: self.library.task_path(),
                        anchor: cx.anchor().cloned().ok_or_else(invalid)?,
                        kind: name.into(),
                        targets: vec![],
                    },
                })
            }
            result => result,
        }
    }
    fn try_network(
        &mut self,
        name: &str,
        a: &[Value],
        token: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.io.adapter().enter(token);
        let result = self.network(name, a, token, cx);
        if !matches!(result, Err(Error::Blocked(_))) {
            self.io.adapter().finish(token);
        }
        result
    }
    /// Pure request construction, shared by ordinary conversion and Free replay.
    fn network_request(&self, name: &str, a: &[Value]) -> Result<Option<R>, Error> {
        let name = network_name(name);
        Ok(Some(match name {
            "io.net" => R::Net { io: 1 },
            "net.narrow" => R::NetNarrow {
                net: self.id(&a[0])?,
                rules: fields(&a[1])?
                    .into_iter()
                    .map(|v| {
                        let f = fields(v)?;
                        Ok(NetRule {
                            address: text(f[0])?,
                            port: word(f[1])?.try_into().map_err(|_| invalid())?,
                            operation: match tag(f[2])? {
                                0 => NetOperation::Connect,
                                1 => NetOperation::ConnectTls,
                                2 => NetOperation::Listen,
                                3 => NetOperation::UdpBind,
                                4 => NetOperation::UdpPeer,
                                5 => NetOperation::ListenTls,
                                6 => NetOperation::UnixConnect,
                                7 => NetOperation::UnixListen,
                                8 => NetOperation::UnixListenRemoveStale,
                                _ => return Err(invalid()),
                            },
                        })
                    })
                    .collect::<Result<_, Error>>()?,
            },
            "net.resolve" => R::Resolve {
                net: self.id(&a[0])?,
                host: text(&a[1])?,
                port: word(&a[2])?.try_into().map_err(|_| invalid())?,
            },
            "net.connect" => R::Connect {
                net: self.id(&a[0])?,
                host: text(&a[1])?,
                port: word(&a[2])?.try_into().map_err(|_| invalid())?,
                timeout: timeout(&a[3])?,
            },
            "net.connect_addr" => R::ConnectAddr {
                net: self.id(&a[0])?,
                addr: addr(&a[1])?,
                timeout: timeout(&a[2])?,
            },
            "net.udp_bind" => R::UdpBind {
                net: self.id(&a[0])?,
                addr: addr(&a[1])?,
            },
            "udp.send_to" => R::UdpSend {
                net: self.id(&a[0])?,
                socket: self.id(&a[1])?,
                bytes: bytes(&a[2])?,
                addr: addr(&a[3])?,
            },
            "udp.recv_from" => R::UdpRecv {
                net: self.id(&a[0])?,
                socket: self.id(&a[1])?,
                max: word(&a[3])?,
            },
            "net.listen" => R::Listen {
                net: self.id(&a[0])?,
                addr: addr(&a[1])?,
            },
            "net.unix_connect" => R::UnixConnect {
                net: self.id(&a[0])?,
                path: text(fields(&a[1])?[0])?,
            },
            "net.unix_listen" => R::UnixListen {
                net: self.id(&a[0])?,
                path: text(fields(&a[1])?[0])?,
            },
            "net.accept" => R::Accept {
                net: self.id(&a[0])?,
                listener: self.id(&a[1])?,
            },
            "net.local_addr" => R::LocalAddr {
                net: self.id(&a[0])?,
                listener: self.id(&a[1])?,
            },
            "net.peer_addr" => R::PeerAddr {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
            },
            "net.read" | "net.read_into" => R::NetRead {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
                max: if name == "net.read" {
                    word(&a[3])?
                } else {
                    fields(&a[2])?.len() as u64
                },
            },
            "net.write" => R::NetWrite {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
                bytes: bytes(&a[2])?,
            },
            "net.write_all" => R::NetWriteAll {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
                bytes: bytes(&a[2])?,
            },
            "net.set_read_timeout" | "net.set_write_timeout" => R::SetTimeout {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
                which: if name == "net.set_read_timeout" {
                    TimeoutKind::Read
                } else {
                    TimeoutKind::Write
                },
                timeout: timeout(&a[2])?,
            },
            "net.shutdown" => R::Shutdown {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
                how: match tag(&a[2])? {
                    0 => ShutdownHow::Read,
                    1 => ShutdownHow::Write,
                    2 => ShutdownHow::Both,
                    _ => return Err(invalid()),
                },
            },
            "net.close" | "udp.close" => R::Close {
                handle: self.id(&a[1])?,
            },
            "write_half.shutdown" => R::Shutdown {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
                how: ShutdownHow::Write,
            },
            "net.split" => R::NetSplit {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
            },
            "net.join" => R::NetJoin {
                net: self.id(&a[0])?,
                read: self.id(&a[1])?,
                write: self.id(&a[2])?,
            },
            "tls.close" => R::TlsClose {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
            },
            "tls.peer" => R::TlsPeer {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
            },
            "tls.negotiated_protocol" => R::TlsProtocol {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
            },
            _ => return Ok(None),
        }))
    }
    fn tls_request(&self, name: &str, a: &[Value]) -> Result<(R, TlsSetup), Error> {
        Ok(match name {
            "tls.connect" | "tls.connect_as" | "tls.connect_with" => {
                let stream = self.id(&a[1])?;
                // A TlsClient carries the roots, an optional identity and the ALPN offer.
                let (roots, identity, alpn) = match name {
                    "tls.connect" => (&a[3], None, vec![]),
                    "tls.connect_as" => (&a[3], Some(&a[4]), vec![]),
                    _ => {
                        let client = fields(&a[3])?;
                        let identity = match tag(client[1])? {
                            0 => None,
                            1 => Some(fields(client[1])?[0]),
                            _ => return Err(invalid()),
                        };
                        let alpn = fields(client[2])?
                            .into_iter()
                            .map(text)
                            .collect::<Result<Vec<_>, _>>()?;
                        (client[0], identity, alpn)
                    }
                };
                let mut pem_roots = None;
                let roots = match tag(roots)? {
                    0 => RootsDigest::WebPki("webpki-roots@1.0.9".into()),
                    1 => {
                        use sha2::Digest;
                        let pem = bytes(fields(roots)?[0])?;
                        let digest = RootsDigest::Pem(sha2::Sha256::digest(&pem).into());
                        pem_roots = Some(pem);
                        digest
                    }
                    _ => return Err(invalid()),
                };
                let mut identity_bytes = None;
                let identity = match identity {
                    Some(identity) => {
                        let identity = fields(identity)?;
                        let (chain, key) = (bytes(identity[0])?, bytes(identity[1])?);
                        let digest = hirsel_rt::net_tls::chain_digest(&chain);
                        identity_bytes = Some((chain, key));
                        Some(digest)
                    }
                    None => None,
                };
                (
                    R::TlsConnect {
                        net: self.id(&a[0])?,
                        stream,
                        server_name: text(&a[2])?,
                        roots,
                        identity,
                        alpn,
                    },
                    TlsSetup::Client {
                        stream,
                        roots: pem_roots,
                        identity: identity_bytes,
                    },
                )
            }
            "tls.accept" => {
                let server = fields(&a[2])?;
                let identities = fields(server[0])?
                    .iter()
                    .map(|identity| {
                        let identity = fields(identity)?;
                        Ok((bytes(identity[0])?, bytes(identity[1])?))
                    })
                    .collect::<Result<Vec<_>, Error>>()?;
                let clients = match tag(server[1])? {
                    0 => ClientAuth::Anonymous,
                    t @ (1 | 2) => {
                        let trust = bytes(fields(server[1])?[0])?;
                        if t == 1 {
                            ClientAuth::Request(trust)
                        } else {
                            ClientAuth::Require(trust)
                        }
                    }
                    _ => return Err(invalid()),
                };
                let alpn = fields(server[2])?
                    .into_iter()
                    .map(text)
                    .collect::<Result<Vec<_>, _>>()?;
                let stream = self.id(&a[1])?;
                let refs: Vec<_> = identities.iter().map(|(c, k)| (&c[..], &k[..])).collect();
                let digest = hirsel_rt::net::TlsServerDigest::of(
                    &refs,
                    clients.as_ref().map(|v| &v[..]),
                    &alpn,
                );
                (
                    R::TlsAccept {
                        net: self.id(&a[0])?,
                        stream,
                        server: digest,
                    },
                    TlsSetup::Server {
                        stream,
                        identities,
                        clients,
                        alpn,
                    },
                )
            }
            _ => return Err(Error::Unavailable(format!("host network {name}"))),
        })
    }
    fn network(
        &mut self,
        name: &str,
        a: &[Value],
        token: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let name = network_name(name);
        let request = match self.network_request(name, a)? {
            Some(request) => request,
            None => {
                let (request, setup) = self.tls_request(name, a)?;
                if self.free_ready.is_none() {
                    setup.install(&mut self.io);
                }
                request
            }
        };
        let cancellation = self
            .library
            .cancellation()
            .filter(|_| self.io.cancellable(token));
        let printed = self.io.stdout().len();
        let completion = self.host_cancelable(request, cancellation)?;
        let mut updates = vec![];
        let value = match completion {
            C::Handle(r) => match r {
                Ok(id) => ok(self.handle(id, "Net")),
                Err(_) => error(variant(0, vec![])),
            },
            C::NetHandle(r) => result(r, |id| {
                self.handle(
                    id,
                    if name == "net.udp_bind" {
                        "UdpSocket"
                    } else if name == "net.narrow" {
                        "Net"
                    } else {
                        "Stream"
                    },
                )
            }),
            C::Datagram(r) => match r {
                Err(e) => net_error(e),
                Ok((b, peer, truncated)) => {
                    let count = b.len() as u64;
                    let mut buffer = bytes(&a[2])?;
                    buffer.extend_from_slice(&b);
                    updates.push((
                        2,
                        sequence(
                            BuiltinType::List,
                            buffer.into_iter().map(|b| int(Primitive::U8, b)),
                        ),
                    ));
                    ok(Value::Record {
                        definition: (&DefPath::core("UdpReceived")).into(),
                        fields: vec![
                            Some(u64_value(count)),
                            Some(address(peer)),
                            Some(Value::Bool(truncated)),
                        ],
                    })
                }
            },
            C::Listened(r) => result(r, |(id, _)| self.handle(id, "Listener")),
            C::Accepted(r) => result(r, |(id, addr)| {
                Value::Tuple(vec![Some(self.handle(id, "Stream")), Some(address(addr))])
            }),
            C::Address(r) => result(r, address),
            C::Halves(Ok((read, write))) => Value::Tuple(vec![
                Some(self.handle(read, "ReadHalf")),
                Some(self.handle(write, "WriteHalf")),
            ]),
            C::Addresses(r) => result(r, |a| {
                sequence(BuiltinType::List, a.into_iter().map(address))
            }),
            C::Count(r) => result(r, u64_value),
            C::Protocol(r) if name == "tls.negotiated_protocol" => result(r, |p| match p {
                Some(p) => variant(1, vec![Value::String(p)]),
                None => variant(0, vec![]),
            }),
            C::Protocol(r) => result(r, |_| a[1].clone()),
            C::Certificates(r) => result(r, |chain| match chain {
                Some(chain) => variant(1, vec![Value::Bytes(chain)]),
                None => variant(0, vec![]),
            }),
            C::NetUnit(r) => result(r, |()| Value::Unit),
            C::NetBytes(r) => match r {
                Err(e) => net_error(e),
                Ok(b) => {
                    let n = b.len() as u64;
                    let mut buf = bytes(&a[2])?;
                    if name == "net.read_into" {
                        buf.get_mut(..b.len())
                            .ok_or_else(invalid)?
                            .copy_from_slice(&b);
                    } else {
                        buf.extend_from_slice(&b);
                    }
                    updates.push((
                        2,
                        sequence(
                            if name == "net.read_into" {
                                BuiltinType::Inline
                            } else {
                                BuiltinType::List
                            },
                            buf.into_iter().map(|b| int(Primitive::U8, b)),
                        ),
                    ));
                    ok(u64_value(n))
                }
            },
            C::Unit(Ok(())) => Value::Unit,
            C::Checked(false) => {
                cx.stdout.extend_from_slice(&self.io.stdout()[printed..]);
                if matches!(
                    name,
                    "net.close"
                        | "tls.close"
                        | "tls.connect"
                        | "tls.connect_as"
                        | "tls.connect_with"
                        | "tls.accept"
                ) && let Value::Handle { id, .. } = a[1]
                {
                    self.handles.remove(&id);
                }
                if name == "net.join" {
                    for v in &a[1..3] {
                        if let Value::Handle { id, .. } = v {
                            self.handles.remove(id);
                        }
                    }
                }
                return hirsel_interp::library::process::script_failure(
                    cx,
                    "scripted net client unmet",
                );
            }
            _ => return Err(invalid()),
        };
        if name == "net.join" {
            for v in &a[1..3] {
                if let Value::Handle { id, .. } = v {
                    self.handles.remove(id);
                }
            }
        }
        if (name == "net.close"
            || name == "udp.close"
            || name == "net.split"
            || name == "tls.close"
            || matches!(
                name,
                "tls.connect" | "tls.connect_as" | "tls.connect_with" | "tls.accept"
            ) && matches!(value, Value::Variant { variant: 1, .. }))
            && let Value::Handle { id, .. } = a[1]
        {
            self.handles.remove(&id);
        }
        let consumed = if name == "net.join" {
            vec![1, 2]
        } else if matches!(
            name,
            "net.close"
                | "udp.close"
                | "net.split"
                | "tls.close"
                | "tls.connect"
                | "tls.connect_as"
                | "tls.connect_with"
                | "tls.accept"
        ) {
            vec![1]
        } else {
            vec![]
        };
        Ok(OperationResult::Complete {
            value,
            arguments: updates,
            consumed,
            released: vec![],
        })
    }
}
// Provider-local ids retain the library token in their low bits. A host route
// is selected once; handle-dependent host/simulation choice remains per call.
const HOST_ROUTE_SHIFT: usize = 16;
#[derive(Clone, Copy)]
#[repr(usize)]
enum HostRoute {
    Library,
    Input,
    Random,
    Signal,
    Telemetry,
    Runner,
    Sleep,
    Projection,
    Watch,
    Mapped,
    Storage,
    Process,
    NetProjection,
    Network,
}
fn host_route(name: &str) -> HostRoute {
    match name {
        "io.stdin" | "io.stderr" | "io.stdout" | "io.random" | "io.tasks" | "io.signals"
        | "stdin.read" | "err.print" | "err.write" | "out.print" | "out.write" | "env.get"
        | "env.narrow" => HostRoute::Input,
        "random.bytes" => HostRoute::Random,
        "signal.wait" => HostRoute::Signal,
        "io.telemetry" => HostRoute::Telemetry,
        "runner.compute" | "runner.call" | "runner.spawn" => HostRoute::Runner,
        "clock.sleep_until" => HostRoute::Sleep,
        "io.dir" | "io.monotonic_clock" | "io.wall_clock" => HostRoute::Projection,
        "watcher.next" => HostRoute::Watch,
        "mapped.with" => HostRoute::Mapped,
        "io.net" => HostRoute::NetProjection,
        "io.process" => HostRoute::Process,
        "dir.narrow" | "dir.read_only" | "dir.remove_all" => HostRoute::Storage,
        _ if name.starts_with("capture.") => HostRoute::Telemetry,
        _ if name.starts_with("file.") || name.starts_with("clock.") => HostRoute::Storage,
        _ if name.starts_with("proc.")
            || name.starts_with("child.")
            || name.starts_with("group.") =>
        {
            HostRoute::Process
        }
        _ if name.starts_with("net.")
            || name.starts_with("tls.")
            || name.starts_with("udp.")
            || name.starts_with("read_half.")
            || name.starts_with("write_half.") =>
        {
            HostRoute::Network
        }
        _ => HostRoute::Library,
    }
}
impl hirsel_interp::OperationsAdapter for HostLibrary {
    fn stop_at_tooling_limit(&self) -> bool {
        matches!(self.io, HostIo::Replay(_))
    }
    fn resume(
        &mut self,
        continuation: u64,
        result: OperationResult,
        arguments: &mut [Value],
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let Some((directory, mapped)) = self.mapped.remove(&continuation) else {
            return self
                .library
                .resume(continuation, result, arguments, context);
        };
        if !matches!(result, OperationResult::Complete { .. }) {
            return Ok(result);
        }
        self.yield_now = true;
        if self.free.is_some() {
            return self.free_mapped_check(directory, mapped, result, context);
        }
        let completion = self.host(R::MapCheck { directory, mapped })?;
        mapped_checked(result, completion)
    }
    fn entry_returned(
        &mut self,
        value: &Value,
        runtime: &hirsel_interp::aggregate::RuntimeTypes,
        context: &mut OperationContext<'_>,
    ) -> Result<bool, Error> {
        use std::io::Write;
        self.echo(context.stdout)?;
        if self.emit_output {
            std::io::stdout()
                .flush()
                .map_err(|e| Error::Unavailable(format!("host stdout: {e}")))?;
        }
        let consume = self.library.entry_returned(value, runtime, context)?;
        if let Some(report) = self.library.exit_report.take()
            && self.emit_output
        {
            std::io::stderr()
                .write_all(report.text.as_bytes())
                .and_then(|()| std::io::stderr().flush())
                .map_err(|e| Error::Unavailable(format!("host exit report: {e}")))?;
        }
        Ok(consume)
    }

    fn operations(&self) -> &dyn Operations {
        &self.library
    }
    fn operations_mut(&mut self) -> &mut dyn Operations {
        &mut self.library
    }
    /// A host wait completes only once ready or cancelled. One parked by a task already
    /// cancelled is polled at every poll: no later cancellation reports it.
    fn watched(&self, token: u64) -> bool {
        self.waits.contains_key(&token) && self.library.cancellation().is_none()
    }
    /// The host requests this point found ready; every wait after a cancellation, which
    /// completes a cancellable wait whether or not it is ready.
    fn woken(&mut self) -> hirsel_interp::Woken {
        let cancellations = self.library.cancellations();
        if std::mem::replace(&mut self.cancellations, cancellations) != cancellations {
            return hirsel_interp::Woken::Every;
        }
        match &self.ready {
            Ready::Every => hirsel_interp::Woken::Every,
            Ready::Tokens(tokens) => hirsel_interp::Woken::Tokens(tokens.clone()),
        }
    }
    fn wait_deadline(&self, token: u64) -> Option<u64> {
        match self.waits.get(&token) {
            Some(wait) if wait.name == "clock.sleep_until" => match wait.arguments.get(1) {
                Some(Value::Handle { kind, id }) if kind == "Instant" => Some(*id),
                _ => None,
            },
            Some(_) => None,
            None => self.library.wait_deadline(token),
        }
    }
    /// The policy's choice, through the tape: recorded live, taken from the recording in
    /// replay, as the native scheduler does.
    fn choose_task(&mut self, runnable: &[u64]) -> Result<usize, Error> {
        let choice = self.library.choose_task(runnable)?;
        if self.free.is_some() || !self.library.schedule_recorded(runnable.len()) {
            return Ok(choice);
        }
        let count = runnable.len() as u64;
        match self.io.adapter().schedule(count, choice as u64) {
            Ok(choice) if choice < count => Ok(choice as usize),
            Ok(_) => Err(invalid()),
            Err(e) => {
                let message = format!("{e:?}");
                self.refused.get_or_insert(message.clone());
                Err(Error::Unavailable(format!("host scheduling: {message}")))
            }
        }
    }
    fn take_yield(&mut self) -> bool {
        let host = std::mem::take(&mut self.yield_now);
        self.library.take_yield() || host
    }
    /// A scheduling point: a scope that exited first removes its timer and records nothing,
    /// the adapter reports the parked requests that are ready (asked once, while a host
    /// request is parked or a deadline armed), and the due scope deadlines expire by
    /// deadline, ties in entry order.
    fn expire_deadlines(&mut self, cx: &mut OperationContext<'_>) -> Result<(), Error> {
        if self.free.is_some() {
            self.free_runner_children(cx)?;
        }
        self.remote_deliveries();
        let mut armed = std::mem::take(&mut self.armed);
        armed.retain(|(token, scope, _)| {
            let live = self.library.deadline_pending(*scope);
            if !live {
                self.io.adapter().finish(*token);
            }
            live
        });
        self.ready = if self.waits.is_empty()
            && armed.is_empty()
            && !self.runner_waits()
            && self.free_waits.is_empty()
            && self.free_drops.is_empty()
            && self.free_checks.is_empty()
            && self.free_runners.is_empty()
            && !self.io.adapter().process_sources()
        {
            Ready::Tokens(Vec::new())
        } else {
            self.io
                .adapter()
                .ready()
                .map_err(|e| Error::Unavailable(format!("host readiness: {e:?}")))?
        };
        armed.sort_by_key(|(token, _, request)| match request {
            R::SleepUntil { deadline, .. } => (*deadline, *token),
            _ => (u64::MAX, *token),
        });
        for (token, scope, request) in armed {
            if !self.ready.contains(token) {
                self.armed.push((token, scope, request));
                continue;
            }
            self.io.adapter().enter(token);
            match self.scope_timer(request.clone()) {
                Err(Error::Blocked(_)) => self.armed.push((token, scope, request)),
                completion => {
                    self.io.adapter().finish(token);
                    let C::Wait(_) = completion? else {
                        return Err(invalid());
                    };
                    self.library.expire_deadline(scope, cx)?;
                }
            }
        }
        let mut canceled: Vec<u64> = self
            .waits
            .values()
            .filter(|wait| self.library.task_cancellation(wait.task).is_some())
            .filter_map(|wait| {
                if !matches!(wait.name.as_str(), "child.wait" | "proc.wait_group") {
                    return None;
                }
                let Value::Handle { id, .. } = wait.arguments.get(1)? else {
                    return None;
                };
                self.handles.get(id).copied()
            })
            .collect();
        canceled.sort_unstable();
        canceled.dedup();
        let printed = self.io.stdout().len();
        let batch = match self.io.adapter().process_batch(&canceled) {
            Err(hirsel_rt::io::AdapterError::ProcessFailed { child, .. }) => {
                cx.stdout.extend_from_slice(&self.io.stdout()[printed..]);
                let (anchor, spawn_path) = self
                    .process_sites
                    .get(&child)
                    .cloned()
                    .ok_or_else(invalid)?;
                let failure = hirsel_interp::Failure {
                    rule: hirsel_diag::RuleId::new("LANG-9").unwrap(),
                    line: cx.sources.location(anchor.site).map_or(0, |p| p.line),
                    site: anchor.site,
                    kind: "scripted child unmet".into(),
                };
                return Err(Error::ProviderFailed(Box::new(
                    hirsel_interp::observation::FailureRecord {
                        failure,
                        anchor,
                        operation: None,
                        cause: None,
                        spawn_path,
                    },
                )));
            }
            Err(e) => return Err(Error::Unavailable(format!("host batch: {e:?}"))),
            Ok(ready) => ready,
        };
        self.ready = match (
            std::mem::replace(&mut self.ready, Ready::Tokens(Vec::new())),
            batch,
        ) {
            (Ready::Every, _) | (_, Ready::Every) => Ready::Every,
            (Ready::Tokens(mut a), Ready::Tokens(b)) => {
                a.extend(b);
                a.sort_unstable();
                a.dedup();
                Ready::Tokens(a)
            }
        };
        if let Some(codec) = &mut self.free {
            codec.observe_ready(&self.ready);
        }
        // Keep entry order for the next poll's ties.
        self.armed.sort_by_key(|(token, _, _)| *token);
        self.runner_children(cx)
    }
    fn wait(&mut self) -> Result<bool, Error> {
        if self.library.free_replay() {
            return Ok(false);
        }
        self.remote_deliveries();
        self.io.adapter().wait().map_err(|e| match e {
            hirsel_rt::io::AdapterError::Signaled(kind) => Error::Signaled(kind),
            e => Error::Unavailable(format!("host readiness: {e:?}")),
        })
    }
    fn ready_now(&mut self) -> Result<bool, Error> {
        if self.free.is_some() {
            // Retry the parked set after actual replay publication, even if
            // the publishing request still waits on its next key epoch.
            let progressed = std::mem::take(&mut self.free_progressed);
            let ready = match &mut self.io {
                HostIo::Live(io) => {
                    self.free.as_ref().unwrap().has_ready()
                        || io
                            .has_ready()
                            .map_err(|e| Error::Unavailable(format!("host readiness: {e:?}")))?
                }
                HostIo::Replay(_) => false,
            };
            return Ok(progressed || ready);
        }
        self.remote_deliveries();
        self.io
            .adapter()
            .has_ready()
            .map_err(|e| Error::Unavailable(format!("host readiness: {e:?}")))
    }
    fn take_calls(&mut self) -> Vec<hirsel_interp::CalleeStart> {
        self.runner_calls()
    }
    fn take_cancels(&mut self) -> Vec<u64> {
        if self.free.is_some() {
            return Vec::new();
        }
        self.runner_cancels()
    }
    fn call_ended(
        &mut self,
        token: u64,
        end: hirsel_interp::CalleeEnd,
        cx: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        self.runner_ended(token, end, cx)
    }
    fn callee_ended(
        &mut self,
        end: &hirsel_interp::CalleeEnd,
        _runtime: &hirsel_interp::aggregate::RuntimeTypes,
        cx: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        self.callee_finished(end, cx)
    }
    fn stops_itself(&self) -> bool {
        self.runners.crash.is_some()
    }
    fn parked(&mut self, token: u64) {
        if self.waits.contains_key(&token) || self.waits_on_runner(token) {
            self.io.adapter().parked(token);
        } else {
            self.library.parked(token);
        }
    }
    fn poll(
        &mut self,
        token: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        if self.free_runners.contains_key(&token) {
            return self.free_poll_runner(token, cx);
        }
        if self.free_drops.contains_key(&token) {
            return self.free_poll_drop(token, cx);
        }
        if self.free_checks.contains_key(&token) {
            return self.free_poll_check(token, cx);
        }
        if self.free_waits.contains_key(&token) {
            return self.free_poll(token, cx);
        }
        if self.waits_on_runner(token) {
            return self.runner_poll(token, cx);
        }
        let Some(wait) = self.waits.get(&token) else {
            if let Some(request) = self.library.cpu_pending_request(token)? {
                self.cpu_receipt(request)?;
            }
            return self.library.poll(token, cx);
        };
        let canceled = self.library.cancellation().is_some() && self.io.cancellable(token);
        if !canceled && !self.ready.contains(token) {
            return Ok(None);
        }
        let name = wait.name.clone();
        let arguments = wait.arguments.clone();
        if name == "file.read" {
            let mut arguments = arguments;
            return match self.file_read_request(&mut arguments, token) {
                Err(Error::Blocked(_)) => Ok(None),
                result => {
                    self.waits.remove(&token);
                    result.map(Some)
                }
            };
        }
        if name == "clock.sleep_until" {
            // Cancelling a pending wait removes its timer; nothing reaches the host.
            if canceled {
                self.io.adapter().finish(token);
                self.waits.remove(&token);
                return Ok(Some(OperationResult::returned(wait_canceled(
                    self.library.cancellation().unwrap(),
                ))));
            }
            return match self.sleep_request(&arguments, token) {
                Err(Error::Blocked(_)) => Ok(None),
                result => {
                    self.waits.remove(&token);
                    result.map(Some)
                }
            };
        }
        if name == "proc.run" || name == "proc.pipeline" {
            // Finishing a cancelled run's token kills and reaps its children.
            if canceled {
                self.io.adapter().finish(token);
                self.waits.remove(&token);
                let failure = ProcessFailure::Canceled(self.library.cancellation().unwrap());
                return Ok(Some(OperationResult::returned(if name == "proc.run" {
                    process_value(ProcessOutcome::Error(failure))
                } else {
                    pipeline_value(PipelineOutcome::Error(failure))
                })));
            }
            return match self.process_request(&name, &arguments, token) {
                Err(Error::Blocked(_)) => Ok(None),
                result => {
                    self.waits.remove(&token);
                    result.map(Some)
                }
            };
        }
        if name.starts_with("child.") || name.starts_with("group.") || name == "proc.wait_group" {
            if canceled {
                self.io.adapter().finish(token);
                self.waits.remove(&token);
                return self.child_request(&name, &arguments, None, cx).map(Some);
            }
            return match self.child_request(&name, &arguments, Some(token), cx) {
                Err(Error::Blocked(_)) => Ok(None),
                result => {
                    self.waits.remove(&token);
                    result.map(Some)
                }
            };
        }
        if name == "watcher.next" {
            // A canceled wait completes Canceled through the host's cancellation check.
            return match self.watch_request(&arguments, token) {
                Err(Error::Blocked(_)) => Ok(None),
                result => {
                    self.waits.remove(&token);
                    result.map(Some)
                }
            };
        }
        if name == "signal.wait" {
            // A canceled wait consumes nothing; the library reports Canceled.
            if canceled {
                self.io.adapter().finish(token);
            } else if !self.host_signal_wait(&arguments, token)? {
                return Ok(None);
            }
            self.waits.remove(&token);
            return self.library.wait_signal(&arguments, cx).map(Some);
        }
        if name == "stdin.read" {
            // A canceled read consumes nothing; the library reports Canceled.
            if canceled {
                self.io.adapter().finish(token);
            } else if !self.host_stdin(&arguments, token)? {
                return Ok(None);
            }
            self.waits.remove(&token);
            return self.library.read_stdin(&arguments, cx).map(Some);
        }
        match self.try_network(&name, &arguments, token, cx) {
            Err(Error::Blocked(_)) => Ok(None),
            result => {
                self.waits.remove(&token);
                result.map(Some)
            }
        }
    }
    fn resolve(&self, path: &hirsel_syntax::DefPath) -> Option<hirsel_interp::OperationId> {
        let mut operation = self.library.resolve(path)?;
        operation.index |= (host_route(operation.name) as usize) << HOST_ROUTE_SHIFT;
        Some(operation)
    }
    fn immediate(&self, operation: hirsel_interp::OperationId) -> bool {
        operation.index >> HOST_ROUTE_SHIFT == HostRoute::Library as usize
            && self.library.immediate(operation)
    }
    fn call(
        &mut self,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        match hirsel_interp::OperationsAdapter::resolve(self, call.path) {
            Some(operation) => {
                hirsel_interp::OperationsAdapter::call_resolved(self, operation, call, cx)
            }
            None => Ok(None),
        }
    }
    fn call_resolved(
        &mut self,
        mut operation: hirsel_interp::OperationId,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let route = operation.index >> HOST_ROUTE_SHIFT;
        operation.index &= (1 << HOST_ROUTE_SHIFT) - 1;
        let name = operation.name;
        if name == "cpu.map_scoped" {
            let request = self.library.cpu_admission_request(call.arguments)?;
            self.cpu_receipt(request)?;
        }
        if name == "iter.next"
            && hirsel_interp::library::walk::directory(&call.arguments[0])
                .is_some_and(|directory| self.id(directory).is_ok())
        {
            return self.walk_next(call, cx, None).map(Some);
        }
        if self.free.is_some()
            && self.free_ready.is_none()
            && let Some(request) = self.free_request(name, call.arguments)?
        {
            return self.free_call(request, call, cx).map(Some);
        }
        if route == HostRoute::Input as usize {
            let result = self.host_input(&name, call, cx);
            self.echo(cx.stdout)?;
            return result;
        }
        if route == HostRoute::Random as usize {
            return self.host_random(call, cx);
        }
        if route == HostRoute::Signal as usize {
            return self.host_signal(call, cx);
        }
        if route == HostRoute::Telemetry as usize
            && (name == "io.telemetry"
                || call.arguments.first().is_some_and(|v| self.id(v).is_ok()))
        {
            return self.host_telemetry(&name, call.arguments).map(Some);
        }
        if route == HostRoute::Runner as usize {
            let result = self.library.call_resolved(operation, call, cx)?;
            if self.free.is_some() {
                return self.free_runner(result.ok_or_else(invalid)?, cx).map(Some);
            }
            self.runner_sends(cx)?;
            return Ok(result);
        }
        if route == HostRoute::Sleep as usize
            && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
        {
            return self.host_sleep(call.arguments, cx).map(Some);
        }
        if route == HostRoute::Projection as usize {
            if !matches!(call.arguments.first(),Some(Value::Handle {kind,..}) if kind=="Io") {
                return Err(invalid());
            }
            return self.storage(&name, call.arguments).map(Some);
        }
        if route == HostRoute::Watch as usize
            && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
        {
            return self.host_watch_next(call.arguments, cx).map(Some);
        }
        if route == HostRoute::Mapped as usize
            && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
        {
            return self.host_mapped_with(call.arguments).map(Some);
        }
        if route == HostRoute::Storage as usize
            && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
        {
            return if name == "file.read" {
                self.host_file_read(call.arguments, cx).map(Some)
            } else {
                self.storage(&name, call.arguments).map(Some)
            };
        }
        if route == HostRoute::Process as usize
            && (name == "io.process" || call.arguments.first().is_some_and(|v| self.id(v).is_ok()))
        {
            if name == "io.process" {
                // Validate the Io operand through the ordinary library before projecting it.
                self.library.call(
                    OperationCall {
                        runtime: call.runtime,
                        path: call.path,
                        generics: call.generics,
                        arguments: call.arguments,
                        result_type: call.result_type,
                        types: call.types,
                        site: call.site,
                    },
                    cx,
                )?;
            }
            return self.host_process(&name, call.arguments, cx).map(Some);
        }
        if route == HostRoute::NetProjection as usize {
            // Validate the Io operand through the ordinary library before projecting it.
            self.library.call(
                OperationCall {
                    runtime: call.runtime,
                    path: call.path,
                    generics: call.generics,
                    arguments: call.arguments,
                    result_type: call.result_type,
                    types: call.types,
                    site: call.site,
                },
                cx,
            )?;
            return self.host_network(&name, call.arguments, cx).map(Some);
        }
        if route == HostRoute::Network as usize
            && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
        {
            return self.host_network(&name, call.arguments, cx).map(Some);
        }
        let result = self.library.call_resolved(operation, call, cx);
        for (scope, clock, at) in self.library.take_deadlines() {
            // Native Free replay has no timer request or live readiness source.
            // Its cancellation checks and scope-exit expiry come from phases.
            if !self.library.free_replay() {
                self.arm_deadline(scope, &clock, at, cx)?;
            }
        }
        result
    }
    fn take_drop_wait(&mut self) -> Option<OperationResult> {
        self.pending_free_drop.take()
    }
    fn drop_handle(
        &mut self,
        kind: &str,
        id: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        if self.free.is_some()
            && matches!(
                kind,
                "Stream"
                    | "ReadHalf"
                    | "WriteHalf"
                    | "Listener"
                    | "UdpSocket"
                    | "File"
                    | "AtomicFile"
                    | "Child"
                    | "ChildGroup"
                    | "Mapped"
                    | "Watcher"
                    | "Span"
            )
            && let Some(&handle) = self.handles.get(&id)
        {
            if self.pending_free_drop.is_some() {
                return Err(invalid());
            }
            self.pending_free_drop = Some(self.free_drop(kind, id, handle, cx)?);
            return Ok(());
        }
        if matches!(
            kind,
            "Stream"
                | "ReadHalf"
                | "WriteHalf"
                | "Listener"
                | "UdpSocket"
                | "File"
                | "AtomicFile"
                | "Child"
                | "ChildGroup"
                | "Mapped"
                | "Watcher"
                | "Span"
        ) && let Some(handle) = self.handles.remove(&id)
        {
            // Dropping a scripted child closes its input, which can leave an Expect unmet.
            let printed = self.io.stdout().len();
            if self.host(R::Close { handle })? == C::Checked(false)
                && matches!(kind, "Child" | "ChildGroup")
            {
                cx.stdout.extend_from_slice(&self.io.stdout()[printed..]);
                self.library.fail_drop("scripted child unmet");
            }
            return Ok(());
        }
        self.library.drop_handle(kind, id, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canceled_host_sleep_keeps_its_scheduling_point_without_consuming_tape() {
        let run = hirsel_tir::RunConfiguration {
            entry: DefPath::core("main"),
            hash_seed: 0,
            inputs: vec![],
        };
        let admitted = hirsel_run_config::RunConfig::empty(
            [0; 32],
            "core@1/main".into(),
            hirsel_run_schema::LaunchKind::Main,
        )
        .validated()
        .unwrap();
        let mut host = HostLibrary::new(
            &run,
            &admitted,
            HostIo::Replay(SimIo::new(Default::default(), vec![])),
        )
        .unwrap();
        hirsel_interp::Operations::cancel_run(&mut host.library);
        let arguments = [
            host.handle(11, "MonotonicClock"),
            Value::Handle {
                kind: "Instant".into(),
                id: 100,
            },
        ];
        let result = host
            .host_sleep(
                &arguments,
                &mut OperationContext {
                    configuration: &run,
                    sources: &Default::default(),
                    stdout: &mut vec![],
                    trace: &mut vec![],
                    trace_mode: Default::default(),
                    anchor: None,
                },
            )
            .unwrap();
        assert_eq!(result, OperationResult::returned(wait_canceled(1)));
        assert!(hirsel_interp::OperationsAdapter::take_yield(&mut host));
        assert!(!hirsel_interp::OperationsAdapter::take_yield(&mut host));
        let HostIo::Replay(io) = &host.io else {
            unreachable!()
        };
        assert_eq!(io.position(), 0);
        io.finish().unwrap();
    }

    #[test]
    fn process_run_with_a_scripted_directory_returns_invalid_input_without_host_io() {
        let run = hirsel_tir::RunConfiguration {
            entry: DefPath::core("main"),
            hash_seed: 0,
            inputs: vec![],
        };
        let admitted = hirsel_run_config::RunConfig::empty(
            [0; 32],
            "core@1/main".into(),
            hirsel_run_schema::LaunchKind::Main,
        )
        .validated()
        .unwrap();
        let mut host = HostLibrary::new(
            &run,
            &admitted,
            HostIo::Replay(SimIo::new(Default::default(), vec![])),
        )
        .unwrap();
        let process = host.handle(1, "Proc");
        // Main admission disallows Test, so create its directory at the adapter
        // seam. It must retain Library's scripted identity beside a host Proc.
        let test = host.library.simulation.grant_test();
        let types = hirsel_engine::registry::session_types();
        let result_type = types.intern(hirsel_types::Type::unit());
        let runtime = hirsel_interp::aggregate::RuntimeTypes::new(types.clone());
        let Some(OperationResult::Complete {
            value: directory, ..
        }) = host
            .library
            .call(
                OperationCall {
                    runtime: &runtime,
                    path: &DefPath::core("test.dir"),
                    generics: &[],
                    arguments: &mut [test, sequence(BuiltinType::List, [])],
                    result_type,
                    types: &types,
                    site: hirsel_span::Span::default(),
                },
                &mut OperationContext {
                    configuration: &run,
                    sources: &Default::default(),
                    stdout: &mut vec![],
                    trace: &mut vec![],
                    trace_mode: Default::default(),
                    anchor: None,
                },
            )
            .unwrap()
        else {
            panic!("test.dir must return a directory");
        };
        let command = Value::Record {
            definition: (&DefPath::core("Command")).into(),
            fields: vec![
                Some(Value::Record {
                    definition: (&DefPath::core("Path")).into(),
                    fields: vec![Some(Value::String("/bin/echo".into()))],
                }),
                Some(sequence(BuiltinType::List, [])),
                Some(variant(1, vec![directory])),
                Some(variant(0, vec![])),
                Some(Value::Bool(false)),
            ],
        };
        let arguments = [
            process,
            command,
            Value::Bytes(vec![]),
            u64_value(1024),
            variant(0, vec![]),
        ];
        assert_eq!(
            host.process_request("proc.run", &arguments, 1).unwrap(),
            OperationResult::returned(error(variant(1, vec![])))
        );
        let HostIo::Replay(io) = &host.io else {
            unreachable!()
        };
        io.finish().unwrap();
    }
}
