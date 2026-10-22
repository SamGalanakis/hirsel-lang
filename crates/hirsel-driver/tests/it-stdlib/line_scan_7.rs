//! Feed-oriented binary lines: Go vectors, independent oracle and ownership.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::{Path, PathBuf};

fn source_error(error: ToolError) -> ! {
    if let ToolError::Query {
        error: hirsel_types::QueryError::Diagnostics(diagnostics),
        sources,
        ..
    } = &error
    {
        for diagnostic in diagnostics {
            eprintln!("{}", diagnostic.render_human(sources));
        }
    }
    panic!("{error}")
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn fixture(name: &str) -> PathBuf {
    root().join(format!("conformance/SEM/SEM-3-line-scan-7-{name}-run.hir"))
}
fn interpret(path: &Path) {
    let outcome = run_path_with_step_budget(
        path,
        SimulationConfig::default(),
        Some(1_000_000),
        Verify::On,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap_or_else(|error| source_error(error));
    assert!(
        matches!(
            outcome,
            RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
        ),
        "{outcome:?}"
    );
}
fn native_fixture(path: &Path, backend: native::Backend) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let outcome = native::run_path_native_backend_with_validation(
        path,
        HostLaunch::empty(),
        None,
        backend,
        native::Output::Streams {
            stdout: &mut stdout,
            stderr: &mut stderr,
        },
        Verify::On,
        native::Validation::EveryStage,
    )
    .unwrap()
    .unwrap();
    assert!(
        matches!(
            outcome,
            RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
        ),
        "{path:?} {backend:?}: {outcome:?}; {}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(stdout.is_empty());
}
#[test]
fn controls_and_vectors() {
    with_compiler_stack(|| {
        for name in ["controls", "vectors"] {
            interpret(&fixture(name));
        }
    });
}
#[test]
fn llvm() {
    with_compiler_stack(|| {
        for name in ["controls", "vectors"] {
            native_fixture(&fixture(name), native::Backend::Llvm);
        }
    });
}
#[test]
fn cranelift() {
    with_compiler_stack(|| {
        for name in ["controls", "vectors"] {
            native_fixture(&fixture(name), native::Backend::Cranelift);
        }
    });
}

fn ready(source: &str) -> (LoadedProgram, NativeInput, native::ReadyProgram) {
    let mut loaded = load_program_source(&fixture("controls"), Some(source)).unwrap();
    loaded
        .check_for_execution()
        .unwrap_or_else(|error| source_error(error));
    let input = loaded.engine.export_native_input().unwrap();
    let validated = loaded.engine.import_native_input(input.clone()).unwrap();
    let ready = native::build(
        &validated,
        &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
    )
    .unwrap();
    (loaded, input, ready)
}
#[test]
fn heap_ownership_and_failure_cleanup() {
    with_compiler_stack(|| {
        let source = std::fs::read_to_string(fixture("controls")).unwrap();
        let (loaded, input, ready) = ready(&source);
        for name in ["new", "feed", "buffered", "offset", "status"] {
            let unit = ready
                .units
                .iter()
                .find(|u| {
                    let d = &u.unit().header.definition.0;
                    d.package.name_is("std") && d.modules == ["line_scan"] && d.name == name
                })
                .unwrap_or_else(|| panic!("missing source line_scan.{name}"));
            assert_eq!(
                unit.unit().header.origin,
                hirsel_lir::ir::UnitOrigin::Source
            );
        }
        let result =
            native::heap_campaign(&ready, &loaded.entry, &native::Sites::new(&input)).unwrap();
        assert_eq!(result.observation.exit_code, Some(0));
        assert!(result.sites > 0);
        eprintln!(
            "line-scan H: {} injected sites; {:?}",
            result.sites, result.counters
        );
    });
}
#[test]
fn one_byte_feeds_read_linear_bytes() {
    with_compiler_stack(|| {
        for n in [64, 128, 256] {
            let source = format!(
                r#"
            pub fn main() -> Result[(), std@1/line_scan.Error] {{
              let mut s = std@1/line_scan.new({n}, false)?
              for i in 0u64..{n}u64 {{
                failure.check(std@1/line_scan.feed(mut s, b"x", false)? == [], Bounds())
                failure.check(std@1/line_scan.buffered(s) == i + 1, Bounds())
              }}
              let lines = std@1/line_scan.feed(mut s, b"", true)?
              failure.check(list.len(lines) == 1 && bytes.len(lines[0]) == {n}, Bounds())
              Ok(())
            }}"#
            );
            let (loaded, input, ready) = ready(&source);
            let result =
                native::run_heap(&ready, &loaded.entry, &native::Sites::new(&input)).unwrap();
            assert_eq!(result.observation.exit_code, Some(0));
            assert!(
                result.counters.indexed_byte_reads <= 3 * n + 8,
                "{n}: {:?}",
                result.counters
            );
            assert_eq!(
                result.counters.dups, 0,
                "carry must move without COW prefix copies"
            );
            eprintln!("{n} one-byte feeds: {:?}", result.counters);
        }
    });
}

fn literal(data: &[u8]) -> String {
    format!(
        "b\"{}\"",
        data.iter()
            .map(|b| format!("\\x{b:02x}"))
            .collect::<String>()
    )
}
// Delimiter positions over a complete input, with no incremental scanner state.
fn oracle(data: &[u8], max: usize, strip: bool) -> String {
    let mut start = 0;
    let mut lines = Vec::new();
    for end in (0..data.len())
        .filter(|&i| data[i] == b'\n')
        .chain(std::iter::once(data.len()))
    {
        if end - start > max {
            return format!("Error(LineTooLong({}))", start + max);
        }
        if end < data.len() || start < end {
            let mut line = &data[start..end];
            if strip && line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            lines.push(literal(line));
        }
        start = end + 1;
    }
    format!("Ok([{}])", lines.join(","))
}
#[test]
fn rust_oracle_all_partitions_and_raw_limits() {
    with_compiler_stack(|| {
        let path = root().join("crates/hirsel-driver/tests/fixtures/line-scan-7/exhaustive.hir");
        let template = std::fs::read_to_string(&path).unwrap();
        let mut source = template.split("pub fn main()").next().unwrap().to_owned();
        source += "pub fn main() -> Result[(), line_scan.Error] {\n";
        let mut rows = 0;
        for data in [
            b"".as_slice(),
            b"\n",
            b"\n\n",
            b"\r",
            b"\r\n",
            b"a\nb",
            b"a\r\n",
            b"a\rb",
            b"\r\r\n",
            b"\x00\xff\n",
            b"ab\n\r",
            b"x\n\r\ny",
        ] {
            for strip in [true, false] {
                let expected = oracle(data, 64, strip);
                let lines = expected
                    .strip_prefix("Ok(")
                    .unwrap()
                    .strip_suffix(')')
                    .unwrap();
                source += &format!("check({}, {lines}, {strip})?\n", literal(data));
                for max in [1, 2, 3, 4] {
                    source += &format!(
                        "limits({}, {max}, {}, {strip})?\n",
                        literal(data),
                        oracle(data, max, strip)
                    );
                    rows += 1;
                }
            }
        }
        // Go scan_test.go:267-308 final tails, CR and terminal blank vectors.
        for data in [
            b"abcdefghijklmn\nopqrstuvwxyz".as_slice(),
            b"abcdefghijklmn\nopqrstuvwxyz\r",
            b"abcdefghijklmn\nopqrstuvwxyz\n\n",
            b"abcdefghijklmn\nopqrstuvwxyz\n\r",
        ] {
            source += &format!(
                "limits({}, 64, {}, true)?\n",
                literal(data),
                oracle(data, 64, true)
            );
            rows += 1;
        }
        // Go scan_test.go:175-212 increasing/decreasing one-byte-read lines.
        let mut data = Vec::new();
        for n in (0..=16).chain((0..16).rev()) {
            data.extend(std::iter::repeat_n(b'x', n));
            data.push(b'\n');
        }
        source += &format!(
            "limits({}, 16, {}, false)?\n",
            literal(&data),
            oracle(&data, 16, false)
        );
        source += "Ok(())\n}\n";
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("line-scan-7-oracle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let generated = dir.join("main.hir");
        std::fs::write(&generated, &source).unwrap();
        let manifest = serde_json::json!({"schema":1,"package":{"name":"line_scan_oracle","version":"0"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":root().join("stdlib/std@1/hirsel.json").canonicalize().unwrap(),"package":{"name":"std","version":"1"},"expose":[]}]});
        std::fs::write(
            dir.join("main.hir.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let outcome = run_path_with_step_budget(
            &generated,
            SimulationConfig::default(),
            Some(60_000_000),
            Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{outcome:?}"
        );
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            native_fixture(&generated, backend);
        }
        std::fs::remove_dir_all(dir).unwrap();
        eprintln!(
            "{rows} raw-limit oracle rows + 24 exhaustive partition rows + 33 rising/falling lines, interpreter/LLVM/Cranelift"
        );
    });
}

#[test]
fn ndjson_example_fixed_stdin_and_errors() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    with_compiler_stack(|| {
        let source = include_str!("../../../../examples/ndjson-lines/main.hir");
        // Compile the example's actual bytes under the lane fixture's std graph.
        let (loaded, _, ready) = ready(source);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("line-scan-7-ndjson-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let tools = native::Toolchain::in_tree_with_runtime(
            &dir.join("runtime"),
            &[&target],
            native::RuntimeBuild::default(),
        )
        .unwrap();
        let cases: &[(&[u8], &[u8], Option<&str>)] = &[
            (
                b"\n{\"x\":1}\r\n\r\n[true, null]\n42",
                b"{\"x\":1}\n[true,null]\n42\n",
                None,
            ),
            (
                b"true\n\n\xff\nfalse\n",
                b"true\n",
                Some("line 3: invalid UTF-8\n"),
            ),
            (
                b"true\n\n{bad}\nfalse\n",
                b"true\n",
                Some("line 3: invalid JSON\n"),
            ),
            (b" \n", b"", Some("line 1: invalid JSON\n")),
            (b"", b"", None),
        ];
        for (input, expected, error) in cases {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let outcome = run_path_with_simulation_source(source, input, &mut stdout, &mut stderr);
            assert_eq!(&stdout, expected);
            assert_eq!(String::from_utf8_lossy(&stderr), error.unwrap_or(""));
            assert!(
                matches!(outcome, RunOutcome::Returned(returned) if returned == if error.is_some() { hirsel_run_schema::Returned::Error } else { hirsel_run_schema::Returned::Ok })
            );
        }
        let grants = r#"{"io":{"stdin":"stdin.bytes@1","stdout":"stdout.bytes@1","stderr":"stderr.bytes@1","dirs":[]}}"#;
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let objects = native::emit_backend_objects(
                &ready,
                backend,
                native::OptLevel::Speed,
                native::Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let executable =
                native::link_standalone_objects(&ready, &loaded.entry, &objects, &tools, &dir)
                    .unwrap();
            for (input, expected, error) in cases {
                let mut child = Command::new(&executable)
                    .args(["--hirsel-grants", grants])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                child.stdin.take().unwrap().write_all(input).unwrap();
                let output = child.wait_with_output().unwrap();
                assert_eq!(&output.stdout, expected, "{backend:?}: {output:?}");
                assert_eq!(output.status.success(), error.is_none(), "{output:?}");
                assert!(
                    String::from_utf8_lossy(&output.stderr).starts_with(error.unwrap_or("")),
                    "{output:?}"
                );
                if error.is_none() {
                    assert!(output.stderr.is_empty());
                }
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
        eprintln!("NDJSON: 5 fixed stdin cases on interpreter/LLVM/Cranelift with explicit grants");
    });
}
fn run_path_with_simulation_source(
    source: &str,
    input: &[u8],
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) -> RunOutcome {
    let mut loaded = load_program_source(&fixture("controls"), Some(source)).unwrap();
    loaded
        .check_for_execution()
        .unwrap_or_else(|error| source_error(error));
    let observed = run_observed_snapshot(
        &mut loaded.engine,
        &RunConfiguration {
            entry: loaded.entry,
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig {
            stdin: input.to_vec(),
            ..Default::default()
        },
    )
    .unwrap();
    let observation = observed.observation.unwrap();
    stdout.extend(&observation.output);
    stderr.extend(&observation.final_state.unwrap().stderr);
    observed.outcome.unwrap()
}

#[test]
fn absolute_offset_overflow_poisoning() {
    with_compiler_stack(|| {
        // Seed only the private implementation's counter in a test module;
        // production callers get no state-forging API.
        let implementation =
            std::fs::read_to_string(root().join("stdlib/std@1/line_scan.hir")).unwrap();
        let source = format!(
            r#"{implementation}
        pub fn main() -> Result[(), Error] {{
          let mut s = new(4, false)?
          s.offset = 18446744073709551614u64
          failure.check(feed(mut s, b"ab", true) == Error(OffsetOverflow()), Bounds())
          failure.check(status(s) == Poisoned() && buffered(s) == 0, Bounds())
          failure.check(offset(s) == 18446744073709551614u64, Bounds())
          failure.check(feed(mut s, b"", true) == Error(ScannerFailed()), Bounds())
          let mut edge = new(4, true)?
          edge.offset = 18446744073709551614u64
          failure.check(feed(mut edge, b"\r", false)? == [], Bounds())
          failure.check(offset(edge) == 18446744073709551615u64, Bounds())
          failure.check(feed(mut edge, b"", true)? == [b""], Bounds())
          let mut pending = new(4, false)?
          pending.offset = 18446744073709551614u64
          let _ = feed(mut pending, b"x", false)?
          failure.check(feed(mut pending, b"y", false) == Error(OffsetOverflow()), Bounds())
          failure.check(buffered(pending) == 0, Bounds())
          Ok(())
        }}"#
        );
        let (mut loaded, input, ready) = ready(&source);
        let observed = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(matches!(
            observed.outcome,
            Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
        ));
        let result =
            native::heap_campaign(&ready, &loaded.entry, &native::Sites::new(&input)).unwrap();
        assert_eq!(result.observation.exit_code, Some(0));
    });
}

#[test]
fn constructor_lowering_and_independent_validation() {
    use hirsel_types::{GenericArgument, SignatureKind, SignatureProvider, TypeKind};
    with_compiler_stack(|| {
        let implementation =
            std::fs::read_to_string(root().join("stdlib/std@1/line_scan.hir")).unwrap();
        let prefix = implementation
            .split("@noalloc\npub fn status")
            .next()
            .unwrap();
        for (label, source, accepted) in [
            (
                "missing opaque cleanup guarantee",
                prefix.replace("Scanner: inert + noalloc_drop", "Scanner: ()"),
                false,
            ),
            ("declared opaque cleanup guarantee", prefix.to_owned(), true),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            // Obtain the producer artifact for diagnostics, then run independent V
            // explicitly below. This is never an execution with validation disabled.
            engine.verify = Verify::Off;
            let file = engine
                .sources
                .add("line-scan-constructor.hir", source)
                .unwrap();
            let context = default_context();
            engine.parse_file(file, &context).unwrap();
            let mut path = entry_path(&context);
            path.0.name = "new".into();
            let lowered = engine.lower_to_tir(&path).unwrap();
            let types = engine.types().clone();
            for name in ["new", "Scanner", "Status", "Error"] {
                path.0.name = name.into();
                let signature = engine
                    .signature(hirsel_syntax::DefId::intern(&path))
                    .unwrap();
                if name == "new" {
                    let SignatureKind::Function(function) = &signature.kind else {
                        panic!()
                    };
                    let result = types.get(function.result).unwrap();
                    let TypeKind::Named { arguments, .. } = &result.kind else {
                        panic!("{result:?}")
                    };
                    let GenericArgument::Type(expected) = arguments[0] else {
                        panic!()
                    };
                    for body in &lowered.value.bodies {
                        for rvalue in &body.rvalues {
                            if let hirsel_tir::Rvalue::Construct {
                                constructor:
                                    hirsel_tir::Constructor::Variant {
                                        definition,
                                        variant: 0,
                                    },
                                fields,
                                ..
                            } = rvalue
                                && definition.0.name == "Result"
                            {
                                let actual = body.places[fields[0].value.place.0 as usize].ty;
                                eprintln!(
                                    "{label}: Result.Ok expected {:?}; emitted {:?}",
                                    types.get(expected).unwrap(),
                                    types.get(actual).unwrap()
                                );
                            }
                        }
                    }
                }
            }
            let snapshot = engine.signature_snapshot();
            let report = hirsel_validate::validate(
                &lowered.value,
                &types,
                snapshot.validation(&path.0.package),
                &mut Vec::new(),
            );
            eprintln!("{label}: independent V {:?}", report.violations);
            if accepted {
                assert!(report.violations.is_empty(), "{report:?}");
            } else {
                assert!(
                    report.violations.iter().any(|v| v.kind
                        == hirsel_validate::ViolationKind::TypeMismatch {
                            entity: "constructor field"
                        }),
                    "{report:?}"
                );
            }
        }
    });
}
