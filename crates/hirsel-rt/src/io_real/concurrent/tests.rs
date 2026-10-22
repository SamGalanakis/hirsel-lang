use super::*;
use crate::io_keys::{Mode, NAMESPACE};
use crate::io_sim::SimIo;
use std::sync::{Barrier, mpsc};
use std::time::Duration;

fn handle(c: Completion) -> u64 {
    match c {
        Completion::Handle(Ok(h)) => h,
        other => panic!("{other:?}"),
    }
}
fn adapter() -> (tempfile::TempDir, ConcurrentIo, u64) {
    let temp = tempfile::tempdir().unwrap();
    let real = RealIo::grant(temp.path()).unwrap();
    let io = real
        .into_concurrent()
        .unwrap_or_else(|_| panic!("Send streams"));
    let directory = handle(
        io.call(Request::Directory {
            io: 1,
            path: String::new(),
        })
        .unwrap(),
    );
    (temp, io, directory)
}
fn create(io: &ConcurrentIo, directory: u64, name: &str) -> u64 {
    handle(
        io.call(Request::Create {
            directory,
            path: name.into(),
        })
        .unwrap(),
    )
}
fn append(directory: u64, file: u64, bytes: &[u8]) -> Request {
    Request::FileAppend {
        directory,
        file,
        bytes: bytes.to_vec(),
    }
}

#[test]
fn shared_real_core_is_send_sync_and_local_stream_conversion_is_lossless() {
    fn send<T: Send>() {}
    fn shared<T: Send + Sync>() {}
    send::<RealCore<SendStreams>>();
    shared::<ConcurrentIo>();
    shared::<crate::readiness::Readiness>();
    let temp = tempfile::tempdir().unwrap();
    let (sink, capture) = crate::host::Sink::capture();
    let mut io = RealIo::grant_with_output(temp.path(), sink).unwrap();
    let stdout = handle(io.call(Request::Stdout { io: 1 }).unwrap());
    let mut io = match io.into_concurrent() {
        Err(io) => io,
        Ok(_) => panic!("local sink moved"),
    };
    io.call(Request::Print {
        stdout,
        bytes: b"home".to_vec(),
    })
    .unwrap();
    assert_eq!(capture.bytes(), b"home");
    let (sink, capture) = crate::host::SendSink::capture();
    io.set_send_output(sink);
    let io = io
        .into_concurrent()
        .unwrap_or_else(|_| panic!("Send output"));
    io.call(Request::Print {
        stdout,
        bytes: b"worker".to_vec(),
    })
    .unwrap();
    assert_eq!(capture.bytes(), b"worker");
    assert!(io.identity(stdout).is_some());
    assert_eq!(io.identity(u64::MAX), None);
}

#[test]
fn real_files_overlap_at_eight_workers_and_cursor_and_replay_survive_at_all_widths() {
    for width in [1, 2, 8] {
        let (temp, mut io, directory) = adapter();
        let files: Vec<_> = (0..width)
            .map(|n| create(&io, directory, &format!("f{n}")))
            .collect();
        let barrier = Arc::new(Barrier::new(width));
        io.before_file = Some(Arc::new(move || {
            barrier.wait();
        }));
        let io = Arc::new(io);
        std::thread::scope(|s| {
            for file in files {
                let io = &io;
                s.spawn(move || {
                    for byte in 0..4 {
                        assert_eq!(
                            io.call(append(directory, file, &[byte])),
                            Ok(Completion::Unit(Ok(())))
                        );
                    }
                });
            }
        });
        for n in 0..width {
            assert_eq!(
                std::fs::read(temp.path().join(format!("f{n}"))).unwrap(),
                [0, 1, 2, 3]
            );
        }
        let tape = io.tape();
        let mut replay = SimIo::new(Tree::new(BTreeMap::new()).unwrap(), tape.clone());
        for event in tape {
            assert_eq!(replay.call(event.request), Ok(event.completion));
        }
        replay.finish().unwrap();
    }
}

#[test]
fn namespace_create_waits_for_inflight_file_even_across_hard_links() {
    let (temp, mut io, directory) = adapter();
    let file = create(&io, directory, "a");
    std::fs::hard_link(temp.path().join("a"), temp.path().join("alias")).unwrap();
    let (entered, arrived) = mpsc::channel();
    let release = Arc::new(Barrier::new(2));
    let gate = release.clone();
    io.before_file = Some(Arc::new(move || {
        entered.send(()).unwrap();
        gate.wait();
    }));
    let io = Arc::new(io);
    let writing = {
        let io = io.clone();
        std::thread::spawn(move || io.call(append(directory, file, b"prefix")))
    };
    arrived.recv_timeout(Duration::from_secs(5)).unwrap();
    let (finished, done) = mpsc::channel();
    let truncating = {
        let io = io.clone();
        std::thread::spawn(move || {
            let result = io.call(Request::Create {
                directory,
                path: "alias".into(),
            });
            finished.send(()).unwrap();
            result
        })
    };
    // This is a negative control on real syscalls: truncation must not publish while
    // the writer is pinned outside the core lock.
    assert!(done.recv_timeout(Duration::from_millis(20)).is_err());
    release.wait();
    writing.join().unwrap().unwrap();
    truncating.join().unwrap().unwrap();
    assert_eq!(std::fs::read(temp.path().join("a")).unwrap(), b"");
}

#[test]
fn live_discovery_aliases_hard_links_and_file_sinks_and_separates_memory_storage() {
    let (temp, mut io, directory) = adapter();
    let a = create(&io, directory, "a");
    std::fs::hard_link(temp.path().join("a"), temp.path().join("b")).unwrap();
    let b = handle(
        io.call(Request::Open {
            directory,
            path: "b".into(),
        })
        .unwrap(),
    );
    assert_eq!(io.identity(a), io.identity(b));
    let shared = Arc::new(Mutex::new(Vec::<u8>::new()));
    let (out, err) = (
        crate::host::SendSink::shared(shared.clone()),
        crate::host::SendSink::shared(shared),
    );
    assert_eq!(out.resource_storage(), err.resource_storage());
    let core = io.core.get_mut().unwrap();
    core.streams.output = crate::host::SendSink::file(
        std::fs::OpenOptions::new()
            .append(true)
            .open(temp.path().join("b"))
            .unwrap(),
    );
    // Reconstruct admission to discover an already-open file and aliased output together.
    let io = ConcurrentIo::new(io.core.into_inner().unwrap());
    let stdout = handle(io.call(Request::Stdout { io: 1 }).unwrap());
    assert_eq!(io.identity(a).unwrap().0, io.identity(stdout).unwrap().0);
    assert!(
        io.keys(&Request::Print {
            stdout,
            bytes: vec![]
        })
        .contains(NAMESPACE, Mode::Shared)
    );
}

#[test]
fn preflight_and_failed_authority_never_touch_the_file() {
    let (temp, io, directory) = adapter();
    let file = create(&io, directory, "a");
    let forged = append(directory + 999, file, b"forged");
    assert!(io.preflight(&forged).is_err());
    assert!(io.call(forged).is_err());
    assert_eq!(std::fs::read(temp.path().join("a")).unwrap(), b"");
}

#[test]
fn declared_auxiliary_domains_snapshot_real_ports_and_shared_inherited_sinks_without_effects() {
    let temp = tempfile::tempdir().unwrap();
    let mut real = RealIo::grant(temp.path()).unwrap();
    let storage = Arc::new(Mutex::new(Vec::<u8>::new()));
    real.set_send_output(crate::host::SendSink::shared(storage.clone()));
    real.set_send_stderr(crate::host::SendSink::shared(storage.clone()));
    real.set_process_grant(&hirsel_run_schema::ProcessGrant {
        provider: "process.spawn@1".into(),
        rules: vec![],
        runs: None,
        children: None,
    });
    let io = real
        .into_concurrent()
        .unwrap_or_else(|_| panic!("Send streams"));
    let net = handle(io.call(Request::Net { io: 1 }).unwrap());
    let process = handle(io.call(Request::Process { io: 1 }).unwrap());
    let connect = Request::Connect {
        net,
        host: "uncontacted.invalid".into(),
        port: 1,
        timeout: crate::net::NetTimeout::Unbounded,
    };
    let run = Request::ProcessRun {
        process,
        command: hirsel_run_schema::Command {
            program: "not-executed".into(),
            args: vec![],
            directory: None,
            env: None,
            own_group: false,
        },
        stdin: vec![],
        max_output: 1,
        deadline: None,
    };
    let history = io.tape();
    let ports = io.domains(&connect);
    assert_eq!(ports, [(io.net_ports(net).unwrap(), net, "ports".into())]);
    assert!(io.keys(&connect).contains(ports[0].0, Mode::Exclusive));
    let inherited = io.domains(&run);
    let sinks = io.registry.lock().unwrap().inherited_domains();
    assert_eq!(inherited.len(), sinks.len());
    for (index, (key, owner, label)) in inherited.iter().enumerate() {
        assert_eq!(
            (*key, *owner, label.as_str()),
            (sinks[index], process, format!("inherited-{index}").as_str())
        );
        assert!(io.keys(&run).contains(*key, Mode::Exclusive));
    }
    let stdout = handle(io.call(Request::Stdout { io: 1 }).unwrap());
    let stderr = handle(io.call(Request::Stderr { io: 1 }).unwrap());
    assert_eq!(
        io.identity(stdout).unwrap().0,
        io.identity(stderr).unwrap().0
    );
    assert_eq!(
        sinks
            .iter()
            .filter(|&&key| key == io.identity(stdout).unwrap().0)
            .count(),
        1
    );
    assert_eq!(io.domains(&run), inherited);
    let mut forged_net = connect.clone();
    let Request::Connect { net: owner, .. } = &mut forged_net else {
        unreachable!()
    };
    *owner = process;
    assert!(io.domains(&forged_net).is_empty());
    let mut forged_process = run.clone();
    let Request::ProcessRun { process: owner, .. } = &mut forged_process else {
        unreachable!()
    };
    *owner = net;
    assert!(io.domains(&forged_process).is_empty());
    // Only grant projections above were issued: metadata starts no provider or process.
    assert_eq!(&io.tape()[..history.len()], history);
    assert_eq!(io.tape().len(), history.len() + 2);
    assert!(storage.lock().unwrap().is_empty());
}

#[test]
fn proc_run_is_an_event_source_and_releases_namespace_while_waiting_at_one_and_eight() {
    for width in [1, 8] {
        let temp = tempfile::tempdir().unwrap();
        let mut real = RealIo::grant(temp.path()).unwrap();
        real.set_process_grant(&hirsel_run_schema::ProcessGrant {
            provider: "process.spawn@1".into(),
            rules: vec![hirsel_run_schema::ProcessRule {
                program: "/bin/sh".into(),
                env: vec![],
            }],
            runs: None,
            children: None,
        });
        let io = Arc::new(
            real.into_concurrent()
                .unwrap_or_else(|_| panic!("Send streams")),
        );
        let directory = handle(
            io.call(Request::Directory {
                io: 1,
                path: String::new(),
            })
            .unwrap(),
        );
        let process = handle(io.call(Request::Process { io: 1 }).unwrap());
        let command = hirsel_run_schema::Command {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "while [ ! -f ready ]; do sleep 0.01; done; printf done".into(),
            ],
            directory: Some(directory),
            env: None,
            own_group: false,
        };
        let request = Request::ProcessRun {
            process,
            command,
            stdin: vec![],
            max_output: 64,
            deadline: None,
        };
        assert_eq!(io.call_at(99, request.clone()), Err(AdapterError::Blocked));
        io.parked(99);

        let workers: Vec<_> = (0..width)
            .map(|n| {
                let io = io.clone();
                std::thread::spawn(move || {
                    io.call(Request::Write {
                        directory,
                        path: if n == 0 {
                            "ready".into()
                        } else {
                            format!("f{n}")
                        },
                        bytes: b"x".to_vec(),
                    })
                    .unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        // A wall deadline makes a lost-notification failure bounded.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let completion = loop {
            assert!(std::time::Instant::now() < deadline, "process event lost");
            let result = io.call_at(99, request.clone());
            if result != Err(AdapterError::Blocked) {
                break result.unwrap();
            }
            assert!(io.wait().unwrap());
        };
        assert!(
            matches!(completion, Completion::Process(crate::process::ProcessOutcome::Output { ref stdout, .. }) if stdout == b"done")
        );
        io.finish(99);
        let tape = io.tape();
        let mut replay = SimIo::new(Tree::new(BTreeMap::new()).unwrap(), tape.clone());
        replay.set_process_grant(&hirsel_run_schema::ProcessGrant {
            provider: "process.spawn@1".into(),
            rules: vec![hirsel_run_schema::ProcessRule {
                program: "/bin/sh".into(),
                env: vec![],
            }],
            runs: None,
            children: None,
        });
        for event in tape {
            let result = replay.call(event.request);
            if matches!(event.completion, Completion::Blocked { .. }) {
                assert_eq!(result, Err(AdapterError::Blocked));
            } else {
                assert_eq!(result, Ok(event.completion));
            }
        }
        replay.finish().unwrap();
    }
}

#[test]
fn concurrent_readers_share_one_cursor_and_repeated_random_grants_share_one_key() {
    let (temp, io, directory) = adapter();
    std::fs::write(temp.path().join("read"), [0, 1, 2, 3, 4, 5, 6, 7]).unwrap();
    let file = handle(
        io.call(Request::Open {
            directory,
            path: "read".into(),
        })
        .unwrap(),
    );
    let io = Arc::new(io);
    let mut read = std::thread::scope(|scope| {
        let joins: Vec<_> = (1..=8)
            .map(|token| {
                let io = &io;
                scope.spawn(move || {
                    io.call_at(
                        token,
                        Request::FileRead {
                            directory,
                            file,
                            max: 1,
                        },
                    )
                    .unwrap()
                })
            })
            .collect();
        joins
            .into_iter()
            .map(|join| match join.join().unwrap() {
                Completion::Bytes(Ok(bytes)) => bytes[0],
                other => panic!("{other:?}"),
            })
            .collect::<Vec<_>>()
    });
    read.sort();
    assert_eq!(read, [0, 1, 2, 3, 4, 5, 6, 7]);
    let a = handle(io.call(Request::Random { io: 1 }).unwrap());
    let b = handle(io.call(Request::Random { io: 1 }).unwrap());
    assert_eq!(io.identity(a), io.identity(b));
}

#[test]
fn identical_mount_roots_and_hard_links_across_roots_discover_one_resource() {
    for identical in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::fs::write(a.join("file"), b"original").unwrap();
        let second = if identical {
            a.clone()
        } else {
            std::fs::hard_link(a.join("file"), b.join("file")).unwrap();
            b
        };
        let mut real = RealIo::from_pinned_mounts(
            [
                (
                    "a".into(),
                    hirsel_run_schema::Access::ReadWrite,
                    crate::host_paths::PinnedDirectory::open(&a).unwrap(),
                ),
                (
                    "b".into(),
                    hirsel_run_schema::Access::ReadWrite,
                    crate::host_paths::PinnedDirectory::open(&second).unwrap(),
                ),
            ],
            std::io::sink(),
        )
        .unwrap();
        real.set_send_output(crate::host::SendSink::capture().0);
        let io = real
            .into_concurrent()
            .unwrap_or_else(|_| panic!("Send streams"));
        let a = handle(
            io.call(Request::Directory {
                io: 1,
                path: "a".into(),
            })
            .unwrap(),
        );
        let b = handle(
            io.call(Request::Directory {
                io: 1,
                path: "b".into(),
            })
            .unwrap(),
        );
        let fa = handle(
            io.call(Request::Open {
                directory: a,
                path: "file".into(),
            })
            .unwrap(),
        );
        let fb = handle(
            io.call(Request::Open {
                directory: b,
                path: "file".into(),
            })
            .unwrap(),
        );
        assert_eq!(io.identity(fa), io.identity(fb));
        assert!(
            io.keys(&Request::Create {
                directory: b,
                path: "file".into()
            })
            .contains(NAMESPACE, Mode::Exclusive)
        );
        assert!(
            io.keys(&Request::FileRead {
                directory: a,
                file: fa,
                max: 1
            })
            .contains(NAMESPACE, Mode::Shared)
        );
    }
}

#[test]
fn home_only_journal_is_retained_and_send_promotion_keeps_gate_history() {
    struct LocalJournal(std::rc::Rc<std::cell::Cell<usize>>);
    impl crate::journal::Journal for LocalJournal {
        fn event(&mut self, _: Option<u64>, _: &Event) -> std::io::Result<()> {
            self.0.set(self.0.get() + 1);
            Ok(())
        }
        fn end(&mut self, _: &hirsel_run_schema::RunOutcome) -> std::io::Result<()> {
            Ok(())
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let count = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut real = RealIo::grant(temp.path()).unwrap();
    real.journal_to(Box::new(LocalJournal(count.clone())));
    let stdout = handle(real.call(Request::Stdout { io: 1 }).unwrap());
    let mut real = match real.into_concurrent() {
        Err(real) => real,
        Ok(_) => panic!("moved home journal"),
    };
    real.call(Request::Print {
        stdout,
        bytes: b"local".to_vec(),
    })
    .unwrap();
    assert_eq!(count.get(), 2);
    let before = real.tape().to_vec();
    real.send_journal_to(Box::new(crate::journal::NoJournal));
    let io = real
        .into_concurrent()
        .unwrap_or_else(|_| panic!("Send journal"));
    io.call(Request::Print {
        stdout,
        bytes: b"shared".to_vec(),
    })
    .unwrap();
    assert_eq!(&io.tape()[..before.len()], before);
}

#[test]
fn mapping_and_open_file_discover_the_same_storage() {
    let (temp, io, directory) = adapter();
    std::fs::write(temp.path().join("mapped"), b"mapped bytes").unwrap();
    let file = handle(
        io.call(Request::Open {
            directory,
            path: "mapped".into(),
        })
        .unwrap(),
    );
    let mapped = handle(
        io.call(Request::Map {
            directory,
            path: "mapped".into(),
        })
        .unwrap(),
    );
    assert_eq!(io.identity(file), io.identity(mapped));
}
