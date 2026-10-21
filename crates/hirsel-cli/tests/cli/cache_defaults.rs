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
                .env("HIRSEL_STD_OBJECTS", "none")
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
#[test]
fn warmed_native_callbacks_finalizers_and_grants_follow_edits_at_one_two_eight() {
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
    for backend in ["llvm", "cranelift"] {
        for jobs in ["1", "2", "8"] {
            std::fs::write(&source, original).unwrap();
            let run = |grants: &str, off: bool| {
                let mut cmd = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                cmd.env("HIRSEL_DAEMON", "off")
                    .env("HIRSEL_RESULTS", "none@1")
                    .env("HIRSEL_STD_OBJECTS", "none")
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
        }
    }
    std::fs::remove_dir_all(cache).unwrap();
}
