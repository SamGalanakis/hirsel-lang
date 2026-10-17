//! Host effects for interpreted launches. The runtime owns CAP-1/2 policy;
//! the evaluator continues to execute only explicit operation results.
use hirsel_interp::library::{Library, SimulationConfig, error, ok, u64_value};
use hirsel_interp::{
    Error, Integer, OperationCall, OperationContext, OperationResult, Operations, Value,
};
use hirsel_rt::{
    io::{Adapter, Completion as C, Request as R},
    io_real::RealIo,
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

pub(crate) struct HostLibrary<'a> {
    output: &'a mut dyn std::io::Write,
    written: usize,
    pub library: Library,
    io: RealIo,
    handles: BTreeMap<u64, u64>,
    next: u64,
}
impl<'a> HostLibrary<'a> {
    pub fn new(
        run: &hirsel_tir::RunConfiguration,
        admitted: &hirsel_run_config::ValidatedRun,
        io: RealIo,
        output: &'a mut dyn std::io::Write,
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
            output,
            written: 0,
            library: Library::new(run, config)?,
            io,
            handles: BTreeMap::new(),
            next: 1 << 63,
        })
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
        use hirsel_interp::library::simulation::Fault;
        use hirsel_rt::io::{ENV_HANDLE, InputError, IoError};
        let a = call.arguments;
        if matches!(name, "io.stdin" | "io.stderr") {
            // The library validates the root and records the semantic projection once.
            let result = self.library.call(call, cx)?;
            let completion = self
                .io
                .call(if name == "io.stdin" {
                    R::Stdin { io: 1 }
                } else {
                    R::Stderr { io: 1 }
                })
                .map_err(|e| Error::Unavailable(format!("host projection: {e:?}")))?;
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
            let completion = self
                .io
                .call(R::EnvGet {
                    env: ENV_HANDLE,
                    name: text(&a[1])?,
                })
                .map_err(|e| Error::Unavailable(format!("host Env read: {e:?}")))?;
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
        let request = match name {
            "stdin.read" => R::ReadStdin {
                stdin: self.id(&a[0])?,
                max: word(&a[2])?,
            },
            "err.print" => R::PrintStderr {
                stderr: self.id(&a[0])?,
                bytes: bytes(&a[1])?,
            },
            _ => return Err(invalid()),
        };
        let completion = self
            .io
            .call(request)
            .map_err(|e| Error::Unavailable(format!("host input: {e:?}")))?;
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
                partial: self
                    .io
                    .tape()
                    .last()
                    .and_then(|e| e.fault.as_ref())
                    .map_or(0, |f| f.partial),
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
        self.library.call(call, cx)
    }
    fn storage(&mut self, name: &str, a: &[Value]) -> Result<OperationResult, Error> {
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
        let completion = self
            .io
            .call(request)
            .map_err(|e| Error::Unavailable(format!("host operation {name}: {e:?}")))?;
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
    fn network(&mut self, name: &str, a: &[Value]) -> Result<OperationResult, Error> {
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
        let completion = self
            .io
            .call(request)
            .map_err(|e| Error::Unavailable(format!("host network: {e:?}")))?;
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
                    } else if name == "net.listen" {
                        "Listener"
                    } else {
                        "Stream"
                    },
                )
            }),
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
impl Operations for HostLibrary<'_> {
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
                "io.stdin" | "io.stderr" | "stdin.read" | "err.print" | "env.get"
            ) {
                return self.host_input(&name, call, cx);
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
            if name == "io.net" {
                // Validate the Io operand through the ordinary library before projecting it.
                self.library.call(
                    OperationCall {
                        path: call.path,
                        generics: call.generics,
                        arguments: call.arguments,
                        result_type: call.result_type,
                        types: call.types,
                        site: call.site,
                    },
                    cx,
                )?;
                return self.network(&name, call.arguments).map(Some);
            }
            if (name.starts_with("net.") || name.starts_with("tls."))
                && call.arguments.first().is_some_and(|v| self.id(v).is_ok())
            {
                return self.network(&name, call.arguments).map(Some);
            }
        }
        let result = self.library.call(call, cx);
        self.output
            .write_all(&cx.stdout[self.written..])
            .map_err(|e| Error::Unavailable(format!("stdout: {e}")))?;
        self.written = cx.stdout.len();
        result
    }
    fn resume(
        &mut self,
        t: u64,
        r: OperationResult,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.library.resume(t, r, cx)
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
            self.io
                .call(R::Close { handle })
                .map_err(|e| Error::Unavailable(format!("network close: {e:?}")))?;
            return Ok(());
        }
        self.library.drop_handle(kind, id, cx)
    }
}
