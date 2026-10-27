//! Independent reference execution of runtime completion tapes.
use hirsel_rt::io::*;
use std::collections::BTreeMap;
pub mod network;
pub fn replay_interpreter(
    tape: &[Event],
    initial_tree: &Tree,
    tree: &Tree,
    handles: &BTreeMap<u64, hirsel_rt::handles::Grant>,
    stdout: &[u8],
) {
    replay_interpreter_with_mounts(
        tape,
        initial_tree,
        tree,
        handles,
        stdout,
        &BTreeMap::from([(String::new(), hirsel_run_schema::Access::ReadWrite)]),
    );
}
pub fn replay_interpreter_with_mounts(
    tape: &[Event],
    initial_tree: &Tree,
    tree: &Tree,
    handles: &BTreeMap<u64, hirsel_rt::handles::Grant>,
    stdout: &[u8],
    mounts: &BTreeMap<String, hirsel_run_schema::Access>,
) {
    replay_interpreter_with_fixture(
        tape,
        ReplayFixture {
            events: tape,
            pem_roots: &[],
            tls_identities: &[],
        },
        initial_tree,
        tree,
        handles,
        stdout,
        mounts,
    );
}

/// Original fixture inputs, independent of the tape being compared. PEM roots and
/// server identities (chain, key) are supplied explicitly because the tape stores only
/// their chains' one-way digests.
#[derive(Clone, Copy)]
pub struct ReplayFixture<'a> {
    pub events: &'a [Event],
    pub pem_roots: &'a [&'a [u8]],
    pub tls_identities: &'a [(&'a [u8], &'a [u8])],
}

/// Keep the script source separate from the comparison tape so corruption
/// controls cannot silently change both sides of the oracle.
pub fn replay_interpreter_with_fixture(
    tape: &[Event],
    fixture: ReplayFixture<'_>,
    initial_tree: &Tree,
    tree: &Tree,
    handles: &BTreeMap<u64, hirsel_rt::handles::Grant>,
    stdout: &[u8],
    mounts: &BTreeMap<String, hirsel_run_schema::Access>,
) {
    let fixture_tape = fixture.events;
    assert_eq!(tape.len(), fixture_tape.len(), "fixture event count");
    let net = fixture_tape
        .iter()
        .find_map(|event| match (&event.request, &event.completion) {
            (Request::Net { .. }, Completion::Handle(Ok(id))) => match &handles[id] {
                hirsel_rt::handles::Grant::Net { rules, .. } => Some(
                    network::fixture(fixture_tape, rules).expect("representable network fixture"),
                ),
                _ => panic!("Net projection returned a foreign grant"),
            },
            _ => None,
        });
    use hirsel_interp::library::simulation::ObservedGrant;
    use hirsel_interp::{
        OperationCall, OperationContext, OperationResult, Operations, Value, library as reference,
    };
    use hirsel_span::{SourceMap, Span};
    use hirsel_tir::RunConfiguration;
    let mut recorded = vec![];
    for replay in [false, true] {
        let run = RunConfiguration {
            entry: hirsel_syntax::DefPath::core("main"),
            hash_seed: 42,
            inputs: if replay { recorded.clone() } else { vec![] },
        };
        let mut library = reference::Library::new(
            &run,
            reference::SimulationConfig {
                net: net.clone(),
                test_seed: match handles.get(&1) {
                    Some(hirsel_rt::handles::Grant::Test { seed }) => *seed,
                    _ => 0,
                },
                mounts: mounts.clone(),
                files: initial_tree.files.clone(),
                file_aliases: initial_tree.file_aliases.clone(),
                links: initial_tree.links.clone(),
                directories: initial_tree.directories.clone(),
                metadata: initial_tree
                    .metadata
                    .iter()
                    .map(|(p, meta)| {
                        (
                            p.clone(),
                            reference::simulation::FileMetadata {
                                kind: meta.kind as u32,
                                size: meta.size,
                                modified_unix_ns: meta.modified_unix_ns,
                                permissions: meta.permissions,
                            },
                        )
                    })
                    .collect(),
                ..Default::default()
            },
        )
        .unwrap();
        let root = if matches!(
            handles.get(&1),
            Some(hirsel_rt::handles::Grant::Test { .. })
        ) {
            "Test"
        } else {
            "Io"
        };
        let io = library.simulation.grant_root(root).unwrap();
        assert_eq!(
            io,
            Value::Handle {
                kind: root.into(),
                id: 1
            }
        );
        let types = hirsel_types::TypeInterner::new(None);
        let result_type = types.intern(hirsel_types::Type::unit());
        let sources = SourceMap::default();
        let mut output = vec![];
        let mut trace = vec![];
        let handle = |kind: &str, id| Value::Handle {
            kind: kind.into(),
            id,
        };
        for (event, fixture_event) in tape.iter().zip(fixture_tape) {
            assert_eq!(event.request, fixture_event.request, "fixture request");
            assert_eq!(event.fault, fixture_event.fault, "fixture fault input");
            let (name, args) = match &event.request {
                Request::Stdin { .. }
                | Request::Stderr { .. }
                | Request::ReadStdin { .. }
                | Request::PrintStderr { .. }
                | Request::EnvGet { .. }
                | Request::EnvNarrow { .. } => {
                    panic!("host input tapes require the explicit stream and Env replay fixture")
                }
                Request::Process { .. }
                | Request::TestProcess { .. }
                | Request::ProcessNarrow { .. }
                | Request::ProcessRun { .. }
                | Request::ProcessPipeline { .. }
                | Request::ProcessSpawn { .. }
                | Request::ProcessSpawnPipeline { .. }
                | Request::GroupWait { .. }
                | Request::ChildWrite { .. }
                | Request::ChildCloseStdin { .. }
                | Request::ChildRead { .. }
                | Request::ChildWait { .. }
                | Request::ChildKill { .. } => {
                    panic!("process tapes replay through tests/it/process.rs")
                }
                Request::Telemetry { .. }
                | Request::TelemetryBegin { .. }
                | Request::TelemetryEvent { .. }
                | Request::TelemetryFinish { .. }
                | Request::TelemetryLog { .. }
                | Request::TelemetryMeasure { .. } => {
                    panic!("telemetry tapes replay through hirsel-rt tests/it/telemetry.rs")
                }
                Request::Ready { .. }
                | Request::Schedule { .. }
                | Request::CpuControl { .. }
                | Request::ProcessTime { .. }
                | Request::ProcessSettle { .. } => {
                    panic!("the interpreter oracle runs no cooperative host scheduler")
                }
                Request::RunnerCall(_)
                | Request::RunnerPoll { .. }
                | Request::RunnerCancel { .. }
                | Request::CallReturn(_)
                | Request::Stopped => {
                    panic!("runner tapes replay through hirsel-cli's runner tests")
                }
                Request::Signals { .. } | Request::SignalWait { .. } => {
                    panic!("signal tapes replay through tests/it/signals.rs")
                }
                Request::RandomBytes { .. } | Request::SleepUntil { .. } => {
                    panic!("random and wait tapes replay through tests/it/waits.rs")
                }
                Request::MonotonicClock { io } => ("io.monotonic_clock", vec![handle("Io", *io)]),
                Request::WallClock { io } => ("io.wall_clock", vec![handle("Io", *io)]),
                Request::Monotonic { clock } => {
                    ("clock.monotonic", vec![handle("MonotonicClock", *clock)])
                }
                Request::Wall { clock } => ("clock.wall", vec![handle("WallClock", *clock)]),
                Request::Random { io } => ("io.random", vec![handle("Io", *io)]),
                Request::Tasks { io } => ("io.tasks", vec![handle("Io", *io)]),
                Request::Net { io } => ("io.net", vec![handle("Io", *io)]),
                Request::TestCheck { test, condition } => (
                    "test.check",
                    vec![handle("Test", *test), Value::Bool(*condition)],
                ),
                Request::TestCheckCase {
                    test,
                    condition,
                    case,
                } => (
                    "test.check_case",
                    vec![
                        handle("Test", *test),
                        Value::Bool(*condition),
                        reference::u64_value(*case),
                    ],
                ),
                Request::TestFail { test } => ("test.fail", vec![handle("Test", *test)]),
                Request::TestClock { test } => ("test.clock", vec![handle("Test", *test)]),
                Request::TestSeed { test } => ("test.seed", vec![handle("Test", *test)]),
                Request::TestTasks { test } => ("test.tasks", vec![handle("Test", *test)]),
                Request::TestLog { test, text } => (
                    "test.log",
                    vec![handle("Test", *test), Value::String(text.clone())],
                ),
                Request::TestSnapshot { test, name, text } => (
                    "test.expect_snapshot",
                    vec![
                        handle("Test", *test),
                        Value::String(name.clone()),
                        Value::String(text.clone()),
                    ],
                ),
                Request::TestDir { test, entries } => (
                    "test.dir",
                    vec![
                        handle("Test", *test),
                        reference::collection::sequence(
                            hirsel_types::BuiltinType::List,
                            entries
                                .iter()
                                .map(|(p, b)| {
                                    Value::Tuple(vec![
                                        Some(reference::path(p)),
                                        Some(Value::Bytes(b.clone())),
                                    ])
                                })
                                .collect::<Vec<_>>(),
                        ),
                    ],
                ),
                Request::Stdout { io } => ("io.stdout", vec![handle("Io", *io)]),
                Request::Directory { io, path } => {
                    ("io.dir", vec![handle("Io", *io), reference::path(path)])
                }
                Request::NarrowDir { directory, path } => (
                    "dir.narrow",
                    vec![handle("Dir", *directory), reference::path(path)],
                ),
                Request::ReadOnlyDir { directory } => {
                    ("dir.read_only", vec![handle("Dir", *directory)])
                }
                Request::Print { stdout, bytes } => (
                    "out.print",
                    vec![
                        handle("Stdout", *stdout),
                        Value::String(String::from_utf8(bytes.clone()).unwrap()),
                    ],
                ),
                Request::TestCrash {
                    test,
                    directory,
                    seed,
                } => (
                    "test.crash",
                    vec![
                        handle("Test", *test),
                        handle("Dir", *directory),
                        reference::u64_value(*seed),
                    ],
                ),
                request @ (Request::Open { .. }
                | Request::OpenAppend { .. }
                | Request::Create { .. }
                | Request::FileRead { .. }
                | Request::FileSeek { .. }
                | Request::FileWrite { .. }
                | Request::FileAppend { .. }
                | Request::NarrowFile { .. }
                | Request::ReadDirPage { .. }
                | Request::Metadata { .. }
                | Request::Mkdir { .. }
                | Request::SetPermissions { .. }
                | Request::RemoveAll { .. }
                | Request::RemoveDir { .. }
                | Request::Rename { .. }
                | Request::Size { .. }
                | Request::Read { .. }
                | Request::Write { .. }
                | Request::Remove { .. }
                | Request::CreateAtomic { .. }
                | Request::WriteAll { .. }
                | Request::Commit { .. }
                | Request::Sync { .. }
                | Request::PutNew { .. }
                | Request::Map { .. }) => file_arguments(request).unwrap(),
                Request::MapView { .. } | Request::MapCheck { .. } => {
                    panic!("mapped.with runs its body: conformance records and replays it")
                }
                Request::Watch { .. }
                | Request::WatchNext { .. }
                | Request::TestDirEvent { .. } => {
                    panic!("watcher tapes replay through tests/it/recording.rs")
                }
                Request::UdpBind { .. }
                | Request::UdpSend { .. }
                | Request::UdpRecv { .. }
                | Request::TestNet { .. }
                | Request::NetNarrow { .. }
                | Request::Resolve { .. }
                | Request::Connect { .. }
                | Request::ConnectAddr { .. }
                | Request::NetRead { .. }
                | Request::NetWrite { .. }
                | Request::NetWriteAll { .. }
                | Request::Shutdown { .. }
                | Request::SetTimeout { .. }
                | Request::PeerAddr { .. }
                | Request::TlsConnect { .. }
                | Request::TlsAccept { .. }
                | Request::TlsPeer { .. }
                | Request::TlsProtocol { .. }
                | Request::NetSplit { .. }
                | Request::NetJoin { .. }
                | Request::TlsClose { .. }
                | Request::Listen { .. }
                | Request::UnixConnect { .. }
                | Request::UnixListen { .. }
                | Request::Accept { .. }
                | Request::LocalAddr { .. } => network::arguments(event, fixture),
                Request::Close { handle: id } => {
                    let grant = library.simulation.observed_state().handles[id].clone();
                    let kind = match grant {
                        ObservedGrant::File { .. } => "File",
                        ObservedGrant::AtomicFile { .. } => "AtomicFile",
                        ObservedGrant::Stream { .. } => "Stream",
                        ObservedGrant::Listener { .. } => "Listener",
                        ObservedGrant::UdpSocket { .. } => "UdpSocket",
                        ObservedGrant::Mapped { .. } => "Mapped",
                        ObservedGrant::Watcher { .. } => "Watcher",
                        _ => panic!("unsupported closed handle: {grant:?}"),
                    };
                    library
                        .drop_handle(
                            kind,
                            *id,
                            &mut OperationContext {
                                configuration: &run,
                                sources: &sources,
                                stdout: &mut output,
                                trace: &mut trace,
                                trace_mode: Default::default(),
                                anchor: None,
                            },
                        )
                        .unwrap();
                    assert_eq!(event.completion, Completion::Unit(Ok(())));
                    continue;
                }
            };
            if let Some(fault) = &fixture_event.fault {
                let f = match fault.error {
                    IoError::Fault(code) => reference::simulation::Fault::Fault {
                        code,
                        partial: fault.partial,
                    },
                    IoError::Canceled(c) => reference::simulation::Fault::Canceled(c),
                    IoError::NotGranted => panic!("grant denial is policy, not an injected fault"),
                    IoError::NotFound => reference::simulation::Fault::File {
                        error: reference::simulation::FileError::NotFound,
                        partial: fault.partial,
                    },
                    IoError::PermissionDenied => reference::simulation::Fault::File {
                        error: reference::simulation::FileError::PermissionDenied,
                        partial: fault.partial,
                    },
                    IoError::AlreadyExists => reference::simulation::Fault::File {
                        error: reference::simulation::FileError::AlreadyExists,
                        partial: fault.partial,
                    },
                    IoError::IsDirectory => reference::simulation::Fault::File {
                        error: reference::simulation::FileError::IsDirectory,
                        partial: fault.partial,
                    },
                    IoError::DirectoryNotEmpty => reference::simulation::Fault::File {
                        error: reference::simulation::FileError::DirectoryNotEmpty,
                        partial: fault.partial,
                    },
                    IoError::CrossDevice => reference::simulation::Fault::File {
                        error: reference::simulation::FileError::CrossDevice,
                        partial: fault.partial,
                    },
                    IoError::NotDirectory => reference::simulation::Fault::File {
                        error: reference::simulation::FileError::NotDirectory,
                        partial: fault.partial,
                    },
                    IoError::Truncated => panic!("a truncation is mapped.with's check input"),
                };
                library
                    .simulation
                    .config
                    .faults
                    .entry(name.into())
                    .or_default()
                    .push_back(f);
            }
            if fixture_event.fault.is_none()
                && let Some(fault) = network::fault(&fixture_event.completion)
            {
                library
                    .simulation
                    .config
                    .faults
                    .entry(name.into())
                    .or_default()
                    .push_back(fault);
            }
            let path = hirsel_syntax::DefPath::core(name);
            let (value, arguments) = if let Request::ReadDirPage {
                directory,
                path,
                after,
                recursive,
            } = &event.request
            {
                let result = library
                    .read_directory_page(
                        &handle("Dir", *directory),
                        path,
                        after,
                        *recursive,
                        &mut OperationContext {
                            configuration: &run,
                            sources: &sources,
                            stdout: &mut output,
                            trace: &mut trace,
                            trace_mode: Default::default(),
                            anchor: None,
                        },
                    )
                    .unwrap();
                let value = match result {
                    Ok(entries) => reference::ok(reference::collection::sequence(
                        hirsel_types::BuiltinType::List,
                        entries.into_iter().map(|e| Value::Record {
                            definition: (&hirsel_syntax::DefPath::core("DirEntry")).into(),
                            fields: vec![
                                Some(reference::path(&e.path)),
                                Some(Value::Variant {
                                    variant: e.kind,
                                    fields: vec![],
                                }),
                            ],
                        }),
                    )),
                    Err(error) => error,
                };
                (value, vec![])
            } else {
                let result = library
                    .call(
                        OperationCall {
                            runtime: &hirsel_interp::aggregate::RuntimeTypes::new(types.clone()),
                            path: &path,
                            generics: &[],
                            arguments: &mut args.to_vec(),
                            result_type,
                            types: &types,
                            site: Span::default(),
                        },
                        &mut OperationContext {
                            configuration: &run,
                            sources: &sources,
                            stdout: &mut output,
                            trace: &mut trace,
                            trace_mode: Default::default(),
                            anchor: None,
                        },
                    )
                    .unwrap()
                    .unwrap();
                let OperationResult::Complete {
                    value, arguments, ..
                } = result
                else {
                    panic!("unexpected reference failure")
                };
                (value, arguments)
            };
            let value = if name == "file.read" && matches!(value, Value::Variant { variant: 0, .. })
            {
                let (_, buffer) = arguments
                    .iter()
                    .find(|(index, _)| *index == 2)
                    .expect("file read buffer");
                let Value::Sequence { elements, .. } = buffer else {
                    panic!("file read list")
                };
                reference::ok(Value::Bytes(
                    elements
                        .iter()
                        .map(|b| match b {
                            Some(Value::Integer(n)) => n.bits() as u8,
                            _ => panic!("file read byte"),
                        })
                        .collect(),
                ))
            } else {
                value
            };
            let expected = crate::rt_support::library::delivered(&event.completion);
            let decoded = match expected {
                Completion::Datagram(_)
                | Completion::NetHandle(_)
                | Completion::NetBytes(_)
                | Completion::Count(_)
                | Completion::NetUnit(_)
                | Completion::Protocol(_)
                | Completion::Halves(_)
                | Completion::Address(_)
                | Completion::Addresses(_) => network::decode(&expected, value, &arguments),
                _ => decode(&expected, value),
            };
            assert_eq!(decoded, expected, "{name}");
        }
        library.simulation.finish_replay().unwrap();
        assert_eq!(output, stdout);
        let observed = library.simulation.observed_state();
        assert_eq!(
            observed.trees[&0],
            (tree.directories.clone(), tree.files.clone())
        );
        let expected: BTreeMap<_, _> = handles
            .iter()
            .map(|(id, grant)| {
                (
                    *id,
                    match grant {
                        hirsel_rt::handles::Grant::Io => ObservedGrant::Root("Io".into()),
                        hirsel_rt::handles::Grant::MonotonicClock => {
                            ObservedGrant::Root("MonotonicClock".into())
                        }
                        hirsel_rt::handles::Grant::WallClock => {
                            ObservedGrant::Root("WallClock".into())
                        }
                        hirsel_rt::handles::Grant::Random => ObservedGrant::Root("Random".into()),
                        hirsel_rt::handles::Grant::Tasks => ObservedGrant::Root("Tasks".into()),
                        hirsel_rt::handles::Grant::Telemetry => {
                            ObservedGrant::Root("Telemetry".into())
                        }
                        hirsel_rt::handles::Grant::Span { telemetry, .. } => ObservedGrant::Span {
                            telemetry: *telemetry,
                        },
                        hirsel_rt::handles::Grant::Signals => ObservedGrant::Root("Signals".into()),
                        hirsel_rt::handles::Grant::Net { rules, world } => ObservedGrant::Net {
                            world: match world {
                                hirsel_rt::net::NetWorld::Host => 0,
                                hirsel_rt::net::NetWorld::Script(id) => *id,
                            },
                            rules: network::rules(rules),
                        },
                        hirsel_rt::handles::Grant::Stream { net, .. } => {
                            ObservedGrant::Stream { net: *net }
                        }
                        hirsel_rt::handles::Grant::Half { net, stream, write } => {
                            ObservedGrant::Half {
                                net: *net,
                                stream: *stream,
                                write: *write,
                            }
                        }
                        hirsel_rt::handles::Grant::UdpSocket { net } => {
                            ObservedGrant::UdpSocket { net: *net }
                        }
                        hirsel_rt::handles::Grant::Listener { net, .. } => {
                            ObservedGrant::Listener { net: *net }
                        }
                        hirsel_rt::handles::Grant::Test { .. } => {
                            ObservedGrant::Root("Test".into())
                        }
                        hirsel_rt::handles::Grant::Process { rules, world } => {
                            ObservedGrant::Proc {
                                world: match world {
                                    hirsel_rt::process::ProcessWorld::Host => 0,
                                    hirsel_rt::process::ProcessWorld::Script(id) => *id,
                                },
                                rules: rules.clone(),
                            }
                        }
                        hirsel_rt::handles::Grant::Child { process, .. } => {
                            ObservedGrant::Child { process: *process }
                        }
                        hirsel_rt::handles::Grant::Stdin => ObservedGrant::Root("Stdin".into()),
                        hirsel_rt::handles::Grant::Stderr => ObservedGrant::Root("Stderr".into()),
                        hirsel_rt::handles::Grant::Stdout => ObservedGrant::Root("Stdout".into()),
                        hirsel_rt::handles::Grant::Env { names } => ObservedGrant::Env {
                            names: names.clone(),
                        },
                        hirsel_rt::handles::Grant::Directory { root, access } => {
                            ObservedGrant::Directory {
                                tree: 0,
                                root: root.clone(),
                                access: *access,
                            }
                        }
                        hirsel_rt::handles::Grant::File {
                            path,
                            directory,
                            position,
                            writable,
                        } => ObservedGrant::File {
                            tree: 0,
                            path: path.clone(),
                            directory: *directory,
                            position: *position,
                            writable: *writable,
                        },
                        hirsel_rt::handles::Grant::AtomicFile {
                            temporary,
                            destination,
                            directory,
                        } => ObservedGrant::AtomicFile {
                            tree: 0,
                            temporary: temporary.clone(),
                            destination: destination.clone(),
                            directory: *directory,
                        },
                        hirsel_rt::handles::Grant::Mapped {
                            directory,
                            length,
                            truncated,
                        } => ObservedGrant::Mapped {
                            tree: 0,
                            directory: *directory,
                            length: *length,
                            truncated: *truncated,
                        },
                        hirsel_rt::handles::Grant::Watcher {
                            directory,
                            root,
                            recursive,
                        } => ObservedGrant::Watcher {
                            tree: 0,
                            directory: *directory,
                            root: root.clone(),
                            recursive: *recursive,
                        },
                    },
                )
            })
            .collect();
        assert_eq!(observed.handles, expected);
        if !replay {
            recorded = library.simulation.events.clone();
        }
    }
}
pub(crate) fn decode(schema: &Completion, value: hirsel_interp::Value) -> Completion {
    use hirsel_interp::Value;
    fn fields(value: Value) -> (u32, Vec<Value>) {
        if let Value::Variant { variant, fields } = value {
            (variant, fields.into_iter().map(Option::unwrap).collect())
        } else {
            panic!("expected variant")
        }
    }
    fn result(value: Value) -> Result<Value, IoError> {
        let (tag, mut payload) = fields(value);
        let value = payload.remove(0);
        if tag == 0 {
            Ok(value)
        } else {
            let (tag, mut payload) = fields(value);
            if payload.is_empty() {
                return Err(match tag {
                    2 => IoError::NotGranted,
                    3 => IoError::NotFound,
                    4 => IoError::PermissionDenied,
                    5 => IoError::AlreadyExists,
                    6 => IoError::IsDirectory,
                    7 => IoError::NotDirectory,
                    8 => IoError::DirectoryNotEmpty,
                    9 => IoError::CrossDevice,
                    10 => IoError::Truncated,
                    _ => panic!("invalid nullary file error"),
                });
            }
            let value = payload.remove(0);
            Err(if tag == 0 {
                let Value::Integer(code) = value else {
                    panic!()
                };
                IoError::Fault(code.bits() as u32)
            } else {
                IoError::Canceled(fields(value).0)
            })
        }
    }
    let id = |v| {
        if let Value::Handle { id, .. } = v {
            id
        } else {
            panic!("expected handle")
        }
    };
    match schema {
        Completion::ClockGrant(_) => {
            let (tag, mut payload) = fields(value);
            Completion::ClockGrant(if tag == 0 {
                Ok(id(payload.remove(0)))
            } else {
                Err(())
            })
        }
        Completion::Monotonic(_) | Completion::Wall(_) => {
            let (tag, mut payload) = fields(value);
            let value = payload.remove(0);
            let error = if tag == 0 {
                None
            } else {
                Some(if fields(value.clone()).0 == 0 {
                    ClockError::Unavailable
                } else {
                    ClockError::OutOfRange
                })
            };
            match schema {
                Completion::Monotonic(_) => {
                    Completion::Monotonic(error.map_or_else(|| Ok(id(value)), Err))
                }
                _ => Completion::Wall(error.map_or_else(
                    || {
                        if let Value::Integer(n) = value {
                            Ok(n.bits() as u64 as i64)
                        } else {
                            panic!()
                        }
                    },
                    Err,
                )),
            }
        }

        Completion::Refused(id) => Completion::Refused(*id),
        Completion::Checked(v) => {
            assert_eq!(value, Value::Unit);
            Completion::Checked(*v)
        }
        Completion::Handle(_) => Completion::Handle(if matches!(value, Value::Handle { .. }) {
            Ok(id(value))
        } else {
            result(value).map(id)
        }),
        Completion::OptionalFile(_) => {
            let (tag, mut payload) = fields(value);
            Completion::OptionalFile(if tag == 0 {
                None
            } else {
                Some(id(payload.remove(0)))
            })
        }
        Completion::Unit(_) => Completion::Unit(result(value).map(|v| {
            assert_eq!(v, Value::Unit);
            ()
        })),
        Completion::Size(_) => Completion::Size(result(value).map(|v| {
            if let Value::Integer(n) = v {
                n.bits() as u64
            } else {
                panic!()
            }
        })),
        Completion::Bytes(_) => Completion::Bytes(
            result(value).map(|v| if let Value::Bytes(b) = v { b } else { panic!() }),
        ),
        Completion::Metadata(_) => Completion::Metadata(result(value).map(|v| {
            let Value::Record { fields: f, .. } = v else {
                panic!("expected Metadata")
            };
            let mut f = f.into_iter().map(Option::unwrap);
            let kind = match fields(f.next().unwrap()).0 {
                0 => FileKind::Regular,
                1 => FileKind::Directory,
                2 => FileKind::Symlink,
                3 => FileKind::Other,
                _ => panic!("invalid FileKind"),
            };
            let Value::Integer(size) = f.next().unwrap() else {
                panic!()
            };
            let (tag, mut payload) = fields(f.next().unwrap());
            let modified_unix_ns = if tag == 0 {
                None
            } else {
                let Value::Integer(n) = payload.remove(0) else {
                    panic!()
                };
                Some(n.bits() as i128)
            };
            Metadata {
                kind,
                size: size.bits() as u64,
                modified_unix_ns,
                permissions: match f.next().unwrap() {
                    Value::Integer(n) => n.bits() as u32,
                    _ => panic!(),
                },
            }
        })),
        Completion::Entries(_) => Completion::Entries(result(value).map(|v| {
            let Value::Sequence { elements, .. } = v else {
                panic!("expected entry list")
            };
            elements
                .into_iter()
                .map(|v| {
                    let Value::Record { fields: f, .. } = v.unwrap() else {
                        panic!("expected DirEntry")
                    };
                    let mut f = f.into_iter().map(Option::unwrap);
                    let Value::Record { fields: p, .. } = f.next().unwrap() else {
                        panic!("expected Path")
                    };
                    let Value::String(path) = p.into_iter().next().unwrap().unwrap() else {
                        panic!()
                    };
                    let kind = match fields(f.next().unwrap()).0 {
                        0 => FileKind::Regular,
                        1 => FileKind::Directory,
                        2 => FileKind::Symlink,
                        3 => FileKind::Other,
                        _ => panic!("invalid FileKind"),
                    };
                    DirEntry { path, kind }
                })
                .collect()
        })),
        _ => panic!("network completion must use its typed decoder"),
    }
}

/// File request translation shared by tape replay and independent precedence probes.
pub(crate) fn file_arguments(
    request: &Request,
) -> Option<(&'static str, Vec<hirsel_interp::Value>)> {
    use hirsel_interp::{Value, library as reference};
    let handle = |kind: &str, id| Value::Handle {
        kind: kind.into(),
        id,
    };
    Some(match request {
        Request::Open { directory, path } => (
            "file.open",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        Request::OpenAppend { directory, path } => (
            "file.open_append",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        Request::Create { directory, path } => (
            "file.create",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        Request::FileRead {
            directory,
            file,
            max,
        } => (
            "file.read",
            vec![
                handle("Dir", *directory),
                handle("File", *file),
                Value::Sequence {
                    kind: hirsel_types::BuiltinType::List,
                    elements: Default::default(),
                    arena: None,
                },
                reference::u64_value(*max),
            ],
        ),
        Request::FileSeek {
            directory,
            file,
            offset,
        } => (
            "file.seek",
            vec![
                handle("Dir", *directory),
                handle("File", *file),
                reference::u64_value(*offset),
            ],
        ),
        Request::FileWrite {
            directory,
            file,
            bytes,
        } => (
            "file.write",
            vec![
                handle("Dir", *directory),
                handle("File", *file),
                Value::Bytes(bytes.clone()),
            ],
        ),
        Request::FileAppend {
            directory,
            file,
            bytes,
        } => (
            "file.append",
            vec![
                handle("Dir", *directory),
                handle("File", *file),
                Value::Bytes(bytes.clone()),
            ],
        ),
        Request::NarrowFile { directory, file } => (
            "file.narrow",
            vec![handle("Dir", *directory), handle("File", *file)],
        ),
        Request::ReadDirPage {
            directory, path, ..
        } => (
            "file.read_dir",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        Request::Metadata { directory, path } => (
            "file.metadata",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        Request::Mkdir { directory, path } => (
            "file.mkdir",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        Request::SetPermissions {
            directory,
            path,
            permissions,
        } => (
            "file.set_permissions",
            vec![
                handle("Dir", *directory),
                reference::path(path),
                Value::Integer(
                    hirsel_interp::Integer::new(hirsel_types::Primitive::U32, *permissions as u128)
                        .unwrap(),
                ),
            ],
        ),
        Request::RemoveAll { directory, path } => (
            "dir.remove_all",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        Request::RemoveDir { directory, path } => (
            "file.remove_dir",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        Request::Rename {
            directory,
            from,
            to,
        } => (
            "file.rename",
            vec![
                handle("Dir", *directory),
                reference::path(from),
                reference::path(to),
            ],
        ),
        Request::Size { directory, path } => (
            "file.size",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        Request::Read { directory, path } => (
            "file.read_bytes",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        Request::Write {
            directory,
            path,
            bytes,
        } => (
            "file.write_bytes",
            vec![
                handle("Dir", *directory),
                reference::path(path),
                Value::Bytes(bytes.clone()),
            ],
        ),
        Request::Remove { directory, path } => (
            "file.remove",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        Request::CreateAtomic { directory, path } => (
            "file.create_atomic",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        Request::WriteAll {
            directory,
            file,
            bytes,
        } => (
            "file.write_all",
            vec![
                handle("Dir", *directory),
                handle("AtomicFile", *file),
                Value::Bytes(bytes.clone()),
            ],
        ),
        Request::Commit { directory, file } => (
            "file.commit",
            vec![handle("Dir", *directory), handle("AtomicFile", *file)],
        ),
        Request::Sync { directory, file } => (
            "file.sync",
            vec![handle("Dir", *directory), handle("File", *file)],
        ),
        Request::PutNew {
            directory,
            path,
            bytes,
            durable,
        } => (
            "file.put_new",
            vec![
                handle("Dir", *directory),
                reference::path(path),
                Value::Bytes(bytes.clone()),
                Value::Variant {
                    variant: u32::from(!*durable),
                    fields: vec![],
                },
            ],
        ),
        Request::Map { directory, path } => (
            "file.map",
            vec![handle("Dir", *directory), reference::path(path)],
        ),
        _ => return None,
    })
}
