//! B15: lossy, missing and misframed tape fields across every request/completion variant.
use hirsel_rt::io::*;
use hirsel_rt::launch::{parse_tape, render_tape};
use hirsel_rt::net::{NetOperation, NetRule, NetTimeout, RootsDigest, ShutdownHow, TimeoutKind};
use hirsel_run_schema::{
    CallForm, CallOutcome, CalleeOutcome, ChildStream, Command, ExitStatus, GroupOutput,
    PipelineOutcome, ProcessFailure, ProcessOutcome, ProcessRule, RunnerCompletion, Signal,
};

// The match is exhaustive: adding a Request variant requires a generated specimen here.
macro_rules! requests {
    ($($name:ident { $($field:ident: $value:expr),* }),*; $($tuple:ident($inner:expr)),* $(,)?) => {{
        fn covered(request: &Request) {
            match request { $(Request::$name { .. } => (),)* $(Request::$tuple(..) => (),)* }
        }
        let values = vec![$(Request::$name { $($field: $value),* },)* $(Request::$tuple($inner),)*];
        for request in &values { covered(request); }
        values
    }};
}
fn watch_event(n: u64, text: &str) -> hirsel_rt::watch::WatchEvent {
    use hirsel_rt::watch::WatchEvent::*;
    match n % 5 {
        0 => Created(format!("{text}#created")),
        1 => Modified(text.to_owned()),
        2 => Removed(format!("{text}#removed")),
        3 => Renamed(text.to_owned(), format!("{text}#to")),
        _ => Overflow,
    }
}
pub(super) fn requests(n: u64) -> Vec<Request> {
    let text = ["", "a b\n\t\0é/../->!,:", "文件"][n as usize % 3].to_owned();
    let bytes = if n % 2 == 0 {
        vec![]
    } else {
        vec![0, 255, b' ', b'\n', n as u8]
    };
    let addr = SocketAddr {
        ip: if n % 2 == 0 { "::1" } else { "127.0.0.1" }
            .parse()
            .unwrap(),
        port: n as u16,
    };
    let timeout = if n % 2 == 0 {
        NetTimeout::Unbounded
    } else {
        NetTimeout::Nanos(n)
    };
    let digest = std::array::from_fn(|i| (n.rotate_right(i as u32) as u8).wrapping_add(i as u8));
    let roots = if n % 2 == 0 {
        RootsDigest::WebPki(text.clone())
    } else {
        RootsDigest::Pem(digest)
    };
    let deadline = (n % 2 != 0).then_some(hirsel_rt::process::Deadline {
        clock: n.wrapping_add(3),
        ticks: n.wrapping_add(17),
    });
    let command = Command {
        program: text.clone(),
        directory: (n % 2 != 0).then_some(n),
        args: vec![String::new(), text.clone()],
        own_group: n & 2 != 0,
        env: if n % 2 == 0 {
            None
        } else {
            Some(vec![(text.clone(), format!("value:{text}"))])
        },
    };
    let runs = if n % 2 == 0 {
        vec![]
    } else {
        vec![hirsel_run_schema::ProcessRun {
            program: "/bin/probe".into(),
            args: vec![String::new(), "é\n".into()],
            outcome: ProcessOutcome::Output {
                status: ExitStatus::Exited(n as i32),
                stdout: bytes.clone(),
                stderr: vec![255, 0],
            },
        }]
    };
    let children = if n % 2 == 0 {
        vec![]
    } else {
        vec![hirsel_run_schema::ChildRun {
            program: "/bin/probe".into(),
            args: vec![String::new()],
            steps: vec![
                hirsel_run_schema::ChildStep::Print(bytes.clone()),
                hirsel_run_schema::ChildStep::PrintErr(vec![]),
                hirsel_run_schema::ChildStep::Expect(bytes.clone()),
                hirsel_run_schema::ChildStep::Exit(ExitStatus::Signaled(9)),
            ],
        }]
    };
    let peers = if n % 2 == 0 {
        vec![]
    } else {
        vec![hirsel_run_schema::NetPeer {
            address: "127.0.0.1".into(),
            port: n as u16,
            resolves_to: vec!["127.0.0.2".into()],
            tls: hirsel_run_schema::PeerTls::Plain,
            connections: vec![hirsel_run_schema::NetConnection::Open(vec![
                hirsel_run_schema::NetStep::Send(bytes.clone()),
                hirsel_run_schema::NetStep::Expect(bytes.clone()),
                hirsel_run_schema::NetStep::Close,
            ])],
        }]
    };
    let listeners = if n % 2 == 0 {
        vec![]
    } else {
        vec![hirsel_run_schema::NetListener {
            address: "127.0.0.1".into(),
            port: n as u16,
            clients: vec![hirsel_run_schema::NetClient {
                address: "127.0.0.2".into(),
                port: n.wrapping_add(1) as u16,
                tls: hirsel_run_schema::PeerTls::Plain,
                steps: vec![
                    hirsel_run_schema::NetStep::Send(bytes.clone()),
                    hirsel_run_schema::NetStep::Close,
                ],
            }],
        }]
    };
    // An invalid offer is recorded too: an empty protocol among them.
    let alpn = match n % 4 {
        0 => vec![],
        1 => vec!["h2".to_owned()],
        2 => vec![String::new()],
        _ => vec!["h2".to_owned(), text.clone() + "#alpn", String::new()],
    };
    requests! {
        ProcessTime { from: n, to: n.saturating_add(5) },
        ProcessSettle { canceled: vec![n] },
        Stdin { io: n.wrapping_add(12) },
        Stderr { io: n.wrapping_add(12) },
        ReadStdin { stdin: n.wrapping_add(26), max: n.wrapping_add(14) },
        PrintStderr { stderr: n.wrapping_add(25), bytes: bytes.clone() },
        EnvGet { env: n.wrapping_add(8), name: if n == 0 { String::new() } else { format!("{text}#name") } },
        EnvNarrow { env: n.wrapping_add(8), names: vec![String::new(), text.clone()] },
        MonotonicClock { io: n.wrapping_add(12) },
        WallClock { io: n.wrapping_add(12) },
        Monotonic { clock: n.wrapping_add(3) },
        Wall { clock: n.wrapping_add(3) },
        Random { io: n.wrapping_add(12) },
        RandomBytes { random: n.wrapping_add(23), count: n.wrapping_add(5) },
        Telemetry { io: n.wrapping_add(31) },
        TelemetryBegin { telemetry: n, parent: bytes.clone(), spec: bytes.clone() },
        TelemetryEvent { telemetry: n, span: n.wrapping_add(1), payload: bytes.clone() },
        TelemetryFinish { telemetry: n, span: n.wrapping_add(2), status: bytes.clone() },
        TelemetryLog { telemetry: n, payload: bytes.clone() },
        TelemetryMeasure { telemetry: n, payload: bytes.clone() },
        SleepUntil { clock: n.wrapping_add(3), deadline: n.wrapping_add(6) },
        Tasks { io: n.wrapping_add(12) },
        Net { io: n.wrapping_add(12) },
        Process { io: n.wrapping_add(12) },
        Signals { io: n.wrapping_add(41) },
        SignalWait { signals: n.wrapping_add(42), kinds: (n % 8) as u8 },
        TestProcess { test: n.wrapping_add(29), runs: runs, children: children },
        ProcessNarrow { process: n.wrapping_add(22), rules: vec![ProcessRule { program: if n == 0 { String::new() } else { format!("{text}#program") }, env: vec![text.clone()] }] },
        ProcessRun { process: n.wrapping_add(22), command: command.clone(), stdin: bytes.clone(), max_output: n.wrapping_add(15), deadline: deadline },
        ProcessPipeline { process: n.wrapping_add(22), stages: vec![command.clone(), Command { args: vec!["second".into(), text.clone()], ..command.clone() }], stdin: bytes.clone(), max_output: n.wrapping_add(15), deadline: deadline },
        ProcessSpawn { process: n.wrapping_add(22), command: command.clone() },
        ProcessSpawnPipeline { process: n.wrapping_add(22), stages: vec![command.clone(), Command { args: vec!["second".into(), text.clone()], ..command.clone() }], max_stderr: n.wrapping_add(16) },
        ChildWrite { process: n.wrapping_add(22), child: n.wrapping_add(2), bytes: bytes.clone() },
        ChildCloseStdin { process: n.wrapping_add(22), child: n.wrapping_add(2) },
        ChildRead { process: n.wrapping_add(22), child: n.wrapping_add(2), stream: if n % 2 == 0 { ChildStream::Stdout } else { ChildStream::Stderr }, max: n.wrapping_add(14) },
        ChildWait { process: n.wrapping_add(22), child: n.wrapping_add(2), deadline: deadline },
        ChildKill { process: n.wrapping_add(22), child: n.wrapping_add(2), signal: [Signal::Interrupt, Signal::Terminate, Signal::Kill][n as usize % 3] },
        GroupWait { process: n.wrapping_add(22), group: n.wrapping_add(10), deadline: deadline },
        TestCheck { test: n.wrapping_add(29), condition: n.wrapping_add(4) % 2 == 0 },
        TestCheckCase { test: n.wrapping_add(29), condition: n.wrapping_add(4) % 2 == 0, case: n.wrapping_add(1) },
        TestFail { test: n.wrapping_add(29) },
        TestClock { test: n.wrapping_add(29) },
        TestSeed { test: n.wrapping_add(29) },
        TestTasks { test: n.wrapping_add(29) },
        Stopped {},
        TestLog { test: n.wrapping_add(29), text: if n == 0 { String::new() } else { format!("{text}#text") } },
        TestSnapshot { test: n.wrapping_add(29), name: if n == 0 { String::new() } else { format!("{text}#name") }, text: if n == 0 { String::new() } else { format!("{text}#text") } },
        TestNet { test: n.wrapping_add(29), peers: peers, listeners: listeners },
        TestDir { test: n.wrapping_add(29), entries: vec![(text.clone(), bytes.clone())] },
        TestCrash { test: n.wrapping_add(29), directory: n.wrapping_add(7), seed: n.wrapping_add(31) },
        Stdout { io: n.wrapping_add(12) },
        Directory { io: n.wrapping_add(12), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        NarrowDir { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        ReadOnlyDir { directory: n.wrapping_add(7) },
        Print { stdout: n.wrapping_add(27), bytes: bytes.clone() },
        Open { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        OpenAppend { directory: n.wrapping_add(7), path: text.clone() },
        Create { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        FileRead { directory: n.wrapping_add(7), file: n.wrapping_add(9), max: n.wrapping_add(14) },
        FileSeek { directory: n.wrapping_add(7), file: n.wrapping_add(9), offset: n.wrapping_add(18) },
        FileWrite { directory: n.wrapping_add(7), file: n.wrapping_add(9), bytes: bytes.clone() },
        FileAppend { directory: n.wrapping_add(7), file: n.wrapping_add(9), bytes: bytes.clone() },
        NarrowFile { directory: n.wrapping_add(7), file: n.wrapping_add(9) },

        ReadDirPage { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") }, after: format!("{text}#after"), recursive: n % 2 == 0 },
        Metadata { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        Mkdir { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        Rename { directory: n.wrapping_add(7), from: if n == 0 { String::new() } else { format!("{text}#from") }, to: if n == 0 { String::new() } else { format!("{text}#to") } },
        RemoveDir { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        SetPermissions { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") }, permissions: n.wrapping_add(19) as u32 },
        RemoveAll { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        Size { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        Read { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        Write { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") }, bytes: bytes.clone() },
        Remove { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        CreateAtomic { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        WriteAll { directory: n.wrapping_add(7), file: n.wrapping_add(9), bytes: bytes.clone() },
        Commit { directory: n.wrapping_add(7), file: n.wrapping_add(9) },
        Sync { directory: n.wrapping_add(7), file: n.wrapping_add(9) },
        PutNew { directory: n.wrapping_add(7), path: text.clone(), bytes: bytes.clone(), durable: n % 2 == 0 },
        Map { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        MapView { directory: n.wrapping_add(7), mapped: n.wrapping_add(9) },
        MapCheck { directory: n.wrapping_add(7), mapped: n.wrapping_add(9) },
        Watch { directory: n.wrapping_add(7), path: if n == 0 { String::new() } else { format!("{text}#path") }, recursive: n % 2 == 0 },
        WatchNext { directory: n.wrapping_add(7), watcher: n.wrapping_add(9) },
        TestDirEvent { test: n.wrapping_add(29), directory: n.wrapping_add(7), event: watch_event(n, &text) },
        Close { handle: n.wrapping_add(11) },
        NetNarrow { net: n.wrapping_add(17), rules: vec![NetRule { address: if n == 0 { String::new() } else { format!("{text}#address") }, port: n.wrapping_add(21) as u16, operation: NetOperation::Connect }] },
        Resolve { net: n.wrapping_add(17), host: if n == 0 { String::new() } else { format!("{text}#host") }, port: n.wrapping_add(21) as u16 },
        Connect { net: n.wrapping_add(17), host: if n == 0 { String::new() } else { format!("{text}#host") }, port: n.wrapping_add(21) as u16, timeout: timeout },
        UdpBind { net: n.wrapping_add(17), addr: addr },
        UdpSend { net: n.wrapping_add(17), socket: n.wrapping_add(24), bytes: bytes.clone(), addr: addr },
        UdpRecv { net: n.wrapping_add(17), socket: n.wrapping_add(24), max: n.wrapping_add(14) },
        ConnectAddr { net: n.wrapping_add(17), addr: addr, timeout: timeout },
        NetRead { net: n.wrapping_add(17), stream: n.wrapping_add(28), max: n.wrapping_add(14) },
        NetWrite { net: n.wrapping_add(17), stream: n.wrapping_add(28), bytes: bytes.clone() },
        NetWriteAll { net: n.wrapping_add(17), stream: n.wrapping_add(28), bytes: bytes.clone() },
        Shutdown { net: n.wrapping_add(17), stream: n.wrapping_add(28), how: [ShutdownHow::Read, ShutdownHow::Write, ShutdownHow::Both][n as usize % 3] },
        SetTimeout { net: n.wrapping_add(17), stream: n.wrapping_add(28), which: if n % 2 == 0 { TimeoutKind::Read } else { TimeoutKind::Write }, timeout: timeout },
        PeerAddr { net: n.wrapping_add(17), stream: n.wrapping_add(28) },
        TlsConnect { net: n.wrapping_add(17), stream: n.wrapping_add(28), server_name: if n == 0 { String::new() } else { format!("{text}#server_name") }, roots: roots.clone(), identity: (n % 2 != 0).then_some(digest), alpn: alpn.clone() },
        TlsAccept { net: n.wrapping_add(17), stream: n.wrapping_add(28), server: hirsel_rt::net::TlsServerDigest { identities: vec![digest], clients: match n % 3 { 0 => hirsel_rt::net::ClientAuth::Anonymous, 1 => hirsel_rt::net::ClientAuth::Request(digest), _ => hirsel_rt::net::ClientAuth::Require(digest) }, alpn: alpn.clone() } },
        TlsPeer { net: n.wrapping_add(17), stream: n.wrapping_add(28) },
        TlsProtocol { net: n.wrapping_add(17), stream: n.wrapping_add(28) },
        NetSplit { net: n.wrapping_add(17), stream: n.wrapping_add(28) },
        NetJoin { net: n.wrapping_add(17), read: n.wrapping_add(31), write: n.wrapping_add(32) },
        TlsClose { net: n.wrapping_add(17), stream: n.wrapping_add(28) },
        Listen { net: n.wrapping_add(17), addr: addr },
        Accept { net: n.wrapping_add(17), listener: n.wrapping_add(13) },
        LocalAddr { net: n.wrapping_add(17), listener: n.wrapping_add(13) },
        UnixConnect { net: n.wrapping_add(17), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        UnixListen { net: n.wrapping_add(17), path: if n == 0 { String::new() } else { format!("{text}#path") } },
        RunnerCancel { key: digest, cause: n as u32 },
        RunnerPoll { key: digest, cancel: (n % 2 != 0).then_some(n as u32) },
        Ready { wait: n.wrapping_add(30), points: n.wrapping_add(20) },
        Schedule { count: n.wrapping_add(2), choice: n % 2 },
        CpuControl { width: n as u32, action: (n % 3) as u32, count: n, outcome: (n % 32) as u32 };
        CpuSummary(hirsel_rt::io::CpuSummary {
            version: 2, retirement: n, ordinal: n % 32, value: bytes.clone(), stop: None,
            native: Some(hirsel_rt::io::CpuNativeSummary { descriptor: n as u32,
            value_bytes: n % 257, retained_count: (n % 1025) as u32,
            pieces: vec![hirsel_rt::io::CpuPiece { credit: 0, fuel: n, net: n as i64, envelope: vec![(n, 1)] }],
            level: n, limit: u64::MAX, fuel: u64::MAX, }),
        }),
        RunnerCall(Box::new(hirsel_run_schema::RunnerRequest {
            runner: if n == 0 { String::new() } else { format!("{text}#runner") },
            form: if n % 2 == 0 { CallForm::Compute } else { CallForm::Call },
            path: format!("{text}#path"),
            grants: bytes.clone(),
            definitions: vec![b'n', n as u8],
            key: digest,
            digest: std::array::from_fn(|i| digest[31 - i]),
            argument: bytes.clone(),
        })),
        CallReturn([CalleeOutcome::Ok(bytes.clone()), CalleeOutcome::Err(vec![]), CalleeOutcome::Failed(bytes.clone())][n as usize % 3].clone())
    }
}

fn completions(n: u64) -> Vec<Completion> {
    let addr = SocketAddr {
        ip: "2001:db8::1".parse().unwrap(),
        port: n as u16,
    };
    let bytes = vec![0, 255, n as u8];
    let text = "é -> !, :\n\0".to_owned();
    let io_error = [
        IoError::NotGranted,
        IoError::Fault(n as u32),
        IoError::Canceled(n as u32),
        IoError::NotFound,
        IoError::PermissionDenied,
        IoError::AlreadyExists,
        IoError::IsDirectory,
        IoError::NotDirectory,
        IoError::DirectoryNotEmpty,
        IoError::CrossDevice,
    ][n as usize % 10]
        .clone();
    let net_error = NetError::Fault(n as u32);
    let process_error = ProcessFailure::Canceled(n as u32);
    macro_rules! result {
        ($ok:expr, $error:expr) => {
            if n % 2 == 0 { Ok($ok) } else { Err($error) }
        };
    }
    let values = vec![
        Completion::Input(result!(bytes.clone(), InputError::Fault(n as u32))),
        Completion::Env(result!(Some(text.clone()), EnvError::NotGranted)),
        Completion::ClockGrant(result!(n, ())),
        Completion::Monotonic(result!(n, ClockError::OutOfRange)),
        Completion::Wall(result!(n as i64, ClockError::Unavailable)),
        Completion::Random(result!(bytes.clone(), RandomError::Unavailable)),
        Completion::Wait(result!((), WaitError::Canceled(n as u32))),
        Completion::Signal(if n % 2 == 0 {
            Ok(hirsel_run_schema::SignalKind::ALL[n as usize / 2 % 3])
        } else {
            Err([
                SignalError::Canceled(n as u32),
                SignalError::NotGranted,
                SignalError::InvalidInput,
            ][n as usize / 2 % 3]
                .clone())
        }),
        Completion::Refused(n),
        Completion::Checked(n % 2 == 0),
        Completion::Handle(result!(n, io_error.clone())),
        Completion::OptionalFile((n % 2 == 0).then_some(n)),
        Completion::Unit(result!((), io_error.clone())),
        Completion::Size(result!(n, io_error.clone())),
        Completion::Bytes(result!(bytes.clone(), io_error.clone())),
        Completion::View(MapView::of(bytes.clone())),
        Completion::Watch(result!(watch_event(n, &text), io_error.clone())),
        Completion::Entries(result!(
            vec![DirEntry {
                path: text.clone(),
                kind: FileKind::Symlink
            }],
            io_error.clone()
        )),
        Completion::Metadata(result!(
            Metadata {
                kind: [
                    FileKind::Regular,
                    FileKind::Directory,
                    FileKind::Symlink,
                    FileKind::Other
                ][n as usize % 4],
                size: n,
                modified_unix_ns: Some(-(n as i128)),
                permissions: n as u32
            },
            io_error
        )),
        Completion::NetHandle(result!(n, net_error.clone())),
        Completion::Listened(result!((n, addr), net_error.clone())),
        Completion::Addresses(result!(vec![addr, SocketAddr::UNNAMED], net_error.clone())),
        Completion::NetBytes(result!(bytes.clone(), net_error.clone())),
        Completion::Datagram(result!((bytes.clone(), addr, true), net_error.clone())),
        Completion::Count(result!(n, net_error.clone())),
        Completion::NetUnit(result!((), net_error.clone())),
        Completion::Certificates(result!(Some(bytes.clone()), net_error.clone())),
        Completion::Protocol(result!(
            (n % 4 == 0).then(|| text.clone()),
            net_error.clone()
        )),
        Completion::Address(result!(addr, net_error.clone())),
        Completion::Halves(result!(
            (n.wrapping_add(31), n.wrapping_add(32)),
            net_error.clone()
        )),
        Completion::Accepted(result!((n, addr), net_error)),
        Completion::Process(if n % 2 == 0 {
            ProcessOutcome::Output {
                status: ExitStatus::Exited(n as i32),
                stdout: bytes.clone(),
                stderr: vec![],
            }
        } else {
            ProcessOutcome::Error(process_error)
        }),
        Completion::Pipeline(if n % 2 == 0 {
            PipelineOutcome::Output {
                statuses: vec![ExitStatus::Exited(n as i32), ExitStatus::Signaled(9)],
                stdout: bytes.clone(),
                stderr: vec![0, 255],
            }
        } else {
            PipelineOutcome::Error(process_error)
        }),
        Completion::Spawned(result!(n, process_error)),
        Completion::ChildUnit(result!((), process_error)),
        Completion::ChildBytes(result!(bytes.clone(), process_error)),
        Completion::ChildExit(result!(ExitStatus::Signaled(n as i32), process_error)),
        Completion::GroupExit(result!(
            GroupOutput {
                statuses: vec![ExitStatus::Exited(n as i32), ExitStatus::Signaled(9)],
                stderr: vec![vec![], vec![0, 255]]
            },
            process_error
        )),
        Completion::Runner(RunnerCompletion {
            outcome: [
                CallOutcome::Ok(bytes.clone()),
                CallOutcome::Failed(vec![]),
                CallOutcome::Faulted(vec![255]),
                CallOutcome::NotRun(n as u32),
                CallOutcome::InDoubt(n as u32),
                CallOutcome::Canceled(n as u32),
            ][n as usize % 6]
                .clone(),
            callee: (n % 2 == 0).then(|| [n as u8; 16]),
        }),
        Completion::Blocked {
            committed: n % 2 == 0,
        },
        Completion::Span {
            handle: n,
            trace: [n as u8; 16],
            span: [!(n as u8); 8],
        },
        Completion::Ready,
        Completion::Held,
        Completion::FileInput(FileInput::Opened(n)),
        Completion::FileInput(FileInput::Size(n)),
        Completion::FileInput(FileInput::Bytes(bytes.clone())),
    ];
    // Adding a completion variant must add a specimen, just like requests.
    for completion in &values {
        match completion {
            Completion::FileInput(_) => (),
            Completion::Input(_) => (),
            Completion::Env(_) => (),
            Completion::ClockGrant(_) => (),
            Completion::Monotonic(_) => (),
            Completion::Wall(_) => (),
            Completion::Random(_) => (),
            Completion::Wait(_) => (),
            Completion::Signal(_) => (),
            Completion::Refused(_) => (),
            Completion::Checked(_) => (),
            Completion::Handle(_) => (),
            Completion::OptionalFile(_) => (),
            Completion::Unit(_) => (),
            Completion::Size(_) => (),
            Completion::Bytes(_) => (),
            Completion::Entries(_) => (),
            Completion::Metadata(_) => (),
            Completion::NetHandle(_) => (),
            Completion::Listened(_) => (),
            Completion::Addresses(_) => (),
            Completion::NetBytes(_) => (),
            Completion::Datagram(_) => (),
            Completion::Count(_) => (),
            Completion::NetUnit(_) => (),
            Completion::Certificates(_) => (),
            Completion::Protocol(_) => (),
            Completion::Address(_) => (),
            Completion::Halves(_) => (),
            Completion::Accepted(_) => (),
            Completion::Process(_) => (),
            Completion::Pipeline(_) => (),
            Completion::Spawned(_) => (),
            Completion::ChildUnit(_) => (),
            Completion::ChildBytes(_) => (),
            Completion::ChildExit(_) => (),
            Completion::GroupExit(_) => (),
            Completion::Runner(_) => (),
            Completion::View(_) => (),
            Completion::Watch(_) => (),
            Completion::Blocked { .. } => (),
            Completion::Ready | Completion::Held => (),
            Completion::Span { .. } => (),
        }
    }
    values
}

fn round_trips(draws: usize) {
    let boundaries = [
        0,
        1,
        2,
        255,
        256,
        u32::MAX as u64,
        i64::MAX as u64,
        u64::MAX,
    ];
    let mut count = 0;
    let mut random = 0x429b_08db_634d_21a5u64;
    for draw in 0..draws {
        random = random
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let n = if draw < boundaries.len() {
            boundaries[draw]
        } else {
            random ^ (random >> 29)
        };
        let requests = requests(n);
        let completions = completions(n);
        let len = requests.len().max(completions.len());
        let events: Vec<_> = (0..len)
            .map(|i| Event {
                request: requests[i % requests.len()].clone(),
                completion: completions[(i + draw) % completions.len()].clone(),
                fault: (draw % 3 != 0).then_some(Fault {
                    error: if draw % 2 == 0 {
                        IoError::Fault(n as u32)
                    } else {
                        IoError::Canceled(n as u32)
                    },
                    partial: n.rotate_left(13) as usize,
                }),
                resources: None,
            })
            .collect();
        let rendered = render_tape(&events).unwrap();
        assert_eq!(
            parse_tape(&rendered).unwrap_or_else(|e| panic!("draw {draw}: {e}\n{rendered}")),
            events,
            "draw {draw}"
        );
        count += events.len();
    }
    eprintln!("B15: {count} generated tape entries round-trip");
}
#[test]
fn generated_tape_round_trip() {
    round_trips(32);
}
#[test]
#[ignore = "full gate: expanded tape property"]
fn full_gate_generated_tape_round_trip() {
    round_trips(256);
}
