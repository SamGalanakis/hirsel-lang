//! B12/B13: independently computed path and program-check outcomes, with no
//! recorded host fault fed back into the interpreter as its own oracle.
use hirsel_interp::{
    OperationCall, OperationContext, OperationResult, Operations, Value, library as reference,
};
use hirsel_rt::{io::*, io_real::RealIo, io_sim::SimIo};
use hirsel_span::{SourceMap, Span};
use hirsel_tir::RunConfiguration;
use std::collections::BTreeMap;
use std::path::PathBuf;

struct Reference {
    run: RunConfiguration,
    library: reference::Library,
}
impl Reference {
    fn new(tree: &Tree) -> Self {
        Self::configured(tree, Default::default())
    }
    fn configured(tree: &Tree, config: reference::SimulationConfig) -> Self {
        let run = RunConfiguration {
            entry: hirsel_syntax::DefPath::core("main"),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut library = reference::Library::new(
            &run,
            reference::SimulationConfig {
                files: tree.files.clone(),
                directories: tree.directories.clone(),
                links: tree.links.clone(),
                ..config
            },
        )
        .unwrap();
        assert_eq!(
            library.simulation.grant_root("Io").unwrap(),
            handle("Io", 1)
        );
        Self { run, library }
    }
    fn raw_call(
        &mut self,
        name: &str,
        mut arguments: Vec<Value>,
        bounded: bool,
    ) -> Result<OperationResult, hirsel_interp::Error> {
        let mut types = hirsel_types::TypeStore::default();
        let unit = types.push(hirsel_types::Type::unit());
        let result_type = if bounded {
            types.push(hirsel_types::Type::with_kind(
                hirsel_types::TypeKind::Named {
                    definition: hirsel_syntax::DefPath::core("Bounded"),
                    arguments: vec![hirsel_types::GenericArgument::Type(unit)].into(),
                },
            ))
        } else {
            unit
        };
        let sources = SourceMap::default();
        let mut output = vec![];
        let mut trace = vec![];
        let path = hirsel_syntax::DefPath::core(name);
        let result = self
            .library
            .call(
                OperationCall {
                    runtime: &Default::default(),
                    path: &path,
                    generics: &[],
                    arguments: &mut arguments,
                    result_type,
                    types: &types,
                    site: Span::default(),
                },
                &mut OperationContext {
                    configuration: &self.run,
                    sources: &sources,
                    stdout: &mut output,
                    trace: &mut trace,
                    trace_mode: Default::default(),
                    anchor: None,
                },
            )?
            .expect("known core operation");
        Ok(result)
    }
    fn call(&mut self, name: &str, arguments: Vec<Value>) -> Result<Value, hirsel_interp::Error> {
        match self.raw_call(name, arguments, false)? {
            OperationResult::Complete { value, .. } => Ok(value),
            other => panic!("unexpected {name}: {other:?}"),
        }
    }
    fn cancel(&mut self) {
        fn granted(value: Value) -> Value {
            match value {
                Value::Variant {
                    variant: 0,
                    mut fields,
                } => fields.remove(0).unwrap(),
                other => panic!("expected granted root: {other:?}"),
            }
        }
        self.library
            .simulation
            .config
            .grants
            .insert("MonotonicClock".into());
        let tasks = granted(self.call("io.tasks", vec![handle("Io", 1)]).unwrap());
        let clock = granted(
            self.call("io.monotonic_clock", vec![handle("Io", 1)])
                .unwrap(),
        );
        let deadline = Value::Record {
            definition: (&hirsel_syntax::DefPath::core("Deadline")).into(),
            fields: vec![Some(clock), Some(handle("Instant", 0))],
        };
        // Enter an already expired scope through the public operation boundary;
        // the callback is deliberately left pending while probing its body.
        assert!(matches!(
            self.raw_call("task.scope_until", vec![tasks, deadline, Value::Unit], true)
                .unwrap(),
            OperationResult::Call { .. }
        ));
        assert_eq!(self.library.cancellation(), Some(3));
    }
}
fn handle(kind: &str, id: u64) -> Value {
    Value::Handle {
        kind: kind.into(),
        id,
    }
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hirsel-io-property-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Draw(u64);
impl Draw {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 ^ (self.0 >> 29)
    }
    fn pick<'a>(&mut self, choices: &'a [&'a str]) -> &'a str {
        choices[self.next() as usize % choices.len()]
    }
}

/// A small recursive specification: resolve one prefix, then the remaining
/// suffix. Link expansion restarts that prefix with a shared hop budget. It
/// uses neither of the implementations' walkers nor host canonicalize (which
/// follows absolute links and can leave the grant).
fn read_reference(tree: &Tree, root: &str, path: &str) -> Result<Vec<u8>, IoError> {
    fn legal(path: &str) -> bool {
        !path.starts_with('/') && !path.contains(['\\', '\0'])
    }
    if !legal(path) {
        return Err(IoError::Fault(1));
    }
    let mut normalized = vec![];
    for part in path.split('/') {
        match part {
            "" | "." => (),
            ".." => {
                normalized.pop().ok_or(IoError::Fault(1))?;
            }
            _ => normalized.push(part),
        }
    }
    fn resolve(
        tree: &Tree,
        root: &str,
        at: String,
        parts: &[&str],
        hops: &mut usize,
    ) -> Result<String, IoError> {
        let Some((part, tail)) = parts.split_first() else {
            return Ok(at);
        };
        match *part {
            "" | "." => resolve(tree, root, at, tail, hops),
            ".." => {
                if at == root {
                    return Err(IoError::Fault(1));
                }
                resolve(
                    tree,
                    root,
                    at.rsplit_once('/').map_or("", |(p, _)| p).into(),
                    tail,
                    hops,
                )
            }
            name => {
                let child = if at.is_empty() {
                    name.into()
                } else {
                    format!("{at}/{name}")
                };
                if let Some(target) = tree.links.get(&child) {
                    *hops += 1;
                    if *hops > 40 || !legal(target) {
                        return Err(IoError::Fault(1));
                    }
                    let expanded = format!(
                        "{target}{}{}",
                        if tail.is_empty() { "" } else { "/" },
                        tail.join("/")
                    );
                    resolve(
                        tree,
                        root,
                        at,
                        &expanded.split('/').collect::<Vec<_>>(),
                        hops,
                    )
                } else {
                    if !tail.is_empty() && !tree.directories.contains(&child) {
                        return Err(if tree.files.contains_key(&child) {
                            IoError::NotDirectory
                        } else {
                            IoError::NotFound
                        });
                    }
                    resolve(tree, root, child, tail, hops)
                }
            }
        }
    }
    let resolved = resolve(tree, root, root.into(), &normalized, &mut 0)?;
    if tree.directories.contains(&resolved) {
        return Err(IoError::IsDirectory);
    }
    tree.files.get(&resolved).cloned().ok_or(IoError::NotFound)
}

fn confinement(seeds: u64, draws: usize) {
    let mut count = 0;
    for seed in 0..seeds {
        let mut rng = Draw(seed);
        let scratch = Scratch::new();
        let mut tree = Tree::new(BTreeMap::from([
            ("top".into(), vec![seed as u8, 255]),
            ("d0/f".into(), b"zero".to_vec()),
            ("d0/n/f".into(), b"nested".to_vec()),
            ("d1/f".into(), b"one".to_vec()),
        ]))
        .unwrap();
        for i in 0..8 {
            let parent = rng.pick(&["", "d0/", "d0/n/", "d1/"]);
            let target = rng.pick(&[
                "d0",
                "d1/f",
                "../top",
                "../d1",
                "../../top",
                "f",
                "n/f",
                "/top",
                "missing",
                ".",
                "..",
                "cycle",
            ]);
            tree.links.insert(format!("{parent}l{i}"), target.into());
        }
        tree.links.extend([
            ("alias".into(), "d0".into()),
            ("d0/up".into(), "../top".into()),
            ("cycle".into(), "cycle".into()),
        ]);
        for directory in &tree.directories {
            std::fs::create_dir_all(scratch.0.join(directory)).unwrap();
        }
        for (name, bytes) in &tree.files {
            std::fs::write(scratch.0.join(name), bytes).unwrap();
        }
        for (name, target) in &tree.links {
            std::os::unix::fs::symlink(target, scratch.0.join(name)).unwrap();
        }
        let mut real = RealIo::grant(&scratch.0).unwrap();
        crate::rt_support::host(&mut real);
        let mut reference = Reference::new(&tree);
        let mut tape = vec![];
        for (request, name, arguments, expected) in [
            (
                Request::Directory {
                    io: 1,
                    path: String::new(),
                },
                "io.dir",
                vec![handle("Io", 1), reference::path("")],
                Completion::Handle(Ok(2)),
            ),
            (
                Request::NarrowDir {
                    directory: 2,
                    path: "d0".into(),
                },
                "dir.narrow",
                vec![handle("Dir", 2), reference::path("d0")],
                Completion::Handle(Ok(3)),
            ),
        ] {
            assert_eq!(real.call(request.clone()), Ok(expected.clone()));
            assert_eq!(
                crate::rt_replay::decode(&expected, reference.call(name, arguments).unwrap()),
                expected
            );
            tape.push(Event {
                request,
                completion: expected,
                fault: None,
            });
        }
        let mut paths: Vec<String> = [
            "top",
            "d0/f",
            "alias/n/../f",
            "alias/../top",
            "d0/up",
            "../top",
            "cycle",
            "d0/f/../f",
            "missing/../top",
            "d0/n/../../top",
            "/top",
            "a\\b",
            "a\0b",
            "",
            ".",
            "d0/",
            "d0/f/",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        paths.extend(tree.links.keys().cloned());
        for _ in 0..draws {
            let mut path = String::new();
            for i in 0..(1 + rng.next() % 6) {
                if i > 0 {
                    path.push('/');
                }
                path.push_str(rng.pick(&[
                    "d0", "d1", "n", "f", "top", "l0", "l1", "l2", "alias", "up", "..", ".", "",
                    "missing",
                ]));
            }
            paths.push(path);
        }
        for (directory, root) in [(2, ""), (3, "d0")] {
            for path in &paths {
                let expected = Completion::Bytes(read_reference(&tree, root, path));
                let request = Request::Read {
                    directory,
                    path: path.clone(),
                };
                assert_eq!(
                    real.call(request.clone()),
                    Ok(expected.clone()),
                    "B12 real seed={seed} root={root:?} path={path:?} links={:?}",
                    tree.links
                );
                let value = reference
                    .call(
                        "file.read_bytes",
                        vec![handle("Dir", directory), reference::path(path)],
                    )
                    .unwrap();
                assert_eq!(
                    crate::rt_replay::decode(&expected, value),
                    expected,
                    "B12 interpreter seed={seed} root={root:?} path={path:?}"
                );
                tape.push(Event {
                    request,
                    completion: expected,
                    fault: None,
                });
                count += 1;
            }
        }
        // No injected host error: the replay walker must independently derive
        // every expected refusal and every successful byte sequence.
        let mut sim = SimIo::new(tree, tape.clone());
        crate::rt_support::host(&mut sim);
        for event in tape {
            assert_eq!(
                sim.call(event.request.clone()),
                Ok(event.completion),
                "B12 replay seed={seed} {:?}",
                event.request
            );
        }
        sim.finish().unwrap();
    }
    eprintln!("B12: {count} reads, three implementations and an independent reference");
}
#[test]
fn generated_confinement_differential() {
    confinement(4, 32);
}
#[test]
#[ignore = "full gate: expanded confinement differential"]
fn full_gate_generated_confinement_differential() {
    confinement(128, 256);
}

fn file_request(kind: usize, directory: u64, file: u64, invalid: bool, round: usize) -> Request {
    let path = if invalid {
        [
            "../escape",
            "/absolute",
            "a\\b",
            "a\0b",
            "f/../../escape",
            "../",
            "..",
            "f/../../../escape",
        ][round % 8]
    } else {
        "f"
    }
    .to_owned();
    let bytes = b"value".to_vec();
    match kind {
        0 => Request::Open { directory, path },
        1 => Request::Create { directory, path },
        2 => Request::FileRead {
            directory,
            file,
            max: 1,
        },
        3 => Request::FileWrite {
            directory,
            file,
            bytes,
        },
        4 => Request::FileAppend {
            directory,
            file,
            bytes,
        },
        5 => Request::FileSeek {
            directory,
            file,
            offset: if invalid { u64::MAX } else { 0 },
        },
        6 => Request::Read { directory, path },
        7 => Request::Write {
            directory,
            path,
            bytes,
        },
        8 => Request::Size { directory, path },
        9 => Request::Walk { directory },
        10 => Request::ReadDir { directory, path },
        11 => Request::Metadata { directory, path },
        12 => Request::Remove { directory, path },
        13 => Request::Mkdir { directory, path },
        14 => Request::RemoveDir { directory, path },
        15 => Request::RemoveAll { directory, path },
        16 => Request::SetPermissions {
            directory,
            path,
            permissions: if invalid { 0o1000 } else { 0o600 },
        },
        17 => Request::Rename {
            directory,
            from: path,
            to: "destination".into(),
        },
        18 => Request::CreateAtomic { directory, path },
        19 => Request::WriteAll {
            directory,
            file,
            bytes,
        },
        20 => Request::Commit { directory, file },
        21 => Request::Sync { directory, file },
        22 => Request::PutNew {
            directory,
            path,
            bytes,
            durable: round % 2 == 0,
        },
        _ => unreachable!(),
    }
}
fn file_completion(request: &Request, error: IoError) -> Completion {
    match request {
        Request::Open { .. } | Request::Create { .. } | Request::CreateAtomic { .. } => {
            Completion::Handle(Err(error))
        }
        Request::FileRead { .. } | Request::Read { .. } => Completion::Bytes(Err(error)),
        Request::FileWrite { .. } | Request::Size { .. } => Completion::Size(Err(error)),
        Request::Walk { .. } => Completion::Paths(Err(error)),
        Request::ReadDir { .. } => Completion::Entries(Err(error)),
        Request::Metadata { .. } => Completion::Metadata(Err(error)),
        _ => Completion::Unit(Err(error)),
    }
}

fn setup_call<A: Adapter>(
    real: &mut A,
    reference: &mut Reference,
    request: Request,
    name: &str,
    arguments: Vec<Value>,
) -> Completion {
    let result = real.call(request).unwrap();
    let value = reference.call(name, arguments).unwrap();
    assert_eq!(
        crate::rt_replay::decode(&result, value),
        result,
        "setup {name}"
    );
    result
}
fn granted_id(completion: Completion) -> u64 {
    match completion {
        Completion::Handle(Ok(id)) | Completion::OptionalFile(Some(id)) => id,
        other => panic!("expected grant: {other:?}"),
    }
}
fn precedence_file_matrix(rounds: usize) {
    let mut count = 0;
    for round in 0..rounds {
        for kind in 0..23 {
            // All subsets: bad directory, denied write, bad path/argument,
            // bad secondary handle, cancellation rather than provider fault.
            for conditions in 0..32 {
                let scratch = Scratch::new();
                std::fs::write(scratch.0.join("f"), b"A").unwrap();
                let tree = Tree::new(BTreeMap::from([("f".into(), b"A".to_vec())])).unwrap();
                let mut real = RealIo::grant(&scratch.0).unwrap();
                crate::rt_support::host(&mut real);
                let mut reference = Reference::new(&tree);
                let mut directory = granted_id(setup_call(
                    &mut real,
                    &mut reference,
                    Request::Directory {
                        io: 1,
                        path: String::new(),
                    },
                    "io.dir",
                    vec![handle("Io", 1), reference::path("")],
                ));
                let mut file = if matches!(kind, 2..=5 | 19 | 20 | 21) {
                    let request = if matches!(kind, 19 | 20) {
                        Request::CreateAtomic {
                            directory,
                            path: "new".into(),
                        }
                    } else if matches!(kind, 3 | 4) {
                        Request::Create {
                            directory,
                            path: "new".into(),
                        }
                    } else {
                        Request::Open {
                            directory,
                            path: "f".into(),
                        }
                    };
                    let (name, args) = crate::rt_replay::file_arguments(&request).unwrap();
                    granted_id(setup_call(&mut real, &mut reference, request, name, args))
                } else {
                    0
                };
                if conditions & 2 != 0 {
                    directory = granted_id(setup_call(
                        &mut real,
                        &mut reference,
                        Request::ReadOnlyDir { directory },
                        "dir.read_only",
                        vec![handle("Dir", directory)],
                    ));
                    if matches!(kind, 2..=5 | 21) {
                        file = granted_id(setup_call(
                            &mut real,
                            &mut reference,
                            Request::NarrowFile { directory, file },
                            "file.narrow",
                            vec![handle("Dir", directory), handle("File", file)],
                        ));
                    }
                }
                if conditions & 1 != 0 {
                    directory = 999;
                }
                if conditions & 8 != 0 {
                    file = 998;
                }
                let request = file_request(kind, directory, file, conditions & 4 != 0, round);
                let (name, args) = crate::rt_replay::file_arguments(&request).unwrap();
                let error = if conditions & 16 != 0 {
                    IoError::Canceled(3)
                } else {
                    IoError::Fault(71 + round as u32)
                };
                if conditions & 16 != 0 {
                    reference.cancel();
                } else {
                    reference
                        .library
                        .simulation
                        .config
                        .faults
                        .entry(name.into())
                        .or_default()
                        .push_back(reference::simulation::Fault::Fault {
                            code: 71 + round as u32,
                            partial: 0,
                        });
                }
                let native = real
                    .preflight(&request)
                    .map(|c| c.unwrap_or_else(|| file_completion(&request, error)));
                let interpreted = reference.call(name, args);
                match (native, interpreted) {
                    (
                        Err(AdapterError::InvalidHandle(_)),
                        Err(hirsel_interp::Error::InvalidTir(_)),
                    ) => (),
                    (Ok(expected), Ok(value)) => assert_eq!(
                        crate::rt_replay::decode(&expected, value),
                        expected,
                        "B13 conditions={conditions:05b} {request:?}"
                    ),
                    (native, interpreted) => panic!(
                        "B13 conditions={conditions:05b} {request:?}: native={native:?} interpreter={interpreted:?}"
                    ),
                }
                count += 1;
            }
        }
    }
    eprintln!("B13: {count} filesystem check-precedence combinations");
}
#[test]
fn io_check_precedence_matrix() {
    precedence_file_matrix(1);
}
#[test]
#[ignore = "full gate: expanded precedence matrix"]
fn full_gate_io_check_precedence_matrix() {
    precedence_file_matrix(8);
    precedence_net_matrix();
    precedence_process_matrix();
    precedence_stream_matrix();
}

fn net_completion(request: &Request, error: NetError) -> Completion {
    match request {
        Request::Resolve { .. } => Completion::Addresses(Err(error)),
        Request::Connect { .. }
        | Request::ConnectAddr { .. }
        | Request::UdpBind { .. }
        | Request::UnixConnect { .. } => Completion::NetHandle(Err(error)),
        Request::Accept { .. } => Completion::Accepted(Err(error)),
        Request::UdpRecv { .. } => Completion::Datagram(Err(error)),
        Request::UdpSend { .. } | Request::NetWrite { .. } => Completion::Count(Err(error)),
        Request::NetRead { .. } => Completion::NetBytes(Err(error)),
        _ => Completion::NetUnit(Err(error)),
    }
}
fn net_arguments(request: &Request, completion: &Completion) -> (&'static str, Vec<Value>) {
    const IDENTITY: (&[u8], &[u8]) = (
        include_bytes!("../../../hirsel-rt/tests/it/tls-data/server.cert"),
        include_bytes!("../../../hirsel-rt/tests/it/tls-data/server.key"),
    );
    crate::rt_replay::network::arguments(
        &Event {
            request: request.clone(),
            completion: completion.clone(),
            fault: None,
        },
        crate::rt_replay::ReplayFixture {
            events: &[],
            pem_roots: &[],
            tls_identities: &[IDENTITY],
        },
    )
}
fn net_setup(
    native: &mut impl Adapter,
    reference: &mut Reference,
    request: Request,
    schema: Completion,
) -> Completion {
    let completion = native.call(request.clone()).unwrap();
    let (name, args) = net_arguments(&request, &schema);
    let value = reference.call(name, args).unwrap();
    let decoded = if let Completion::Listened(Ok((id, addr))) = completion {
        assert_eq!(value, reference::ok(handle("Listener", id)));
        let local = reference
            .call(
                "net.local_addr",
                vec![
                    match &request {
                        Request::Listen { net, .. } => handle("Net", *net),
                        _ => unreachable!(),
                    },
                    handle("Listener", id),
                ],
            )
            .unwrap();
        assert_eq!(
            crate::rt_replay::network::decode(&Completion::Address(Ok(addr)), local, &[]),
            Completion::Address(Ok(addr))
        );
        completion.clone()
    } else {
        crate::rt_replay::network::decode(&schema, value, &[])
    };
    assert_eq!(decoded, completion, "net setup {request:?}");
    completion
}
fn net_id(completion: Completion) -> u64 {
    match completion {
        Completion::NetHandle(Ok(id))
        | Completion::Listened(Ok((id, _)))
        | Completion::Accepted(Ok((id, _))) => id,
        other => panic!("expected net handle: {other:?}"),
    }
}
fn precedence_net_matrix() {
    use hirsel_rt::net::{ClientAuth, NetOperation as O, TlsServerDigest};
    use hirsel_run_schema::{NetClient, NetConnection, NetGrant, NetListener, NetPeer, PeerTls};
    let address = SocketAddr {
        ip: "127.0.0.1".parse().unwrap(),
        port: 443,
    };
    let mut count = 0;
    for kind in 0..14 {
        for conditions in 0..16 {
            let denied = conditions & 2 != 0;
            let invalid = conditions & 4 != 0;
            let operation = match kind {
                0..=2 => O::Connect,
                3 => O::UdpBind,
                4 => O::UdpPeer,
                6 => O::UnixConnect,
                _ => O::Connect,
            };
            let mut rules: std::collections::BTreeSet<_> = [
                O::Connect,
                O::ConnectTls,
                O::Listen,
                O::UdpBind,
                O::UdpPeer,
                O::UnixConnect,
            ]
            .into_iter()
            .map(|operation| NetRule {
                operation,
                address: if operation == O::UnixConnect {
                    "/tmp/hirsel-property.sock".into()
                } else {
                    "127.0.0.1".into()
                },
                port: if operation == O::UnixConnect { 0 } else { 443 },
            })
            .collect();
            if denied && matches!(kind, 0..=4 | 6) {
                rules.retain(|r| r.operation != operation);
            }
            if denied && matches!(kind, 8..=10) {
                rules.retain(|r| r.operation != O::Connect);
            }
            let peer = |address: &str, port| NetPeer {
                address: address.into(),
                port,
                resolves_to: vec![],
                tls: PeerTls::Plain,
                connections: vec![NetConnection::Open(vec![])],
            };
            let grant = NetGrant {
                provider: "net.script@2".into(),
                rules: crate::rt_replay::network::rules(&rules),
                peers: Some(vec![
                    peer("127.0.0.1", 443),
                    peer("/tmp/hirsel-property.sock", 0),
                ]),
                listeners: vec![NetListener {
                    address: "127.0.0.1".into(),
                    port: 443,
                    clients: vec![NetClient {
                        address: "127.0.0.1".into(),
                        port: 1000,
                        tls: PeerTls::Plain,
                        steps: vec![],
                    }],
                }],
            };
            let mut native = hirsel_rt::io_net_script::ScriptIo::default();
            crate::rt_support::host(&mut native);
            native.set_net_grant(&grant).unwrap();
            let mut reference = Reference::configured(
                &Tree::default(),
                reference::SimulationConfig {
                    net: Some(grant),
                    ..Default::default()
                },
            );
            let mut net = granted_id(setup_call(
                &mut native,
                &mut reference,
                Request::Net { io: 1 },
                "io.net",
                vec![handle("Io", 1)],
            ));
            let mut secondary = 0;
            if matches!(kind, 4 | 5) {
                secondary = net_id(net_setup(
                    &mut native,
                    &mut reference,
                    Request::UdpBind { net, addr: address },
                    Completion::NetHandle(Ok(0)),
                ));
            } else if kind == 7 || kind == 12 && !denied {
                secondary = net_id(net_setup(
                    &mut native,
                    &mut reference,
                    Request::Listen { net, addr: address },
                    Completion::Listened(Ok((0, address))),
                ));
                if kind == 12 {
                    secondary = net_id(net_setup(
                        &mut native,
                        &mut reference,
                        Request::Accept {
                            net,
                            listener: secondary,
                        },
                        Completion::Accepted(Ok((0, address))),
                    ));
                }
            } else if kind >= 8 {
                secondary = net_id(net_setup(
                    &mut native,
                    &mut reference,
                    Request::Connect {
                        net,
                        host: "127.0.0.1".into(),
                        port: 443,
                        timeout: NetTimeout::Unbounded,
                    },
                    Completion::NetHandle(Ok(0)),
                ));
            }
            let server = TlsServerDigest::of(
                &[(
                    include_bytes!("../../../hirsel-rt/tests/it/tls-data/server.cert"),
                    include_bytes!("../../../hirsel-rt/tests/it/tls-data/server.key"),
                )],
                ClientAuth::Anonymous,
            );
            if kind == 12 {
                native.tls_server(
                    secondary,
                    &[(
                        include_bytes!("../../../hirsel-rt/tests/it/tls-data/server.cert"),
                        include_bytes!("../../../hirsel-rt/tests/it/tls-data/server.key"),
                    )],
                    ClientAuth::Anonymous,
                );
            }
            if conditions & 1 != 0 {
                net = 999;
            }
            if conditions & 8 != 0 {
                secondary = 998;
            }
            let timeout = if invalid {
                NetTimeout::Nanos(0)
            } else {
                NetTimeout::Unbounded
            };
            let host = if invalid { "INVALID HOST" } else { "127.0.0.1" }.into();
            let request = match kind {
                0 => Request::Resolve {
                    net,
                    host,
                    port: 443,
                },
                1 => Request::Connect {
                    net,
                    host,
                    port: 443,
                    timeout,
                },
                2 => Request::ConnectAddr {
                    net,
                    addr: address,
                    timeout,
                },
                3 => Request::UdpBind { net, addr: address },
                4 => Request::UdpSend {
                    net,
                    socket: secondary,
                    bytes: vec![0; if invalid { 65508 } else { 1 }],
                    addr: address,
                },
                5 => Request::UdpRecv {
                    net,
                    socket: secondary,
                    max: if invalid { 0 } else { 1 },
                },
                6 => Request::UnixConnect {
                    net,
                    path: if invalid {
                        "relative"
                    } else {
                        "/tmp/hirsel-property.sock"
                    }
                    .into(),
                },
                7 => Request::Accept {
                    net,
                    listener: secondary,
                },
                8 => Request::NetRead {
                    net,
                    stream: secondary,
                    max: if invalid { 0 } else { 1 },
                },
                9 => Request::NetWrite {
                    net,
                    stream: secondary,
                    bytes: b"A".to_vec(),
                },
                10 => Request::NetWriteAll {
                    net,
                    stream: secondary,
                    bytes: b"A".to_vec(),
                },
                11 => Request::TlsConnect {
                    net,
                    stream: secondary,
                    server_name: if invalid { "bad name" } else { "localhost" }.into(),
                    roots: RootsDigest::WebPki("webpki-roots@1.0.9".into()),
                    identity: None,
                },
                12 => Request::TlsAccept {
                    net,
                    stream: secondary,
                    server: if invalid {
                        TlsServerDigest {
                            identities: vec![],
                            clients: ClientAuth::Anonymous,
                        }
                    } else {
                        server
                    },
                },
                13 => Request::TlsClose {
                    net,
                    stream: secondary,
                },
                _ => unreachable!(),
            };
            let fallback = net_completion(&request, NetError::Canceled(3));
            let expected = native
                .preflight(&request)
                .map(|v| v.unwrap_or(fallback.clone()));
            if conditions == 0 {
                assert_eq!(native.preflight(&request), Ok(None), "healthy {request:?}");
            }
            reference.cancel();
            let (name, args) = net_arguments(&request, &fallback);
            match (expected, reference.call(name, args)) {
                (Err(AdapterError::InvalidHandle(_)), Err(hirsel_interp::Error::InvalidTir(_))) => {
                    ()
                }
                (Ok(expected), Ok(value)) => assert_eq!(
                    crate::rt_replay::network::decode(&expected, value, &[]),
                    expected,
                    "B13 network conditions={conditions:04b} {request:?}"
                ),
                (native, interpreted) => panic!(
                    "B13 network conditions={conditions:04b} {request:?}: native={native:?} interpreter={interpreted:?}"
                ),
            }
            count += 1;
        }
    }
    eprintln!("B13: {count} network precedence combinations");
}

fn variant(tag: u32, values: Vec<Value>) -> Value {
    Value::Variant {
        variant: tag,
        fields: values.into_iter().map(Some).collect(),
    }
}
fn list(values: Vec<Value>) -> Value {
    Value::Sequence {
        kind: hirsel_types::BuiltinType::List,
        elements: values.into_iter().map(Some).collect(),
        arena: None,
    }
}
fn record(name: &str, values: Vec<Value>) -> Value {
    Value::Record {
        definition: (&hirsel_syntax::DefPath::core(name)).into(),
        fields: values.into_iter().map(Some).collect(),
    }
}
fn command_value(command: &hirsel_run_schema::Command) -> Value {
    record(
        "Command",
        vec![
            reference::path(&command.program),
            list(command.args.iter().cloned().map(Value::String).collect()),
            command
                .directory
                .map_or_else(reference::none, |id| reference::some(handle("Dir", id))),
            command.env.as_ref().map_or_else(reference::none, |vars| {
                reference::some(list(
                    vars.iter()
                        .map(|(n, v)| {
                            Value::Tuple(vec![
                                Some(Value::String(n.clone())),
                                Some(Value::String(v.clone())),
                            ])
                        })
                        .collect(),
                ))
            }),
            Value::Bool(command.own_group),
        ],
    )
}
fn deadline_value(deadline: Option<hirsel_rt::process::Deadline>) -> Value {
    deadline.map_or_else(reference::none, |d| {
        reference::some(record(
            "Deadline",
            vec![
                handle("MonotonicClock", d.clock),
                handle("Instant", d.ticks),
            ],
        ))
    })
}
fn process_arguments(request: &Request) -> (&'static str, Vec<Value>) {
    match request {
        Request::ProcessRun {
            process,
            command,
            stdin,
            max_output,
            deadline,
        } => (
            "proc.run",
            vec![
                handle("Proc", *process),
                command_value(command),
                Value::Bytes(stdin.clone()),
                reference::u64_value(*max_output),
                deadline_value(*deadline),
            ],
        ),
        Request::ProcessPipeline {
            process,
            stages,
            stdin,
            max_output,
            deadline,
        } => (
            "proc.pipeline",
            vec![
                handle("Proc", *process),
                list(stages.iter().map(command_value).collect()),
                Value::Bytes(stdin.clone()),
                reference::u64_value(*max_output),
                deadline_value(*deadline),
            ],
        ),
        Request::ProcessSpawn { process, command } => (
            "proc.spawn",
            vec![handle("Proc", *process), command_value(command)],
        ),
        Request::ProcessSpawnPipeline {
            process,
            stages,
            max_stderr,
        } => (
            "proc.spawn_pipeline",
            vec![
                handle("Proc", *process),
                list(stages.iter().map(command_value).collect()),
                reference::u64_value(*max_stderr),
            ],
        ),
        Request::ChildWrite {
            process,
            child,
            bytes,
        } => (
            "child.write",
            vec![
                handle("Proc", *process),
                handle("Child", *child),
                Value::Bytes(bytes.clone()),
            ],
        ),
        Request::ChildRead {
            process,
            child,
            stream,
            max,
        } => (
            if *stream == hirsel_run_schema::ChildStream::Stdout {
                "child.read_stdout"
            } else {
                "child.read_stderr"
            },
            vec![
                handle("Proc", *process),
                handle("Child", *child),
                list(vec![]),
                reference::u64_value(*max),
            ],
        ),
        Request::ChildWait {
            process,
            child,
            deadline,
        } => (
            "child.wait",
            vec![
                handle("Proc", *process),
                handle("Child", *child),
                deadline_value(*deadline),
            ],
        ),
        Request::GroupWait {
            process,
            group,
            deadline,
        } => (
            "proc.wait_group",
            vec![
                handle("Proc", *process),
                handle("ChildGroup", *group),
                deadline_value(*deadline),
            ],
        ),
        _ => unreachable!(),
    }
}
fn process_completion(request: &Request, error: hirsel_run_schema::ProcessFailure) -> Completion {
    match request {
        Request::ProcessRun { .. } => {
            Completion::Process(hirsel_run_schema::ProcessOutcome::Error(error))
        }
        Request::ProcessPipeline { .. } => {
            Completion::Pipeline(hirsel_run_schema::PipelineOutcome::Error(error))
        }
        Request::ProcessSpawn { .. } | Request::ProcessSpawnPipeline { .. } => {
            Completion::Spawned(Err(error))
        }
        Request::ChildWrite { .. } => Completion::ChildUnit(Err(error)),
        Request::ChildRead { .. } => Completion::ChildBytes(Err(error)),
        Request::ChildWait { .. } => Completion::ChildExit(Err(error)),
        Request::GroupWait { .. } => Completion::GroupExit(Err(error)),
        _ => unreachable!(),
    }
}
fn process_error_value(completion: Completion) -> Value {
    use hirsel_run_schema::{PipelineOutcome, ProcessFailure as F, ProcessOutcome};
    let error = match completion {
        Completion::Process(ProcessOutcome::Error(e))
        | Completion::Pipeline(PipelineOutcome::Error(e))
        | Completion::Spawned(Err(e))
        | Completion::ChildUnit(Err(e))
        | Completion::ChildBytes(Err(e))
        | Completion::ChildExit(Err(e))
        | Completion::GroupExit(Err(e)) => e,
        other => panic!("expected process error: {other:?}"),
    };
    let value = match error {
        F::NotGranted => variant(0, vec![]),
        F::InvalidInput => variant(1, vec![]),
        F::NotFound => variant(2, vec![]),
        F::PermissionDenied => variant(3, vec![]),
        F::OutputLimit => variant(4, vec![]),
        F::Fault(n) => variant(5, vec![reference::u32_value(n)]),
        F::TimedOut => variant(6, vec![]),
        F::Canceled(n) => variant(7, vec![variant(n, vec![])]),
        F::InputClosed => variant(8, vec![]),
    };
    reference::error(value)
}
fn precedence_process_matrix() {
    use hirsel_run_schema::{
        ChildRun, ChildStep, Command, ExitStatus, ProcessFailure, ProcessGrant, ProcessRule,
    };
    let command = Command {
        program: "/bin/probe".into(),
        args: vec![],
        directory: None,
        env: None,
        own_group: false,
    };
    let mut count = 0;
    for kind in 0..8 {
        for conditions in 0..16 {
            let grant = ProcessGrant {
                provider: "process.script@1".into(),
                rules: vec![ProcessRule {
                    program: command.program.clone(),
                    env: vec![],
                }],
                runs: Some(vec![]),
                children: Some(vec![ChildRun {
                    program: command.program.clone(),
                    args: vec![],
                    steps: vec![ChildStep::Exit(ExitStatus::Exited(0))],
                }]),
            };
            let mut native = hirsel_rt::io_net_script::ScriptIo::default();
            crate::rt_support::host(&mut native);
            native.set_process_grant(&grant);
            let mut reference = Reference::configured(
                &Tree::default(),
                reference::SimulationConfig {
                    process: Some(grant),
                    ..Default::default()
                },
            );
            let mut process = granted_id(setup_call(
                &mut native,
                &mut reference,
                Request::Process { io: 1 },
                "io.process",
                vec![handle("Io", 1)],
            ));
            let mut child = 0;
            if kind >= 4 {
                let spawn = if kind == 7 {
                    Request::ProcessSpawnPipeline {
                        process,
                        stages: vec![command.clone()],
                        max_stderr: 10,
                    }
                } else {
                    Request::ProcessSpawn {
                        process,
                        command: command.clone(),
                    }
                };
                let (name, args) = process_arguments(&spawn);
                let result = native.call(spawn).unwrap();
                let value = reference.call(name, args).unwrap();
                child = match result {
                    Completion::Spawned(Ok(id)) => id,
                    other => panic!("spawn setup: {other:?}"),
                };
                let expected = reference::ok(handle(
                    if kind == 7 { "ChildGroup" } else { "Child" },
                    child,
                ));
                assert_eq!(value, expected);
                if kind == 4 && conditions & 2 != 0 {
                    assert_eq!(
                        native
                            .call(Request::ChildCloseStdin { process, child })
                            .unwrap(),
                        Completion::ChildUnit(Ok(()))
                    );
                    assert_eq!(
                        reference
                            .call(
                                "child.close_stdin",
                                vec![handle("Proc", process), handle("Child", child)]
                            )
                            .unwrap(),
                        Value::Unit
                    );
                }
            }
            let mut command = command.clone();
            if conditions & 2 != 0 {
                command.program = "/bin/denied".into();
            }
            if conditions & 4 != 0 {
                command.program = "relative".into();
            }
            if conditions & 1 != 0 {
                process = 999;
            }
            if conditions & 8 != 0 {
                child = 998;
            }
            let request = match kind {
                0 => Request::ProcessRun {
                    process,
                    command,
                    stdin: vec![],
                    max_output: 1,
                    deadline: None,
                },
                1 => Request::ProcessPipeline {
                    process,
                    stages: vec![command],
                    stdin: vec![],
                    max_output: 1,
                    deadline: None,
                },
                2 => Request::ProcessSpawn { process, command },
                3 => Request::ProcessSpawnPipeline {
                    process,
                    stages: if conditions & 4 != 0 {
                        vec![]
                    } else {
                        vec![command]
                    },
                    max_stderr: 1,
                },
                4 => Request::ChildWrite {
                    process,
                    child,
                    bytes: b"A".to_vec(),
                },
                5 => Request::ChildRead {
                    process,
                    child,
                    stream: hirsel_run_schema::ChildStream::Stdout,
                    max: if conditions & 4 != 0 { 0 } else { 1 },
                },
                6 => Request::ChildWait {
                    process,
                    child,
                    deadline: None,
                },
                7 => Request::GroupWait {
                    process,
                    group: child,
                    deadline: None,
                },
                _ => unreachable!(),
            };
            let expected = native.preflight(&request).map(|c| {
                c.unwrap_or_else(|| process_completion(&request, ProcessFailure::Canceled(3)))
            });
            if conditions == 0 {
                assert_eq!(native.preflight(&request), Ok(None), "healthy {request:?}");
            }
            reference.cancel();
            let (name, args) = process_arguments(&request);
            match (expected, reference.call(name, args)) {
                (Err(AdapterError::InvalidHandle(_)), Err(hirsel_interp::Error::InvalidTir(_))) => {
                    ()
                }
                (Ok(expected), Ok(value)) => assert_eq!(
                    value,
                    process_error_value(expected),
                    "B13 process conditions={conditions:04b} {request:?}"
                ),
                (native, interpreted) => panic!(
                    "B13 process conditions={conditions:04b} {request:?}: native={native:?} interpreter={interpreted:?}"
                ),
            }
            count += 1;
        }
    }
    eprintln!("B13: {count} process precedence combinations");
}
#[test]
fn io_check_precedence_network_matrix() {
    precedence_net_matrix();
}
#[test]
fn io_check_precedence_process_matrix() {
    precedence_process_matrix();
}

fn precedence_stream_matrix() {
    for kind in 0..4 {
        for conditions in 0..4 {
            let mut native = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
            crate::rt_support::host(&mut native);
            native.clock_grants(true, false);
            let mut reference = Reference::new(&Tree::default());
            reference
                .library
                .simulation
                .config
                .grants
                .insert("MonotonicClock".into());
            let (projection, name, handle_kind) = match kind {
                0 => (Request::Stdin { io: 1 }, "io.stdin", "Stdin"),
                1 => (Request::Stdout { io: 1 }, "io.stdout", "Stdout"),
                2 => (Request::Stderr { io: 1 }, "io.stderr", "Stderr"),
                3 => (
                    Request::MonotonicClock { io: 1 },
                    "io.monotonic_clock",
                    "MonotonicClock",
                ),
                _ => unreachable!(),
            };
            let completion = native.call(projection).unwrap();
            let value = reference.call(name, vec![handle("Io", 1)]).unwrap();
            let mut id = match completion {
                Completion::Handle(Ok(id)) | Completion::ClockGrant(Ok(id)) => id,
                other => panic!("stream setup {other:?}"),
            };
            assert_eq!(value, reference::ok(handle(handle_kind, id)));
            if conditions & 1 != 0 {
                id = 999;
            }
            let request = match kind {
                0 => Request::ReadStdin {
                    stdin: id,
                    max: if conditions & 2 != 0 { 0 } else { 1 },
                },
                1 => Request::Print {
                    stdout: id,
                    bytes: b"A".to_vec(),
                },
                2 => Request::PrintStderr {
                    stderr: id,
                    bytes: b"A".to_vec(),
                },
                3 => Request::SleepUntil {
                    clock: id,
                    deadline: u64::MAX,
                },
                _ => unreachable!(),
            };
            let (name, arguments, expected) = match kind {
                0 => (
                    "stdin.read",
                    vec![
                        handle("Stdin", id),
                        list(vec![]),
                        reference::u64_value(if conditions & 2 != 0 { 0 } else { 1 }),
                    ],
                    Completion::Input(Err(InputError::Canceled(3))),
                ),
                1 => (
                    "out.print",
                    vec![handle("Stdout", id), Value::String("A".into())],
                    Completion::Unit(Err(IoError::Canceled(3))),
                ),
                2 => (
                    "err.print",
                    vec![handle("Stderr", id), Value::String("A".into())],
                    Completion::Unit(Err(IoError::Canceled(3))),
                ),
                3 => (
                    "clock.sleep_until",
                    vec![handle("MonotonicClock", id), handle("Instant", u64::MAX)],
                    Completion::Wait(Err(WaitError::Canceled(3))),
                ),
                _ => unreachable!(),
            };
            let expected = native.preflight(&request).map(|v| v.unwrap_or(expected));
            reference.cancel();
            match (expected, reference.call(name, arguments)) {
                (Err(AdapterError::InvalidHandle(_)), Err(hirsel_interp::Error::InvalidTir(_))) => {
                    ()
                }
                (Ok(expected), Ok(value)) => {
                    let expected = match expected {
                        Completion::Input(Err(InputError::InvalidInput)) => {
                            reference::error(variant(2, vec![]))
                        }
                        Completion::Input(Err(InputError::Canceled(cause)))
                        | Completion::Wait(Err(WaitError::Canceled(cause)))
                        | Completion::Unit(Err(IoError::Canceled(cause))) => {
                            reference::error(variant(1, vec![variant(cause, vec![])]))
                        }
                        other => panic!("unexpected stream outcome {other:?}"),
                    };
                    assert_eq!(
                        value, expected,
                        "B13 stream conditions={conditions:02b} {request:?}"
                    );
                }
                (native, interpreted) => panic!(
                    "B13 stream conditions={conditions:02b} {request:?}: native={native:?} interpreter={interpreted:?}"
                ),
            }
        }
    }
}
#[test]
fn io_check_precedence_stream_matrix() {
    precedence_stream_matrix();
}
