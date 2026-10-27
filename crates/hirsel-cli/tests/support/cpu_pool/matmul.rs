use std::{fs, process::Command};

fn quick_matmul(width: u32) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let (name, n, expected) = match width {
        4 => ("Matmul::T4", 42, 2420334760u32),
        8 => ("Matmul::T8", 44, 4014223764),
        16 => ("Matmul::T16", 46, 418055336),
        _ => unreachable!(),
    };
    fs::write(
        root.join("matmul_parallel.hir"),
        include_str!("../../../../../bench/langarena/matmul_parallel.hir"),
    )
    .unwrap();
    fs::write(
        root.join("harness.hir"),
        include_str!("../../../../../bench/langarena/harness.hir"),
    )
    .unwrap();
    fs::write(
        root.join("cpu_harness.hir"),
        include_str!("../../../../../bench/langarena/cpu_harness.hir"),
    )
    .unwrap();
    fs::write(
        root.join("main.hir"),
        format!(
            r#"
pub fn main(rt: Io) -> Result[(), String] {{
  let mut config: harness.Config = map.empty()
  map.insert(mut config, "n", Number("{n}"))
  map.insert(mut config, "iterations", Number("3"))
  map.insert(mut config, "checksum", Number("{expected}"))
  let report = matmul_parallel.execute(rt, "{name}", config)?
  failure.check(report.actual == report.expected, Bounds())
  Ok(())
}}
"#
        ),
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    let std = std::path::Path::new("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_matmul","version":"1"},
        "sources":[{"path":"main.hir","module":[]},{"path":"matmul_parallel.hir","module":["matmul_parallel"]},
          {"path":"harness.hir","module":["harness"]},{"path":"cpu_harness.hir","module":["cpu_harness"]}],
        "dependencies":[{"manifest":std,"package":{"name":"std","version":"1"},"expose":[]}],
        "entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"io":{"dirs":[],"monotonic_clock":"clock.monotonic.ns@1", "tasks":"tasks.free@1",
        "cpu":{"provider":"cpu.scoped@1","max_workers":width,"max_domains":1,
         "max_physical_bytes":1073741824u64,"max_batch_bytes":16777216}}}).to_string()).unwrap();
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        let compiled = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["build", "--validate-stages", "--backend", backend])
            .arg(&manifest)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{backend} build: {}\n{}",
            String::from_utf8_lossy(&compiled.stdout),
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut run = Command::new(&executable);
        run.arg("--hirsel-grants").arg(&grants);
        use std::os::unix::process::CommandExt;
        unsafe {
            run.pre_exec(|| {
                for (resource, limit) in [(libc::RLIMIT_CPU, 60), (libc::RLIMIT_AS, 2 << 30)] {
                    if libc::setrlimit(
                        resource,
                        &libc::rlimit {
                            rlim_cur: limit,
                            rlim_max: limit,
                        },
                    ) != 0
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let executed = run.output().unwrap();
        assert!(
            executed.status.success(),
            "{name} {backend}: {}\n{}",
            String::from_utf8_lossy(&executed.stdout),
            String::from_utf8_lossy(&executed.stderr)
        );
        eprintln!("{name} {backend}: n={n}, iterations=3, warmup=1, checksum={expected}");
    }
}
