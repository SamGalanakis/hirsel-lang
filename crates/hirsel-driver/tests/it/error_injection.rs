//! LANG-8 payload-directed injection through the package check: the "no arm"
//! fix is a text edit that makes the program check, a plain `?` hashes like
//! the `? Arm` it replaces, and unconstructed private error arms warn.
use hirsel_driver::*;
use hirsel_engine::*;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

fn file(source: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "hirsel-error-injection-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.hir");
    std::fs::write(&path, source).unwrap();
    path
}

fn check(path: &Path) -> CheckOutput {
    with_compiler_stack(|| {
        let mut program =
            load_program_source_with_prechecks(path, None, &Prechecks::none()).unwrap();
        let output = program.check(LoweringPolicy::Comptime).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        output
    })
}

const NO_ARM: &str = r#"type ParseError = Empty()

type ConfigError = Missing()

type AppError = Input(ParseError) | Limit(u64)

fn parse(n: u64) -> Result[u64, ParseError] {
  if n == 0 { Error(Empty()) } else { Ok(n) }
}

fn config(n: u64) -> Result[u64, ConfigError] {
  if n == 0 { Error(Missing()) } else { Ok(n) }
}

fn run(n: u64) -> Result[u64, AppError] {
  let a = parse(n)?
  let b = config(a)?
  if b > 9 { return Error(Limit(b)) }
  Ok(b)
}
"#;

#[test]
fn applying_the_no_arm_fix_makes_the_program_check() {
    let path = file(NO_ARM);
    let output = check(&path);
    let errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    let [d] = &errors[..] else {
        panic!("{:?}", output.diagnostics)
    };
    assert_eq!(
        d.message,
        "plain ? has no arm for `ConfigError` in `AppError`"
    );
    let fix = d.fix.as_ref().expect("the no-arm fix");
    assert_eq!(fix.replacement, " | Config(ConfigError)");
    assert_eq!(
        fix.message,
        "(a) add the arm `Config(ConfigError)` to `AppError`; this changes the error contract of every function that returns it"
    );
    // The edit replaces the prose for (a); (b) and (c) stay as help.
    assert!(
        d.notes.iter().all(|n| !n.starts_with("help: (a)")),
        "{:?}",
        d.notes
    );
    assert!(d.notes.iter().any(|n| n.starts_with("help: (b)")));
    assert!(d.notes.iter().any(|n| n.starts_with("help: (c)")));
    let human = d.render_human(&output.sources);
    assert!(
        human.contains("5 | type AppError = Input(ParseError) | Limit(u64)"),
        "{human}"
    );
    let mut text = NO_ARM.to_string();
    text.insert_str(fix.span.start as usize, &fix.replacement);
    assert!(
        text.contains("type AppError = Input(ParseError) | Limit(u64) | Config(ConfigError)\n")
    );
    std::fs::write(&path, &text).unwrap();
    let fixed = check(&path);
    assert!(fixed.diagnostics.is_empty(), "{:?}", fixed.diagnostics);
}

#[test]
fn the_fix_on_a_serializable_error_names_its_schema_change() {
    let path = file(
        "@serializable\ntype RateError = Unknown()\n\n@serializable\ntype PriceError = Sku(u64)\n\n\
         fn lookup() -> Result[u64, RateError] {\n  Error(Unknown())\n}\n\n\
         fn price() -> Result[u64, PriceError] {\n  let n = lookup()?\n  if n == 0 { return Error(Sku(n)) }\n  Ok(n)\n}\n",
    );
    let output = check(&path);
    let d = &output.diagnostics[0];
    let fix = d.fix.as_ref().expect("the no-arm fix");
    assert_eq!(fix.replacement, " | Rate(RateError)");
    assert!(
        fix.message.ends_with(
            "of every function that returns it and its stored and wire schema (STORE-7, SER-2)"
        ),
        "{}",
        fix.message
    );
    assert!(
        d.notes.iter().any(|n| n.contains("STORE-7, SER-2")),
        "{:?}",
        d.notes
    );
}

#[test]
fn ambiguous_plain_question_mark_has_no_fix() {
    let path = file(
        "type StoreError = Corrupt()\n\ntype DbError = Open(StoreError) | Commit(StoreError)\n\n\
         fn open() -> Result[u64, StoreError] {\n  Error(Corrupt())\n}\n\n\
         fn save() -> Result[u64, DbError] {\n  let n = open()?\n  Ok(n)\n}\n",
    );
    let output = check(&path);
    let [d] = &output.diagnostics[..] else {
        panic!("{:?}", output.diagnostics)
    };
    assert!(d.fix.is_none());
    assert!(d.notes.iter().any(|n| n.contains("`? Open` or `? Commit`")));
}

/// Definition hashes of a program, by definition, as `hirsel hash` prints them.
fn hashes(source: &str) -> String {
    hash_path(&file(source)).unwrap()
}

#[test]
fn plain_question_mark_hashes_like_the_arm_it_selects() {
    let named = r#"type ParseError = Empty()

type FileProblem = NotFound()

type AppError = Input(ParseError) | Files(FileProblem) | Usage(String)

type Mixed[E] = Source(E) | Files(FileProblem)

fn parse() -> Result[u64, ParseError] {
  Error(Empty())
}

fn missing() -> Result[u64, FileProblem] {
  Error(NotFound())
}

fn usage() -> Result[u64, String] {
  Error("usage")
}

fn generic[E](f: fn() -> Result[u64, E]) -> Result[u64, Mixed[E]] uses {f} {
  let a = f() ? Source
  let b = missing() ? Files
  Ok(a + b)
}

fn flatten(e: Mixed[ParseError]) -> AppError {
  match e {
    Source(p) => Input(p),
    Files(f) => Files(f),
  }
}

fn closure() -> Result[u64, AppError] {
  let n = generic(fn() -> Result[u64, ParseError] uses {} {
    let v = parse()?
    Ok(v)
  }) ? flatten
  Ok(n)
}

fn run() -> Result[u64, AppError] {
  let a = parse() ? Input
  let b = missing() ? Files
  let c = usage() ? Usage
  Ok(a + b + c)
}
"#
    .to_string();
    let plain = named
        .replace("f() ? Source", "f()?")
        .replace("missing() ? Files", "missing()?")
        .replace("parse() ? Input", "parse()?");
    assert_eq!(
        plain.matches(" ? ").count(),
        2,
        "only the String arm and the function mapper stay"
    );
    let (named, plain) = (hashes(&named), hashes(&plain));
    assert!(named.lines().count() >= 10, "{named}");
    assert_eq!(named, plain);
}

#[test]
fn unconstructed_private_error_arms_warn() {
    let path = file(
        "type ParseError = Empty() | Stale()\n\ntype AppError = Input(ParseError) | Gone()\n\n\
         pub type Public = Used() | Unused()\n\n\
         fn parse() -> Result[u64, ParseError] {\n  Error(Empty())\n}\n\n\
         fn public() -> Result[u64, Public] {\n  Error(Used())\n}\n\n\
         fn run() -> Result[u64, AppError] {\n  let n = parse()?\n  let _ = public()\n  Ok(n)\n}\n",
    );
    let output = check(&path);
    let warnings: Vec<_> = output
        .diagnostics
        .iter()
        .map(|d| {
            assert_eq!(d.severity, hirsel_diag::Severity::Warning, "{d:?}");
            assert_eq!(d.rule.as_str(), "LANG-8");
            (
                output.sources.slice(d.primary).unwrap().to_string(),
                d.message.clone(),
            )
        })
        .collect();
    // `Input` is constructed by the injecting `?`; a `pub` type is not linted.
    assert_eq!(
        warnings,
        [
            (
                "Stale".to_string(),
                "arm `Stale` of error type `ParseError` is never constructed in this package"
                    .to_string()
            ),
            (
                "Gone".to_string(),
                "arm `Gone` of error type `AppError` is never constructed in this package"
                    .to_string()
            ),
        ]
    );
}
