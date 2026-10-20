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
mod runners;
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
    pub io: HostIo,
    /// The first adapter refusal: a replay divergence or a live provider error.
    pub refused: Option<String>,
    /// Replay output already copied to the process's streams.
    echoed: (usize, usize),
    handles: BTreeMap<u64, u64>,
    next: u64,
    next_wait: u64,
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
            projections: admitted.io_projections(),
            mounts: Default::default(),
            args: record.args.clone().unwrap_or_default(),
            env: record.env.clone().unwrap_or_default(),
            random_seed: record.inputs.random_seed,
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
        Ok(Self {
            next_wait: 1 << 62,
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
            handles: BTreeMap::new(),
            next: 1 << 63,
            mapped: BTreeMap::new(),
            next_continuation: 1 << 61,
        })
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
    /// Issue `watcher.next` under `token`: its event, or Blocked while none has arrived.
    fn watch_request(&mut self, a: &[Value], token: u64) -> Result<OperationResult, Error> {
        let request = R::WatchNext {
            directory: self.id(&a[0])?,
            watcher: self.id(&a[1])?,
        };
        self.io.adapter().enter(token);
        let completion = self.host(request);
        if !matches!(completion, Err(Error::Blocked(_))) {
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
    /// Every host request goes through here. A provider that would block parks the
    /// task; any other adapter error is a refusal (a replay divergence or host fault).
    fn host(&mut self, request: R) -> Result<C, Error> {
        self.host_cancelable(request, self.library.cancellation())
    }
    fn host_cancelable(&mut self, request: R, cancellation: Option<u32>) -> Result<C, Error> {
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
        result.map_err(|e| match e {
            hirsel_rt::io::AdapterError::Blocked => Error::Blocked(vec![]),
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
        if let HostIo::Replay(_) = self.io {
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
    fn host_input(
        &mut self,
        name: &str,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        use hirsel_rt::io::{ENV_HANDLE, IoError};
        let a = &*call.arguments;
        if matches!(
            name,
            "io.stdin" | "io.stderr" | "io.stdout" | "io.random" | "io.tasks"
        ) {
            // The library validates the root and records the semantic projection once.
            let result = self.library.call(call, cx)?;
            let completion = self.host(match name {
                "io.stdin" => R::Stdin { io: 1 },
                "io.stderr" => R::Stderr { io: 1 },
                "io.stdout" => R::Stdout { io: 1 },
                "io.random" => R::Random { io: 1 },
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
        // The root Env has no host row; a narrowed one maps to its host Env.
        let env = match &a[0] {
            Value::Handle { id, .. } => self.handles.get(id).copied().unwrap_or(ENV_HANDLE),
            _ => ENV_HANDLE,
        };
        if name == "env.narrow" {
            let names = fields(&a[1])?
                .into_iter()
                .map(text)
                .collect::<Result<_, _>>()?;
            let result = self.library.call(call, cx)?;
            match (&result, self.host(R::EnvNarrow { env, names })?) {
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
            let name = text(&a[1])?;
            let result = self.library.call(call, cx)?;
            let completion = self.host(R::EnvGet { env, name })?;
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
        if matches!(name, "err.print" | "out.print") {
            use hirsel_interp::library::FreePrint;
            match self.library.free_print_begin(name, a, cx)? {
                None => {}
                Some(FreePrint::Done(result)) => return Ok(Some(result)),
                Some(FreePrint::Write(write)) => {
                    let request = if name == "err.print" {
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
        if matches!(name, "stdin.read" | "err.print" | "out.print")
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
        let request = if name == "err.print" {
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
        if let Some(cause) = self.library.cancellation() {
            return Ok(OperationResult::returned(wait_canceled(cause)));
        }
        self.yield_now = true;
        let token = self.next_wait;
        self.next_wait += 1;
        match self.sleep_request(a, token) {
            Err(Error::Blocked(_)) => {}
            result => return result,
        }
        self.waits.insert(
            token,
            HostWait {
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
        match self.host_cancelable(request.clone(), None) {
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
    /// Read host stdin into the simulation the library then consumes. False when no
    /// byte is available yet: the token keeps the descriptor's readiness interest.
    fn host_stdin(&mut self, a: &[Value], token: u64) -> Result<bool, Error> {
        let request = R::ReadStdin {
            stdin: self.id(&a[0])?,
            max: word(&a[2])?,
        };
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
                match self.host(R::Process { io: 1 })? {
                    C::Handle(Ok(id)) => ok(self.handle(id, "Proc")),
                    C::Handle(Err(hirsel_rt::io::IoError::NotGranted)) => error(variant(0, vec![])),
                    _ => return Err(invalid()),
                },
            ));
        }
        if name == "proc.narrow" {
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
            return Ok(OperationResult::returned(
                match self.host(R::ProcessNarrow { process, rules })? {
                    C::Spawned(Ok(id)) => ok(self.handle(id, "Proc")),
                    C::Spawned(Err(f)) => error(hirsel_interp::library::process::failure_value(f)),
                    _ => return Err(invalid()),
                },
            ));
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
    /// Issue a child request, under `token` when it can park.
    fn child_request(
        &mut self,
        name: &str,
        a: &[Value],
        token: Option<u64>,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        use hirsel_interp::library::process::{failure_value, script_failure, status_value};
        let process = self.id(&a[0])?;
        let request = match name {
            "proc.spawn" => match self.command(&a[1])? {
                Some(command) => R::ProcessSpawn { process, command },
                None => {
                    return Ok(OperationResult::returned(error(failure_value(
                        ProcessFailure::InvalidInput,
                    ))));
                }
            },
            "proc.spawn_pipeline" => {
                let mut stages = vec![];
                for stage in fields(&a[1])? {
                    match self.command(stage)? {
                        Some(command) => stages.push(command),
                        None => {
                            return Ok(OperationResult::returned(error(failure_value(
                                ProcessFailure::InvalidInput,
                            ))));
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
        };
        if let Some(token) = token {
            self.io.adapter().enter(token);
        }
        let cancellation = self
            .library
            .cancellation()
            .filter(|_| token.is_none_or(|t| self.io.cancellable(t)));
        let completion = self.host_cancelable(request, cancellation);
        if let Some(token) = token
            && !matches!(completion, Err(Error::Blocked(_)))
        {
            self.io.adapter().finish(token);
        }
        let completion = completion?;
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
    /// Issue `proc.run` or `proc.pipeline` under `token`: its completion, or Blocked while
    /// the children run.
    fn process_request(
        &mut self,
        name: &str,
        a: &[Value],
        token: u64,
    ) -> Result<OperationResult, Error> {
        let (process, stdin, max_output) = (self.id(&a[0])?, bytes(&a[2])?, word(&a[3])?);
        let deadline = self.deadline(&a[4])?;
        let request = if name == "proc.run" {
            match self.command(&a[1])? {
                Some(command) => R::ProcessRun {
                    process,
                    command,
                    stdin,
                    max_output,
                    deadline,
                },
                None => {
                    return Ok(OperationResult::returned(process_value(
                        ProcessOutcome::Error(ProcessFailure::InvalidInput),
                    )));
                }
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
                None => {
                    return Ok(OperationResult::returned(pipeline_value(
                        PipelineOutcome::Error(ProcessFailure::InvalidInput),
                    )));
                }
            }
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
    fn storage(&mut self, name: &str, a: &[Value]) -> Result<OperationResult, Error> {
        self.yield_now = true;
        let path = |v: &Value| text(fields(v)?[0]);
        let request = match name {
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
            "file.walk" => R::Walk {
                directory: self.id(&a[0])?,
            },
            "file.read_dir" => R::ReadDir {
                directory: self.id(&a[0])?,
                path: path(&a[1])?,
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
        };
        let completion = self.host(request)?;
        let mut updates = vec![];
        let value = match completion {
            C::Handle(r) => match r {
                Ok(id) => ok(self.handle(
                    id,
                    match name {
                        "file.open" | "file.create" => "File",
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
                    let mut buffer = a[2].clone();
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
            C::Paths(r) => match r {
                Ok(paths) => ok(sequence(
                    BuiltinType::Iter,
                    paths
                        .into_iter()
                        .map(|p| ok(hirsel_interp::library::path(&p))),
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
        let token = self.next_wait;
        self.next_wait += 1;
        match self.try_network(name, a, token, cx) {
            Err(Error::Blocked(_)) => {
                self.waits.insert(
                    token,
                    HostWait {
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
    fn network(
        &mut self,
        name: &str,
        a: &[Value],
        token: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let request = match name {
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
            "tls.close" => R::TlsClose {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
            },
            "tls.connect" | "tls.connect_as" | "tls.connect_with" => {
                let stream = self.id(&a[1])?;
                self.io.tls();
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
                let roots = match tag(roots)? {
                    0 => RootsDigest::WebPki("webpki-roots@1.0.9".into()),
                    1 => {
                        use sha2::Digest;
                        let pem = bytes(fields(roots)?[0])?;
                        self.io.tls_roots(stream, &pem);
                        RootsDigest::Pem(sha2::Sha256::digest(pem).into())
                    }
                    _ => return Err(invalid()),
                };
                let identity = match identity {
                    Some(identity) => {
                        let identity = fields(identity)?;
                        let (chain, key) = (bytes(identity[0])?, bytes(identity[1])?);
                        self.io.tls_identity(stream, &chain, &key);
                        Some(hirsel_rt::net_tls::chain_digest(&chain))
                    }
                    None => None,
                };
                R::TlsConnect {
                    net: self.id(&a[0])?,
                    stream,
                    server_name: text(&a[2])?,
                    roots,
                    identity,
                    alpn,
                }
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
                let identities: Vec<(&[u8], &[u8])> =
                    identities.iter().map(|(c, k)| (&c[..], &k[..])).collect();
                let trust;
                let clients = match tag(server[1])? {
                    0 => ClientAuth::Anonymous,
                    t @ (1 | 2) => {
                        trust = bytes(fields(server[1])?[0])?;
                        if t == 1 {
                            ClientAuth::Request(&trust[..])
                        } else {
                            ClientAuth::Require(&trust[..])
                        }
                    }
                    _ => return Err(invalid()),
                };
                let alpn = fields(server[2])?
                    .into_iter()
                    .map(text)
                    .collect::<Result<Vec<_>, _>>()?;
                let stream = self.id(&a[1])?;
                self.io.tls();
                self.io.tls_server(stream, &identities, clients, &alpn);
                R::TlsAccept {
                    net: self.id(&a[0])?,
                    stream,
                    server: hirsel_rt::net::TlsServerDigest::of(&identities, clients, &alpn),
                }
            }
            "tls.peer" => R::TlsPeer {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
            },
            "tls.negotiated_protocol" => R::TlsProtocol {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
            },
            _ => return Err(Error::Unavailable(format!("host network {name}"))),
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
                return hirsel_interp::library::process::script_failure(
                    cx,
                    "scripted net client unmet",
                );
            }
            _ => return Err(invalid()),
        };
        if (name == "net.close"
            || name == "udp.close"
            || name == "tls.close"
            || matches!(
                name,
                "tls.connect" | "tls.connect_as" | "tls.connect_with" | "tls.accept"
            ) && matches!(value, Value::Variant { variant: 1, .. }))
            && let Value::Handle { id, .. } = a[1]
        {
            self.handles.remove(&id);
        }
        let consumed = if matches!(
            name,
            "net.close"
                | "udp.close"
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
impl hirsel_interp::OperationsAdapter for HostLibrary {
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
        let OperationResult::Complete {
            value,
            mut released,
            ..
        } = result
        else {
            return Ok(result);
        };
        self.yield_now = true;
        // A truncated mapping's body result is dropped after the check.
        let value = match self.host(R::MapCheck { directory, mapped })? {
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
    fn entry_returned(
        &mut self,
        value: &Value,
        runtime: &hirsel_interp::aggregate::RuntimeTypes,
        context: &mut OperationContext<'_>,
    ) -> Result<bool, Error> {
        use std::io::Write;
        self.echo(context.stdout)?;
        std::io::stdout()
            .flush()
            .map_err(|e| Error::Unavailable(format!("host stdout: {e}")))?;
        let consume = self.library.entry_returned(value, runtime, context)?;
        if let Some(report) = self.library.exit_report.take() {
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
        if !self.library.schedule_recorded(runnable.len()) {
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
        let mut armed = std::mem::take(&mut self.armed);
        armed.retain(|(token, scope, _)| {
            let live = self.library.deadline_pending(*scope);
            if !live {
                self.io.adapter().finish(*token);
            }
            live
        });
        self.ready = if self.waits.is_empty() && armed.is_empty() && !self.runner_waits() {
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
            match self.host_cancelable(request.clone(), None) {
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
        // Keep entry order for the next poll's ties.
        self.armed.sort_by_key(|(token, _, _)| *token);
        self.runner_children(cx)
    }
    fn wait(&mut self) -> Result<bool, Error> {
        self.io
            .adapter()
            .wait()
            .map_err(|e| Error::Unavailable(format!("host readiness: {e:?}")))
    }
    fn ready_now(&mut self) -> Result<bool, Error> {
        self.io
            .adapter()
            .has_ready()
            .map_err(|e| Error::Unavailable(format!("host readiness: {e:?}")))
    }
    fn take_calls(&mut self) -> Vec<hirsel_interp::CalleeStart> {
        self.runner_calls()
    }
    fn take_cancels(&mut self) -> Vec<u64> {
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
        if self.waits_on_runner(token) {
            return self.runner_poll(token, cx);
        }
        let Some(wait) = self.waits.get(&token) else {
            return self.library.poll(token, cx);
        };
        let canceled = self.library.cancellation().is_some() && self.io.cancellable(token);
        if !canceled && !self.ready.contains(token) {
            return Ok(None);
        }
        let name = wait.name.clone();
        let arguments = wait.arguments.clone();
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
    fn call(
        &mut self,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        if call.path.package_is("core", "1") {
            let name = call
                .path
                .0
                .modules
                .iter()
                .cloned()
                .chain([call.path.0.name.clone()])
                .collect::<Vec<_>>()
                .join(".");
            if matches!(
                name.as_str(),
                "io.stdin"
                    | "io.stderr"
                    | "io.stdout"
                    | "io.random"
                    | "io.tasks"
                    | "stdin.read"
                    | "err.print"
                    | "out.print"
                    | "env.get"
                    | "env.narrow"
            ) {
                let result = self.host_input(&name, call, cx);
                self.echo(cx.stdout)?;
                return result;
            }
            if name == "random.bytes" {
                return self.host_random(call, cx);
            }
            if matches!(
                name.as_str(),
                "runner.compute" | "runner.call" | "runner.spawn"
            ) {
                let result = self.library.call(call, cx)?;
                self.runner_sends(cx)?;
                return Ok(result);
            }
            if name == "clock.sleep_until"
                && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
            {
                return self.host_sleep(call.arguments, cx).map(Some);
            }
            if name == "io.dir" || name == "io.monotonic_clock" || name == "io.wall_clock" {
                if !matches!(call.arguments.first(),Some(Value::Handle {kind,..}) if kind=="Io") {
                    return Err(invalid());
                }
                return self.storage(&name, call.arguments).map(Some);
            }
            if name == "watcher.next" && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
            {
                return self.host_watch_next(call.arguments, cx).map(Some);
            }
            if name == "mapped.with" && call.arguments.first().is_some_and(|v| self.id(v).is_ok()) {
                return self.host_mapped_with(call.arguments).map(Some);
            }
            if (name.starts_with("file.")
                || name == "dir.narrow"
                || name == "dir.read_only"
                || name == "dir.remove_all"
                || name.starts_with("clock."))
                && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
            {
                return self.storage(&name, call.arguments).map(Some);
            }
            if name == "io.process"
                || (name.starts_with("proc.")
                    || name.starts_with("child.")
                    || name.starts_with("group."))
                    && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
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
            if name == "io.net" {
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
            if (name.starts_with("net.") || name.starts_with("tls.") || name.starts_with("udp."))
                && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
            {
                return self.host_network(&name, call.arguments, cx).map(Some);
            }
        }
        let result = self.library.call(call, cx);
        for (scope, clock, at) in self.library.take_deadlines() {
            self.arm_deadline(scope, &clock, at, cx)?;
        }
        result
    }
    fn drop_handle(
        &mut self,
        kind: &str,
        id: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        if matches!(
            kind,
            "Stream"
                | "Listener"
                | "UdpSocket"
                | "File"
                | "AtomicFile"
                | "Child"
                | "ChildGroup"
                | "Mapped"
                | "Watcher"
        ) && let Some(handle) = self.handles.remove(&id)
        {
            // Dropping a scripted child closes its input, which can leave an Expect unmet.
            if self.host(R::Close { handle })? == C::Checked(false)
                && matches!(kind, "Child" | "ChildGroup")
            {
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
        let mut types = hirsel_types::TypeStore::default();
        let result_type = types.push(hirsel_types::Type::unit());
        let Some(OperationResult::Complete {
            value: directory, ..
        }) = host
            .library
            .call(
                OperationCall {
                    runtime: &Default::default(),
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
