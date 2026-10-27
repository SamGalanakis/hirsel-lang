//! Binary tape payloads. Fixed-width little-endian scalars, u32 enum tags and
//! u64 collection lengths follow bincode 1.3.3's fixint encoding. Unlike serde
//! derives these declarations pin field order independently of JSON attributes.
//! Byte slices are copied as one span, never formatted or encoded per byte.

use std::collections::{BTreeMap, BTreeSet};
pub(crate) trait Wire: Sized {
    fn encode(&self, out: &mut Vec<u8>);
    fn decode(input: &mut Decode<'_>) -> Result<Self, String>;
    fn encode_many(values: &[Self], out: &mut Vec<u8>) {
        for value in values {
            value.encode(out);
        }
    }
    fn decode_many(input: &mut Decode<'_>, n: usize) -> Result<Vec<Self>, String> {
        if n > input.0.len() {
            return Err("binary collection exceeds payload".into());
        }
        (0..n).map(|_| Self::decode(input)).collect()
    }
}
pub(crate) struct Decode<'a>(pub &'a [u8]);
impl<'a> Decode<'a> {
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if n > self.0.len() {
            return Err("short binary tape payload".into());
        }
        let (value, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(value)
    }
}
pub(crate) fn decode<T: Wire>(bytes: &[u8]) -> Result<T, String> {
    let mut input = Decode(bytes);
    let value = T::decode(&mut input)?;
    if !input.0.is_empty() {
        return Err("trailing binary tape payload".into());
    }
    Ok(value)
}
macro_rules! integer {
    ($($t:ty),*) => { $(impl Wire for $t {
        fn encode(&self, out: &mut Vec<u8>) { out.extend_from_slice(&self.to_le_bytes()); }
        fn decode(input: &mut Decode<'_>) -> Result<Self, String> { Ok(Self::from_le_bytes(input.take(std::mem::size_of::<Self>())?.try_into().unwrap())) }
    })* };
}
integer!(u16, u32, u64, i32, i64, i128);
impl Wire for u8 {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(*self);
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        Ok(input.take(1)?[0])
    }
    fn encode_many(values: &[Self], out: &mut Vec<u8>) {
        out.extend_from_slice(values);
    }
    fn decode_many(input: &mut Decode<'_>, n: usize) -> Result<Vec<Self>, String> {
        Ok(input.take(n)?.to_vec())
    }
}
impl Wire for usize {
    fn encode(&self, out: &mut Vec<u8>) {
        (*self as u64).encode(out);
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        usize::try_from(u64::decode(input)?).map_err(|_| "binary tape size overflow".into())
    }
}
impl Wire for bool {
    fn encode(&self, out: &mut Vec<u8>) {
        u8::from(*self).encode(out);
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        match u8::decode(input)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("bad binary boolean".into()),
        }
    }
}
impl Wire for () {
    fn encode(&self, _: &mut Vec<u8>) {}
    fn decode(_: &mut Decode<'_>) -> Result<Self, String> {
        Ok(())
    }
}
impl<T: Wire> Wire for Option<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            None => 0u8.encode(out),
            Some(v) => {
                1u8.encode(out);
                v.encode(out);
            }
        }
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        match u8::decode(input)? {
            0 => Ok(None),
            1 => Ok(Some(T::decode(input)?)),
            _ => Err("bad binary option".into()),
        }
    }
}
impl<T: Wire, E: Wire> Wire for Result<T, E> {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Ok(v) => {
                0u32.encode(out);
                v.encode(out)
            }
            Err(e) => {
                1u32.encode(out);
                e.encode(out)
            }
        }
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        match u32::decode(input)? {
            0 => Ok(Ok(T::decode(input)?)),
            1 => Ok(Err(E::decode(input)?)),
            _ => Err("bad binary result".into()),
        }
    }
}
impl<T: Wire> Wire for Box<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        (**self).encode(out);
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        Ok(Box::new(T::decode(input)?))
    }
}
impl<T: Wire> Wire for Vec<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        self.len().encode(out);
        T::encode_many(self, out);
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        let n = usize::decode(input)?;
        T::decode_many(input, n)
    }
}
impl<T: Wire, const N: usize> Wire for [T; N] {
    fn encode(&self, out: &mut Vec<u8>) {
        T::encode_many(self, out)
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        T::decode_many(input, N)?
            .try_into()
            .map_err(|_| "bad binary array".into())
    }
}
impl Wire for String {
    fn encode(&self, out: &mut Vec<u8>) {
        self.len().encode(out);
        out.extend_from_slice(self.as_bytes());
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        let n = usize::decode(input)?;
        String::from_utf8(input.take(n)?.to_vec()).map_err(|_| "binary tape UTF-8".into())
    }
}
macro_rules! tuple {
    ($($t:ident:$n:tt),*) => { impl<$($t:Wire),*> Wire for ($($t,)*) {
        fn encode(&self,out:&mut Vec<u8>) { $(self.$n.encode(out);)* }
        fn decode(input:&mut Decode<'_>)->Result<Self,String> { Ok(($($t::decode(input)?,)*)) }
    } };
}
tuple!(A:0,B:1);
tuple!(A:0,B:1,C:2);
impl<K: Wire + Ord, V: Wire> Wire for BTreeMap<K, V> {
    fn encode(&self, out: &mut Vec<u8>) {
        self.len().encode(out);
        for (k, v) in self {
            k.encode(out);
            v.encode(out)
        }
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        let n = usize::decode(input)?;
        if n > input.0.len() {
            return Err("binary map exceeds payload".into());
        }
        let mut map = Self::new();
        for _ in 0..n {
            if map.insert(K::decode(input)?, V::decode(input)?).is_some() {
                return Err("duplicate binary map key".into());
            }
        }
        Ok(map)
    }
}
impl<T: Wire + Ord> Wire for BTreeSet<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        self.len().encode(out);
        for v in self {
            v.encode(out)
        }
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        let n = usize::decode(input)?;
        if n > input.0.len() {
            return Err("binary set exceeds payload".into());
        }
        let mut set = Self::new();
        for _ in 0..n {
            if !set.insert(T::decode(input)?) {
                return Err("duplicate binary set entry".into());
            }
        }
        Ok(set)
    }
}
impl Wire for std::net::IpAddr {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::V4(v) => {
                0u32.encode(out);
                v.octets().encode(out)
            }
            Self::V6(v) => {
                1u32.encode(out);
                v.octets().encode(out)
            }
        }
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        match u32::decode(input)? {
            0 => Ok(Self::V4(<[u8; 4]>::decode(input)?.into())),
            1 => Ok(Self::V6(<[u8; 16]>::decode(input)?.into())),
            _ => Err("bad binary IP".into()),
        }
    }
}

macro_rules! wire_struct {
    ($ty:path { $($field:ident),* $(,)? }) => {
        impl Wire for $ty {
            fn encode(&self, out: &mut Vec<u8>) { $(self.$field.encode(out);)* }
            fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
                Ok(Self { $($field: Wire::decode(input)?),* })
            }
        }
    };
}
macro_rules! wire_enum {
    ($ty:path { $($tag:literal => $variant:ident $(($($tuple:ident),*))? $({$($field:ident),*})?),* $(,)? } $(retired { $($old:literal => $reason:literal),* $(,)? })?) => {
        impl Wire for $ty {
            fn encode(&self, out: &mut Vec<u8>) {
                match self {
                    $(Self::$variant $(($($tuple),*))? $({$($field),*})? => {
                        ($tag as u32).encode(out);
                        $($($tuple.encode(out);)*)?
                        $($($field.encode(out);)*)?
                    }),*
                }
            }
            fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
                Ok(match u32::decode(input)? {
                    $($tag => Self::$variant $(($(wire_enum!(@decode $tuple input)),*))? $({$($field: Wire::decode(input)?),*})?),*,
                    $($($old => return Err($reason.into()),)*)?
                    _ => return Err("unknown binary tape variant".into()),
                })
            }
        }
    };
    (@decode $name:ident $input:ident) => { Wire::decode($input)? };
}

// Free host observations and final results retain adapter outcomes as data.
// Tags are append-only; ordinary cooperative payloads are unchanged.
wire_enum!(crate::io::AdapterError {
    0 => InvalidHandle(v0),
    1 => ReplayMismatch(v0),
    2 => MissingInput(v0),
    3 => UnconsumedInput(v0),
    4 => InvalidCompletion,
    5 => RecordingFailed(v0),
    6 => Blocked,
    7 => Unsupported(v0),
    8 => Signaled(v0),
    9 => Stopped,
    10 => ProcessFailed { child, site, operation_site, path }
});

wire_enum!(crate::io::IoError {
    0 => Fault(v0),
    1 => Canceled(v0),
    2 => NotFound,
    3 => PermissionDenied,
    4 => AlreadyExists,
    5 => IsDirectory,
    6 => NotDirectory,
    7 => DirectoryNotEmpty,
    8 => CrossDevice,
    9 => NotGranted,
    10 => Truncated
});

wire_enum!(crate::io::InputError {
    0 => Fault(v0),
    1 => Canceled(v0),
    2 => InvalidInput,
    3 => InvalidUtf8
});

wire_enum!(crate::io::EnvError {
    0 => NotGranted,
    1 => InvalidUtf8
});

wire_enum!(crate::io::Request {
    0 => Stdin{ io },
    1 => Stderr{ io },
    2 => ReadStdin{ stdin, max },
    3 => PrintStderr{ stderr, bytes },
    4 => EnvGet{ env, name },
    5 => EnvNarrow{ env, names },
    6 => MonotonicClock{ io },
    7 => WallClock{ io },
    8 => Monotonic{ clock },
    9 => Wall{ clock },
    10 => Random{ io },
    11 => RandomBytes{ random, count },
    12 => Telemetry{ io },
    13 => TelemetryBegin{ telemetry, parent, spec },
    14 => TelemetryEvent{ telemetry, span, payload },
    15 => TelemetryFinish{ telemetry, span, status },
    16 => TelemetryLog{ telemetry, payload },
    17 => TelemetryMeasure{ telemetry, payload },
    18 => SleepUntil{ clock, deadline },
    19 => Tasks{ io },
    20 => Net{ io },
    21 => Process{ io },
    22 => Signals{ io },
    23 => SignalWait{ signals, kinds },
    24 => TestProcess{ test, runs, children },
    25 => ProcessNarrow{ process, rules },
    26 => ProcessRun{ process, command, stdin, max_output, deadline },
    27 => ProcessPipeline{ process, stages, stdin, max_output, deadline },
    28 => ProcessSpawn{ process, command },
    29 => ProcessSpawnPipeline{ process, stages, max_stderr },
    30 => ChildWrite{ process, child, bytes },
    31 => ChildCloseStdin{ process, child },
    32 => ChildRead{ process, child, stream, max },
    33 => ChildWait{ process, child, deadline },
    34 => ChildKill{ process, child, signal },
    35 => GroupWait{ process, group, deadline },
    36 => TestCheck{ test, condition },
    37 => TestCheckCase{ test, condition, case },
    38 => TestFail{ test },
    39 => TestClock{ test },
    40 => TestSeed{ test },
    41 => TestTasks{ test },
    42 => TestLog{ test, text },
    43 => TestSnapshot{ test, name, text },
    44 => TestNet{ test, peers, listeners },
    45 => TestDir{ test, entries },
    46 => TestCrash{ test, directory, seed },
    47 => Stdout{ io },
    48 => Directory{ io, path },
    49 => NarrowDir{ directory, path },
    50 => ReadOnlyDir{ directory },
    51 => Print{ stdout, bytes },
    52 => Open{ directory, path },
    53 => Create{ directory, path },
    54 => FileRead{ directory, file, max },
    55 => FileSeek{ directory, file, offset },
    56 => FileWrite{ directory, file, bytes },
    57 => FileAppend{ directory, file, bytes },
    58 => NarrowFile{ directory, file },
    // Tag 59 (eager Walk) is permanently reserved by "Stream file.walk through sorted directory cursors"; never reuse.
    115 => ReadDirPage{ directory, path, after, recursive },
    61 => Metadata{ directory, path },
    62 => Mkdir{ directory, path },
    63 => Rename{ directory, from, to },
    64 => RemoveDir{ directory, path },
    65 => SetPermissions{ directory, path, permissions },
    66 => RemoveAll{ directory, path },
    67 => Size{ directory, path },
    68 => Read{ directory, path },
    69 => Write{ directory, path, bytes },
    70 => Remove{ directory, path },
    71 => CreateAtomic{ directory, path },
    72 => WriteAll{ directory, file, bytes },
    73 => Commit{ directory, file },
    74 => Sync{ directory, file },
    75 => PutNew{ directory, path, bytes, durable },
    76 => Map{ directory, path },
    77 => MapView{ directory, mapped },
    78 => MapCheck{ directory, mapped },
    79 => Watch{ directory, path, recursive },
    80 => WatchNext{ directory, watcher },
    81 => TestDirEvent{ test, directory, event },
    82 => Close{ handle },
    83 => NetNarrow{ net, rules },
    84 => Resolve{ net, host, port },
    85 => Connect{ net, host, port, timeout },
    86 => UdpBind{ net, addr },
    87 => UdpSend{ net, socket, bytes, addr },
    88 => UdpRecv{ net, socket, max },
    89 => ConnectAddr{ net, addr, timeout },
    90 => NetRead{ net, stream, max },
    91 => NetWrite{ net, stream, bytes },
    92 => NetWriteAll{ net, stream, bytes },
    93 => Shutdown{ net, stream, how },
    94 => SetTimeout{ net, stream, which, timeout },
    95 => PeerAddr{ net, stream },
    96 => NetSplit{ net, stream },
    97 => NetJoin{ net, read, write },
    98 => TlsConnect{ net, stream, server_name, roots, identity, alpn },
    99 => TlsAccept{ net, stream, server },
    100 => TlsPeer{ net, stream },
    101 => TlsProtocol{ net, stream },
    102 => TlsClose{ net, stream },
    103 => Listen{ net, addr },
    104 => Accept{ net, listener },
    105 => LocalAddr{ net, listener },
    106 => UnixConnect{ net, path },
    107 => UnixListen{ net, path },
    108 => RunnerCall(v0),
    109 => RunnerCancel{ key, cause },
    110 => CallReturn(v0),
    111 => Stopped,
    112 => Ready{ wait, points },
    113 => Schedule{ count, choice },
    114 => OpenAppend{ directory, path },
    116 => RunnerPoll{ key, cancel },
    117 => ProcessTime{ from, to },
    118 => ProcessSettle{ canceled },
    119 => CpuControl{ width, action, count, outcome }
} retired { 60 => "retired eager read_dir recording; re-record with streaming file.read_dir", 59 => "retired Request::Walk recording; re-record with streaming file.walk" });

wire_enum!(crate::io::ClockError {
    0 => Unavailable,
    1 => OutOfRange
});

wire_enum!(crate::io::WaitError {
    0 => Clock(v0),
    1 => Canceled(v0)
});

wire_enum!(crate::io::SignalError {
    0 => Canceled(v0),
    1 => NotGranted,
    2 => InvalidInput
});

wire_enum!(crate::io::RandomError {
    0 => Unavailable
});

wire_enum!(crate::io::Completion {
    0 => Input(v0),
    1 => Env(v0),
    2 => ClockGrant(v0),
    3 => Monotonic(v0),
    4 => Wall(v0),
    5 => Random(v0),
    6 => Wait(v0),
    7 => Signal(v0),
    8 => Refused(v0),
    9 => Checked(v0),
    10 => Handle(v0),
    11 => OptionalFile(v0),
    12 => Unit(v0),
    13 => Size(v0),
    14 => Bytes(v0),
    // Tag 15 (eager Paths) is permanently reserved by "Stream file.walk through sorted directory cursors"; never reuse.
    16 => Entries(v0),
    17 => Metadata(v0),
    18 => NetHandle(v0),
    19 => Listened(v0),
    20 => Addresses(v0),
    21 => NetBytes(v0),
    22 => Datagram(v0),
    23 => Count(v0),
    24 => NetUnit(v0),
    25 => Certificates(v0),
    26 => Protocol(v0),
    27 => Address(v0),
    28 => Halves(v0),
    29 => Accepted(v0),
    30 => Process(v0),
    31 => Pipeline(v0),
    32 => Spawned(v0),
    33 => ChildUnit(v0),
    34 => ChildBytes(v0),
    35 => ChildExit(v0),
    36 => GroupExit(v0),
    37 => Blocked{ committed },
    38 => Ready,
    39 => Runner(v0),
    40 => View(v0),
    41 => Watch(v0),
    42 => Span{ handle, trace, span },
    43 => FileInput(v0),
    44 => Held
});

wire_struct!(crate::io::Fault { error, partial });

wire_struct!(crate::io::Event {
    request,
    completion,
    fault,
    resources
});

impl Wire for crate::io_keys::Key {
    fn encode(&self, out: &mut Vec<u8>) {
        self.0.encode(out);
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        Ok(Self(Wire::decode(input)?))
    }
}

wire_enum!(crate::io_keys::Mode {
    0 => Shared,
    1 => Exclusive
});

wire_enum!(crate::io_keys::Kind {
    0 => File,
    1 => Sink { file_backed },
    2 => Other,
    3 => Half { stream }
});

/// Only the canonical form: ascending keys, each once.
impl Wire for crate::io_keys::KeySet {
    fn encode(&self, out: &mut Vec<u8>) {
        self.keys().to_vec().encode(out);
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        let keys = Vec::<(crate::io_keys::Key, crate::io_keys::Mode)>::decode(input)?;
        if !keys.windows(2).all(|pair| pair[0].0 < pair[1].0) {
            return Err("non-canonical recorded key set".into());
        }
        Ok(Self::new(keys))
    }
}

/// Identities in ascending handle order, each handle once.
impl Wire for crate::io_keys::Footprint {
    fn encode(&self, out: &mut Vec<u8>) {
        self.keys.encode(out);
        self.domains.encode(out);
        self.identities.encode(out);
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        let value = Self {
            keys: Wire::decode(input)?,
            domains: Wire::decode(input)?,
            identities: Wire::decode(input)?,
        };
        if !value
            .identities
            .windows(2)
            .all(|pair| pair[0].0 < pair[1].0)
        {
            return Err("non-canonical recorded identities".into());
        }
        Ok(value)
    }
}

wire_enum!(crate::io::FileKind {
    0 => Regular,
    1 => Directory,
    2 => Symlink,
    3 => Other
});

wire_struct!(crate::io::Metadata {
    kind,
    size,
    modified_unix_ns,
    permissions
});

wire_struct!(crate::io::DirEntry { path, kind });

wire_struct!(crate::io::Tree {
    directories,
    files,
    file_aliases,
    links,
    metadata
});

wire_enum!(crate::net::NetOperation {
    0 => Connect,
    1 => ConnectTls,
    2 => Listen,
    3 => UdpBind,
    4 => UdpPeer,
    5 => ListenTls,
    6 => UnixConnect,
    7 => UnixListen,
    8 => UnixListenRemoveStale
});

wire_struct!(crate::net::NetRule {
    address,
    port,
    operation
});

wire_enum!(crate::net::NetTimeout {
    0 => Unbounded,
    1 => Nanos(v0)
});

wire_enum!(crate::net::ShutdownHow {
    0 => Read,
    1 => Write,
    2 => Both
});

wire_enum!(crate::net::TimeoutKind {
    0 => Read,
    1 => Write
});

wire_struct!(crate::net::SocketAddr { ip, port });

wire_enum!(crate::net::TlsFailure {
    0 => UnknownIssuer,
    1 => Expired,
    2 => NotYetValid,
    3 => NameMismatch,
    4 => Revoked,
    5 => BadCertificate,
    6 => AlertReceived(v0),
    7 => HandshakeFailure,
    8 => PeerMisbehaved,
    9 => Truncated,
    10 => CertificateRequired
});

wire_enum!(crate::net::NetError {
    0 => NotGranted,
    1 => Canceled(v0),
    2 => InvalidInput,
    3 => UnknownHost,
    4 => ResolverFailure,
    5 => ConnectionRefused,
    6 => ConnectionReset,
    7 => ConnectionAborted,
    8 => NotConnected,
    9 => AddrInUse,
    10 => AddrNotAvailable,
    11 => HostUnreachable,
    12 => NetworkUnreachable,
    13 => NetworkDown,
    14 => BrokenPipe,
    15 => TimedOut,
    16 => Tls(v0),
    17 => Fault(v0)
});

wire_enum!(crate::net::RootsDigest {
    0 => WebPki(v0),
    1 => Pem(v0)
});

wire_struct!(crate::net::TlsServerDigest {
    identities,
    clients,
    alpn
});

wire_enum!(crate::watch::WatchEvent {
    0 => Created(v0),
    1 => Modified(v0),
    2 => Removed(v0),
    3 => Renamed(v0, v1),
    4 => Overflow
});

wire_struct!(crate::process::Deadline { clock, ticks });

wire_enum!(hirsel_run_schema::CallForm {
    0 => Compute,
    1 => Call
});

wire_struct!(hirsel_run_schema::RunnerRequest {
    runner,
    form,
    path,
    grants,
    definitions,
    key,
    digest,
    argument
});

wire_enum!(hirsel_run_schema::CallOutcome {
    0 => Ok(v0),
    1 => Failed(v0),
    2 => Faulted(v0),
    3 => NotRun(v0),
    4 => InDoubt(v0),
    5 => Canceled(v0)
});

wire_struct!(hirsel_run_schema::RunnerCompletion { outcome, callee });

wire_enum!(hirsel_run_schema::CalleeOutcome {
    0 => Ok(v0),
    1 => Err(v0),
    2 => Failed(v0)
});

wire_struct!(hirsel_run_schema::ProcessRule { program, env });

wire_struct!(hirsel_run_schema::ChildRun {
    program,
    args,
    steps
});

wire_enum!(hirsel_run_schema::ChildStep {
    0 => Print(v0),
    1 => PrintErr(v0),
    2 => Expect(v0),
    3 => Exit(v0)
});

wire_struct!(hirsel_run_schema::ProcessRun {
    program,
    args,
    outcome
});

wire_enum!(hirsel_run_schema::ProcessOutcome {
    0 => Output{ status, stdout, stderr },
    1 => Error(v0)
});

wire_enum!(hirsel_run_schema::PipelineOutcome {
    0 => Output{ statuses, stdout, stderr },
    1 => Error(v0)
});

wire_struct!(hirsel_run_schema::GroupOutput { statuses, stderr });

wire_enum!(hirsel_run_schema::ExitStatus {
    0 => Exited(v0),
    1 => Signaled(v0)
});

wire_enum!(hirsel_run_schema::ProcessFailure {
    0 => NotGranted,
    1 => InvalidInput,
    2 => NotFound,
    3 => PermissionDenied,
    4 => OutputLimit,
    5 => Fault(v0),
    6 => TimedOut,
    7 => Canceled(v0),
    8 => InputClosed
});

wire_struct!(hirsel_run_schema::Command {
    program,
    args,
    directory,
    env,
    own_group
});

wire_enum!(hirsel_run_schema::ChildStream {
    0 => Stdout,
    1 => Stderr
});

wire_enum!(hirsel_run_schema::Signal {
    0 => Interrupt,
    1 => Terminate,
    2 => Kill
});

wire_struct!(hirsel_run_schema::NetPeer {
    address,
    port,
    resolves_to,
    tls,
    connections
});

wire_struct!(hirsel_run_schema::NetListener {
    address,
    port,
    clients
});

wire_struct!(hirsel_run_schema::NetClient {
    address,
    port,
    tls,
    steps
});

wire_enum!(hirsel_run_schema::PeerTls {
    0 => Plain,
    1 => Certificate(v0),
    2 => Fails(v0),
    3 => Presents(v0, v1),
    4 => Serves(v0),
    5 => Offers(v0, v1)
});

wire_struct!(hirsel_run_schema::TlsServer {
    identities,
    clients,
    alpn
});

wire_struct!(hirsel_run_schema::TlsIdentity { chain, key });

wire_enum!(hirsel_run_schema::TlsFailure {
    0 => UnknownIssuer,
    1 => Expired,
    2 => NotYetValid,
    3 => NameMismatch,
    4 => Revoked,
    5 => BadCertificate,
    6 => AlertReceived(v0),
    7 => HandshakeFailure,
    8 => PeerMisbehaved,
    9 => Truncated,
    10 => CertificateRequired
});

wire_enum!(hirsel_run_schema::NetConnection {
    0 => Refused,
    1 => Unanswered,
    2 => Open(v0)
});

wire_enum!(hirsel_run_schema::NetStep {
    0 => Send(v0),
    1 => Expect(v0),
    2 => Stall,
    3 => Close,
    4 => Reset,
    5 => Truncate
});

wire_enum!(hirsel_run_schema::SignalKind {
    0 => Interrupt,
    1 => Terminate,
    2 => Hangup
});

wire_enum!(hirsel_run_schema::BudgetKind {
    0 => Fuel,
    1 => Memory
});

wire_enum!(hirsel_run_schema::Returned {
    0 => Ok,
    1 => Error,
    2 => ExitCode(v0)
});

wire_struct!(hirsel_run_schema::Site {
    definition,
    file,
    line,
    column
});

wire_struct!(hirsel_run_schema::Failure {
    rule,
    site,
    operation
});

wire_enum!(hirsel_run_schema::Environmental {
    0 => CodecLimit,
    1 => Recording(v0),
    2 => Output(v0),
    3 => Stopped
});

wire_enum!(hirsel_run_schema::RunOutcome {
    0 => Returned(v0),
    1 => Failed{ primary, causes },
    2 => Blocked(v0),
    3 => BudgetExceeded(v0),
    4 => Signaled(v0),
    5 => Refused(v0),
    6 => Diverged(v0),
    7 => Environmental(v0),
    8 => Tooling(v0),
    9 => ToolingLimit(v0),
    10 => UndefinedBehavior(v0),
    11 => Trapped(v0)
});

wire_enum!(crate::stop::Stop {
    0 => Blocked(v0),
    1 => BudgetExceeded(v0),
    2 => Refused(v0),
    3 => Diverged(v0),
    4 => Environmental(v0),
    5 => Failed,
    6 => Signaled(v0),
    7 => Requested,
    8 => UndefinedBehavior(v0)
});

wire_struct!(hirsel_run_schema::Wait {
    spawn_path,
    site,
    kind,
    targets
});

wire_struct!(hirsel_run_schema::ExecutionDiagnostic {
    rule,
    site,
    message
});

wire_struct!(hirsel_run_schema::ToolingLimit { kind, limit, site });

impl Wire for crate::io::MapView {
    fn encode(&self, out: &mut Vec<u8>) {
        self.length.encode(out);
        // Host views normally capture their bytes only when recording is enabled.
        // A mapping without an owned capture is borrowed for this serialization.
        match &self.bytes {
            Some(bytes) => bytes.encode(out),
            None => self.contents().encode(out),
        }
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        let length = u64::decode(input)?;
        let bytes = Vec::<u8>::decode(input)?;
        if length != bytes.len() as u64 {
            return Err("binary mapping length mismatch".into());
        }
        Ok(Self::of(bytes))
    }
}
impl<T: Wire> Wire for crate::net::ClientAuth<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Anonymous => 0u32.encode(out),
            Self::Request(v) => {
                1u32.encode(out);
                v.encode(out);
            }
            Self::Require(v) => {
                2u32.encode(out);
                v.encode(out);
            }
        }
    }
    fn decode(input: &mut Decode<'_>) -> Result<Self, String> {
        Ok(match u32::decode(input)? {
            0 => Self::Anonymous,
            1 => Self::Request(Wire::decode(input)?),
            2 => Self::Require(Wire::decode(input)?),
            _ => return Err("bad binary client auth".into()),
        })
    }
}

wire_struct!(crate::journal::Prepared {
    id,
    task,
    issue,
    request
});

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn free_adapter_error_tags_roundtrip_and_reject_unknown_variants() {
        use crate::io::AdapterError::*;
        let errors = [
            InvalidHandle(17),
            ReplayMismatch(18),
            MissingInput(19),
            UnconsumedInput(20),
            InvalidCompletion,
            RecordingFailed("disk".into()),
            Blocked,
            Unsupported("provider".into()),
            Signaled(hirsel_run_schema::SignalKind::Terminate),
            Stopped,
            ProcessFailed {
                child: 9,
                site: 10,
                operation_site: 11,
                path: vec![0, 1],
            },
        ];
        for (tag, error) in errors.into_iter().enumerate() {
            let mut bytes = Vec::new();
            error.encode(&mut bytes);
            assert_eq!(&bytes[..4], &(tag as u32).to_le_bytes());
            assert_eq!(decode::<crate::io::AdapterError>(&bytes).unwrap(), error);
            bytes.push(0);
            assert!(decode::<crate::io::AdapterError>(&bytes).is_err());
        }
        assert!(decode::<crate::io::AdapterError>(&11u32.to_le_bytes()).is_err());
        assert!(decode::<crate::io::AdapterError>(&[6, 0, 0]).is_err());
    }

    #[test]
    fn open_append_tag_is_additive_and_unknown_request_refuses() {
        use crate::io::Request;
        for (tag, request) in [
            (
                52u32,
                Request::Open {
                    directory: 7,
                    path: "log".into(),
                },
            ),
            (
                53,
                Request::Create {
                    directory: 7,
                    path: "log".into(),
                },
            ),
            (
                114,
                Request::OpenAppend {
                    directory: 7,
                    path: "log".into(),
                },
            ),
        ] {
            let mut encoded = vec![];
            request.encode(&mut encoded);
            assert_eq!(&encoded[..4], &tag.to_le_bytes());
            assert_eq!(decode::<Request>(&encoded).unwrap(), request);
        }
        assert_eq!(
            decode::<Request>(&u32::MAX.to_le_bytes()).unwrap_err(),
            "unknown binary tape variant"
        );
    }

    #[test]
    fn retired_walk_binary_recording_requires_recording_again() {
        let mut old = 59u32.to_le_bytes().to_vec();
        old.extend_from_slice(&7u64.to_le_bytes());
        assert_eq!(
            decode::<crate::io::Request>(&old).unwrap_err(),
            "retired Request::Walk recording; re-record with streaming file.walk"
        );
        let request = crate::io::Request::ReadDirPage {
            directory: 7,
            path: String::new(),
            after: String::new(),
            recursive: false,
        };
        let mut bytes = vec![];
        request.encode(&mut bytes);
        assert_eq!(&bytes[..4], &115u32.to_le_bytes());
        assert_eq!(decode::<crate::io::Request>(&bytes).unwrap(), request);
    }
    #[test]
    fn fixed_integer_vectors_and_bad_lengths() {
        // Bincode 1.3 fixed-int vectors: LE scalar, u64 byte-span count,
        // u32 result discriminant followed by the value, u8 option tag.
        let mut bytes = vec![];
        0x1234u16.encode(&mut bytes);
        vec![0u8, 255].encode(&mut bytes);
        Ok::<u32, u32>(0x12345678).encode(&mut bytes);
        Some(9u8).encode(&mut bytes);
        assert_eq!(
            bytes,
            [
                0x34, 0x12, 2, 0, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 0, 0x78, 0x56, 0x34, 0x12, 1,
                9
            ]
        );
        assert!(decode::<Vec<u8>>(&u64::MAX.to_le_bytes()).is_err());
        assert!(decode::<Vec<String>>(&u64::MAX.to_le_bytes()).is_err());
        assert!(decode::<bool>(&[2]).is_err());
        assert!(decode::<u64>(&[1, 2]).is_err());
    }
}

wire_enum!(crate::io::FileInput {
    0 => Opened(v0),
    1 => Size(v0),
    2 => Bytes(v0)
});
