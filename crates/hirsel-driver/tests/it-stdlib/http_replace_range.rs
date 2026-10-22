//! Rust slice::copy_within vectors (f6c1107f8be1, coretests/tests/slice.rs:2030-2088),
//! adapted for Hirsel immutable aliases, MEM-14 and bounded-arena accounting.
use hirsel_driver::*;
use hirsel_engine::*;

fn parity(label: &str, body: &str, rule: Option<&str>) {
    with_compiler_stack(|| {
        let source = if body.contains("ReplaceTrace(") {
            format!(
                "resource type ReplaceTrace {{ stdout: Stdout, crash: bool }}\ndrop(x) {{ let _ = out.print(x.stdout, \"closed-once\\n\")\n if x.crash {{ failure.check(false, Arithmetic()) }} }}\n@noalloc\nfn grant_error(e: GrantError) -> () uses {{}} {{ () }}\npub fn main(rt: Io, args: Args) -> Result[(), ()] {{\n let stdout = io.stdout(rt) ? grant_error\n{body}\nOk(())\n}}\n"
            )
        } else {
            format!(
                "type ReplaceBuffer {{ buffer: List[u8] }}\nflat type PackedBytes {{ data: Bytes }}\npub fn main(t: Test) -> Result[(), ()] {{\n{body}\nOk(())\n}}\n"
            )
        };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/std_source/main.hir");
        let mut program = load_program_source(&path, Some(&source))
            .unwrap_or_else(|e| panic!("{label}: {}\n{source}", details(&e)));
        program
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{label}: {}\n{source}", details(&e)));
        let config = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let recorded =
            run_observed_snapshot(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        let observation = recorded.observation.unwrap();
        match rule {
            None => assert!(observation.failures.is_empty(), "{label}: {observation:?}"),
            Some(rule) => assert_eq!(
                observation.failures.first().map(|f| f.rule.clone()),
                Some(hirsel_diag::RuleId::new(rule).unwrap()),
                "{label}: {observation:?}"
            ),
        }
        if body.contains("ReplaceTrace(") {
            assert_eq!(
                String::from_utf8_lossy(&observation.output)
                    .matches("closed-once")
                    .count(),
                1,
                "{label}: {observation:?}"
            );
            assert_eq!(observation.effects.iter().filter(|e| matches!(&e.effect, hirsel_interp::observation::Effect::Finalizer { definition } if definition.0.name == "ReplaceTrace")).count(), 1);
        }
        let checked =
            run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        assert_eq!(
            checked.observation.as_ref(),
            Some(&observation),
            "{label}: checked"
        );
        let events = observation
            .effects
            .iter()
            .filter_map(|e| match &e.effect {
                hirsel_interp::observation::Effect::CapabilityConsumed(e) => Some(e.clone()),
                _ => None,
            })
            .collect();
        let replayed = run_observed_snapshot(
            &mut program.engine,
            &RunConfiguration {
                inputs: events,
                ..config
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert_eq!(
            replayed.observation.as_ref(),
            Some(&observation),
            "{label}: replay"
        );
        let expected = native::Observation::of_reference(&observation);
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Heap { campaign: true },
            },
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::None,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Default,
                },
            },
            native::Configuration {
                options,
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
        ];
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("http-replace-range")
            .join(label);
        std::fs::create_dir_all(&dir).unwrap();
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &matrix).unwrap();
        for (name, result) in native::differential_in(
            &validated,
            &program.entry,
            &expected,
            &matrix,
            &tools,
            &dir,
            &native::Fixture::default(),
        ) {
            assert_eq!(
                result,
                native::Outcome::Agree,
                "{label}: {name}: {result:?}"
            );
        }
    });
}

#[test]
fn ranges_aliases_and_source_representations() {
    let mut body = String::new();
    for indirect in [false, true] {
        let op = if indirect {
            "replace"
        } else {
            "bytes.replace_range"
        };
        if indirect {
            body.push_str("let replace = bytes.replace_range\n");
        }
        body.push_str(&format!(r#"{{
let mut b: List[u8] = [0u8,1u8,2u8,3u8,4u8]
list.reserve(mut b, 128)
{op}(mut b, 1, b"\x09\x08")
test.check(t, list.len(b) == 5 && b[0] == 0u8 && b[1] == 9u8 && b[2] == 8u8 && b[3] == 3u8 && b[4] == 4u8)
{op}(mut b, 0, b"")
{op}(mut b, 5, b"")
{op}(mut b, 4, b"z")
test.check(t, list.len(b) == 5 && b[4] == 122u8)
let mut empty: List[u8] = []
{op}(mut empty, 0, b"")
}}
"#));
        for (from, to, start, expected) in [
            (0, 3, 10, "Hello, WorHel"),
            (10, 13, 0, "ld!lo, World!"),
            (0, 12, 1, "HHello, World"),
            (0, 13, 0, "Hello, World!"),
            (13, 13, 5, "Hello, World!"),
            (5, 5, 13, "Hello, World!"),
        ] {
            body.push_str(&format!(
                r#"{{
let mut b: List[u8] = []
bytes.append(mut b, b"Hello, World!")
let alias = copy b
let original = bytes.from_list(copy b)
let source = bytes.slice(original, {from}, {to})
{op}(mut b, {start}, source)
test.check(t, bytes.from_list(copy b) == b{expected:?})
test.check(t, original == b"Hello, World!" && bytes.from_list(copy alias) == original)
test.check(t, source == bytes.slice(original, {from}, {to}) && list.len(b) == 13)
}}
"#
            ));
        }
        for (from, to, start, expected) in [
            (1, 5, 0, "\\x01\\x02\\x03\\x04\\x04"),
            (0, 4, 1, "\\x00\\x00\\x01\\x02\\x03"),
        ] {
            body.push_str(
                &format!(
                    r#"{{
let mut b: List[u8] = [0u8,1u8,2u8,3u8,4u8]
let old = bytes.from_list(copy b)
{op}(mut b, {start}, bytes.slice(old, {from}, {to}))
test.check(t, bytes.from_list(copy b) == b"{expected}" && old == b"\x00\x01\x02\x03\x04")
}}
"#
                )
                .replace("\\\\x", "\\x"),
            );
        }
        body.push_str(&format!(r#"{{
let mut record = ReplaceBuffer(buffer: [1u8,2u8,3u8,4u8,5u8])
let other = copy record
let text = "abcd"
{op}(mut record.buffer, 1, string.as_bytes(text))
test.check(t, record.buffer[1] == 97u8 && record.buffer[4] == 100u8 && other.buffer[1] == 2u8 && text == "abcd")
let mut independent: List[u8] = []
bytes.append(mut independent, b"flat")
let frozen = bytes.from_list(take independent)
let packed = PackedBytes(data: b"flat")
{op}(mut record.buffer, 0, packed.data)
test.check(t, packed.data == b"flat")
{op}(mut record.buffer, 0, frozen)
test.check(t, record.buffer[0] == 102u8 && frozen == b"flat")
}}
"#));
    }
    parity("ranges", &body, None);
}

#[test]
fn invalid_initialized_ranges_and_source_precedence() {
    for indirect in [false, true] {
        let op = if indirect {
            "replace"
        } else {
            "bytes.replace_range"
        };
        for (index, (start, data)) in [
            (6, "b\"\""),
            (6, "b\"x\""),
            (u64::MAX, "b\"\""),
            (u64::MAX, "b\"x\""),
            (4, "b\"xx\""),
            (5, "b\"x\""),
            (0, "b\"123456\""),
        ]
        .into_iter()
        .enumerate()
        {
            parity(
                &format!("bounds-{indirect}-{index}"),
                &format!(
                    "let replace = bytes.replace_range\nlet mut b: List[u8] = [0u8,1u8,2u8,3u8,4u8]\nlist.reserve(mut b, 128)\nlet alias = copy b\n{op}(mut b, {start}, {data})\ntest.check(t, list.len(alias) == 5)"
                ),
                Some("MEM-14"),
            );
        }
        for (index, (start, end)) in [(10, 14), (2, 1), (u64::MAX, u64::MAX)]
            .into_iter()
            .enumerate()
        {
            parity(
                &format!("source-{indirect}-{index}"),
                &format!(
                    "let replace = bytes.replace_range\nlet mut b: List[u8] = [0u8]\n{op}(mut b, 99, bytes.slice(b\"Hello, World!\", {start}, {end}))"
                ),
                Some("SEM-3"),
            );
        }
    }
}

#[test]
fn bounded_arena_charges_only_nonempty_shared_initialized_copies() {
    let mut body = String::new();
    for indirect in [false, true] {
        for (cap, shared, start, data, expected) in [
            (5, false, 1, "xy", "Ok(5u64)"),
            (10, true, 1, "xy", "Ok(5u64)"),
            (9, true, 1, "xy", "Error(OutOfSpace())"),
            (5, true, 5, "", "Ok(5u64)"),
            (10, true, 0, "abcde", "Ok(5u64)"),
            (9, true, 0, "abcde", "Error(OutOfSpace())"),
        ] {
            let op = if indirect {
                "replace"
            } else {
                "bytes.replace_range"
            };
            let share = if shared { "let old = copy b" } else { "" };
            let check = if shared {
                "failure.check(old == [97u8,98u8,99u8,100u8,101u8], TextInvariant())"
            } else {
                ""
            };
            body.push_str(&format!(
                r#"{{
let outcome = arena.bounded({cap}, fn(a: Arena) -> u64 {{
  let mut b: List[u8] in a = [97u8,98u8,99u8,100u8,101u8]
  {share}
  let replace = bytes.replace_range
  {op}(mut b, {start}, b{data:?})
  {check}
  list.len(b)
}})
test.check(t, outcome == {expected})
}}
"#
            ));
        }
    }
    parity("arena", &body, None);
    parity(
        "arena-bounds-first",
        r#"
let outcome = arena.bounded(5, fn(a: Arena) -> u64 {
  let mut b: List[u8] in a = [97u8,98u8,99u8,100u8,101u8]
  let old = copy b
  bytes.replace_range(mut b, 6, b"x")
  list.len(old)
})
"#,
        Some("MEM-14"),
    );
}

#[test]
fn failing_replacements_close_resources_once_and_shield_the_first_cause() {
    for crash in [false, true] {
        for (index, (call, rule)) in [
            ("bytes.replace_range(mut b, 6, owned)", "MEM-14"),
            ("bytes.replace_range(mut b, 4, owned)", "MEM-14"),
            (
                "bytes.replace_range(mut b, 0, bytes.slice(owned, 2, 1))",
                "SEM-3",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let body = format!(
                r#"
let guard = ReplaceTrace(stdout: stdout, crash: {crash})
let mut b: List[u8] = [97u8,98u8,99u8,100u8,101u8]
let alias = copy b
let owned = bytes.from_list(copy b)
{call}
failure.check(owned == bytes.from_list(copy alias), TextInvariant())
"#
            );
            parity(&format!("cleanup-{crash}-{index}"), &body, Some(rule));
        }
    }
    parity(
        "cleanup-arena",
        r#"
let outcome = arena.bounded(9, fn(a: Arena) -> u64 {
  let guard = ReplaceTrace(stdout: stdout, crash: true)
  let mut b: List[u8] in a = [97u8,98u8,99u8,100u8,101u8]
  let alias = copy b
  bytes.replace_range(mut b, 0, b"abcde")
  list.len(alias)
})
"#,
        Some("MEM-6"),
    );
}

fn details(e: &ToolError) -> String {
    match e {
        ToolError::Query {
            error: hirsel_types::QueryError::Diagnostics(d),
            sources,
            ..
        } => d
            .iter()
            .map(|d| d.render_human(sources))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => e.to_string(),
    }
}
// Owned transport adapter: existing net.write_all remains a complete-call API.
// Below that boundary the scripted peer accepts 1 byte, 2 bytes, then remainder.
struct ShortWrites {
    library: hirsel_interp::library::Library,
    accepted: Vec<u8>,
    chunks: Vec<usize>,
    timeout_failure: bool,
}
impl hirsel_interp::OperationsAdapter for ShortWrites {
    fn operations(&self) -> &dyn hirsel_interp::Operations {
        &self.library
    }
    fn operations_mut(&mut self) -> &mut dyn hirsel_interp::Operations {
        &mut self.library
    }
    fn call_resolved(
        &mut self,
        operation: hirsel_interp::OperationId,
        call: hirsel_interp::OperationCall<'_>,
        context: &mut hirsel_interp::OperationContext<'_>,
    ) -> Result<Option<hirsel_interp::OperationResult>, hirsel_interp::Error> {
        use hirsel_interp::{OperationResult, Operations, Value};
        if call.path.is_core("net.set_write_timeout") && self.timeout_failure {
            // Inject an existing provider error at timeout installation. The
            // original runtime argument remains live and unchanged.
            let mut arguments = call.arguments.to_vec();
            arguments[2] = Value::Variant {
                variant: 1,
                fields: vec![Some(hirsel_interp::library::u64_value(0))],
            };
            return Operations::call_resolved(
                &mut self.library,
                operation,
                hirsel_interp::OperationCall {
                    arguments: &mut arguments,
                    ..call
                },
                context,
            );
        }
        if !call.path.is_core("net.write_all") {
            return Operations::call_resolved(&mut self.library, operation, call, context);
        }
        let Value::Bytes(data) = &call.arguments[2] else {
            panic!("write bytes")
        };
        let data = data.clone();
        let mut offset = 0;
        let mut result = None;
        for limit in [1, 2, usize::MAX] {
            if offset == data.len() {
                break;
            }
            let count = limit.min(data.len() - offset);
            let mut arguments = call.arguments.to_vec();
            arguments[2] = Value::Bytes(data[offset..offset + count].to_vec());
            result = Operations::call_resolved(
                &mut self.library,
                operation,
                hirsel_interp::OperationCall {
                    runtime: call.runtime,
                    path: call.path,
                    generics: call.generics,
                    arguments: &mut arguments,
                    result_type: call.result_type,
                    types: call.types,
                    site: call.site,
                },
                context,
            )?;
            if !matches!(
                &result,
                Some(OperationResult::Complete {
                    value: Value::Variant { variant: 0, .. },
                    ..
                })
            ) {
                return Ok(result);
            }
            self.accepted
                .extend_from_slice(&data[offset..offset + count]);
            self.chunks.push(count);
            offset += count;
        }
        Ok(result)
    }
}

#[test]
fn http_short_writes_keep_the_source_until_completion_or_error() {
    with_compiler_stack(|| {
        for size in [3, 65537] {
            for mode in ["success", "before", "partial", "timeout"] {
                let payload = "x".repeat(size);
                let expected = format!(
                    "HTTP/1.1 200 \r\nContent-Length: {size}\r\nConnection: keep-alive\r\n\r\n{payload}"
                );
                let steps = match mode {
                    "success" => format!("Expect(b{expected:?}), Close()"),
                    "partial" => format!("Expect(b{:?}), Reset()", &expected[..3]),
                    _ => "Reset()".into(),
                };
                let outcome = if mode == "success" {
                    "test.check(t, outcome == Ok(true))"
                } else {
                    "test.check(t, match outcome { Error(Net(_)) => true, _ => false })"
                };
                let source = format!(
                    r#"
fn network(e: NetError) -> http.HttpError {{ Net(e) }}
pub fn main(t: Test) -> Result[(), http.HttpError] {{
  let payload=b{payload:?}
  let n=test.net(t,[NetPeer(address:"example.com",port:80u16,resolves_to:[],tls:Plain(),connections:[Open([
    Send(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n"), {steps}
  ])])],[])
  let stream=net.connect(n,"example.com",80u16,Unbounded()) ? network
  let mut c=http.server_connection(test.tasks(t),n,take stream,http.default_config())
  let outcome=http.serve_next(mut c,None(),fn(r:http.ServerRequestView) -> Result[http.ServerResponse,http.HttpError] uses {{}} {{
    Ok(http.ServerResponse(status:200u16,headers:[],body:unique payload,close:false))
  }})
  {outcome}
  test.check(t,bytes.len(payload)=={size})
  Ok(())
}}
"#
                );
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/std_source/main.hir");
                let mut program = load_program_source(&path, Some(&source))
                    .unwrap_or_else(|e| panic!("{}", details(&e)));
                program
                    .check_for_execution()
                    .unwrap_or_else(|e| panic!("{}", details(&e)));
                let mut reference = None;
                let mut tape = vec![];
                for (checked, replay) in [(false, false), (true, false), (false, true)] {
                    let config = RunConfiguration {
                        entry: program.entry.clone(),
                        hash_seed: 0,
                        inputs: if replay { tape.clone() } else { vec![] },
                    };
                    let signature = hirsel_types::SignatureProvider::signature(
                        &mut program.engine,
                        (&config.entry).into(),
                    )
                    .unwrap();
                    let runtime = if checked {
                        program.engine.checked_runtime_types()
                    } else {
                        program.engine.runtime_types()
                    }
                    .unwrap();
                    let sources = program.engine.sources.clone();
                    let mut library =
                        hirsel_interp::library::Library::new(&config, SimulationConfig::default())
                            .unwrap();
                    let arguments = library
                        .entry_arguments(program.engine.types(), &signature)
                        .unwrap();
                    let mut operations = ShortWrites {
                        library,
                        accepted: vec![],
                        chunks: vec![],
                        timeout_failure: mode == "timeout",
                    };
                    let execution = hirsel_interp::run_with_runtime(
                        &mut program.engine,
                        &sources,
                        &config,
                        arguments,
                        hirsel_interp::Limits::default(),
                        &mut operations,
                        &runtime,
                    )
                    .unwrap();
                    operations.library.simulation.finish_replay().unwrap();
                    let observed = execution.observable(&sources).unwrap();
                    assert!(observed.failures.is_empty(), "{size}/{mode}: {observed:?}");
                    assert_eq!(observed.exit_code, Some(0));
                    let accepted = if mode == "success" {
                        expected.as_bytes()
                    } else if mode == "partial" {
                        &expected.as_bytes()[..3]
                    } else {
                        &[]
                    };
                    assert_eq!(operations.accepted, accepted, "{size}/{mode}");
                    if !accepted.is_empty() {
                        assert_eq!(&operations.chunks[..2], &[1, 2]);
                    }
                    let closes = operations
                        .library
                        .simulation
                        .events
                        .iter()
                        .filter(|e| e.operation == "net.close")
                        .count();
                    assert_eq!(closes, 1, "{size}/{mode}: close once");
                    if let Some(reference) = &reference {
                        assert_eq!(&observed, reference);
                    } else {
                        tape = operations.library.simulation.events.clone();
                        reference = Some(observed);
                    }
                }
            }
        }
    });
}
