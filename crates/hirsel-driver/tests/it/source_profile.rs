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
        let dir_scratch = tempfile::Builder::new()
            .prefix("hirsel-source-profile-")
            .tempdir()
            .unwrap();
        let dir = dir_scratch.path().to_path_buf();
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
        let tools = Toolchain::in_tree_with_runtimes(
            &dir.join("runtime"),
            &[&target],
            &[
                RuntimeBuild {
                    measurement: true,
                    ..Default::default()
                },
                RuntimeBuild::default(),
            ],
        )
        .unwrap();
        let objects = native::emit_backend_objects(
            &ready,
            backend,
            OptLevel::None,
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
        profile_evidence(backend, &objects, &exe, &report);
        for (kind, total) in report["totals"].as_object().unwrap() {
            let sum: u64 = report["sites"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["counts"][kind].as_u64().unwrap_or(0))
                .sum();
            assert_eq!(sum, total.as_u64().unwrap(), "{kind}");
        }
        assert_eq!(report["totals"]["count_increments"], 8);
        assert_eq!(report["totals"]["count_decrements"], 30);
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
        let dwarfdump = std::env::var_os("LLVM_SYS_231_PREFIX")
            .map(std::path::PathBuf::from)
            .expect("LLVM_SYS_231_PREFIX names the pinned toolchain input")
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
        if backend == Backend::Cranelift {
            let normal = native::emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                Some(&sources),
            )
            .unwrap();
            for object in &normal {
                assert!(object.imports.iter().all(|s| !s.name.contains("profile_")));
                assert!(
                    !object
                        .bytes
                        .windows(b"hirsel.profile.site.".len())
                        .any(|s| s == b"hirsel.profile.site.")
                );
            }
            let clif = native::emit_clif(&ready).unwrap();
            assert!(!clif.contains("hirsel_rt_v2_profile_"), "{clif}");
            // The fixture run binds its mount to the link directory. Keep that
            // directory identical, so tape identity stays identical as well.
            let measured_exe = dir.join("measured");
            std::fs::rename(&exe, &measured_exe).unwrap();
            let normal_exe = native::link_objects(
                &ready,
                &loaded.entry,
                &normal,
                &tools.with_runtime(RuntimeBuild::default()),
                &dir,
            )
            .unwrap();
            let tape = dir.join("tape-normal");
            let output = std::process::Command::new(&normal_exe)
                .env_clear()
                .env("HIRSEL_RT_ROOT", &root)
                .env("HIRSEL_RT_TAPE", &tape)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                (output.stdout, output.stderr, std::fs::read(&tape).unwrap()),
                observations[0]
            );
            for executable in [&measured_exe, &normal_exe] {
                let replay = std::process::Command::new(executable)
                    .env_clear()
                    .env("HIRSEL_RT_REPLAY", &tape)
                    .output()
                    .unwrap();
                assert!(replay.status.success(), "{replay:?}");
                assert_eq!(
                    (&replay.stdout, &replay.stderr),
                    (&observations[0].0, &observations[0].1)
                );
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    });
}

/// An outer runtime marker makes a missing leave observable even when the next
/// compiled operation would enter another site and mask the leaked context.
#[test]
fn cranelift_nested_indirect_failure_and_returned_error_restore_the_outer_site() {
    check_profile_context(Backend::Cranelift);
}

#[test]
fn llvm_nested_indirect_failure_and_returned_error_restore_the_outer_site() {
    check_profile_context(Backend::Llvm);
}

fn check_profile_context(backend: Backend) {
    use hirsel_lir::descriptor_registry::DescriptorRegistry;
    use hirsel_lir::emission::symbol_name;
    use std::path::PathBuf;
    use std::process::Command;

    hirsel_engine::with_compiler_stack(|| {
        let dir_scratch = tempfile::Builder::new()
            .prefix("hirsel-profile-context-")
            .tempdir()
            .unwrap();
        let dir = dir_scratch.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("context.hir");
        std::fs::write(
            &source,
            r#"resource type Kept { value: Bytes }
drop(x) { let y = copy x.value }
fn leaf(b: Bytes, fail: bool) { let x = copy b; if fail { let z: u8 = 0; let _ = 1u8 / z } }
fn other(b: Bytes, fail: bool) { leaf(b, fail) }
fn returned(b: Bytes) -> Result[(), ()] { let x = copy b; Error(()) }
pub fn probe(fail: bool) {
  let kept = Kept(value: b"kept")
  let b = b"leaf"
  match returned(b) { Ok(()) => (), Error(()) => () }
  let f: fn(Bytes, bool) -> () = if fail { leaf } else { other }
  f(b, fail)
}
pub fn main() -> Result[(), ()] { probe(false); Ok(()) }
"#,
        )
        .unwrap();
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
        let objects = native::emit_backend_objects(
            &ready,
            backend,
            OptLevel::None,
            Partition::Whole,
            true,
            Some(&sources),
        )
        .unwrap();
        let clif = native::emit_clif(&ready).unwrap();
        assert!(clif.contains("call_indirect"), "{clif}");
        let registry = DescriptorRegistry::new(&ready.table, &ready.finalizers);
        let probe = ready
            .units
            .iter()
            .map(|u| u.unit())
            .find(|u| u.header.definition.0.name == "probe")
            .unwrap();
        let symbol = symbol_name(&probe.header.definition, Some(&probe.header.instance));
        let c = dir.join("context.c");
        std::fs::write(
            &c,
            CONTEXT_HARNESS
                .replace("PROBE_SYMBOL", &symbol)
                .replace("REGISTRY_SYMBOL", &registry.symbol())
                .replace("REGISTRY_COUNT", &registry.external_count().to_string()),
        )
        .unwrap();
        for invalid in [0, u32::MAX] {
            let mut invalid_sources = sources.clone();
            let source = invalid_sources.get_mut(&probe.header.definition).unwrap();
            if invalid == 0 {
                source.anchors.clear(); // Mapped selector zero no longer exists.
            } else {
                source.anchors[0].1 = invalid; // Mapped byte range overflows its file.
            }
            let error = native::emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                true,
                Some(&invalid_sources),
            )
            .unwrap_err();
            assert!(
                error
                    .message
                    .contains("profile anchor is outside its source revision"),
                "{error}"
            );
        }
        let tools = Toolchain::in_tree_with_runtime(
            &dir.join("runtime"),
            &[&target],
            RuntimeBuild {
                measurement: true,
                ..Default::default()
            },
        )
        .unwrap();
        let c_object = dir.join("context.o");
        let compiled = Command::new(&tools.clang)
            .args([
                "-std=c11",
                "-O0",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Wno-unused-command-line-argument",
                "-DHIRSEL_RT_ENABLE_STATS",
                "-c",
                "-I",
            ])
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hirsel-rt/include"))
            .arg(c)
            .arg("-o")
            .arg(&c_object)
            .output()
            .unwrap();
        assert!(compiled.status.success(), "{compiled:?}");
        let mut paths = vec![c_object];
        for (i, object) in objects.iter().enumerate() {
            let path = dir.join(format!("unit-{i}.o"));
            std::fs::write(&path, &object.bytes).unwrap();
            paths.push(path);
        }
        let exe = dir.join("context");
        tools.link_executable(&target, &paths, &exe).unwrap();
        let output = native::launch(&exe, &target, &tools, &dir, false).unwrap();
        assert_eq!(output.exit, Some(0), "{output:?}");
        assert_eq!(output.stdout, b"3 restored contexts; no live blocks\n");
        // The launcher exposes the existing runtime report, including the drop
        // callback; the C control above also exercised its failed-call cleanup.
        let exe = native::link_objects(&ready, &loaded.entry, &objects, &tools, &dir).unwrap();
        let json = dir.join("profile.json");
        let output = Command::new(&exe)
            .env_clear()
            .env("HIRSEL_RT_PROFILE", &json)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let report: Value = serde_json::from_slice(&std::fs::read(json).unwrap()).unwrap();
        profile_evidence(backend, &objects, &exe, &report);
        assert_conservation(&report);
        for kind in ["count_increments", "count_decrements"] {
            let count: u64 = report["sites"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|s| s["file"] == "standalone@0/context.hir" && s["line"] == 2)
                .map(|s| s["counts"][kind].as_u64().unwrap_or(0))
                .sum();
            assert_eq!(count, 1, "finalizer {kind}: {report}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    });
}

const CONTEXT_HARNESS: &str = r#"
#include "hirsel_rt_v2.h"
#include <assert.h>
#include <stdio.h>
struct profile_site {
    uint64_t id;
    const char *package, *file, *function, *operation;
    uint32_t line, column;
};
extern uint64_t hirsel_rt_v2_profile_enter(hirsel_rt_v2_ctx *, const struct profile_site *);
extern void hirsel_rt_v2_profile_leave(hirsel_rt_v2_ctx *, uint64_t);
extern const hirsel_rt_v2_layout *const registry[] __asm__("REGISTRY_SYMBOL");
extern uint8_t probe(hirsel_rt_v2_ctx *, uint8_t) __asm__("PROBE_SYMBOL");
static const struct profile_site outer = {42, "control", "outer.c", "main", "sentinel", 1, 1};
int main(void) {
    hirsel_rt_v2_config cfg = {.abi_version = HIRSEL_RT_V2_ABI_VERSION,
        .layouts = registry, .layout_count = REGISTRY_COUNT};
    hirsel_rt_v2_ctx *ctx = NULL;
    assert(hirsel_rt_v2_ctx_create(&cfg, &ctx) == 0);
    assert(hirsel_rt_v2_profile_enter(ctx, &outer) == 0);
    const uint8_t fail[] = {0, 1, 0};
    for (unsigned i = 0; i < 3; i++) {
        assert(probe(ctx, fail[i]) == fail[i]);
        assert(hirsel_rt_v2_profile_enter(ctx, &outer) == outer.id);
        assert(hirsel_rt_v2_failure_active(ctx) == fail[i]);
        hirsel_rt_v2_stats stats;
        hirsel_rt_v2_ctx_stats(ctx, &stats);
        assert(stats.live_blocks == 0);
        hirsel_rt_v2_failure_clear(ctx);
    }
    hirsel_rt_v2_profile_leave(ctx, 0);
    assert(hirsel_rt_v2_profile_enter(ctx, &outer) == 0);
    hirsel_rt_v2_profile_leave(ctx, 0);
    hirsel_rt_v2_ctx_destroy(ctx);
    puts("3 restored contexts; no live blocks");
    return 0;
}
"#;

#[test]
fn cranelift_generated_and_scoped_task_sites_keep_their_source_coordinates() {
    check_scoped_profile(Backend::Cranelift);
}

#[test]
fn llvm_generated_and_scoped_task_sites_keep_their_source_coordinates() {
    check_scoped_profile(Backend::Llvm);
}

fn check_scoped_profile(backend: Backend) {
    hirsel_engine::with_compiler_stack(|| {
        let dir_scratch = tempfile::Builder::new()
            .prefix("hirsel-profile-scoped-")
            .tempdir()
            .unwrap();
        let dir = dir_scratch.path().to_path_buf();
        std::fs::create_dir_all(dir.join("root")).unwrap();
        let source = dir.join("scoped.hir");
        let text = r#"fn generator() -> Result[Body[fn(Bytes) -> () uses {}], String] { Ok(codegen.body("fn(b: Bytes) -> () uses {} { let x = copy b }")) }
fn generated(b: Bytes) -> () uses {} = comptime generator()
fn parent(b: Bytes) { let x = copy b }
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  task.scope(tasks, fn(s: Scope) {
    let child = task.async(s, fn() {
      for _ in 0u64..3u64 { generated(b"child") }
      let b = b"local"
      let x = copy b
    })
    parent(b"before")
    task.await(s, take child)
    parent(b"after")
  })
  Ok(())
}
"#;
        std::fs::write(&source, text).unwrap();
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
        let objects = native::emit_backend_objects(
            &ready,
            backend,
            OptLevel::None,
            Partition::Whole,
            true,
            Some(&sources),
        )
        .unwrap();
        let tools = Toolchain::in_tree_with_runtime(
            &dir.join("runtime"),
            &[&target],
            RuntimeBuild {
                measurement: true,
                ..Default::default()
            },
        )
        .unwrap();
        let exe = native::link_objects(&ready, &loaded.entry, &objects, &tools, &dir).unwrap();
        let json = dir.join("profile.json");
        let output = std::process::Command::new(&exe)
            .env_clear()
            .env("HIRSEL_RT_ROOT", dir.join("root"))
            .env("HIRSEL_RT_PROFILE", &json)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let report: Value = serde_json::from_slice(&std::fs::read(json).unwrap()).unwrap();
        profile_evidence(backend, &objects, &exe, &report);
        for (file, line, count) in [
            ("<generated:standalone@0/generated>", 1, 3),
            ("standalone@0/scoped.hir", 3, 2),
        ] {
            for kind in ["count_increments", "count_decrements"] {
                let sum: u64 = report["sites"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|s| s["file"] == file && s["line"] == line)
                    .map(|s| s["counts"][kind].as_u64().unwrap_or(0))
                    .sum();
                assert_eq!(sum, count, "{file}:{line} {kind}: {report}");
            }
        }
        // Copy retains the enclosing closure block's anchor; cleanup Release
        // names the local's declaration. These are the existing LLVM contracts.
        for (kind, line) in [("count_increments", 7), ("count_decrements", 10)] {
            let count: u64 = report["sites"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|s| s["file"] == "standalone@0/scoped.hir" && s["line"] == line)
                .map(|s| s["counts"][kind].as_u64().unwrap_or(0))
                .sum();
            assert_eq!(count, 1, "line {line} {kind}: {report}");
        }
        assert_eq!(report["totals"]["count_increments"], 6);
        assert_eq!(report["totals"]["count_decrements"], 15);
        assert_conservation(&report);
        std::fs::remove_dir_all(dir).unwrap();
    });
}

fn assert_conservation(report: &Value) {
    for (kind, count) in report["totals"].as_object().unwrap() {
        let sum: u64 = report["sites"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["counts"][kind].as_u64().unwrap_or(0))
            .sum();
        assert_eq!(sum, count.as_u64().unwrap(), "{kind}: {report}");
    }
}

fn profile_evidence(
    backend: Backend,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    exe: &std::path::Path,
    report: &Value,
) {
    use sha2::{Digest, Sha256};
    for (i, bytes) in objects.iter().map(|o| o.bytes.as_slice()).enumerate() {
        eprintln!(
            "{} object {i} sha256 {:x}",
            backend.name(),
            Sha256::digest(bytes)
        );
    }
    eprintln!(
        "{} executable sha256 {:x}",
        backend.name(),
        Sha256::digest(std::fs::read(exe).unwrap())
    );
    eprintln!("{} source profile: {report}", backend.name());
}
