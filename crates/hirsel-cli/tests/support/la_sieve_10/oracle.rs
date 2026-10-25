//! Pinned LangArena sieve.rs:21–57; full byte flags checked by trial division.

use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new(limits: &[u64]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "la-sieve-10-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        // Instrument the actual shipped step, without replacing initialization,
        // marking, scanning or checksum arithmetic with a test implementation.
        let kernel = include_str!("../../../../../bench/langarena/sieve.hir")
            .split("pub fn execute")
            .next()
            .unwrap();
        let mut source = kernel.replace("iteration: u64)", "iteration: u64, rt: Io)");
        source = source.replace(
            "  state.value_out =",
            r#"
  let mut flags = ""
  for byte in primes { flags = string.concat(flags, f"{byte}") }
  let Ok(output) = io.stdout(rt) else { return }
  let _ = out.print(output, f"{limit}:{flags}:{last}:{count}\n")
  state.value_out ="#,
        );
        source.push_str("\npub fn main(rt: Io) -> Result[(), String] {\n");
        for (i, limit) in limits.iter().enumerate() {
            source.push_str(&format!("  let mut s{i} = State(limit: {limit}, value_out: 0)\n  for iteration in 0u64..4u64 {{ step(mut s{i}, iteration, rt) }}\n  let Ok(output{i}) = io.stdout(rt) else {{ return Error(\"stdout\") }}\n  let _ = out.print(output{i}, f\"sum:{{s{i}.value_out}}\\n\")\n"));
        }
        source.push_str("  Ok(())\n}\n");
        std::fs::write(dir.join("main.hir"), source).unwrap();
        let stdlib = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(dir.join("hirsel.json"), serde_json::json!({"schema":1,"package":{"name":"la_sieve_10","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}]}).to_string()).unwrap();
        std::fs::write(
            dir.join("grants.json"),
            r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}"#,
        )
        .unwrap();
        Self(dir)
    }
    fn run(&self, backend: Option<&str>) -> std::process::Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        cmd.arg("run")
            .arg(self.0.join("hirsel.json"))
            .arg("--grants")
            .arg(self.0.join("grants.json"));
        if let Some(backend) = backend {
            cmd.args(["--native", "--backend", backend]);
        }
        cmd.output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn expected(limits: &[u64]) -> String {
    let mut output = String::new();
    for &limit in limits {
        let prime = |n: u64| n >= 2 && !(2..n).any(|d| n % d == 0);
        let flags: String = (0..=limit)
            .map(|n| if prime(n) { '1' } else { '0' })
            .collect();
        let primes: Vec<_> = (2..=limit).filter(|&n| prime(n)).collect();
        let last = primes.last().unwrap();
        for _ in 0..4 {
            output.push_str(&format!("{limit}:{flags}:{last}:{}\n", primes.len()));
        }
        output.push_str(&format!("sum:{}\n", 4 * (last + primes.len() as u64)));
    }
    output
}
fn flags(backend: Option<&str>) {
    // Exhaustive bounded table plus square boundaries, including every brief vector.
    let mut limits: Vec<u64> = (2..=32).collect();
    limits.extend([48, 49, 50, 80, 81, 82, 120, 121, 122, 168, 169, 170, 1000]);
    let fixture = Fixture::new(&limits);
    let output = fixture.run(backend);
    assert!(output.status.success(), "{backend:?}: {output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected(&limits));
    assert!(output.stderr.is_empty(), "{output:?}");
    // Four identical complete flag rows prove initialization on every repeated
    // step; 1000 has one warmup plus three measured steps, (997+168)*4=4660.
    assert!(expected(&[1000]).ends_with("sum:4660\n"));
}
#[test]
fn la_sieve_10_flags_h() {
    flags(None);
}
#[test]
fn la_sieve_10_flags_llvm() {
    flags(Some("llvm"));
}
#[test]
fn la_sieve_10_flags_cranelift() {
    flags(Some("cranelift"));
}

#[test]
fn la_sieve_10_allocations() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native::{
            self, OptLevel, Options, Partition, RuntimeBuild, TargetSpec, Toolchain,
        };
        let fixture = Fixture::new(&[1000]);
        // Keep one checksum output but omit flag formatting from this diagnostic.
        let path = fixture.0.join("main.hir");
        let mut text = std::fs::read_to_string(&path).unwrap();
        let start = text.find("  let mut flags =").unwrap();
        let end = text.find("  state.value_out =").unwrap();
        text.replace_range(start..end, "");
        std::fs::write(&path, text).unwrap();
        let mut loaded = hirsel_driver::load_program(&fixture.0.join("hirsel.json")).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let sources = native::profile_sources(&input);
        let validated = loaded.engine.import_native_input(input).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let program = native::build(&validated, &Options::new(target.clone())).unwrap();
        let toolchain = Toolchain::in_tree_with_runtime(
            &fixture.0.join("runtime"),
            &[&target],
            RuntimeBuild {
                measurement: true,
                ..RuntimeBuild::default()
            },
        )
        .unwrap();
        std::fs::create_dir(fixture.0.join("measured")).unwrap();
        let objects = native::emit_objects(
            &program,
            OptLevel::Speed,
            Partition::Whole,
            true,
            Some(&sources),
        )
        .unwrap();
        let exe = native::link_standalone_objects(
            &program,
            &loaded.entry,
            &objects,
            &toolchain,
            &fixture.0.join("measured"),
        )
        .unwrap();
        let report = fixture.0.join("counts");
        let output = Command::new(exe)
            .arg("--hirsel-grants")
            .arg(fixture.0.join("grants.json"))
            .env("HIRSEL_RT_MEASUREMENT", &report)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"sum:4660\n");
        let profile = std::fs::read_to_string(report.with_extension("sites.json")).unwrap();
        println!("SIEVE_ALLOCATIONS {profile}");
        let profile: serde_json::Value = serde_json::from_str(&profile).unwrap();
        // Retain only byte-list backing allocations, attributed to step's
        // reserve/push site; runtime startup and checksum formatting stay separate.
        let sites: Vec<_> = profile["sites"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|site| {
                site["function"]
                    .as_str()
                    .is_some_and(|f| f.ends_with("step"))
                    && site["counts"]["allocations"].as_u64().unwrap_or(0) > 0
            })
            .collect();
        assert!(!sites.is_empty(), "{profile}");
        let allocations: u64 = sites
            .iter()
            .map(|s| s["counts"]["allocations"].as_u64().unwrap())
            .sum();
        println!("SIEVE_BACKING_ALLOCATIONS {allocations}");
        // Four fresh steps each reserve one backing block; no growth allocation.
        assert_eq!(allocations, 4);
    });
}
