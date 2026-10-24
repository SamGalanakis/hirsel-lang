//! Pinned google/uuid, uuid-rs and RFC 9562 expectations on four engines.
use hirsel_driver::{native::*, *};
use hirsel_engine::*;
use hirsel_rt::io::{Completion, Event, Request};
use serde_json::Value;
use std::path::PathBuf;

const VECTORS: &str = include_str!("../fixtures/uuid-upstream-25/vectors.json");
const PROBE: &str = include_str!("../fixtures/uuid-upstream-25/probe.hir");
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/uuid-upstream-25/probe.hir")
}
fn bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
fn literal(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn byte_literal(hex: &str) -> String {
    format!(
        "b\"{}\"",
        bytes(hex)
            .iter()
            .map(|b| format!("\\x{b:02x}"))
            .collect::<String>()
    )
}
fn oracle(output: Vec<u8>) -> Observation {
    Observation {
        exit_code: Some(0),
        blocked: false,
        budget: None,
        signaled: None,
        output,
        failures: vec![],
        roots: vec![],
    }
}
fn selected(group: &str) -> Vec<Value> {
    let rows: Vec<Value> = serde_json::from_str(VECTORS).unwrap();
    assert_eq!(rows.len(), 444);
    let rows: Vec<_> = rows.into_iter().filter(|r| r["group"] == group).collect();
    assert!(!rows.is_empty());

    rows
}
fn receipt(group: &str, rows: &[Value]) {
    eprintln!("UUID_SELECTED {group} {}", serde_json::to_string(&rows.iter().map(|r| serde_json::json!({"id":r["id"],"op":r["op"],"input":r["input_sha256"],"expected":r["expected_sha256"]})).collect::<Vec<_>>()).unwrap());
}
fn engine_selected(name: &str) -> bool {
    std::env::var("UUID_ENGINES").map_or(true, |s| s.split(',').any(|e| e == name))
}
fn checked(source: &str) -> LoadedProgram {
    let mut loaded = load_program_source(&fixture(), Some(source)).unwrap();
    loaded.check_for_execution().unwrap_or_else(|e| {
        if let ToolError::Query {
            error: QueryError::Diagnostics(ds),
            sources,
            ..
        } = &e
        {
            for d in ds {
                eprintln!("{}", d.render_human(sources));
            }
        }
        panic!("source-check: {e:?}")
    });
    eprintln!(
        "UUID_IDENTITY compiler={COMPILER_IDENTITY:?} llvm={} source={:?}",
        llvm_version(),
        hirsel_syntax::ContentHash::of_bytes(source.as_bytes())
    );
    loaded
}
fn partition(group: &str, start: usize, end: usize) {
    with_compiler_stack(|| {
        let selected_rows = selected(group);
        let rows = &selected_rows[start..end];
        receipt(group, rows);
        let mut source = format!(
            "{PROBE}\npub fn main(rt: Io) -> Result[(), OutputError] {{\nlet mut output: List[u8] = []\n"
        );
        let mut expected = Vec::new();
        for row in rows {
            let id = row["id"].as_str().unwrap();
            let input = &row["input"];
            let bits = row["oracle"]["uuid_rs"]["bits"].as_str().unwrap_or("");
            if row["op"] == "parse" {
                source.push_str(&format!(
                    "parse_case({}, {}, {}, mut output)\n",
                    literal(id),
                    literal(input["text"].as_str().unwrap()),
                    byte_literal(bits)
                ));
            } else {
                source.push_str(&format!(
                    "build_case({}, {}, {}u64, {}, {}, mut output)\n",
                    literal(id),
                    row["op"] == "v7",
                    input["millis"],
                    byte_literal(input["random_hex"].as_str().unwrap()),
                    byte_literal(bits)
                ));
            }
            expected.extend_from_slice(row["expected_output"].as_str().unwrap().as_bytes());
        }
        source.push_str(
            "out.write(io.stdout(rt) ? grant_error, bytes.from_list(take output))?\nOk(())\n}\n",
        );
        let mut loaded = checked(&source);
        let input = loaded.engine.export_native_input().expect("V-export");
        let validated = loaded.engine.import_native_input(input).expect("V-import");
        eprintln!("UUID_INPUT {:?}", input_identity(validated.snapshot()));
        let expected = oracle(expected);
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let observed = Observation::of_reference(
            reference
                .observation
                .as_ref()
                .unwrap_or_else(|| panic!("reference tooling: {:?}", reference.outcome)),
        );
        let mut errors = vec![];
        if observed != expected {
            errors.push(format!("reference mismatch {observed:?}"));
        }
        eprintln!(
            "UUID_CELL {group} reference {} ids={}",
            if observed == expected {
                "passed"
            } else {
                "failed"
            },
            rows.len()
        );
        let matrix: Vec<_> = [
            Engine::Heap { campaign: false },
            Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| Configuration {
            options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            engine,
        })
        .collect();
        let dir = tempfile::tempdir().unwrap();
        let mut tools = Toolchain::for_matrix(dir.path(), &matrix).unwrap();
        tools.launch_timeout = std::time::Duration::from_secs(60);
        let outcomes = differential(
            &validated,
            &loaded.entry,
            &expected,
            &matrix,
            &tools,
            dir.path(),
        );
        assert_eq!(outcomes.len(), 3);
        for (engine, result) in outcomes {
            eprintln!("UUID_CELL {group} {engine} {result:?} ids={}", rows.len());
            if !matches!(result, Outcome::Agree) {
                errors.push(format!("{engine}: {result:?}"));
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    });
}
macro_rules! batch {
    ($name:ident, $group:literal, $start:literal, $end:literal) => {
        #[test]
        fn $name() {
            partition($group, $start, $end);
        }
    };
}
batch!(parse_00_00, "parse_00", 0, 8);
batch!(parse_00_08, "parse_00", 8, 16);
batch!(parse_00_16, "parse_00", 16, 24);
batch!(parse_00_24, "parse_00", 24, 32);
batch!(parse_01_00, "parse_01", 0, 8);
batch!(parse_01_08, "parse_01", 8, 16);
batch!(parse_01_16, "parse_01", 16, 24);
batch!(parse_01_24, "parse_01", 24, 32);
batch!(parse_02_00, "parse_02", 0, 8);
batch!(parse_02_08, "parse_02", 8, 16);
batch!(parse_02_16, "parse_02", 16, 24);
batch!(parse_02_24, "parse_02", 24, 32);
batch!(parse_03_00, "parse_03", 0, 8);
batch!(parse_03_08, "parse_03", 8, 16);
batch!(parse_03_16, "parse_03", 16, 24);
batch!(parse_03_24, "parse_03", 24, 32);
batch!(parse_04_00, "parse_04", 0, 8);
batch!(parse_04_08, "parse_04", 8, 16);
batch!(parse_04_16, "parse_04", 16, 24);
batch!(parse_04_24, "parse_04", 24, 32);
batch!(parse_05_00, "parse_05", 0, 8);
batch!(parse_05_08, "parse_05", 8, 16);
batch!(parse_05_16, "parse_05", 16, 24);
batch!(parse_05_24, "parse_05", 24, 32);
batch!(parse_06_00, "parse_06", 0, 8);
batch!(parse_06_08, "parse_06", 8, 16);
batch!(parse_06_16, "parse_06", 16, 24);
batch!(parse_06_24, "parse_06", 24, 32);
batch!(parse_07_00, "parse_07", 0, 8);
batch!(parse_07_08, "parse_07", 8, 16);
batch!(parse_07_16, "parse_07", 16, 24);
batch!(parse_07_24, "parse_07", 24, 32);
batch!(v4_00_00, "v4_00", 0, 8);
batch!(v4_00_08, "v4_00", 8, 16);
batch!(v4_01_00, "v4_01", 0, 8);
batch!(v4_01_08, "v4_01", 8, 16);
batch!(v4_02_00, "v4_02", 0, 8);
batch!(v4_02_08, "v4_02", 8, 16);
batch!(v4_03_00, "v4_03", 0, 8);
batch!(v4_03_08, "v4_03", 8, 16);
batch!(v7_00_00, "v7_00", 0, 8);
batch!(v7_00_08, "v7_00", 8, 16);
batch!(v7_01_00, "v7_01", 0, 8);
batch!(v7_01_08, "v7_01", 8, 16);
batch!(v7_02_00, "v7_02", 0, 8);
batch!(v7_02_08, "v7_02", 8, 16);
batch!(v7_03_00, "v7_03", 0, 8);
batch!(v7_03_08, "v7_03", 8, 16);
batch!(mutations_00, "mutations_00", 0, 16);
batch!(mutations_01, "mutations_01", 0, 16);
batch!(controls_00, "controls", 0, 6);
batch!(controls_06, "controls", 6, 12);

// The existing checked-H Operations seam supplies the same tape as production
// replay. It owns only capability values; H still audits memory and cleanup.
struct Script(std::collections::VecDeque<Event>);
impl hirsel_heapcheck::Operations for Script {
    fn call(
        &mut self,
        op: &hirsel_core_contract::OperationId,
        args: &[hirsel_heapcheck::OpValue],
        output: &mut Vec<u8>,
    ) -> hirsel_heapcheck::OpOutcome {
        use hirsel_heapcheck::{OpOutcome as O, OpValue as V};
        use hirsel_lir::layout_types::ScalarKind;
        // Ordinary text leaves reached by project(), ported from
        // native/heap.rs::text. These consume no capability events.
        match (
            op.definition.0.modules[0].as_str(),
            op.definition.0.name.as_str(),
            args,
        ) {
            (
                "bytes",
                "decimal_word",
                [
                    V::Bits {
                        scalar: ScalarKind::U64,
                        bits: value,
                    },
                    V::Sequence(buffer),
                ],
            ) => {
                // As the production H oracle, use Rust's formatter independently
                // of the runtime numeric kernel. Preserve the untouched suffix.
                let digits = u64::try_from(*value).unwrap().to_string();
                assert!(digits.len() <= buffer.len());
                let mut buffer = buffer
                    .iter()
                    .map(|value| match value {
                        V::Bits {
                            scalar: ScalarKind::U8,
                            bits,
                        } => *bits,
                        _ => panic!("decimal buffer must contain bytes"),
                    })
                    .collect::<Vec<_>>();
                for (slot, byte) in buffer.iter_mut().zip(digits.bytes()) {
                    *slot = u128::from(byte);
                }
                return O::Updated {
                    value: V::Bits {
                        scalar: ScalarKind::U64,
                        bits: digits.len() as u128,
                    },
                    arguments: vec![(
                        1,
                        V::Elements {
                            scalar: ScalarKind::U8,
                            bits: buffer,
                        },
                    )],
                };
            }
            (
                "bytes",
                "append",
                [
                    V::Elements {
                        scalar: ScalarKind::U8,
                        bits,
                    },
                    V::Bytes(bytes),
                ],
            ) => {
                let mut bits = bits.clone();
                bits.extend(bytes.iter().map(|b| u128::from(*b)));
                return O::Updated {
                    value: V::Unit,
                    arguments: vec![(
                        0,
                        V::Elements {
                            scalar: ScalarKind::U8,
                            bits,
                        },
                    )],
                };
            }
            (
                "bytes",
                "from_list",
                [
                    V::Elements {
                        scalar: ScalarKind::U8,
                        bits,
                    },
                ],
            ) => return O::Value(V::ByteSequence(bits.iter().map(|b| *b as u8).collect())),
            ("string", "from_bytes", [V::Bytes(bytes)]) => {
                assert!(std::str::from_utf8(bytes).is_ok());
                return O::Value(V::Variant {
                    tag: 0,
                    value: Box::new(V::Slice {
                        argument: 0,
                        start: 0,
                        length: bytes.len() as u64,
                    }),
                });
            }
            _ => {}
        }
        let request = match (
            op.definition.0.modules[0].as_str(),
            op.definition.0.name.as_str(),
            args,
        ) {
            ("io", "wall_clock", [V::Handle(io)]) => Request::WallClock { io: *io },
            ("io", "random", [V::Handle(io)]) => Request::Random { io: *io },
            ("io", "stdout", [V::Handle(io)]) => Request::Stdout { io: *io },
            ("clock", "wall", [V::Handle(clock)]) => Request::Wall { clock: *clock },
            ("random", "bytes", [V::Handle(random), V::Bits { bits, .. }]) => {
                Request::RandomBytes {
                    random: *random,
                    count: *bits as u64,
                }
            }
            ("out", "print", [V::Handle(stdout), V::Bytes(bytes)]) => Request::Print {
                stdout: *stdout,
                bytes: bytes.clone(),
            },
            _ => panic!("unexpected operation {op:?} {args:?}"),
        };
        let event = self.0.pop_front().expect("extra capability request");
        assert_eq!(request, event.request, "scripted request order/arguments");
        let ok = |v| V::Variant {
            tag: 0,
            value: Box::new(v),
        };
        let error = |tag| V::Variant {
            tag: 1,
            value: Box::new(V::Variant {
                tag,
                value: Box::new(V::Unit),
            }),
        };
        let value = match event.completion {
            Completion::ClockGrant(Ok(h)) | Completion::Handle(Ok(h)) => ok(V::Handle(h)),
            Completion::Wall(Ok(n)) => ok(V::Bits {
                scalar: ScalarKind::I64,
                bits: n as u64 as u128,
            }),
            Completion::Wall(Err(hirsel_rt::io::ClockError::Unavailable)) => error(0),
            Completion::Wall(Err(hirsel_rt::io::ClockError::OutOfRange)) => error(1),
            Completion::Random(Ok(b)) => ok(V::ByteSequence(b)),
            Completion::Random(Err(_)) => error(0),
            Completion::Unit(Ok(())) => {
                if let Request::Print { bytes, .. } = request {
                    output.extend(bytes);
                }
                ok(V::Unit)
            }
            other => panic!("unexpected completion {other:?}"),
        };
        O::Value(value)
    }
}
fn effects(group: &str) {
    with_compiler_stack(|| {
        let rows = selected(group);
        receipt(group, &rows);
        let mut source = format!(
            "{PROBE}\npub fn main(rt: Io) -> Result[(), OutputError] {{\nlet clock = io.wall_clock(rt) ? grant_error\nlet r = io.random(rt) ? grant_error\nlet output = io.stdout(rt) ? grant_error\n"
        );
        let mut events = vec![
            Event {
                request: Request::WallClock { io: 1 },
                completion: Completion::ClockGrant(Ok(2)),
                fault: None,
            },
            Event {
                request: Request::Random { io: 1 },
                completion: Completion::Handle(Ok(3)),
                fault: None,
            },
            Event {
                request: Request::Stdout { io: 1 },
                completion: Completion::Handle(Ok(4)),
                fault: None,
            },
        ];
        let mut expected = Vec::new();
        for row in &rows {
            let input = &row["input"];
            let seven = row["op"] == "v7";
            let expression = if seven {
                "uuid.v7(clock, r)"
            } else {
                "uuid.v4(r)"
            };
            let arms = if seven {
                "Error(BeforeEpoch()) => \"BeforeEpoch\", Error(Clock(Unavailable())) => \"ClockUnavailable\", Error(Clock(OutOfRange())) => \"ClockOutOfRange\", Error(Random(Unavailable())) => \"RandomUnavailable\""
            } else {
                "Error(Unavailable()) => \"RandomUnavailable\""
            };
            source.push_str(&format!("{{\nlet answer = match {expression} {{ Ok(id) => project(id), {arms} }}\nout.print(output, f\"{}|{{answer}}\\n\")?\n}}\n",row["id"].as_str().unwrap()));
            let fault = input["fault"].as_str().unwrap_or("");
            if seven {
                let wall = match fault {
                    "unavailable" => Err(hirsel_rt::io::ClockError::Unavailable),
                    "range" => Err(hirsel_rt::io::ClockError::OutOfRange),
                    _ => Ok(input["wall"].as_i64().unwrap()),
                };
                events.push(Event {
                    request: Request::Wall { clock: 2 },
                    completion: Completion::Wall(wall),
                    fault: None,
                });
            }
            if input["reads"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.as_str().unwrap().starts_with("random.bytes:"))
            {
                let random = bytes(input["random_hex"].as_str().unwrap());
                events.push(Event {
                    request: Request::RandomBytes {
                        random: 3,
                        count: random.len() as u64,
                    },
                    completion: Completion::Random(if fault == "random" {
                        Err(hirsel_rt::io::RandomError::Unavailable)
                    } else {
                        Ok(random)
                    }),
                    fault: None,
                });
            }
            let line = row["expected_output"].as_str().unwrap().as_bytes();
            expected.extend(line);
            events.push(Event {
                request: Request::Print {
                    stdout: 4,
                    bytes: line.to_vec(),
                },
                completion: Completion::Unit(Ok(())),
                fault: None,
            });
        }
        source.push_str("Ok(())\n}\n");
        let mut loaded = checked(&source);
        let input = loaded.engine.export_native_input().expect("V-export");
        let validated = loaded.engine.import_native_input(input).expect("V-import");
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let dir = scratch.path();
        // Use the real source package graph for public production replay admission.
        let path = dir.join("main.hir");
        std::fs::write(&path, &source).unwrap();
        let std_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = serde_json::json!({"schema":1,"package":{"name":"uuid_probe","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":std_manifest,"package":{"name":"std","version":"1"},"expose":[]}]});
        std::fs::write(
            path.with_extension("hir.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let mut run: hirsel_run_config::RunConfig =
            serde_json::from_slice(&export_run_path(&path).unwrap()).unwrap();
        run.io = Some(hirsel_run_schema::IoGrants {
            stdout: Some("stdout.bytes@1".into()),
            wall_clock: Some("clock.wall.unix_ns@1".into()),
            random: Some("random.splitmix64@1".into()),
            ..Default::default()
        });
        let run = run.validated().unwrap();
        let record = hirsel_rt::launch::Recording {
            arguments: vec![],
            run_identity: run.config().run_identity(),
            hash_seed: 0,
            random_seed: 0,
            test_seed: 0,
            binding: Some(hirsel_rt::launch::RecordedRun {
                canonical: run.canonical().to_vec(),
                placement: None,
                admitted_at: 0,
                revocation: None,
                instance: None,
            }),
            tree: hirsel_rt::io::Tree::new(Default::default()).unwrap(),
            events: events.clone(),
        };
        let tape = dir.join("script.tape");
        std::fs::write(&tape, hirsel_rt::launch::render_recording(&record).unwrap()).unwrap();
        let expected = oracle(expected);
        if engine_selected("reference") {
            let reference =
                replay_observations(&path, &tape, Some(1_000_000)).expect("reference replay");
            assert_eq!(
                Observation::of_reference(&reference),
                expected,
                "reference {group}"
            );
            eprintln!("UUID_CELL {group} reference passed ids={}", rows.len());
        }
        if engine_selected("checked-H") {
            let heap = ready.heap_program();
            let mut script = Script(events.into());
            let h = hirsel_heapcheck::run_ready(
                &heap,
                &loaded.entry,
                &[hirsel_heapcheck::Arg::Handle(1)],
                &mut script,
                &hirsel_heapcheck::Config {
                    context_handles: vec![(1, "Io".into())],
                    ..Default::default()
                },
            );
            let audit = h.result.expect("checked-H trap");
            assert_eq!(
                audit.observation.returned,
                Some(hirsel_heapcheck::Observed::Variant {
                    tag: 0,
                    fields: vec![Some(hirsel_heapcheck::Observed::Unit)]
                })
            );
            assert!(audit.observation.signaled.is_none());
            assert!(audit.observation.failures.is_empty());
            assert!(!audit.observation.blocked);
            assert_eq!(h.output, expected.output);
            assert!(script.0.is_empty(), "unconsumed scripted input");
            assert_eq!(h.counters.live_heap_bytes, 0);
            eprintln!("UUID_CELL {group} checked-H passed ids={}", rows.len());
        }
        for backend in [Backend::Llvm, Backend::Cranelift] {
            if !engine_selected(&format!("{backend:?}")) {
                continue;
            }
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let outcome = run_path_native_backend_with_validation(
                &path,
                HostLaunch::Replay(&tape),
                None,
                backend,
                Output::Streams {
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                Verify::On,
                Validation::EveryStage,
            )
            .expect("native source check")
            .expect("native replay tool failure");
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{backend:?}: {outcome:?}"
            );
            assert_eq!(stdout, expected.output, "{backend:?} {group}");
            assert!(
                stderr.is_empty(),
                "{backend:?}: {}",
                String::from_utf8_lossy(&stderr)
            );
            eprintln!("UUID_CELL {group} {backend:?} passed ids={}", rows.len());
        }
    });
}
#[test]
fn effects_00() {
    effects("effects_00");
}
#[test]
fn effects_01() {
    effects("effects_01");
}
#[test]
fn effects_02() {
    effects("effects_02");
}
#[test]
fn effects_03() {
    effects("effects_03");
}
