//! Native source attribution against a hand-counted program, not compiler IR dumps.
use hirsel_driver::native::{
    self, Backend, OptLevel, Options, Partition, RuntimeBuild, TargetSpec, Toolchain,
};
use serde_json::Value;

#[test]
fn two_source_lines_have_three_and_five_count_pairs_and_dwarf() {
    check_source_profile(Backend::Llvm);
}

#[test]
fn cranelift_two_source_lines_have_three_and_five_count_pairs_and_dwarf() {
    check_source_profile(Backend::Cranelift);
}

fn check_source_profile(backend: Backend) {
    hirsel_engine::with_compiler_stack(|| {
        let dir = std::env::temp_dir().join(format!(
            "hirsel-source-profile-{}-{}",
            std::process::id(),
            backend.name()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let source = dir.join("known.hir");
        std::fs::write(&source, "fn first(b: Bytes) -> () { let x = copy b }\nfn second(b: Bytes) -> () { let y = copy b }\npub fn main(rt: Io) -> Result[(), GrantError] {\n  for i in 0u64..3u64 { first(b\"a\") }\n  for j in 0u64..5u64 { second(b\"b\") }\n  let tasks = io.tasks(rt)?\n  let stdout = io.stdout(rt)?\n  task.scope(tasks, fn(s: Scope) {\n    let mut set: TaskSet[u64] of s = task.set(s)\n    for i in 0u64..6u64 {\n      task.add(s, mut set, task.async(s, fn() -> u64 { i + 1u64 }))\n    }\n    task.cancel_all(s, take set)\n  })\n  let _ = out.print(stdout, \"done\\n\")\n  Ok(())\n}\n").unwrap();
        let mut loaded = hirsel_driver::load_program(&source).unwrap();
        loaded.check_for_execution().unwrap();
        let snapshot = loaded.engine.export_native_input().unwrap();
        let sources = native::profile_sources(&snapshot);
        let validated = loaded.engine.import_native_input(snapshot).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let mut options = Options::new(target.clone());
        options.elide = false;
        options.reuse = false;
        options.regions = false;
        let ready = native::build(&validated, &options).unwrap();
        let tools = Toolchain::in_tree_with_runtime(
            &dir.join("runtime"),
            &[&target],
            RuntimeBuild {
                measurement: true,
                ..Default::default()
            },
        )
        .unwrap();
        let objects = native::emit_backend_objects(
            &ready,
            backend,
            if backend == Backend::Llvm {
                OptLevel::Speed
            } else {
                OptLevel::None
            },
            Partition::Whole,
            true,
            Some(&sources),
        )
        .unwrap();
        let exe = native::link_objects(&ready, &loaded.entry, &objects, &tools, &dir).unwrap();
        let json = dir.join("profile.json");
        let result = std::process::Command::new(&exe)
            .env_clear()
            .env("HIRSEL_RT_ROOT", &root)
            .env("HIRSEL_RT_PROFILE", &json)
            .env("HIRSEL_RT_MEASUREMENT", dir.join("totals"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
        for (kind, total) in report["totals"].as_object().unwrap() {
            let sum: u64 = report["sites"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["counts"][kind].as_u64().unwrap_or(0))
                .sum();
            assert_eq!(sum, total.as_u64().unwrap(), "{kind}");
        }
        if backend == Backend::Llvm {
            for (line, count) in [(1, 3), (2, 5)] {
                for kind in ["count_increments", "count_decrements"] {
                    let sum: u64 = report["sites"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|s| s["line"] == line && s["file"] == "standalone@0/known.hir")
                        .map(|s| s["counts"][kind].as_u64().unwrap_or(0))
                        .sum();
                    assert_eq!(sum, count, "line {line} {kind}: {report}");
                }
            }
        } else {
            // Cranelift currently emits aggregate runtime counters, without
            // LLVM's ProfileEnter/ProfileLeave site instrumentation. Keep a
            // native profile control here and verify source mapping via DWARF
            // below; per-site counters require a separate backend feature.
            assert_eq!(report["totals"]["count_increments"], 8);
            assert_eq!(report["totals"]["count_decrements"], 30);
            assert_eq!(report["sites"].as_array().unwrap().len(), 1);
            assert_eq!(report["sites"][0]["id"], 0);
        }
        let dwarfdump = std::env::var_os("LLVM_SYS_211_PREFIX")
            .map(std::path::PathBuf::from)
            .expect("LLVM_SYS_211_PREFIX names the pinned toolchain input")
            .join("bin/llvm-dwarfdump");
        let dwarf = std::process::Command::new(&dwarfdump)
            .args(["--verify", exe.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            dwarf.status.success(),
            "{}",
            String::from_utf8_lossy(&dwarf.stdout)
        );
        let lines = std::process::Command::new(&dwarfdump)
            .args(["--debug-line", exe.to_str().unwrap()])
            .output()
            .unwrap();
        let text = String::from_utf8(lines.stdout).unwrap();
        assert!(text.contains("known.hir"), "{text}");
        for line in [1, 2] {
            assert!(
                text.lines().any(|row| row.starts_with("0x")
                    && row.split_whitespace().nth(1) == Some(&line.to_string())),
                "line {line}: {text}"
            );
        }
        // The same executable and fixture, with the segment profiler toggled. Keep
        // tool destinations separate while asserting the program bytes and tape agree.
        let mut observations = vec![];
        for enabled in [false, true] {
            let tape = dir.join(format!("tape-{enabled}"));
            let measurement = dir.join(format!("segments-{enabled}"));
            let mut command = std::process::Command::new(&exe);
            command
                .env_clear()
                .env("HIRSEL_RT_ROOT", &root)
                .env("HIRSEL_RT_TAPE", &tape)
                .env("HIRSEL_RT_MEASUREMENT", &measurement);
            if enabled {
                command.env("HIRSEL_RT_MC_PROFILE", "1");
            }
            let result = command.output().unwrap();
            assert!(result.status.success(), "{:?}", result);
            assert_eq!(result.stdout, b"done\n");
            observations.push((result.stdout, result.stderr, std::fs::read(tape).unwrap()));
            let text = std::fs::read_to_string(measurement).unwrap();
            assert_eq!(text.contains("mc-profile 1\n"), enabled);
            if enabled {
                let rows: Vec<Value> = text
                    .lines()
                    .filter_map(|line| line.strip_prefix("mc "))
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                assert_eq!(
                    rows.iter()
                        .filter(|r| r["operation"] == "task_spawn")
                        .count(),
                    6
                );
                let tasks: std::collections::BTreeSet<_> =
                    rows.iter().map(|r| r["task"].to_string()).collect();
                assert_eq!(tasks.len(), 7);
                for (i, row) in rows.iter().enumerate() {
                    assert!(row["after"].as_u64().unwrap() <= i as u64);
                    assert!(row["ns"].as_u64().is_some());
                }
            }
        }
        assert_eq!(
            observations[0], observations[1],
            "profiling changed stdout, stderr or tape"
        );
        std::fs::remove_dir_all(dir).unwrap();
    });
}
