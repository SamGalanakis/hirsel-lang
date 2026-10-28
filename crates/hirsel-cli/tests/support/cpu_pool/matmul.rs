use std::{fs, process::Command};
include!("matmul_stages.rs");

static COMPILER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn quick_matmul(width: u32) {
    let _compiler = COMPILER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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

fn every_cell(width: u32) {
    let _compiler = COMPILER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    for (name, source) in [
        (
            "matmul_parallel.hir",
            include_str!("../../../../../bench/langarena/matmul_parallel.hir"),
        ),
        (
            "harness.hir",
            include_str!("../../../../../bench/langarena/harness.hir"),
        ),
        (
            "cpu_harness.hir",
            include_str!("../../../../../bench/langarena/cpu_harness.hir"),
        ),
    ] {
        let source = if name == "matmul_parallel.hir" {
            matmul_stage_probes(source)
        } else {
            source.to_owned()
        };
        fs::write(root.join(name), source).unwrap();
    }
    let mut sizes = vec![1, width.saturating_sub(1), width, width + 1, 7];
    sizes.sort_unstable();
    sizes.dedup();
    let source = include_str!("cells.hir")
        .replace("WIDTH", &format!("{width}u32"))
        .replace(
            "SIZES",
            &format!(
                "[{}]",
                sizes
                    .iter()
                    .map(|n| format!("{n}u64"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        );
    fs::write(root.join("main.hir"), source).unwrap();
    let manifest = root.join("hirsel.json");
    let std = std::path::Path::new("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_cells","version":"1"},
        "sources":[{"path":"main.hir","module":[]},{"path":"matmul_parallel.hir","module":["matmul_parallel"]},
          {"path":"harness.hir","module":["harness"]},{"path":"cpu_harness.hir","module":["cpu_harness"]}],
        "dependencies":[{"manifest":std,"package":{"name":"std","version":"1"},"expose":[]}],
        "entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(
        &grants,
        serde_json::json!({"io":{"dirs":[],"tasks":"tasks.free@1",
        "cpu":{"provider":"cpu.scoped@1","max_workers":width,"max_domains":1,
         "max_physical_bytes":1073741824u64,"max_batch_bytes":16777216}}})
        .to_string(),
    )
    .unwrap();
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        for mut command in [
            {
                let mut build = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                build
                    .args(["build", "--validate-stages", "--backend", backend])
                    .arg(&manifest)
                    .arg("-o")
                    .arg(&executable);
                build
            },
            {
                let mut run = Command::new(&executable);
                run.arg("--hirsel-grants")
                    .arg(&grants)
                    .env("HIRSEL_RT_CPU_PROBE", root.join("stages.probe"));
                run
            },
        ] {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{backend} W={width} sizes={sizes:?}: {output:?}"
            );
        }
        matmul_job_probes(root, width, sizes.len());
        eprintln!(
            "CPU transpose/output-init/map/join/assembly invariants and every-cell {backend} W={width}, sizes={sizes:?}, cells={}",
            sizes.iter().map(|n| n * n).sum::<u32>()
        );
    }
}
