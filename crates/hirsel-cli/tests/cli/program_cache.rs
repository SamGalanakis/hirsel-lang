//! The program cache (docs/building.md, "The program cache"): an unchanged program's
//! `run --native` and `build` are served from it with exactly a fresh run's or build's
//! output, and an edit to any source the program reads is a miss.
use std::process::{Command, Output};

const HELLO: &str = "pub type Failure = Grant(GrantError) | Output(OutputError)\n\n\
pub fn main(rt: Io) -> Result[(), Failure] {\n  let output = io.stdout(rt) ? Grant\n  \
out.print(output, \"hello, world\\n\") ? Output\n  Ok(())\n}\n";

/// Prints, then fails a check: its report names a source site.
const FAILS: &str = "pub type Failure = Grant(GrantError) | Output(OutputError)\n\n\
pub fn main(rt: Io) -> Result[(), Failure] {\n  let output = io.stdout(rt) ? Grant\n  \
out.print(output, \"before\\n\") ? Output\n  \
failure.check(1u64 + 1u64 == 3u64, Arithmetic())\n  Ok(())\n}\n";

const GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}"#;

/// A fresh native cache directory, for this test alone.
fn cache() -> std::path::PathBuf {
    let dir = super::file("").with_extension("cache");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn hirsel(cache: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command
        .env("HIRSEL_NATIVE_CACHE", cache)
        .env_remove("HIRSEL_PROGRAM_CACHE")
        .arg("--timings=json");
    command
}

fn run(cache: &std::path::Path, source: &std::path::Path) -> Output {
    let grants = source.with_extension("grants.json");
    std::fs::write(&grants, GRANTS).unwrap();
    hirsel(cache)
        .args(["run", "--native"])
        .arg(source)
        .arg("--grants")
        .arg(&grants)
        .output()
        .unwrap()
}

/// The program cache's (hits, misses) and the units the command built.
fn counts(output: &Output) -> (u64, u64, u64) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report: serde_json::Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    let count = |name: &str| report["counts"][name].as_u64().unwrap();
    (
        count("program hits"),
        count("program misses"),
        count("units"),
    )
}

/// stderr without its timings report.
fn reported(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut lines: Vec<_> = stderr.lines().collect();
    lines.pop();
    lines.join("\n")
}

#[test]
fn an_unchanged_native_run_is_served_from_the_cache() {
    let cache = cache();
    let source = super::std_file(HELLO);
    let cold = run(&cache, &source);
    assert_eq!(cold.status.code(), Some(0), "{cold:?}");
    let (hits, misses, units) = counts(&cold);
    assert_eq!((hits, misses), (0, 1), "{cold:?}");
    assert!(units > 0, "{cold:?}");
    let warm = run(&cache, &source);
    assert_eq!(warm.status.code(), Some(0), "{warm:?}");
    assert_eq!(counts(&warm), (1, 0, 0), "{warm:?}");
    assert_eq!(warm.stdout, b"hello, world\n");
    assert_eq!(reported(&warm), reported(&cold));
    // Any edit to a source the program reads is a miss.
    std::fs::write(&source, HELLO.replace("hello, world", "hello, again")).unwrap();
    let edited = run(&cache, &source);
    assert_eq!(edited.status.code(), Some(0), "{edited:?}");
    assert_eq!(counts(&edited).1, 1, "{edited:?}");
    assert_eq!(edited.stdout, b"hello, again\n");
}

#[test]
fn a_cached_run_reports_a_failure_as_a_fresh_one_does() {
    let cache = cache();
    let source = super::std_file(FAILS);
    let cold = run(&cache, &source);
    assert_ne!(cold.status.code(), Some(0), "{cold:?}");
    assert!(reported(&cold).contains("runtime failure"), "{cold:?}");
    assert!(reported(&cold).contains(".hir:6:3)"), "{cold:?}");
    let warm = run(&cache, &source);
    assert_eq!(counts(&warm).0, 1, "{warm:?}");
    assert_eq!(warm.status.code(), cold.status.code());
    assert_eq!(warm.stdout, cold.stdout);
    assert_eq!(reported(&warm), reported(&cold));
}

#[test]
fn an_unchanged_build_copies_the_cached_executable() {
    let cache = cache();
    let source = super::std_file(HELLO);
    let build = |output: &std::path::Path| {
        hirsel(&cache)
            .args(["build", "--debug"])
            .arg(&source)
            .arg("-o")
            .arg(output)
            .output()
            .unwrap()
    };
    let (a, b) = (cache.join("a"), cache.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let cold = build(&a.join("program"));
    assert_eq!(cold.status.code(), Some(0), "{cold:?}");
    assert_eq!(counts(&cold).1, 1, "{cold:?}");
    let warm = build(&b.join("program"));
    assert_eq!(warm.status.code(), Some(0), "{warm:?}");
    assert_eq!(counts(&warm), (1, 0, 0), "{warm:?}");
    // The same executable, and the same split debug file beside it under its
    // build-id name.
    let files = |dir: &std::path::Path| -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| {
                (
                    e.file_name().into_string().unwrap(),
                    std::fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    };
    let built = files(&a);
    assert_eq!(
        built.len(),
        2,
        "{:?}",
        built.iter().map(|f| &f.0).collect::<Vec<_>>()
    );
    assert!(built.iter().any(|(name, _)| name.ends_with(".debug")));
    assert!(files(&b) == built);
}
