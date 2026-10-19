//! The memcheck archive's client requests, end to end. The native memcheck campaign sees a
//! runtime-owned block only through the archive's requests: a slab slot is a mempool chunk
//! from allocation to free. A C program linked against that archive and launched as the
//! campaign launches programs (`Toolchain::under_memcheck`) reads a small block after its
//! release and must fail with memcheck's exit code, and its twin, which stops reading before
//! the release, must exit 0. A refactor that drops a request (`free_small`'s `pool_free`, for
//! one) leaves the freed slot addressable and fails the first test.
use hirsel_driver::native::{RuntimeBuild, TargetSpec, Toolchain, launch};
use std::path::PathBuf;
use std::process::Command;

/// Allocates an 8-byte block of an empty layout from a small class, writes and reads it,
/// releases it and, with `USE_AFTER_FREE`, reads it once more. Natively both exit 0.
const PROGRAM: &str = r#"
#include "hirsel_rt_v2.h"

static const hirsel_rt_v2_layout empty = {HIRSEL_RT_V2_LAYOUT_MAGIC, HIRSEL_RT_V2_LAYOUT_VERSION, 0, 8, 0, 0, 0, 0, 0, 0};
static const hirsel_rt_v2_layout *const registry[] = {&empty};
/* Registry entry i is descriptor index i + 2. */
#define EMPTY 2

int main(void) {
    hirsel_rt_v2_config config = {.abi_version = HIRSEL_RT_V2_ABI_VERSION, .layouts = registry, .layout_count = 1};
    hirsel_rt_v2_ctx *ctx = 0;
    if (hirsel_rt_v2_ctx_create(&config, &ctx) != HIRSEL_RT_V2_LAUNCH_OK) return 2;
    hirsel_rt_v2_header *block = hirsel_rt_v2_alloc(ctx, EMPTY, 8, 8, HIRSEL_RT_V2_DOMAIN_HEAP, 0);
    volatile unsigned char *payload = hirsel_rt_v2_payload(block);
    payload[0] = 1;
    if (payload[0] != 1) return 3;
    if (hirsel_rt_v2_release(ctx, block, EMPTY, 0, 0, 0) != HIRSEL_RT_V2_STATUS_OK) return 4;
#ifdef USE_AFTER_FREE
    unsigned char after = payload[0];
    (void)after;
#endif
    hirsel_rt_v2_ctx_destroy(ctx);
    return 0;
}
"#;

/// The exit code of `PROGRAM` linked against the x86-64 memcheck archive and launched
/// under memcheck as the campaign launches it.
fn memcheck_exit(name: &str, defines: &[&str]) -> Option<i32> {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("memcheck-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let tools = Toolchain::in_tree_with_runtime(
        &PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-runtime"),
        &[&target],
        RuntimeBuild {
            memcheck: true,
            ..Default::default()
        },
    )
    .unwrap()
    .under_memcheck()
    .unwrap();
    let source = dir.join("program.c");
    std::fs::write(&source, PROGRAM).unwrap();
    let executable = dir.join("program");
    let object = dir.join("program.o");
    let include = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hirsel-rt/include");
    let output = Command::new(&tools.clang)
        .args(["-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-c"])
        // The launcher passes its linker, which a compile does not use.
        .args(["-Wno-unused-command-line-argument", "-I"])
        .arg(&include)
        .args(defines.iter().map(|d| format!("-D{d}")))
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    tools
        .link_executable(&target, &[object], &executable)
        .unwrap();
    let launched = launch(&executable, &target, &tools, &dir, false).unwrap();
    eprintln!("{}", String::from_utf8_lossy(&launched.stderr));
    launched.exit
}

#[test]
fn memcheck_reports_a_read_of_a_released_slab_block() {
    assert_eq!(
        memcheck_exit("use-after-free", &["USE_AFTER_FREE"]),
        Some(97)
    );
}

#[test]
fn memcheck_passes_the_same_block_read_only_before_its_release() {
    assert_eq!(memcheck_exit("clean", &[]), Some(0));
}
