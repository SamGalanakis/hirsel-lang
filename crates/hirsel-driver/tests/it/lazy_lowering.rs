//! Dependencies are checked completely and lowered only where the program reaches them.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::{ItemPath, PackageId};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hirsel-lazy-lowering-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn write(&self, name: &str, source: &str) {
        std::fs::write(self.0.join(name), source).unwrap();
    }
    fn app(&self, source: &str, dependency: (&str, &Path)) {
        self.write("app.hir", source);
        let (name, manifest) = dependency;
        self.write(
            "app.json",
            &serde_json::json!({"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"app.hir","module":[]}],"dependencies":[{"manifest":manifest,"package":{"name":name,"version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        );
    }
    fn library(&self, source: &str) -> PathBuf {
        self.write("lib.hir", source);
        self.write(
            "lib.json",
            &serde_json::json!({"schema":1,"package":{"name":"lib","version":"1"},"sources":[{"path":"lib.hir","module":[]}],"dependencies":[]}).to_string(),
        );
        self.0.join("lib.json")
    }
    fn check(&self) -> LoadedProgram {
        let mut program =
            load_program_source_with_images(&self.0.join("app.json"), None, &StdImages::none())
                .unwrap();
        let output = program
            .check(hirsel_engine::LoweringPolicy::Reachable)
            .unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        program
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn path(package: &str, modules: &[&str], name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: package.into(),
            version: "1".into(),
        },
        modules: modules.iter().map(|m| (*m).into()).collect(),
        name: name.into(),
    })
}
fn lowered(program: &LoadedProgram, package: &str, modules: &[&str], name: &str) -> usize {
    program
        .engine
        .recompute_count(&path(package, modules, name), QueryKind::Lower)
}

const LIBRARY: &str = r#"
pub fn unused() -> u64 {
  unused_helper()
}

fn unused_helper() -> u64 {
  1
}

fn unused_reader() -> u64 {
  LIMIT
}

const LIMIT: u64 = 3

pub fn limit() -> u64 {
  LIMIT
}

pub fn keep[T](take x: T) -> T {
  keep_inner(take x)
}

fn keep_inner[T](take x: T) -> T {
  x
}

pub fn adder(n: u64) -> fn(u64) -> u64 {
  fn(x: u64) -> u64 {
    bump(x) + n
  }
}

fn bump(x: u64) -> u64 {
  x + 1
}

pub fn pick() -> fn(u64) -> u64 {
  referenced
}

fn referenced(x: u64) -> u64 {
  x
}

pub resource type Token {
  n: u64,
}
drop(self) {
  release(self.n)
}

fn release(n: u64) {}

pub fn token() -> Token {
  Token(n: 1)
}

@loop_source(open, next)
pub type View {
  source: String,
}

pub type Cursor {
  source: String,
  position: u64,
}

@noalloc
pub fn view(s: String) -> View in s {
  View(source: copy s)
}

@noalloc
fn open(v: View) -> Cursor in v {
  Cursor(source: copy v.source, position: 0)
}

@noalloc
fn next(mut it: Cursor) -> Option[String] in it {
  if it.position == 0 {
    it.position = 1
    Some(copy it.source)
  } else {
    None
  }
}
"#;

const APP: &str = r#"
fn first(s: String) -> String in s {
  let mut out: String in s = ""
  for piece in lib@1/view(s) {
    out = copy piece
  }
  out
}

pub fn main() {
  let a = lib@1/keep(1u64)
  let f = lib@1/adder(2)
  let b = f(a)
  let g = lib@1/pick()
  let c = g(b)
  let t = lib@1/token()
  let n = lib@1/limit()
  let s = first("a")
}
"#;

#[test]
fn dependency_bodies_lower_exactly_when_reached_or_evaluated_at_compile_time() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let library = f.library(LIBRARY);
        f.app(APP, ("lib", &library));
        let mut program = f.check();
        // Every app body lowers, reachable or not.
        for name in ["main", "first"] {
            assert_eq!(lowered(&program, "app", &[], name), 1, "{name}");
        }
        // Reached through calls, generic calls, closure bodies, function
        // references, finalizers and loop protocol steps.
        for name in [
            "keep",
            "keep_inner",
            "adder",
            "bump",
            "pick",
            "referenced",
            "Token",
            "release",
            "token",
            "limit",
            "view",
            "open",
            "next",
        ] {
            assert_eq!(lowered(&program, "lib", &[], name), 1, "{name}");
        }
        // Compile-time evaluation runs inside lowering, so constants and their
        // readers still lower at check time to keep CT diagnostics identical.
        for name in ["LIMIT", "unused_reader"] {
            assert_eq!(lowered(&program, "lib", &[], name), 1, "{name}");
        }
        for name in ["unused", "unused_helper"] {
            assert_eq!(lowered(&program, "lib", &[], name), 0, "{name}");
        }
        // The launch closure is the same walk: it lowers nothing new.
        let entry = program.entry.clone();
        program.engine.semantic_program_revision(&entry).unwrap();
        for name in ["unused", "unused_helper"] {
            assert_eq!(lowered(&program, "lib", &[], name), 0, "{name}");
        }
        for name in ["keep", "next", "Token", "release"] {
            assert_eq!(lowered(&program, "lib", &[], name), 1, "{name}");
        }
    });
}

#[test]
fn unreached_dependency_bodies_lower_when_the_program_starts_reaching_them() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let library = f.library(LIBRARY);
        f.app("pub fn main() {}\n", ("lib", &library));
        let program = f.check();
        assert_eq!(lowered(&program, "app", &[], "main"), 1);
        for name in ["unused", "keep", "adder", "Token", "next"] {
            assert_eq!(lowered(&program, "lib", &[], name), 0, "{name}");
        }
        f.app(
            "pub fn main() {\n  let n = lib@1/unused()\n}\n",
            ("lib", &library),
        );
        let program = f.check();
        for name in ["unused", "unused_helper"] {
            assert_eq!(lowered(&program, "lib", &[], name), 1, "{name}");
        }
    });
}

#[test]
fn empty_main_lowers_no_std_body_that_it_does_not_reach() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        // The manifest is written into a temporary directory: name std absolutely.
        let std = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        f.app("pub fn main() {}\n", ("std", &std));
        let program = f.check();
        assert_eq!(lowered(&program, "app", &[], "main"), 1);
        for (module, name) in [
            ("unicode", "normalize_nfc"),
            ("u64", "to_string"),
            ("unicode", "graphemes"),
        ] {
            assert_eq!(
                lowered(&program, "std", &[module], name),
                0,
                "{module}.{name}"
            );
        }
        // Each check holds all of std checked from source: end the first before
        // the second, or both stay alive (shadowing drops nothing).
        drop(program);
        f.app(
            "pub fn main() {\n  let s = std@1/u64.to_string(42u64)\n}\n",
            ("std", &std),
        );
        let program = f.check();
        assert_eq!(lowered(&program, "std", &["u64"], "to_string"), 1);
        assert_eq!(lowered(&program, "std", &["unicode"], "normalize_nfc"), 0);
    });
}

/// Diagnostics of `lib`'s files, rendered with their paths, from a check where
/// `lib` is either the program package (every body lowers) or a dependency.
fn library_diagnostics(f: &Fixture, files: &[(&str, &str)], as_dependency: bool) -> Vec<String> {
    let sources: Vec<_> = files
        .iter()
        .map(|(name, source)| {
            f.write(name, source);
            serde_json::json!({"path": name, "module": [name.trim_end_matches(".hir")]})
        })
        .collect();
    let mut library = serde_json::json!({"schema":1,"package":{"name":"lib","version":"1"},"sources":sources,"dependencies":[]});
    let root = if as_dependency {
        f.write("lib.json", &library.to_string());
        f.app("pub fn main() {}\n", ("lib", &f.0.join("lib.json")));
        "app.json"
    } else {
        f.write("main.hir", "pub fn main() {}\n");
        library["sources"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"path":"main.hir","module":[]}));
        library["entry"] = serde_json::json!({"module":[],"name":"main"});
        f.write("lib.json", &library.to_string());
        "lib.json"
    };
    let mut program =
        load_program_source_with_images(&f.0.join(root), None, &StdImages::none()).unwrap();
    let output = program
        .check(hirsel_engine::LoweringPolicy::Reachable)
        .unwrap();
    let mut rendered = vec![format!("{:?}", output.completion)];
    rendered.extend(
        output
            .diagnostics
            .iter()
            .map(|d| d.render_human(&output.sources)),
    );
    rendered
}

#[test]
fn a_package_reports_identically_as_a_dependency_and_as_the_program() {
    with_compiler_stack(|| {
        for files in [
            &[("a.hir", "pub fn unused() -> u64 {\n  true\n}\n")][..],
            &[(
                "a.hir",
                "pub fn value() -> u64 {\n  comptime (1u64 / 0u64)\n}\n",
            )],
            // A body whose callee header is rejected in another file.
            &[
                ("a.hir", "pub fn broken() -> Missing {\n  1\n}\n"),
                (
                    "b.hir",
                    "pub fn user() -> u64 {\n  let x = a.broken()\n  1\n}\n",
                ),
            ],
            // Later bodies after a failed compile-time evaluation are withheld.
            &[(
                "a.hir",
                "fn reader() -> u64 {\n  C\n}\n\nfn later() -> u64 {\n  comptime (2u64 / 0u64)\n}\n\nconst C: u64 = 1u64 / 0u64\n",
            )],
            // A constant initializer reads itself through a constant reader.
            // Both policies lower the reader in order; its re-entrant
            // lowering is currently unavailable rather than CT-4.
            &[(
                "a.hir",
                "fn reader() -> u64 {\n  C\n}\n\nconst C: u64 = reader()\n",
            )],
            // A failed S1 evaluation in one file does not withhold another
            // file's source diagnostics or its own compile-time failure.
            &[
                (
                    "a.hir",
                    "pub fn value() -> u64 { comptime (1u64 / 0u64) }\n",
                ),
                ("b.hir", "pub fn bad() -> u64 { true }\n"),
                (
                    "c.hir",
                    "pub fn value() -> u64 { comptime (2u64 / 0u64) }\n",
                ),
            ],
        ] {
            let f = Fixture::new();
            let program = library_diagnostics(&f, files, false);
            let dependency = library_diagnostics(&f, files, true);
            assert!(program != ["Complete"], "{files:?}");
            assert_eq!(dependency, program, "{files:?}");
        }
    });
}

#[test]
fn program_growth_is_decided_before_any_files_comptime_lowering() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let files = [
            (
                "a.hir",
                "pub fn value() -> u64 { comptime (1u64 / 0u64) }\n",
            ),
            (
                "b.hir",
                "fn grow[T: copyable](x: T) { grow((copy x, copy x)) }\n",
            ),
        ];
        for as_dependency in [false, true] {
            let diagnostics = library_diagnostics(&f, &files, as_dependency);
            assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
            assert_eq!(diagnostics[0], "Complete");
            assert!(diagnostics[1].contains("GEN-5"), "{diagnostics:?}");
            assert!(!diagnostics[1].contains("CT-4"), "{diagnostics:?}");
        }
    });
}
