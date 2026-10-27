//! Ordinary commands persist units without an opt-in; explicit none stays a control.
use std::process::Command;

#[test]
fn default_native_store_reuses_unchanged_units_and_none_recomputes() {
    let source = super::file(
        "fn helper(x:u64)->u64 { x + 1u64 }\npub fn main()->Result[(),()] { failure.check(helper(3u64)==4u64, Arithmetic()); Ok(()) }\n",
    );
    let cache = source.with_extension("cache");
    let exe = source.with_extension("exe");
    for profile in ["--debug", "--release"] {
        let build = |provider: Option<&str>| {
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command
                .env("HIRSEL_DAEMON", "off")
                .env("HIRSEL_RESULTS", "none@1")
                .env("HIRSEL_JOBS", "2")
                .env("XDG_CACHE_HOME", &cache)
                .env_remove("HIRSEL_OBJECTS")
                .env_remove("HIRSEL_VALIDATE_STAGES");
            if let Some(provider) = provider {
                command.env("HIRSEL_OBJECTS", provider);
            }
            let result = command
                .args(["build", profile, "--timings=json"])
                .arg(&source)
                .arg("-o")
                .arg(&exe)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{profile} {provider:?}: {result:?}"
            );
            let stderr = std::str::from_utf8(&result.stderr).unwrap();
            let timings: serde_json::Value =
                serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
            (timings, std::fs::read(&exe).unwrap())
        };
        let (cold, bytes) = build(None);
        assert!(cold["counts"]["object misses"].as_u64().unwrap() > 0);
        let (warm, cached) = build(None);
        assert!(warm["counts"]["object hits"].as_u64().unwrap() > 0);
        assert_eq!(warm["counts"]["object misses"], 0);
        assert_eq!(warm["counts"]["units"], 0, "Ready hits must skip lowering");
        assert_eq!(bytes, cached, "default hit changed executable bytes");
        let (off, fresh) = build(Some("none@1"));
        assert_eq!(off["counts"]["object hits"], 0);
        assert!(off["counts"]["units"].as_u64().unwrap() > 0);
        assert_eq!(bytes, fresh, "cache-off control changed executable bytes");
        assert!(Command::new(&exe).status().unwrap().success());
    }
    std::fs::remove_dir_all(cache).unwrap();
}

/// Callback bodies, generic instances, finalizers and launch grants remain current
/// when the ordinary object provider has already cached the program.
fn warmed_native_callback_edits(backend: &str, jobs: &str) {
    let original = r#"
resource type R { n:u64 }
drop(x) { let _ = 10u64 / x.n; () }
fn keep[T:copyable](take x:T)->T { x }
fn apply(f:fn(u64)->u64, x:u64)->u64 { f(x) }
pub fn main(rt:Io)->Result[(),GrantError] {
  let out = io.stdout(rt)?
  let r = R(n:1u64)
  let number:u64 = 4u64
  let flag:bool = true
  let n = keep(take number)
  let yes = keep(take flag)
  let got = apply(fn(x:u64)->u64 { x + 1u64 }, n)
  let _ = out.print(out, if yes && got == 5u64 { "five\n" } else { "six\n" })
  Ok(())
}
"#;
    let source = super::file(original);
    let cache = source.with_extension("cache");
    let granted = r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}"#;
    std::fs::write(&source, original).unwrap();
    let run = |grants: &str, off: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        cmd.env("HIRSEL_DAEMON", "off")
            .env("HIRSEL_RESULTS", "none@1")
            .env("HIRSEL_JOBS", jobs)
            .env("XDG_CACHE_HOME", cache.join(backend).join(jobs))
            .env_remove("HIRSEL_OBJECTS")
            .env_remove("HIRSEL_VALIDATE_STAGES");
        if off {
            cmd.env("HIRSEL_OBJECTS", "none@1");
        }
        let output = cmd
            .args(["run", "--native", "--backend", backend, "--timings=json"])
            .arg(&source)
            .args(["--grants", grants])
            .output()
            .unwrap();
        let stderr = std::str::from_utf8(&output.stderr).unwrap();
        let report: serde_json::Value =
            serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
        (output, report)
    };
    let (cold, _) = run(granted, false);
    assert_eq!(cold.status.code(), Some(0), "{backend} j{jobs}: {cold:?}");
    assert_eq!(cold.stdout, b"five\n");
    let (warm, counts) = run(granted, false);
    assert_eq!(warm.status.code(), Some(0), "{warm:?}");
    assert_eq!(warm.stdout, cold.stdout);
    assert!(counts["counts"]["object hits"].as_u64().unwrap() > 0);
    assert_eq!(counts["counts"]["object misses"], 0);
    let (denied, _) = run("{}", false);
    assert_eq!(denied.status.code(), Some(3), "{denied:?}");
    assert!(
        denied.stdout.is_empty(),
        "a warmed launch reused old grants"
    );
    let edited = original.replace("x + 1u64", "x + 2u64");
    std::fs::write(&source, &edited).unwrap();
    let (callback, report) = run(granted, false);
    assert_eq!(callback.status.code(), Some(0), "{callback:?}");
    assert_eq!(callback.stdout, b"six\n");
    assert!(report["counts"]["object misses"].as_u64().unwrap() > 0);
    std::fs::write(
        &source,
        edited.replace("10u64 / x.n", "10u64 / (x.n - 1u64)"),
    )
    .unwrap();
    let (finalizer, _) = run(granted, false);
    let (fresh, _) = run(granted, true);
    assert_eq!(finalizer.status.code(), Some(2), "{finalizer:?}");
    assert_eq!(finalizer.status.code(), fresh.status.code());
    assert_eq!(finalizer.stdout, b"six\n");
    assert_eq!(finalizer.stdout, fresh.stdout);
    assert!(String::from_utf8_lossy(&finalizer.stderr).contains("SEM-1"));
    assert!(String::from_utf8_lossy(&fresh.stderr).contains("SEM-1"));
    std::fs::remove_dir_all(cache).unwrap();
}

#[test]
fn warmed_native_callbacks_finalizers_and_grants_llvm_one() {
    warmed_native_callback_edits("llvm", "1");
}

#[test]
fn warmed_native_callbacks_finalizers_and_grants_llvm_eight() {
    warmed_native_callback_edits("llvm", "8");
}

#[test]
fn warmed_native_callbacks_finalizers_and_grants_cranelift_one() {
    warmed_native_callback_edits("cranelift", "1");
}

#[test]
fn warmed_native_callbacks_finalizers_and_grants_cranelift_eight() {
    warmed_native_callback_edits("cranelift", "8");
}

/// A std-free package isolates cache selection from installed-seed behavior.
struct HermeticCacheFixture(std::path::PathBuf);
impl HermeticCacheFixture {
    fn new() -> Self {
        let source = super::file("pub fn main()->Result[(),()] { Ok(()) }\n");
        let root = source.with_extension("cache-environment");
        std::fs::create_dir(&root).unwrap();
        std::fs::rename(source, root.join("main.hir")).unwrap();
        std::fs::write(
            root.join("hirsel.json"),
            serde_json::json!({
                "schema":1,"package":{"name":"tiny","version":"0"},
                "entry":{"module":[],"name":"main"},
                "sources":[{"path":"main.hir","module":[]}],"dependencies":[]
            })
            .to_string(),
        )
        .unwrap();
        Self(root)
    }
    fn command(&self) -> Command {
        let mut command =
            Command::new(std::fs::canonicalize(env!("CARGO_BIN_EXE_hirsel")).unwrap());
        for name in [
            "HOME",
            "XDG_CACHE_HOME",
            "HIRSEL_OBJECTS",
            "HIRSEL_RESULTS",
            "HIRSEL_RESULTS_DIR",
            "HIRSEL_PRODUCTS",
            "HIRSEL_VERIFY_CACHE",
            "HIRSEL_VALIDATE_STAGES",
        ] {
            command.env_remove(name);
        }
        command
            .current_dir(&self.0)
            .env("HIRSEL_DAEMON", "off")
            .args(["build", "--debug", "--timings=json"])
            .arg(self.0.join("hirsel.json"))
            .arg("-o")
            .arg(self.0.join("app"));
        command
    }
    fn build(&self, command: &mut Command) -> (serde_json::Value, Vec<u8>) {
        let output = command.output().unwrap();
        assert!(output.status.success(), "{command:?}: {output:?}");
        let stderr = std::str::from_utf8(&output.stderr).unwrap();
        let report = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
        let run = Command::new(self.0.join("app"))
            .env_clear()
            .output()
            .unwrap();
        assert!(run.status.success(), "{run:?}");
        assert!(run.stdout.is_empty() && run.stderr.is_empty(), "{run:?}");
        (report, std::fs::read(self.0.join("app")).unwrap())
    }
}
impl Drop for HermeticCacheFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn native_cache_without_home_recomputes_and_explicit_dir_requires_a_location() {
    let fixture = HermeticCacheFixture::new();
    let (no_home, bytes) = fixture.build(&mut fixture.command());
    assert_eq!(no_home["counts"]["object hits"], 0);
    assert!(no_home["counts"]["units"].as_u64().unwrap() > 0);
    for value in ["", "relative"] {
        let (report, fresh) = fixture.build(
            fixture
                .command()
                .env("HOME", value)
                .env("XDG_CACHE_HOME", value),
        );
        assert_eq!(report["counts"]["object hits"], 0);
        assert_eq!(report["counts"]["store hits"], 0);
        assert_eq!(bytes, fresh);
        let explicit = fixture
            .command()
            .env("HOME", value)
            .env("XDG_CACHE_HOME", value)
            .env("HIRSEL_OBJECTS", "dir@1")
            .output()
            .unwrap();
        assert!(!explicit.status.success(), "{explicit:?}");
        assert!(
            String::from_utf8_lossy(&explicit.stderr).contains("dir@1: no cache directory"),
            "{explicit:?}"
        );
    }
    let explicit = fixture
        .command()
        .env("HIRSEL_OBJECTS", "dir@1")
        .output()
        .unwrap();
    assert!(!explicit.status.success());
    assert!(String::from_utf8_lossy(&explicit.stderr).contains("dir@1: no cache directory"));
    assert!(!fixture.0.join(".cache").exists());
    assert!(!fixture.0.join("relative").exists());
}

#[test]
fn native_private_cache_reuses_units_and_unwritable_dir_never_reports_a_hit() {
    let fixture = HermeticCacheFixture::new();
    let cache = fixture.0.join("cache");
    let (cold, bytes) = fixture.build(
        fixture
            .command()
            .env("XDG_CACHE_HOME", &cache)
            .env("HIRSEL_RESULTS_DIR", ""),
    );
    assert!(cold["counts"]["object misses"].as_u64().unwrap() > 0);
    let (warm, cached) = fixture.build(
        fixture
            .command()
            .env("XDG_CACHE_HOME", &cache)
            .env("HIRSEL_RESULTS_DIR", ""),
    );
    assert!(warm["counts"]["object hits"].as_u64().unwrap() > 0);
    assert_eq!(warm["counts"]["units"], 0);
    assert_eq!(bytes, cached);
    assert!(
        cache
            .join(format!("hirsel/results-v{}", hirsel_engine::store::FORMAT))
            .is_dir()
    );
    let (_, fresh) = fixture.build(&mut fixture.command());
    assert_eq!(bytes, fresh);
    // ENOTDIR works even when the harness runs as root; chmod(0) would not.
    let blocked = fixture.0.join("blocked");
    std::fs::write(&blocked, "not a directory").unwrap();
    for _ in 0..2 {
        let (report, fresh) = fixture.build(
            fixture
                .command()
                .env("XDG_CACHE_HOME", &blocked)
                .env("HIRSEL_OBJECTS", "dir@1")
                .env("HIRSEL_RESULTS", "dir@1"),
        );
        assert_eq!(report["counts"]["object hits"], 0);
        assert_eq!(report["counts"]["store hits"], 0);
        assert!(report["counts"]["units"].as_u64().unwrap() > 0);
        assert_eq!(bytes, fresh);
    }
    assert_eq!(std::fs::read_to_string(blocked).unwrap(), "not a directory");
}

#[test]
fn native_empty_xdg_uses_home_and_relocated_compiler_needs_no_home() {
    let fixture = HermeticCacheFixture::new();
    let home = fixture.0.join("home");
    let (_, bytes) = fixture.build(
        fixture
            .command()
            .env("HOME", &home)
            .env("XDG_CACHE_HOME", ""),
    );
    let (warm, cached) = fixture.build(fixture.command().env("HOME", &home));
    assert!(warm["counts"]["object hits"].as_u64().unwrap() > 0);
    assert_eq!(bytes, cached);
    assert!(home.join(".cache/hirsel").is_dir());
    // Copy the compiler into a relocated bundle. This package is std-free:
    // neither an image nor a seed may hide a writable-cache dependency.
    let bundle = fixture.0.join("bundle");
    std::fs::create_dir(&bundle).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_hirsel"), bundle.join("hirsel")).unwrap();
    let command = fixture.command();
    let mut moved = Command::new(bundle.join("hirsel"));
    moved.args(command.get_args()).current_dir(&fixture.0);
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            moved.env(key, value);
        } else {
            moved.env_remove(key);
        }
    }
    let (report, relocated) = fixture.build(&mut moved);
    assert_eq!(report["counts"]["object hits"], 0);
    assert_eq!(bytes, relocated);
}

/// A persistent product follows its package between the root and dependency
/// positions. The lookup of B has the same result under both manifest graphs.
#[test]
fn verify_cache_modes_follow_a_package_between_root_and_dependency() {
    let fixture = HermeticCacheFixture::new();
    let library = fixture.0.join("library.json");
    std::fs::write(
        fixture.0.join("library.hir"),
        "pub type A { b:B }\npub type B { n:u8 }\n",
    )
    .unwrap();
    std::fs::write(
        &library,
        serde_json::json!({
            "schema":1,"package":{"name":"library","version":"1"},
            "sources":[{"path":"library.hir","module":[]}],"dependencies":[]
        })
        .to_string(),
    )
    .unwrap();
    check_root_and_dependency_cache(&fixture, &library, "library");
}

/// Index resolution probes the local parameter as a manifest name. Its
/// negative lookup must verify when only the importing manifest graph changes.
#[test]
fn verify_cache_negative_lookup_modes_follow_a_package_between_root_and_dependency() {
    let fixture = HermeticCacheFixture::new();
    let library = fixture.0.join("library.json");
    std::fs::write(
        fixture.0.join("library.hir"),
        "fn value(data: Bytes)->u8 { data[0] }\n",
    )
    .unwrap();
    std::fs::write(
        &library,
        serde_json::json!({
            "schema":1,"package":{"name":"library","version":"1"},
            "sources":[{"path":"library.hir","module":[]}],"dependencies":[]
        })
        .to_string(),
    )
    .unwrap();
    check_root_and_dependency_cache(&fixture, &library, "library");
}

/// Checking std itself must seed products that verify in an ordinary importer.
/// Same-root verification alone misses manifest provenance in successful reads.
#[test]
fn verify_cache_modes_agree_across_all_std_modules() {
    let fixture = HermeticCacheFixture::new();
    let library = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    check_root_and_dependency_cache(&fixture, &library, "std");
}

fn check_root_and_dependency_cache(
    fixture: &HermeticCacheFixture,
    library: &std::path::Path,
    package: &str,
) {
    let client = fixture.0.join("hirsel.json");
    std::fs::write(
        &client,
        serde_json::json!({
            "schema":1,"package":{"name":"tiny","version":"0"},
            "entry":{"module":[],"name":"main"},
            "sources":[{"path":"main.hir","module":[]}],
            "dependencies":[{"manifest":library,"package":{"name":package,"version":"1"},"expose":[]}]
        })
        .to_string(),
    )
    .unwrap();
    let check = |path: &std::path::Path, verify: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .env("HIRSEL_DAEMON", "off")
            .env("HIRSEL_PRODUCTS", fixture.0.join("no-image"))
            .env("HIRSEL_RESULTS", "dir@1")
            .env("HIRSEL_RESULTS_DIR", fixture.0.join("results"))
            .env("HIRSEL_JOBS", "2")
            .env_remove("HIRSEL_VERIFY_CACHE")
            .args(["check", "--json", "--timings=json"]);
        if verify {
            command.arg("--verify-cache");
        }
        let output = command.arg(path).output().unwrap();
        assert!(output.status.success(), "{command:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let stderr = std::str::from_utf8(&output.stderr).unwrap();
        let report: serde_json::Value =
            serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
        report["counts"].clone()
    };
    let cold = check(library, false);
    // Verify the cold products before any warm adoption can replace them.
    let same_root = check(library, true);
    assert_eq!(same_root["definitions"], cold["definitions"], "{same_root}");
    assert_eq!(same_root["store misses"], 0, "{same_root}");
    let warm = check(library, false);
    assert!(cold["definitions"].as_u64().unwrap() > 0, "{cold}");
    assert_eq!(cold["store hits"], 0, "{cold}");
    let definitions = cold["definitions"].as_u64().unwrap();
    if package == "std" {
        // Independently index the manifest-selected declarations and register
        // their closed `with` specializations, without checking bodies or
        // reading store counters. The latter are ordinary source products too.
        // Each source declaration requires one Signature and one complete Modes
        // publication, replacing the unproved 4240 literal. Checked
        // summaries are embedded in Modes, not separate physical store products.
        let inventory = hirsel_engine::with_compiler_stack(|| {
            let mut declarations = hirsel_driver::COMPILER
                .with_owned_engines()
                .load_package_declarations(library)
                .unwrap();
            let indexed = declarations.engine.indexed_definitions();
            declarations.engine.register_specializations(&indexed);
            let required = declarations.engine.front_source_roots();
            declarations.engine.register_specializations(&required);
            indexed
                .into_iter()
                .chain(declarations.engine.front_source_roots())
                .collect::<std::collections::BTreeSet<_>>()
                .len() as u64
        });
        assert_eq!(
            definitions, inventory,
            "the complete checked std source inventory"
        );
    }
    assert_eq!(
        warm["definitions"], 0,
        "all source bodies must be adopted: {warm}"
    );
    assert_eq!(warm["store signature hits"], definitions, "{warm}");
    assert_eq!(warm["store summary hits"], 0, "{warm}");
    assert_eq!(warm["store mode hits"], definitions, "{warm}");
    assert_eq!(warm["store hits"], 2 * definitions, "{warm}");
    assert_eq!(warm["store misses"], 0, "{warm}");
    assert_eq!(cold["store summary misses"], 0, "{cold}");
    assert_eq!(warm["store summary misses"], 0, "{warm}");
    for kind in ["signature", "mode"] {
        assert_eq!(cold[format!("store {kind} misses")], definitions, "{cold}");
        assert_eq!(warm[format!("store {kind} misses")], 0, "{warm}");
    }
    assert_eq!(cold["store misses"], 2 * definitions, "{cold}");
    // Verify before a dependency run can replace a root-written record.
    let verified = check(&client, true);
    assert_eq!(
        verified["definitions"].as_u64().unwrap(),
        cold["definitions"].as_u64().unwrap() + 1,
        "every library definition plus the client's main must be recomputed: {verified} vs {cold}"
    );
    // A recomputation alone is insufficient: a rejected cache variant also
    // recomputes, without comparing its payload. Only the new client may miss.
    // The one new client misses both physical source products; every cached
    // library product must still be compared, with no extra miss.
    assert_eq!(verified["store misses"], 2, "{verified}");
    assert_eq!(verified["store summary misses"], 0, "{verified}");
    for kind in ["signature", "mode"] {
        assert_eq!(verified[format!("store {kind} misses")], 1, "{verified}");
    }
    eprintln!(
        "{package}: {} cold definitions, {} warm hits, {} replayed definitions, {} importer definitions, {} importer misses",
        cold["definitions"],
        warm["store hits"],
        warm["definitions"],
        verified["definitions"],
        verified["store misses"]
    );
}
