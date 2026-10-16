//! Native source attribution against a hand-counted program, not compiler IR dumps.
use hirsel_driver::native::{
    self, OptLevel, Options, Partition, RuntimeBuild, TargetSpec, Toolchain,
};
use serde_json::Value;

#[test]
fn two_source_lines_have_three_and_five_count_pairs_and_dwarf() {
    hirsel_engine::with_compiler_stack(|| {
        let dir =
            std::env::temp_dir().join(format!("hirsel-source-profile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("known.hir");
        std::fs::write(&source, "fn first(b: Bytes) -> () { let x = copy b }\nfn second(b: Bytes) -> () { let y = copy b }\npub fn main() -> Result[(), ()] {\n  for i in 0u64..3u64 { first(b\"a\") }\n  for j in 0u64..5u64 { second(b\"b\") }\n  Ok(())\n}\n").unwrap();
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
        let exe = native::link_profile(
            &ready,
            &loaded.entry,
            OptLevel::Speed,
            Partition::Whole,
            &tools,
            &dir,
            Some(&sources),
        )
        .unwrap();
        let json = dir.join("profile.json");
        let result = std::process::Command::new(&exe)
            .env_clear()
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
        for (line, count) in [(1, 3), (2, 5)] {
            for kind in ["count_increments", "count_decrements"] {
                let sum: u64 = report["sites"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|s| s["line"] == line && s["file"] == source.to_str().unwrap())
                    .map(|s| s["counts"][kind].as_u64().unwrap_or(0))
                    .sum();
                assert_eq!(sum, count, "line {line} {kind}: {report}");
            }
        }
        let dwarfdump = std::env::var_os("LLVM_SYS_211_PREFIX")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/usr/lib/llvm-21".into())
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
        std::fs::remove_dir_all(dir).unwrap();
    });
}
