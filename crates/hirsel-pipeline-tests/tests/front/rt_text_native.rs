//! Compiled text calls, failure anchors and generated drop glue on both targets.
use hirsel_driver::native::{
    Observation, OptLevel, Options, Outcome, Toolchain, build, c_call, differential, emit,
    full_matrix,
};
use hirsel_engine::{
    RunConfiguration, SimulationConfig, default_context, entry_path, run_observed_snapshot,
    with_compiler_stack,
};
use hirsel_lir::layout_types::TargetSpec;
use std::path::Path;
use std::process::Command;

fn run(command: &mut Command) -> Vec<u8> {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
const SUCCESS: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let decimal = std@1/u64.to_string(18446744073709551615u64)
  let replaced = std@1/bytes.replace(b"aaaaa", b"aa", b"x")
  let decoded = string.from_bytes(take replaced) == Ok("xxa")
  let invalid_source: Bytes = b"\xff"
  let invalid = string.from_bytes(copy invalid_source) == Error(invalid_source)
  let s: String = "é😀x"
  let mut valid: bool = true
  for pos in string.positions(s) {
    let empty = string.slice(s, pos, pos)
    valid = valid && string.byte_len(empty) == 0
  }
  let _ = out.print(io.stdout(rt)? grant_error, if decimal == "18446744073709551615" && decoded && invalid && valid { "ok\n" } else { "bad\n" })
  Ok(())
}

@noalloc
fn grant_error(refusal: GrantError) -> () uses {} { () }
"#;
const REPLACE_FAIL: &str = r#"
resource type R { bytes: String }
drop(r) { let _ = 1u64 / 0u64; () }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let r = R(bytes: std@1/u64.to_string(7))
  let src = std@1/bytes.replace(b"123", b"1", b"x")
  let _ = out.print(io.stdout(rt)?, "before\n")
  let replaced = std@1/bytes.replace(src, b"", b"x")
  Ok(())
}
"#;
const SLICE_FAIL: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let s = std@1/u64.to_string(12)
  for start in string.positions(s) {
    for end in string.positions(s) {
      let part = string.slice(s, start, end)
    }
  }
  Ok(())
}
"#;
const DROP: &str = r#"
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let s = std@1/u64.to_string(18446744073709551615u64)
  Ok(())
}
"#;

fn checked(
    source: &str,
) -> (
    hirsel_engine::ValidatedNativeInput,
    hirsel_syntax::DefPath,
    Observation,
) {
    let mut loaded = hirsel_driver::load_program_source(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../hirsel-driver/tests/fixtures/std_numeric.hir"),
        Some(source),
    )
    .unwrap();
    loaded.check_for_execution().unwrap();
    let mut engine = loaded.engine;
    let context = default_context();
    let entry = entry_path(&context);
    let execution = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
    )
    .unwrap();
    let reference = Observation::of_reference(execution.observation.as_ref().unwrap());
    let input = engine.export_native_input().unwrap();
    (engine.import_native_input(input).unwrap(), entry, reference)
}

fn compile(
    tools: &Toolchain,
    target: &TargetSpec,
    source: &Path,
    object: Option<&Path>,
    executable: &Path,
    sanitize: bool,
) {
    let arm = target.triple.starts_with("aarch64");
    let mut command = Command::new(if arm { &tools.aarch64_cc } else { &tools.clang });
    command.args([
        "-std=c11",
        "-DHIRSEL_RT_ENABLE_STATS",
        "-O1",
        "-g",
        "-Wall",
        "-Wextra",
        "-Werror",
    ]);
    if sanitize && !arm {
        command.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
    }
    command.arg(source);
    if let Some(object) = object {
        command.arg("-I").arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("crates/hirsel-rt/include"),
        );
        command.arg("-include").arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("crates/hirsel-llvm/tests/native/fixture.h"),
        );
        command.arg(object);
    }
    command
        .arg(tools.archive(&target.triple).unwrap())
        .args([
            "-lgcc_s",
            "-lutil",
            "-lrt",
            "-lpthread",
            "-lm",
            "-ldl",
            "-o",
        ])
        .arg(executable);
    run(&mut command);
}
fn execute(tools: &Toolchain, target: &TargetSpec, executable: &Path) {
    if target.triple.starts_with("aarch64") {
        run(Command::new(&tools.qemu_aarch64)
            .arg("-L")
            .arg(&tools.aarch64_sysroot)
            .arg(executable));
    } else {
        run(Command::new(executable)
            .env("ASAN_OPTIONS", "detect_leaks=1")
            .env("UBSAN_OPTIONS", "halt_on_error=1"));
    }
}

#[test]
#[ignore = "full gate"]
fn full_gate_text_calls_failures_c_abi_and_compiled_drops_agree_on_both_targets() {
    with_compiler_stack(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("text-native");
        std::fs::create_dir_all(&scratch).unwrap();
        let matrix = full_matrix();
        let tools = Toolchain::for_matrix(&scratch.join("runtime"), &matrix).unwrap();
        for (name, source) in [
            ("success", SUCCESS),
            ("replace-failure", REPLACE_FAIL),
            ("slice-failure", SLICE_FAIL),
        ] {
            let (input, entry, reference) = checked(source);
            if name == "success" {
                assert_eq!(reference.output, b"ok\n");
            } else {
                assert_eq!(reference.failures[0].rule, "SEM-3");
            }
            let work = scratch.join(name);
            std::fs::create_dir_all(&work).unwrap();
            for (configuration, outcome) in
                differential(&input, &entry, &reference, &matrix, &tools, &work)
            {
                assert_eq!(outcome, Outcome::Agree, "{name}: {configuration}");
            }
        }
        let (input, entry, _) = checked(DROP);
        for target in [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ]
        .iter()
        {
            let probe = scratch.join(format!("text-c-{}", target.triple));
            compile(
                &tools,
                target,
                &root.join("native-tests/library/text.c"),
                None,
                &probe,
                true,
            );
            execute(&tools, target, &probe);
            for opt in [OptLevel::None, OptLevel::Speed] {
                let program = build(
                    &input,
                    &Options {
                        regions: false,
                        ..Options::new(target.clone())
                    },
                )
                .unwrap();
                let object = emit(&program, opt, |_| true).unwrap();
                assert!(
                    !object
                        .imports
                        .iter()
                        .any(|s| s.name == "hirsel_rt_v2_bytes_from_list")
                );
                let symbol = &object
                    .exports
                    .iter()
                    .find(|s| s.definition.as_ref() == Some(&entry))
                    .unwrap()
                    .name;
                let unit = program
                    .units
                    .iter()
                    .map(|u| u.unit())
                    .find(|u| u.header.definition == entry)
                    .unwrap();
                let abi = &program.table.fn_abis[unit.header.abi.0 as usize];
                let call = c_call(&program.table, abi, symbol, "call_main");
                let object_path = scratch.join("drop.o");
                std::fs::write(&object_path, object.bytes).unwrap();
                let harness = scratch.join("drop.c");
                let header = root.join("stdlib/native/library.h");
                std::fs::write(
                    &harness,
                    format!(
                        r#"
#include "{}"
#include <assert.h>
{}
int main(void) {{
  for (uint32_t flags = 0; flags <= HIRSEL_RT_V2_CONFIG_DIAGNOSTIC; flags++) {{
    hirsel_rt_v2_config config = {{ .abi_version=HIRSEL_RT_V2_ABI_VERSION, .flags=flags }};
    hirsel_rt_v2_ctx *ctx = NULL; assert(hirsel_rt_v2_ctx_create(&config, &ctx) == 0);
    uint64_t io=1, args=2;
    for (int round=0; round < 1000; round++) {{
      uint32_t result=99; assert(call_main(ctx, NULL, &result, &io, &args) == 0 && result == 0);
      hirsel_rt_v2_stats stats; hirsel_rt_v2_ctx_stats(ctx, &stats); assert(stats.live_blocks == 0 && stats.backing_allocations > 0);
    }}
    hirsel_rt_v2_audit audit; hirsel_rt_v2_task_exit(ctx, &audit);
    assert(audit.live_blocks == 0 && audit.retained_bytes == 0); hirsel_rt_v2_ctx_destroy(ctx);
  }}
}}
"#,
                        header.display(),
                        call
                    ),
                )
                .unwrap();
                let executable = scratch.join("drop");
                compile(
                    &tools,
                    target,
                    &harness,
                    Some(&object_path),
                    &executable,
                    true,
                );
                execute(&tools, target, &executable);
            }
        }
    });
}
