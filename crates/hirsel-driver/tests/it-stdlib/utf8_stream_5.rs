//! Streaming UTF-8 lane: Rust validation/error spans, owned output and bounded state.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/utf8-stream-5-{name}.hir"))
}

fn interpret(path: &Path, steps: u64) {
    let outcome = run_path_with_step_budget(
        path,
        SimulationConfig::default(),
        Some(steps),
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
}

#[test]
fn controls() {
    with_compiler_stack(|| interpret(&fixture("controls"), 1_000_000));
}

#[test]
fn vectors() {
    with_compiler_stack(|| interpret(&fixture("vectors"), 20_000_000));
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
fn llvm() {
    with_compiler_stack(|| {
        for name in ["controls", "vectors"] {
            native_fixture(&fixture(name), native::Backend::Llvm);
        }
        native_fixture(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/SEM/SEM-3-utf8-stream-5-run.hir"),
            native::Backend::Llvm,
        );
    });
}

#[test]
fn cranelift() {
    with_compiler_stack(|| {
        for name in ["controls", "vectors"] {
            native_fixture(&fixture(name), native::Backend::Cranelift);
        }
        native_fixture(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/SEM/SEM-3-utf8-stream-5-run.hir"),
            native::Backend::Cranelift,
        );
    });
}

#[test]
fn original_chunk_loss_reproduces() {
    // The original whole-buffer API intentionally remains unchanged.
    with_compiler_stack(|| {
        let source = r#"pub fn main() -> Result[(), ()] {
          let a = std@1/utf8.decode_lossy(b"\xe2")
          let b = std@1/utf8.decode_lossy(b"\x82\xac")
          let mut out: List[u8] = []
          std@1/text.append(mut out, a)
          std@1/text.append(mut out, b)
          let decoded = std@1/text.finish_valid(take out)
          failure.check(decoded == "���", TextInvariant())
          Ok(())
        }"#;
        execute_source(source, 1_000_000);
        let failed = source.replace("decoded == \"���\"", "decoded == \"€\"");
        let outcome = source_outcome(&failed, 1_000_000);
        assert!(matches!(outcome, RunOutcome::Failed { .. }), "{outcome:?}");
        eprintln!(
            "Original per-chunk decode_lossy: expected euro, got three replacements; failed observation: {outcome:?}"
        );
    });
}

fn execute_source(source: &str, steps: u64) {
    let outcome = source_outcome(source, steps);
    assert!(
        matches!(
            outcome,
            RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
        ),
        "{outcome:?}"
    );
}

fn source_outcome(source: &str, steps: u64) -> RunOutcome {
    let mut loaded = load_program_source(&fixture("vectors"), Some(source)).unwrap();
    loaded
        .check_for_execution()
        .unwrap_or_else(|e| panic!("{e}"));
    loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
        limits: hirsel_interp::Limits {
            steps: Some(steps),
            call_depth: 256,
        },
        ..Default::default()
    }));
    let observed = run_observed_snapshot(
        &mut loaded.engine,
        &RunConfiguration {
            entry: loaded.entry,
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
    )
    .unwrap();
    observed.outcome.unwrap_or_else(|e| panic!("{e:?}"))
}

fn oracle_row(bytes: &[u8]) -> String {
    let literal: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
    let expected: String = String::from_utf8_lossy(bytes)
        .chars()
        .map(|c| format!("\\u{{{:x}}}", u32::from(c)))
        .collect();
    let error = match std::str::from_utf8(bytes) {
        Ok(_) => "None()".into(),
        Err(error) => format!(
            "Some({}({}))",
            if error.error_len().is_some() {
                "InvalidSequence"
            } else {
                "IncompleteSequence"
            },
            error.valid_up_to()
        ),
    };
    format!("check(b\"{literal}\", \"{expected}\", {error})\n")
}

struct GeneratedProgram(PathBuf);
impl GeneratedProgram {
    fn new(name: &str, source: &str) -> Self {
        let dir_scratch = tempfile::Builder::new()
            .prefix(&format!("utf8-stream-5-{name}-"))
            .tempdir_in(Path::new(env!("CARGO_TARGET_TMPDIR")))
            .unwrap();
        let dir = dir_scratch.path().to_path_buf();
        let path = dir.join("main.hir");
        std::fs::write(&path, source).unwrap();
        let std_manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = serde_json::json!({
            "schema": 1,
            "package": { "name": "standalone", "version": "0" },
            "entry": { "module": [], "name": "main" },
            "sources": [{ "path": "main.hir", "module": [] }],
            "dependencies": [{ "manifest": std_manifest,
                "package": { "name": "std", "version": "1" }, "expose": [] }]
        });
        std::fs::write(
            dir.join("main.hir.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let _ = dir_scratch.keep();
        Self(path)
    }
}
impl Drop for GeneratedProgram {
    fn drop(&mut self) {
        std::fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
    }
}

#[test]
fn go_vectors_and_seeded_random_rust_oracle() {
    with_compiler_stack(|| {
        let vector_source = std::fs::read_to_string(fixture("vectors")).unwrap();
        let helpers = vector_source.split("pub fn main()").next().unwrap();
        let mut source = format!("{helpers}\npub fn main() -> Result[(), ()] {{\n");
        let table = fixture("go-vectors").with_extension("tsv");
        let table = std::fs::read_to_string(table).unwrap();
        let mut rows = 0;
        let mut prefixes = 0;
        for line in table.lines().filter(|line| !line.starts_with('#')) {
            let (scalar, encoded) = line.split_once('\t').unwrap();
            let bytes: Vec<_> = encoded
                .split("\\x")
                .skip(1)
                .map(|b| u8::from_str_radix(b, 16).unwrap())
                .collect();
            assert!(!bytes.is_empty());
            if scalar != "invalid" {
                let cp = u32::from_str_radix(scalar.strip_prefix("0x").unwrap(), 16).unwrap();
                if let Some(ch) = char::from_u32(cp) {
                    assert_eq!(bytes, ch.to_string().as_bytes());
                } else {
                    assert!(std::str::from_utf8(&bytes).is_err());
                }
            }
            // Every EOF prefix and every byte split of each Go table row.
            for end in 0..=bytes.len() {
                source += &oracle_row(&bytes[..end]);
                prefixes += 1;
            }
            rows += 1;
        }
        assert_eq!(rows, 71);
        let mut seed = 0x7538_7374_7265_616du64;
        for index in 0..128 {
            let mut bytes = Vec::new();
            for _ in 0..index % 9 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                bytes.push((seed >> 24) as u8);
            }
            source += &oracle_row(&bytes);
        }
        source += "Ok(())\n}\n";
        execute_source(&source, 60_000_000);
        let program = GeneratedProgram::new("oracle", &source);
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            native_fixture(&program.0, backend);
        }
        eprintln!(
            "{rows} Go rows / {prefixes} EOF prefixes + 128 seeded Rust-oracle vectors on interpreter, LLVM and Cranelift"
        );
    });
}

pub(super) fn source_origin() {
    with_compiler_stack(|| {
        let mut loaded = load_program(&fixture("controls")).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        for name in ["decoder", "feed", "offset", "status", "pending_bytes"] {
            let unit = ready
                .units
                .iter()
                .find(|u| {
                    let d = &u.unit().header.definition.0;
                    d.package.name_is("std") && d.modules == ["utf8"] && d.name == name
                })
                .unwrap_or_else(|| panic!("missing source utf8.{name}"));
            assert_eq!(
                unit.unit().header.origin,
                hirsel_lir::ir::UnitOrigin::Source
            );
        }
        eprintln!(
            "compiler={:?} llvm={}",
            hirsel_driver::COMPILER_IDENTITY,
            native::llvm_version()
        );
    });
}

fn ready_source(source: &str) -> (LoadedProgram, NativeInput, native::ReadyProgram) {
    let mut loaded = load_program_source(&fixture("controls"), Some(source)).unwrap();
    loaded
        .check_for_execution()
        .unwrap_or_else(|e| panic!("{e}"));
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
fn heap_ownership_and_injected_cleanup() {
    with_compiler_stack(|| {
        let source = std::fs::read_to_string(fixture("controls")).unwrap();
        let (loaded, input, ready) = ready_source(&source);
        let run =
            native::heap_campaign(&ready, &loaded.entry, &native::Sites::new(&input)).unwrap();
        assert_eq!(run.observation.exit_code, Some(0));
        assert!(run.sites > 0);
        eprintln!(
            "H controls: {} injected failure sites, {:?}",
            run.sites, run.counters
        );
    });
}

#[test]
fn state_memory_is_bounded_for_long_one_byte_input() {
    with_compiler_stack(|| {
        let mut peaks = Vec::new();
        for iterations in [64, 256, 1024] {
            let source = format!(
                r#"
            pub fn main() -> Result[(), ()] {{
              let mut d = std@1/utf8.decoder(Strict())
              for i in 0u64..{iterations}u64 {{
                failure.check(std@1/utf8.feed(mut d, b"\xf0", false) == Ok(""), TextInvariant())
                failure.check(std@1/utf8.feed(mut d, b"\x90", false) == Ok(""), TextInvariant())
                failure.check(std@1/utf8.feed(mut d, b"\x80", false) == Ok(""), TextInvariant())
                failure.check(std@1/utf8.pending_bytes(d) == 3, TextInvariant())
                failure.check(std@1/utf8.feed(mut d, b"\x80", false) == Ok("𐀀"), TextInvariant())
                failure.check(std@1/utf8.pending_bytes(d) == 0, TextInvariant())
              }}
              failure.check(std@1/utf8.offset(d) == {iterations}u64 * 4u64, TextInvariant())
              failure.check(std@1/utf8.feed(mut d, b"", true) == Ok(""), TextInvariant())
              Ok(())
            }}"#
            );
            execute_source(&source, 10_000_000);
            if iterations == 1024 {
                // Keep 4096 interpreter feeds. H's instruction ledger is much
                // denser; 256/1024 feeds compare peak storage within its limit.
                eprintln!("Interpreter: 4096 one-byte feeds passed");
                continue;
            }
            let (loaded, input, ready) = ready_source(&source);
            let run = native::run_heap(&ready, &loaded.entry, &native::Sites::new(&input)).unwrap();
            assert_eq!(run.observation.exit_code, Some(0));
            peaks.push(run.counters.peak_live_heap_bytes);
            eprintln!(
                "H {iterations} scalars / {} one-byte feeds: {:?}",
                iterations * 4,
                run.counters
            );
        }
        assert_eq!(
            peaks[0], peaks[1],
            "state must retain no earlier input or output"
        );
    });
}

#[test]
fn offset_overflow_poisoning_at_unreachable_stream_size() {
    with_compiler_stack(|| {
        // Reuse the exact implementation as the primary module so this harness
        // can seed its private offset without adding a public state-forging API.
        let implementation = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/utf8.hir"),
        )
        .unwrap()
        .replace("text.finish", "std@1/text.finish");
        let source = format!(
            r#"{implementation}
        pub fn main() -> Result[(), ()] {{
          let mut d = decoder(Strict())
          d.offset = 18446744073709551614u64
          failure.check(feed(mut d, b"ab", false) == Error(OffsetOverflow()), TextInvariant())
          failure.check(status(d) == Failed(), TextInvariant())
          failure.check(offset(d) == 18446744073709551614u64, TextInvariant())
          failure.check(feed(mut d, b"", true) == Error(DecoderFailed()), TextInvariant())
          let mut edge = decoder(Strict())
          edge.offset = 18446744073709551614u64
          failure.check(feed(mut edge, b"a", false) == Ok("a"), TextInvariant())
          failure.check(offset(edge) == 18446744073709551615u64, TextInvariant())
          failure.check(feed(mut edge, b"", true) == Ok(""), TextInvariant())
          let mut pending = decoder(Lossy())
          pending.offset = 18446744073709551614u64
          failure.check(feed(mut pending, b"\xf0", false) == Ok(""), TextInvariant())
          failure.check(feed(mut pending, b"\x90", false) == Error(OffsetOverflow()), TextInvariant())
          failure.check(pending_bytes(pending) == 0, TextInvariant())
          Ok(())
        }}"#
        );
        execute_source(&source, 1_000_000);
        let (loaded, input, ready) = ready_source(&source);
        let run =
            native::heap_campaign(&ready, &loaded.entry, &native::Sites::new(&input)).unwrap();
        assert_eq!(run.observation.exit_code, Some(0));
    });
}

#[test]
fn existing_utf8_text_controls_both_backends() {
    with_compiler_stack(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/SEM");
        for name in [
            "SEM-3-utf8-run.hir",
            "SEM-3-string-common-run.hir",
            "SEM-3-sse-run.hir",
            "SEM-1-format-into-appends-run.hir",
        ] {
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                native_fixture(&root.join(name), backend);
            }
        }
    });
}
