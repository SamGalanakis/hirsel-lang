//! Host effects for interpreted launches. The runtime owns CAP-1/2 policy;
//! the evaluator continues to execute only explicit operation results.
use hirsel_interp::library::{Library, SimulationConfig, error, ok, u64_value};
use hirsel_interp::{
    Error, Integer, OperationCall, OperationContext, OperationResult, Operations, Value,
};
use hirsel_rt::{
    io::{Adapter, Completion as C, Request as R},
    io_real::RealIo,
    io_sim::SimIo,
    net::*,
};
use hirsel_syntax::DefPath;
use hirsel_types::{BuiltinType, Primitive};
use std::collections::BTreeMap;

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
    let f = match v {
        Value::Record { fields, .. }
        | Value::Variant { fields, .. }
        | Value::Tuple(fields)
        | Value::Sequence {
            elements: fields, ..
        } => fields,
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
    }
}
fn address(a: SocketAddr) -> Value {
    let (t, b) = match a.ip {
        std::net::IpAddr::V4(ip) => (0, ip.octets().to_vec()),
        std::net::IpAddr::V6(ip) => (1, ip.octets().to_vec()),
    };
    Value::Record {
        definition: DefPath::core("SocketAddr"),
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
    fn tls_roots(&mut self, pem: &[u8]) {
        match self {
            Self::Live(io) => io.tls_roots(pem),
            Self::Replay(io) => io.tls_roots(pem),
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
    yield_now: bool,
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
            ..Default::default()
        };
        Ok(Self {
            next_wait: 1 << 62,
            waits: BTreeMap::new(),
            yield_now: false,
            library: Library::new(run, config)?,
            io,
            refused: None,
            echoed: (0, 0),
            handles: BTreeMap::new(),
            next: 1 << 63,
        })
    }
    /// Every host request goes through here. A provider that would block parks the
    /// task; any other adapter error is a refusal (a replay divergence or host fault).
    fn host(&mut self, request: R) -> Result<C, Error> {
        self.io.call(request).map_err(|e| match e {
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
        let a = call.arguments;
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
        if name == "env.get" {
            let result = self.library.call(call, cx)?;
            let completion = self.host(R::EnvGet {
                env: ENV_HANDLE,
                name: text(&a[1])?,
            })?;
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
    /// records them; a scripted `test.process` Proc stays with the library.
    fn host_process(&mut self, name: &str, a: &[Value]) -> Result<OperationResult, Error> {
        let request = if name == "io.process" {
            R::Process { io: 1 }
        } else {
            let directory = match tag(&a[4])? {
                0 => None,
                _ => match self.id(fields(&a[4])?[0]) {
                    Ok(directory) => Some(directory),
                    Err(_) => {
                        return Ok(OperationResult::returned(
                            hirsel_interp::library::process::outcome_value(
                                &hirsel_rt::process::ProcessOutcome::Error(
                                    hirsel_rt::process::ProcessFailure::InvalidInput,
                                ),
                            ),
                        ));
                    }
                },
            };
            R::ProcessRun {
                process: self.id(&a[0])?,
                program: text(fields(&a[1])?[0])?,
                args: fields(&a[2])?
                    .into_iter()
                    .map(text)
                    .collect::<Result<_, _>>()?,
                stdin: bytes(&a[3])?,
                directory,
                max_output: word(&a[5])?,
            }
        };
        let completion = self
            .io
            .call(request)
            .map_err(|e| Error::Unavailable(format!("host process: {e:?}")))?;
        Ok(OperationResult::returned(match completion {
            C::Handle(Ok(id)) => ok(self.handle(id, "Proc")),
            C::Handle(Err(hirsel_rt::io::IoError::NotGranted)) => error(variant(0, vec![])),
            C::Process(outcome) => hirsel_interp::library::process::outcome_value(&outcome),
            _ => return Err(invalid()),
        }))
    }
    fn storage(&mut self, name: &str, a: &[Value]) -> Result<OperationResult, Error> {
        self.yield_now = true;
        if !matches!(
            name,
            "io.dir"
                | "dir.narrow"
                | "file.narrow"
                | "io.monotonic_clock"
                | "io.wall_clock"
                | "clock.monotonic"
                | "clock.wall"
        ) && let Some(cause) = self.library.cancellation()
        {
            if name == "file.commit" {
                let handle = self.id(&a[1])?;
                self.host(R::Close { handle })?;
                if let Value::Handle { id, .. } = a[1] {
                    self.handles.remove(&id);
                }
            }
            return Ok(
                OperationResult::returned(file_error(hirsel_rt::io::IoError::Canceled(cause)))
                    .transferring(if name == "file.commit" {
                        vec![1]
                    } else {
                        vec![]
                    }),
            );
        }
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
            "file.write_all" => R::WriteAll {
                directory: self.id(&a[0])?,
                file: self.id(&a[1])?,
                bytes: bytes(&a[2])?,
            },
            "file.commit" => R::Commit {
                directory: self.id(&a[0])?,
                file: self.id(&a[1])?,
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
                    definition: DefPath::core("Metadata"),
                    fields: vec![
                        Some(variant(meta.kind as u32, vec![])),
                        Some(u64_value(meta.size)),
                        Some(meta.modified_unix_ns.map_or_else(
                            || variant(0, vec![]),
                            |n| variant(1, vec![int(Primitive::I128, n as u128)]),
                        )),
                    ],
                }),
                Err(e) => file_error(e),
            },
            C::Entries(r) => match r {
                Ok(entries) => ok(sequence(
                    BuiltinType::List,
                    entries.into_iter().map(|entry| Value::Record {
                        definition: DefPath::core("DirEntry"),
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
        cx: &OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.yield_now = true;
        let token = self.next_wait;
        self.next_wait += 1;
        match self.try_network(name, a, token) {
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
    ) -> Result<OperationResult, Error> {
        self.io.adapter().enter(token);
        let result = self.network(name, a, token);
        if !matches!(result, Err(Error::Blocked(_))) {
            self.io.adapter().finish(token);
        }
        result
    }
    fn network(&mut self, name: &str, a: &[Value], token: u64) -> Result<OperationResult, Error> {
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
            "net.listen" => R::Listen {
                net: self.id(&a[0])?,
                addr: addr(&a[1])?,
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
            "net.close" => R::Close {
                handle: self.id(&a[1])?,
            },
            "tls.close" => R::TlsClose {
                net: self.id(&a[0])?,
                stream: self.id(&a[1])?,
            },
            "tls.connect" => {
                let roots = match tag(&a[3])? {
                    0 => RootsDigest::WebPki("webpki-roots@1.0.9".into()),
                    1 => {
                        use sha2::Digest;
                        let pem = bytes(fields(&a[3])?[0])?;
                        self.io.tls_roots(&pem);
                        RootsDigest::Pem(sha2::Sha256::digest(pem).into())
                    }
                    _ => return Err(invalid()),
                };
                R::TlsConnect {
                    net: self.id(&a[0])?,
                    stream: self.id(&a[1])?,
                    server_name: text(&a[2])?,
                    roots,
                }
            }
            _ => return Err(Error::Unavailable(format!("host network {name}"))),
        };
        let cancellation = self
            .library
            .cancellation()
            .filter(|_| self.io.cancellable(token));
        let completion = if let Some(cause) = cancellation.filter(|_| {
            matches!(
                name,
                "net.resolve"
                    | "net.connect"
                    | "net.connect_addr"
                    | "net.accept"
                    | "net.read"
                    | "net.read_into"
                    | "net.write"
                    | "net.write_all"
                    | "tls.connect"
                    | "tls.close"
            )
        }) {
            let e = NetError::Canceled(cause);
            match name {
                "net.resolve" => C::Addresses(Err(e)),
                "net.connect" | "net.connect_addr" => C::NetHandle(Err(e)),
                "net.accept" => C::Accepted(Err(e)),
                "net.read" | "net.read_into" => C::NetBytes(Err(e)),
                "net.write" => C::Count(Err(e)),
                "tls.connect" | "tls.close" => {
                    let handle = self.id(&a[1])?;
                    self.host(R::Close { handle })?;
                    C::NetUnit(Err(e))
                }
                _ => C::NetUnit(Err(e)),
            }
        } else {
            self.host(request)?
        };
        let mut updates = vec![];
        let value = match completion {
            C::Handle(r) => match r {
                Ok(id) => ok(self.handle(id, "Net")),
                Err(_) => error(variant(0, vec![])),
            },
            C::NetHandle(r) => result(r, |id| {
                self.handle(
                    id,
                    if name == "net.narrow" {
                        "Net"
                    } else {
                        "Stream"
                    },
                )
            }),
            C::Listened(r) => result(r, |(id, _)| self.handle(id, "Listener")),
            C::Accepted(r) => result(r, |(id, addr)| {
                Value::Tuple(vec![Some(self.handle(id, "Stream")), Some(address(addr))])
            }),
            C::Address(r) => result(r, address),
            C::Addresses(r) => result(r, |a| {
                sequence(BuiltinType::List, a.into_iter().map(address))
            }),
            C::Count(r) => result(r, u64_value),
            C::NetUnit(r) if name == "tls.connect" => result(r, |()| a[1].clone()),
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
            _ => return Err(invalid()),
        };
        if (name == "net.close"
            || name == "tls.close"
            || name == "tls.connect" && matches!(value, Value::Variant { variant: 1, .. }))
            && let Value::Handle { id, .. } = a[1]
        {
            self.handles.remove(&id);
        }
        let consumed = if name == "net.close" || name == "tls.close" || name == "tls.connect" {
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
    fn take_yield(&mut self) -> bool {
        let host = std::mem::take(&mut self.yield_now);
        self.library.take_yield() || host
    }
    fn wait(&mut self) -> Result<bool, Error> {
        self.io
            .adapter()
            .wait()
            .map_err(|e| Error::Unavailable(format!("host readiness: {e:?}")))
    }
    fn poll(
        &mut self,
        token: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let Some(wait) = self.waits.get(&token) else {
            return self.library.poll(token, cx);
        };
        let canceled = self.library.cancellation().is_some() && self.io.cancellable(token);
        if !canceled
            && !self
                .io
                .adapter()
                .ready(token)
                .map_err(|e| Error::Unavailable(format!("host readiness: {e:?}")))?
        {
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
        match self.try_network(&name, &arguments, token) {
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
            ) {
                let result = self.host_input(&name, call, cx);
                self.echo(cx.stdout)?;
                return result;
            }
            if name == "random.bytes" {
                return self.host_random(call, cx);
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
            if (name.starts_with("file.") || name == "dir.narrow" || name.starts_with("clock."))
                && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
            {
                return self.storage(&name, call.arguments).map(Some);
            }
            if name == "io.process"
                || name == "proc.run" && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
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
                return self.host_process(&name, call.arguments).map(Some);
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
            if (name.starts_with("net.") || name.starts_with("tls."))
                && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
            {
                return self.host_network(&name, call.arguments, cx).map(Some);
            }
        }
        self.library.call(call, cx)
    }
    fn drop_handle(
        &mut self,
        kind: &str,
        id: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        if matches!(kind, "Stream" | "Listener" | "File" | "AtomicFile")
            && let Some(handle) = self.handles.remove(&id)
        {
            self.host(R::Close { handle })?;
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
            hirsel_run_config::LaunchKind::Main,
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
                    arguments: &[test, sequence(BuiltinType::List, [])],
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
        let arguments = [
            process,
            Value::Record {
                definition: DefPath::core("Path"),
                fields: vec![Some(Value::String("/bin/echo".into()))],
            },
            sequence(BuiltinType::List, []),
            Value::Bytes(vec![]),
            variant(1, vec![directory]),
            u64_value(1024),
        ];
        assert_eq!(
            host.host_process("proc.run", &arguments).unwrap(),
            OperationResult::returned(error(variant(1, vec![])))
        );
        let HostIo::Replay(io) = &host.io else {
            unreachable!()
        };
        io.finish().unwrap();
    }
}
