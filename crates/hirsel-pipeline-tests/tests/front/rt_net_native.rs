//! Every native network export through compiled calls (`native-tests/library/net-stream.hir`),
//! with `net.read_into` through its compiled library body:
//! a live run records against an in-process loopback server, and the tape replays with the
//! server gone. Replay under rules that deny a recorded connect refuses (exit 3). A
//! net.script@2 grant runs the same program against scripted peers and replays it.
#[path = "../../../hirsel-rt/tests/support/io.rs"]
mod rt_io_support;
#[path = "../../../hirsel-driver/tests/support/source_std.rs"]
mod source_std;
use hirsel_driver::native::{
    Launched, OptLevel, Options, Partition, Sites, Toolchain, build, link, observe,
};
use hirsel_engine::{check_progress, default_context, entry_path, with_compiler_stack};
use hirsel_lir::layout_types::TargetSpec;
use hirsel_rt::io::Request;
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;

/// The lines `net-stream.hir` prints before the server's reply.
const PREFIX: &str = "narrow beyond refused\nname not granted\nzero timeout invalid\n\
    resolved one address\ncopied resolved list\n";

/// The launcher's root Net grant (`HIRSEL_RT_NET`): a RunConfig `io.net` field as JSON that
/// connects to the loopback port; `peers` makes it a net.script@2 world.
fn grant(port: u16, peers: Option<&str>) -> String {
    let rules = format!(r#"[{{"address":"127.0.0.1","service":"{port}","operation":"connect"}}]"#);
    match peers {
        None => format!(r#"{{"provider":"net.stream@1","rules":{rules}}}"#),
        Some(peers) => {
            format!(r#"{{"provider":"net.script@2","rules":{rules},"peers":{peers}}}"#)
        }
    }
}

/// The scripted twin of `serve()`: the first connection expects the request and answers
/// before closing, the second is opened and closed. Byte strings are JSON arrays.
fn peers(port: u16) -> String {
    format!(
        r#"[{{"address":"127.0.0.1","port":{port},"resolves_to":["127.0.0.1"],"tls":"plain","connections":[{{"open":[{{"expect":{:?}}},{{"send":{:?}}},"close"]}},{{"open":[]}}]}}]"#,
        b"ping over\n", b"pong\n"
    )
}

/// The process the driver's `launch_with` runs, plus the launcher's network grant and the
/// server port as argument 0.
fn run(
    executable: &Path,
    target: &TargetSpec,
    tools: &Toolchain,
    work: &Path,
    tape: (&str, &Path),
    rules: &str,
    port: u16,
) -> Launched {
    let root = work.join("root");
    let record = work.join("observation");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&record);
    std::fs::create_dir_all(&root).unwrap();
    let mut command = if target.triple.starts_with("aarch64") {
        let mut command = Command::new(&tools.qemu_aarch64);
        command
            .arg("-L")
            .arg(&tools.aarch64_sysroot)
            .arg(executable);
        command
    } else {
        Command::new(executable)
    };
    let output = command
        .arg(port.to_string())
        .env_clear()
        .env("HIRSEL_RT_ROOT", &root)
        .env("HIRSEL_RT_OBSERVATION", &record)
        .env("HIRSEL_RT_DIAGNOSTIC", "1")
        .env("HIRSEL_RT_NET", rules)
        .env(tape.0, tape.1)
        .output()
        .unwrap();
    Launched {
        audit_required: true,
        exit: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        record: std::fs::read_to_string(&record).unwrap_or_default(),
    }
}

/// Two connections: the first is answered once the client half-closes, the second is
/// only opened and closed by the program.
fn serve() -> (u16, std::thread::JoinHandle<(Vec<u8>, Vec<u8>)>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let thread = std::thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        let mut request = vec![];
        first.read_to_end(&mut request).unwrap();
        first.write_all(b"pong\n").unwrap();
        drop(first);
        let (mut second, _) = listener.accept().unwrap();
        let mut rest = vec![];
        second.read_to_end(&mut rest).unwrap();
        (request, rest)
    });
    (port, thread)
}

fn kind(request: &Request) -> Option<&'static str> {
    Some(match request {
        Request::NetNarrow { .. } => "narrow",
        Request::Resolve { .. } => "resolve",
        Request::Connect { .. } => "connect",
        Request::ConnectAddr { .. } => "connect_addr",
        // The program's one `read_into` has a two-byte buffer; its `read`s ask for three.
        Request::NetRead { max: 2, .. } => "read_into",
        Request::NetRead { .. } => "read",
        Request::NetWrite { .. } => "write",
        Request::NetWriteAll { .. } => "write_all",
        Request::Shutdown { .. } => "shutdown",
        Request::SetTimeout { .. } => "set_timeout",
        Request::PeerAddr { .. } => "peer_addr",
        Request::Close { .. } => "close",
        _ => return None,
    })
}

fn campaign(targets: Vec<TargetSpec>, opts: &'static [OptLevel], scratch_name: &'static str) {
    with_compiler_stack(move || {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join(scratch_name);
        std::fs::create_dir_all(&scratch).unwrap();
        let tools = Toolchain::in_tree_diagnostic(
            &scratch.join("runtime"),
            &targets.iter().collect::<Vec<_>>(),
        )
        .unwrap();
        let source =
            std::fs::read_to_string(root.join("native-tests/library/net-stream.hir")).unwrap();
        let mut engine = source_std::engine();
        let context = default_context();
        let file = engine.sources.add("net-stream.hir", source).unwrap();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&context);
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let sites = Sites::new(input.snapshot());
        for target in &targets {
            let program = build(&input, &Options::new(target.clone())).unwrap();
            for &opt in opts {
                let work = scratch.join(format!("{}-{opt:?}", target.triple));
                std::fs::create_dir_all(&work).unwrap();
                let executable =
                    link(&program, &entry, opt, Partition::Whole, &tools, &work).unwrap();
                let tape = work.join("tape");
                let (port, server) = serve();
                let rules = grant(port, None);
                let recorded = run(
                    &executable,
                    target,
                    &tools,
                    &work,
                    ("HIRSEL_RT_TAPE", &tape),
                    &rules,
                    port,
                );
                let observed = observe(&recorded, &program, &sites).unwrap();
                let expected = format!("{PREFIX}pong\nclosed\n");
                assert_eq!(
                    observed.exit_code,
                    Some(0),
                    "{recorded:?}\n{}",
                    std::fs::read_to_string(&tape).unwrap_or_default()
                );
                assert!(observed.failures.is_empty(), "{observed:?}");
                assert_eq!(String::from_utf8_lossy(&observed.output), expected);
                let (request, rest) = server.join().unwrap();
                assert_eq!(request, b"ping over\n");
                assert!(rest.is_empty());
                // The tape holds every exported operation as a typed network event.
                let recording =
                    hirsel_rt::launch::parse_recording(&std::fs::read_to_string(&tape).unwrap())
                        .unwrap();
                let kinds: std::collections::BTreeSet<_> = recording
                    .events
                    .iter()
                    .filter_map(|e| kind(&e.request))
                    .collect();
                assert_eq!(
                    kinds,
                    [
                        "narrow",
                        "resolve",
                        "connect",
                        "connect_addr",
                        "read",
                        "read_into",
                        "write",
                        "write_all",
                        "shutdown",
                        "set_timeout",
                        "peer_addr",
                        "close",
                    ]
                    .into(),
                    "{:?}",
                    recording.events
                );
                // Independent interpreter execution and strict interpreter replay,
                // with the real server gone. Compare every completion, surviving
                // grant, final tree and stdout with the original native tape.
                use hirsel_rt::{
                    io::Adapter,
                    io_sim::SimIo,
                    net::{NetOperation, NetRule},
                };
                let mut sim = SimIo::new(recording.tree.clone(), recording.events.clone());
                sim.set_net_rules(
                    [NetRule {
                        address: "127.0.0.1".into(),
                        port,
                        operation: NetOperation::Connect,
                    }]
                    .into(),
                );
                for event in &recording.events {
                    assert_eq!(sim.call(event.request.clone()).unwrap(), event.completion);
                }
                sim.finish().unwrap();
                rt_io_support::replay_interpreter(
                    &recording.events,
                    &recording.tree,
                    sim.tree(),
                    sim.handles().observed(),
                    &observed.output,
                );
                let mut altered = recording.events.clone();
                let bytes = altered
                    .iter_mut()
                    .find_map(|event| match &mut event.completion {
                        hirsel_rt::io::Completion::NetBytes(Ok(bytes)) if !bytes.is_empty() => {
                            Some(bytes)
                        }
                        _ => None,
                    })
                    .expect("loopback reply bytes");
                bytes[0] ^= 0x20;
                let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    rt_io_support::replay_interpreter_with_fixture(
                        &altered,
                        rt_io_support::ReplayFixture {
                            events: &recording.events,
                            pem_roots: &[],
                        },
                        &recording.tree,
                        sim.tree(),
                        sim.handles().observed(),
                        &observed.output,
                        &std::collections::BTreeMap::from([(
                            String::new(),
                            hirsel_run_config::Access::ReadWrite,
                        )]),
                    );
                }));
                assert!(
                    rejected.is_err(),
                    "an altered TCP completion passed the independent comparison"
                );
                // The server is gone: replay uses no socket.
                let replayed = run(
                    &executable,
                    target,
                    &tools,
                    &work,
                    ("HIRSEL_RT_REPLAY", &tape),
                    &rules,
                    port,
                );
                assert_eq!(
                    observe(&replayed, &program, &sites).unwrap(),
                    observed,
                    "{replayed:?}"
                );
                // Replay re-derives grants: without the rule, the recorded connect refuses.
                let refused = run(
                    &executable,
                    target,
                    &tools,
                    &work,
                    ("HIRSEL_RT_REPLAY", &tape),
                    r#"{"provider":"net.stream@1","rules":[]}"#,
                    port,
                );
                assert_eq!(refused.exit, Some(3), "{refused:?}");
                assert!(
                    String::from_utf8_lossy(&refused.stderr).contains("refused"),
                    "{refused:?}"
                );
                // A net.script@2 grant serves the same program from its peers with no
                // socket (the server is gone), and its tape replays under the same grant.
                let script = grant(port, Some(&peers(port)));
                let scripted = run(
                    &executable,
                    target,
                    &tools,
                    &work,
                    ("HIRSEL_RT_TAPE", &tape),
                    &script,
                    port,
                );
                let scripted_observation = observe(&scripted, &program, &sites).unwrap();
                assert_eq!(scripted_observation.exit_code, Some(0), "{scripted:?}");
                assert_eq!(
                    String::from_utf8_lossy(&scripted_observation.output),
                    expected
                );
                let replayed = run(
                    &executable,
                    target,
                    &tools,
                    &work,
                    ("HIRSEL_RT_REPLAY", &tape),
                    &script,
                    port,
                );
                assert_eq!(
                    observe(&replayed, &program, &sites).unwrap(),
                    scripted_observation,
                    "{replayed:?}"
                );
                // Peers never widen policy: without the rule, the scripted connect is
                // NotGranted and the program returns Error.
                let narrow = format!(
                    r#"{{"provider":"net.script@2","rules":[],"peers":{}}}"#,
                    peers(port)
                );
                let denied = run(
                    &executable,
                    target,
                    &tools,
                    &work,
                    ("HIRSEL_RT_TAPE", &tape),
                    &narrow,
                    port,
                );
                assert_eq!(denied.exit, Some(1), "{denied:?}");
                // A peer table under the host provider is not an admissible grant.
                let invalid = run(
                    &executable,
                    target,
                    &tools,
                    &work,
                    ("HIRSEL_RT_TAPE", &tape),
                    &format!(
                        r#"{{"provider":"net.stream@1","rules":[],"peers":{}}}"#,
                        peers(port)
                    ),
                    port,
                );
                assert_eq!(invalid.exit, Some(3), "{invalid:?}");
            }
        }
    });
}

#[test]
fn net_exports_record_against_loopback_and_replay_in_smoke() {
    campaign(
        vec![TargetSpec::x86_64_unknown_linux_gnu()],
        &[OptLevel::Speed],
        "net-native-smoke",
    );
}

#[test]
#[ignore = "full gate"]
fn full_gate_net_exports_record_and_replay_on_both_targets() {
    campaign(
        vec![
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ],
        &[OptLevel::None, OptLevel::Speed],
        "net-native",
    );
}

/// `net.read_into` compiles to one library body per buffer size, whose only call is the
/// fixed runtime entry with the size as its constant `capacity`. Validation C accepts the
/// built body and refuses forged calls: one without `capacity`, one lending the buffer
/// read-only, and one passing `capacity` as the buffer.
#[test]
fn read_into_body_calls_its_fixed_entry_and_validation_refuses_forged_calls() {
    use hirsel_lir::ir::{ArgumentValue, Callee, Inst, UnitOrigin};
    use hirsel_lir::validate::{Inputs, validate};
    with_compiler_stack(|| {
        let source = "pub fn fill(n: Net, mut s: Stream of n) -> Result[u64, NetError] {\n  \
            let mut buffer: Inline[u8, 6] = [0u8, 0u8, 0u8, 0u8, 0u8, 0u8]\n  \
            net.read_into(n, mut s, mut buffer)\n}\n\
            pub fn main(rt: Io, args: Args) -> Result[(), ()] {\n  Ok(())\n}\n";
        let mut engine = source_std::engine();
        let context = default_context();
        let file = engine.sources.add("read-into.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let ready = build(
            &input,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let inputs = Inputs {
            operations: &ready.operations,
            target: &ready.target,
            layouts: &ready.table,
            manifest: &ready.manifest,
        };
        let path = |name: &str| hirsel_rc::library::core_path(name);
        let bodies: Vec<_> = ready
            .units
            .iter()
            .map(|u| u.unit())
            .filter(|u| {
                u.header.origin == UnitOrigin::Library
                    && u.header.definition == path("net.read_into")
            })
            .collect();
        let [body] = bodies.as_slice() else {
            panic!("one net.read_into body expected, found {}", bodies.len());
        };
        let [call] = body.tables.calls.as_slice() else {
            panic!("one call expected: {:?}", body.tables.calls);
        };
        let Callee::Operation(operation) = call.callee else {
            panic!("an operation call expected: {call:?}");
        };
        assert_eq!(
            body.tables.operations[operation.index()].definition,
            path("$intrinsic.NetReadInto")
        );
        let arguments = call.arguments.get(&body.pools.call_arguments).unwrap();
        assert_eq!(arguments.len(), 4);
        let ArgumentValue::Value(capacity) = arguments[3].value else {
            panic!("capacity is a value: {arguments:?}");
        };
        let constant = body
            .instructions
            .iter()
            .find_map(|inst| match inst {
                Inst::Const { result, constant } if *result == capacity => Some(*constant),
                _ => None,
            })
            .expect("capacity is a constant");
        assert_eq!(
            body.tables.constants[constant.index()].value,
            hirsel_lir::ir::ConstantValue::Integer(6)
        );
        let raw = (*body).clone();
        validate(raw.clone(), &inputs).unwrap_or_else(|v| panic!("{:?}", v.violations));
        let mut missing = raw.clone();
        missing.tables.calls[0].arguments.len = 3;
        assert!(
            validate(missing, &inputs).is_err(),
            "a read_into call without capacity validated"
        );
        let mut borrowed = raw.clone();
        let at = borrowed.tables.calls[0].arguments.start as usize + 2;
        borrowed.pools.call_arguments[at].passing = hirsel_core_contract::Passing::Borrow;
        assert!(
            validate(borrowed, &inputs).is_err(),
            "a read_into call lending its buffer read-only validated"
        );
        let mut shifted = raw;
        let at = shifted.tables.calls[0].arguments.start as usize + 3;
        shifted.pools.call_arguments[at].parameter = 2;
        assert!(
            validate(shifted, &inputs).is_err(),
            "a read_into call passing capacity as the buffer validated"
        );
    });
}
