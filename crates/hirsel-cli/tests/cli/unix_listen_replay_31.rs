//! Snapshot and host Unix listeners replay from the tape's authority, without providers.
use hirsel_driver::native::*;
use hirsel_rt::io::{Completion, Request};
use serde_json::json;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

const SOURCE: &str = r#"pub type AppError = Grant(GrantError) | Net(NetError) | Arg(ArgError)
pub fn main(rt: Io, args: Args) -> Result[(), AppError] {
  let n = io.net(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let p = arg.path(args, 0) ? Arg
  let denied = arg.text(args, 1) ? Arg == "denied"
  let mut listener = match net.unix_listen(n, p) {
    Ok(l) => take l,
    Error(e) => {
      failure.check(denied && e == AddrInUse(), Arithmetic())
      let _ = out.print(output, "denied\n")
      return Ok(())
    },
  }
  failure.check(!denied, Arithmetic())
  let mut (stream, _) = net.accept(n, mut listener) ? Net
  let mut data: List[u8] = []
  failure.check(net.read(n, mut stream, mut data, 1) ? Net == 1 && data == [113u8], Arithmetic())
  net.write_all(n, mut stream, b"a") ? Net
  net.close(n, take stream)
  let _ = out.print(output, "accepted\n")
  Ok(())
}
"#;

fn package(root: &Path) -> std::path::PathBuf {
    fs::write(root.join("main.hir"), SOURCE).unwrap();
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, json!({"schema":1,"package":{"name":"unix_replay","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    manifest
}

#[test]
fn unix_listen_snapshot_and_stale_host_tapes_replay_at_each_width() {
    hirsel_engine::with_compiler_stack(|| {
        let base = std::env::var_os("UNIX_LISTEN_REPLAY_31_EVIDENCE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let scratch = tempfile::Builder::new()
            .prefix("ul31-")
            .tempdir_in(base)
            .unwrap();
        let root = scratch.path().canonicalize().unwrap();
        let manifest = package(&root);
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap();
        loaded.check_for_execution().unwrap();
        let snapshot = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(snapshot).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let ready = build(&input, &options).unwrap();
        let sites = Sites::new(input.snapshot());
        let matrix = [Configuration {
            options,
            engine: Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        }];
        let mut tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        let mut objects = Vec::new();
        for backend in [Backend::Llvm, Backend::Cranelift] {
            objects.push((
                backend,
                emit_backend_objects(
                    &ready,
                    backend,
                    OptLevel::None,
                    Partition::Whole,
                    false,
                    None,
                )
                .unwrap(),
            ));
        }
        for case in [
            "script-listen",
            "script-remove-stale",
            "host-stale-granted",
            "host-stale-denied",
        ] {
            let host = case.starts_with("host");
            let denied = case.ends_with("denied");
            let remove = case.contains("granted") || case == "script-remove-stale";
            let socket = root.join("app.sock");
            let socket = socket.to_str().unwrap();
            let mut net = json!({"provider":if host {"net.stream@1"} else {"net.script@2"},"rules":[{"address":socket,"service":"0","operation":if remove {"unix_listen_remove_stale"} else {"unix_listen"}}]});
            if !host {
                net["peers"] = json!([]);
                net["listeners"] = json!([{"address":socket,"port":0,"clients":[{"address":"0.0.0.0","port":0,"tls":"plain","steps":[{"send":[113]},{"expect":[97]},"close"]}]}]);
            }
            let fixture = Fixture {
                args: vec![
                    socket.into(),
                    if denied { "denied" } else { "accept" }.into(),
                ],
                mounts: Default::default(),
                net: Some(serde_json::from_value(net).unwrap()),
                ..Default::default()
            };
            let unavailable = Fixture {
                net: None,
                ..fixture.clone()
            };
            let expected: &[u8] = if denied { b"denied\n" } else { b"accepted\n" };
            let mut runnables = Vec::new();
            for (backend, objects) in &objects {
                let dir = root.join(format!("{case}-{backend:?}"));
                fs::create_dir(&dir).unwrap();
                let runnable =
                    link_fixture_runnable(&ready, &loaded.entry, objects, &tools, &dir, &fixture)
                        .unwrap();
                runnables.push((backend, runnable, dir));
            }
            for (backend, runnable, dir) in &runnables {
                for width in [1, 2, 4] {
                    tools.wrapper = vec![
                        "/usr/bin/env".into(),
                        format!("HIRSEL_RT_WORKERS={width}"),
                        "HIRSEL_JOBS=4".into(),
                    ];
                    if host {
                        // A closed listener leaves a stale socket inode on Linux.
                        drop(std::os::unix::net::UnixListener::bind(socket).unwrap());
                    }
                    let client = (host && !denied).then(|| {
                        let socket = socket.to_owned();
                        std::thread::spawn(move || {
                            use std::io::{Read, Write};
                            let end = Instant::now() + Duration::from_secs(20);
                            let mut stream = loop {
                                if let Ok(stream) = std::os::unix::net::UnixStream::connect(&socket)
                                {
                                    break stream;
                                }
                                assert!(Instant::now() < end, "listener never opened");
                                std::thread::sleep(Duration::from_millis(2));
                            };
                            stream
                                .set_read_timeout(Some(Duration::from_secs(20)))
                                .unwrap();
                            stream.write_all(b"q").unwrap();
                            let mut reply = [0];
                            stream.read_exact(&mut reply).unwrap();
                            assert_eq!(reply, *b"a");
                        })
                    });
                    let tape = dir.join(format!("width-{width}.tape"));
                    let recorded = launch_fixture(
                        runnable,
                        &ready.target,
                        &tools,
                        dir,
                        true,
                        Tape::Live {
                            record: Some(&tape),
                            revocations: None,
                        },
                        &fixture,
                    )
                    .unwrap();
                    if let Some(client) = client {
                        client.join().unwrap();
                    }
                    let observed = observe(&recorded, &ready, &sites).unwrap();
                    assert_eq!(observed.output, expected, "{case}/{backend:?}/{width}");
                    assert_eq!(observed.exit_code, Some(0));
                    assert!(observed.failures.is_empty() && !observed.blocked);
                    if denied {
                        fs::remove_file(socket).unwrap();
                    }
                    assert!(!Path::new(socket).exists(), "recording left a socket");
                    // A regular file at the socket path must survive every replay untouched.
                    fs::write(socket, b"replay must not bind or unlink").unwrap();
                    let bytes = fs::read(&tape).unwrap();
                    let record = hirsel_rt::host_launch::read_recording(&tape).unwrap();
                    let listen = record
                        .events
                        .iter()
                        .find(|e| matches!(e.request, Request::UnixListen { .. }))
                        .unwrap();
                    assert!(
                        matches!(
                            listen.completion,
                            Completion::Listened(Err(hirsel_rt::net::NetError::AddrInUse))
                        ) == denied
                    );
                    for (name, replay) in [
                        (
                            "reference",
                            hirsel_driver::replay_observations
                                as fn(&Path, &Path, Option<u64>) -> _,
                        ),
                        ("checked", hirsel_driver::replay_observations_checked),
                    ] {
                        let value = replay(&manifest, &tape, Some(1_000_000)).unwrap();
                        assert_eq!(value.output, expected, "{name}");
                        assert_eq!(value.exit_code, Some(0));
                    }
                    for (target, executable, replay_dir) in &runnables {
                        let replayed = launch_fixture(
                            executable,
                            &ready.target,
                            &tools,
                            replay_dir,
                            true,
                            Tape::Replay(&tape),
                            &unavailable,
                        )
                        .unwrap();
                        assert_eq!(
                            observe(&replayed, &ready, &sites).unwrap(),
                            observed,
                            "{case}/{backend:?}->{target:?}/{width}"
                        );
                    }
                    // An otherwise valid tape with a changed path still mismatches.
                    let mut bad = record;
                    let event = bad
                        .events
                        .iter_mut()
                        .find(|e| matches!(e.request, Request::UnixListen { .. }))
                        .unwrap();
                    let Request::UnixListen { path, .. } = &mut event.request else {
                        unreachable!()
                    };
                    path.push_str(".changed");
                    let tampered = dir.join("tampered.tape");
                    fs::write(
                        &tampered,
                        hirsel_rt::launch::render_recording(&bad).unwrap(),
                    )
                    .unwrap();
                    assert!(
                        hirsel_driver::replay_observations(&manifest, &tampered, Some(1_000_000))
                            .unwrap_err()
                            .to_string()
                            .contains("ReplayMismatch")
                    );
                    for (_, executable, replay_dir) in &runnables {
                        let refused = launch_fixture(
                            executable,
                            &ready.target,
                            &tools,
                            replay_dir,
                            true,
                            Tape::Replay(&tampered),
                            &unavailable,
                        )
                        .unwrap();
                        assert_eq!(refused.exit, Some(3));
                        let error = observe(&refused, &ready, &sites).unwrap_err();
                        assert!(error.to_string().contains("ReplayMismatch"), "{error}");
                    }
                    assert_eq!(fs::read(&tape).unwrap(), bytes);
                    assert_eq!(fs::read(socket).unwrap(), b"replay must not bind or unlink");
                    fs::remove_file(socket).unwrap();
                    eprintln!("UNIX_LISTEN_REPLAY_31_PASS {case}/{backend:?}/width-{width}");
                }
            }
        }
    });
}
