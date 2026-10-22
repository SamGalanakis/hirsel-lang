//! Go bytes.ReplaceAll's compatible vectors, plus an independent scalar oracle.
//! Go ebe2feefc400f813aefdc8a036e66d2619da6649, bytes_test.go:1763-1821
//! (BSD-3-Clause; third-party/NOTICES.md). Empty old fails SEM-3 in Hirsel.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::{Path, PathBuf};

struct Program(PathBuf);
impl Program {
    fn new(name: &str, source: &str) -> Self {
        let dir_scratch = tempfile::Builder::new()
            .prefix(&format!("bytes-replace-bulk-6-{name}-"))
            .tempdir_in(Path::new(env!("CARGO_TARGET_TMPDIR")))
            .unwrap();
        let dir = dir_scratch.path().to_path_buf();
        let path = dir.join("main.hir");
        std::fs::write(&path, source).unwrap();
        let manifest = serde_json::json!({
            "schema": 1, "package": {"name": "standalone", "version": "0"},
            "entry": {"module": [], "name": "main"},
            "sources": [{"path": "main.hir", "module": []}],
            "dependencies": [{"manifest": Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../stdlib/std@1/hirsel.json").canonicalize().unwrap(),
                "package": {"name": "std", "version": "1"}, "expose": []}]
        });
        std::fs::write(
            path.with_extension("hir.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let _ = dir_scratch.keep();
        Self(path)
    }
}
impl Drop for Program {
    fn drop(&mut self) {
        std::fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
    }
}

fn scalar(src: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    assert!(!old.is_empty());
    let mut out = Vec::new();
    let mut i = 0;
    while i < src.len() {
        if src[i..].starts_with(old) {
            out.extend_from_slice(new);
            i += old.len();
        } else {
            out.push(src[i]);
            i += 1;
        }
    }
    out
}

fn literal(data: &[u8]) -> String {
    format!(
        "b\"{}\"",
        data.iter()
            .map(|b| format!("\\x{b:02x}"))
            .collect::<String>()
    )
}

fn vectors() -> String {
    let mut source = String::from(
        r#"
fn check(src: Bytes, old: Bytes, new: Bytes, expected: Bytes) {
  let replaced = std@1/bytes.replace(src, old, new)
  failure.check(replaced == expected, TextInvariant())
}
fn independent(old: Bytes, new: Bytes) -> unique Bytes {
  let mut data: List[u8] = []
  bytes.append(mut data, b"banana")
  let src = bytes.from_list(take data)
  let mut old_data: List[u8] = []
  let mut new_data: List[u8] = []
  bytes.append(mut old_data, old)
  bytes.append(mut new_data, new)
  let old_owned = bytes.from_list(take old_data)
  let new_owned = bytes.from_list(take new_data)
  std@1/bytes.replace(src, old_owned, new_owned)
}
pub fn main() -> Result[(), ()] {
  failure.check(independent(b"ana", b"<>") == b"b<>na", TextInvariant())
  failure.check(independent(b"z", b"x") == b"banana", TextInvariant())
  failure.check(independent(b"a", b"a") == b"banana", TextInvariant())
  let src = bytes.slice(b"PREFIXaaaaaTAIL", 6, 11)
  let old = bytes.slice(b"!aaTAIL", 1, 3)
  let new = bytes.slice(b"!xTAIL", 1, 2)
  check(src, old, new, b"xxa")
  failure.check(src == b"aaaaa" && old == b"aa" && new == b"x", TextInvariant())
"#,
    );
    let rows: &[(&[u8], &[u8], &[u8])] = &[
        (b"hello", b"l", b"L"),
        (b"hello", b"x", b"X"),
        (b"", b"x", b"X"),
        (b"radar", b"r", b"<r>"),
        (b"banana", b"a", b"<>"),
        (b"banana", b"an", b"<>"),
        (b"banana", b"ana", b"<>"),
        (b"banana", b"a", b"a"),
        (b"aaaaa", b"aa", b""),
        (b"a", b"aaa", b"x"),
        (b"\xff\0\xff\0\xff", b"\xff\0", b"\x80"),
        (b"ababababababac", b"ababac", b"\0\xff"),
        (b"aaaaabaaaaabaaaaac", b"aaaaab", b"Z"),
        (b"abc", b"abc", b""),
        (b"abc", b"abc", b"xyzxyz"),
    ];
    let mut append = |src: &[u8], old: &[u8], new: &[u8]| {
        source += &format!(
            "check({}, {}, {}, {})\n",
            literal(src),
            literal(old),
            literal(new),
            literal(&scalar(src, old, new))
        );
    };
    for &(src, old, new) in rows {
        append(src, old, new);
    }
    let mut seed = 0x6279_7465_7336u64;
    for size in 0..64 {
        let mut draw = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 24) as u8
        };
        let src: Vec<u8> = (0..size).map(|_| draw() % 4).collect();
        let old: Vec<u8> = (0..1 + size % 7).map(|_| draw() % 4).collect();
        let new: Vec<u8> = (0..size % 9).map(|_| draw()).collect();
        append(&src, &old, &new);
    }
    source += "Ok(())\n}\n";
    source
}

fn interpret(path: &Path) -> RunOutcome {
    run_path_with_step_budget(
        path,
        SimulationConfig::default(),
        Some(10_000_000),
        Verify::On,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap_or_else(|error| panic!("{error}"))
}
fn native(path: &Path, backend: native::Backend) -> RunOutcome {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let outcome = native::run_path_native_backend_with_validation(
        path,
        HostLaunch::Live {
            config: LaunchConfig::Grants {
                input: std::ffi::OsStr::new("{}"),
                arguments: None,
            },
            record: None,
            revocations: None,
        },
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
    assert!(stdout.is_empty(), "{stdout:?}");
    outcome
}
fn ok(outcome: RunOutcome) {
    assert!(
        matches!(
            outcome,
            RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
        ),
        "{outcome:?}"
    );
}

#[test]
fn interpreter_vectors() {
    with_compiler_stack(|| {
        let p = Program::new("interpreter", &vectors());
        ok(interpret(&p.0));
    });
}
#[test]
fn llvm_vectors() {
    with_compiler_stack(|| {
        let p = Program::new("llvm", &vectors());
        ok(native(&p.0, native::Backend::Llvm));
    });
}
#[test]
fn cranelift_vectors() {
    with_compiler_stack(|| {
        let p = Program::new("cranelift", &vectors());
        ok(native(&p.0, native::Backend::Cranelift));
    });
}
#[test]
fn empty_old_fails() {
    with_compiler_stack(|| {
        for src in ["b\"\"", "b\"banana\""] {
            let p = Program::new(
                "empty",
                &format!(
                    "pub fn main() -> Result[(), ()] {{ let _ = std@1/bytes.replace({src}, b\"\", b\"<>\"); Ok(()) }}"
                ),
            );
            for outcome in [
                interpret(&p.0),
                native(&p.0, native::Backend::Llvm),
                native(&p.0, native::Backend::Cranelift),
            ] {
                assert!(
                    matches!(outcome, RunOutcome::Failed { primary: hirsel_run_schema::Failure { ref rule, .. }, .. } if rule.as_str() == "SEM-3"),
                    "{outcome:?}"
                );
            }
        }
    });
}
#[test]
fn heap_failure_cleanup_and_backing() {
    with_compiler_stack(|| {
        let unused = vec![255; 4096];
        for (name, src, old, new) in [
            ("no", vec![b'a'; 512], b"z".as_slice(), b"x".as_slice()),
            (
                "sparse",
                [vec![b'a'; 255], vec![0], vec![b'a'; 256]].concat(),
                b"\0",
                b"xyz",
            ),
            ("dense", vec![b'a'; 64], b"a", b"12345678"),
            ("deletion", vec![b'a'; 512], b"a", b""),
            ("unused-replacement", vec![b'a'], b"z", unused.as_slice()),
        ] {
            let expected = scalar(&src, old, new);
            let p = Program::new(
                name,
                &format!(
                    "pub fn main() -> Result[(), ()] {{ let out = std@1/bytes.replace({}, {}, {}); failure.check(out == {}, TextInvariant()); Ok(()) }}",
                    literal(&src),
                    literal(old),
                    literal(new),
                    literal(&expected)
                ),
            );
            let mut loaded = load_program(&p.0).unwrap();
            loaded.check_for_execution().unwrap();
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input.clone()).unwrap();
            let ready = native::build(
                &validated,
                &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            )
            .unwrap();
            let run =
                native::heap_campaign(&ready, &loaded.entry, &native::Sites::new(&input)).unwrap();
            assert_eq!(run.observation.exit_code, Some(0));
            assert_eq!(run.counters.live_heap_bytes, 0);
            assert_eq!(run.counters.allocations, run.counters.frees);
            // Modeled capacity stays proportional to emitted bytes. In
            // particular, a large unused replacement must reserve nothing.
            assert!(run.counters.peak_live_heap_bytes <= 3 * expected.len() as u64 + 128);
            eprintln!(
                "{name}: logical_output={} failure_sites={} {:?}",
                expected.len(),
                run.sites,
                run.counters
            );
        }
    });
}

#[test]
// The returned buffer is fresh on the heap. Materializing it into a bounded
// arena fails after replacement and must release that temporary while unwinding.
// The construction campaign above separately fails every replacement site.
fn low_memory_unwinds_and_recovers() {
    with_compiler_stack(|| {
        let source = r#"
fn attempt(cap: u64) -> Result[u64, OutOfSpace] {
  arena.bounded(cap, fn(a: Arena) -> u64 {
    let src: Bytes in a = copy_to(a, b"aaaa")
    let replaced: Bytes in a = copy_to(a, std@1/bytes.replace(src, b"a", b"12345678"))
    bytes.len(replaced)
  })
}
pub fn main() -> Result[(), ()] {
  failure.check(attempt(5) == Error(OutOfSpace()), TextInvariant())
  failure.check(attempt(128) == Ok(32u64), TextInvariant())
  Ok(())
}
"#;
        let p = Program::new("low-memory", source);
        ok(interpret(&p.0));
        ok(native(&p.0, native::Backend::Llvm));
        ok(native(&p.0, native::Backend::Cranelift));
        let mut loaded = load_program(&p.0).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input.clone()).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let run =
            native::heap_campaign(&ready, &loaded.entry, &native::Sites::new(&input)).unwrap();
        assert_eq!(run.observation.exit_code, Some(0));
        assert_eq!(run.counters.live_heap_bytes, 0);
        eprintln!(
            "low-memory: {} injected sites {:?}",
            run.sites, run.counters
        );
    });
}
