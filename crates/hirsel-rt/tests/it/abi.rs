//! The `hirsel_rt_v2` C interface: header constants and record layouts agree with the Rust
//! definitions, every export is declared, launch statuses, and panics abort at the boundary.

use crate::support;

use std::collections::BTreeSet;
use std::mem::{offset_of, size_of};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr;

use hirsel_rt::context::{
    CONFIG_DIAGNOSTIC, LAUNCH_ABI_MISMATCH, LAUNCH_INVALID_CONFIG, LAUNCH_OK,
    hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy,
};
use hirsel_rt::*;
use support::*;

fn crate_file(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

/// `#define HIRSEL_RT_V2_<name> <value>` integer constants of the header.
fn header_constant(header: &str, name: &str) -> u64 {
    let key = format!("#define HIRSEL_RT_V2_{name} ");
    let line = header
        .lines()
        .find(|line| line.starts_with(&key))
        .unwrap_or_else(|| panic!("{name} is not defined"));
    let value = line[key.len()..].trim().trim_end_matches('u');
    match value {
        "UINT64_MAX" => u64::MAX,
        "UINT32_MAX" => u32::MAX as u64,
        _ if value.starts_with("0x") => u64::from_str_radix(&value[2..], 16).unwrap(),
        _ => value.parse().unwrap_or_else(|_| panic!("{name} = {value}")),
    }
}

#[test]
fn header_constants_match_the_runtime() {
    let header = crate_file("include/hirsel_rt_v2.h");
    let expected: &[(&str, u64)] = &[
        ("ABI_VERSION", ABI_VERSION as u64),
        ("STATUS_OK", STATUS_OK as u64),
        ("STATUS_FAILURE", STATUS_FAILURE as u64),
        ("LAUNCH_OK", LAUNCH_OK as u64),
        ("LAUNCH_ABI_MISMATCH", LAUNCH_ABI_MISMATCH as u64),
        ("LAUNCH_INVALID_CONFIG", LAUNCH_INVALID_CONFIG as u64),
        ("CONFIG_DIAGNOSTIC", CONFIG_DIAGNOSTIC as u64),
        ("HEADER_SIZE", HEADER_SIZE as u64),
        ("COUNT_IMMORTAL", COUNT_IMMORTAL as u64),
        ("CLASS_COUNT", CLASS_COUNT as u64),
        ("SMALL_MAX", SMALL_MAX),
        ("SMALL_ALIGN_MAX", SMALL_ALIGN_MAX),
        ("SLAB_SIZE", SLAB_SIZE as u64),
        ("LARGE_CLASS", LARGE_CLASS as u64),
        ("DOMAIN_HEAP", DOMAIN_HEAP as u64),
        ("DOMAIN_STATIC", DOMAIN_STATIC as u64),
        ("DOMAIN_REGION", DOMAIN_REGION as u64),
        ("STATE_FREE", STATE_FREE as u64),
        ("STATE_LIVE", STATE_LIVE as u64),
        ("STATE_FINALIZE_PENDING", STATE_FINALIZE_PENDING as u64),
        ("STATE_FINALIZING", STATE_FINALIZING as u64),
        ("STATE_FIELDS_PENDING", STATE_FIELDS_PENDING as u64),
        ("STATE_RAW", STATE_RAW as u64),
        ("FREE_POISON", alloc::FREE_POISON as u64),
        ("UNINIT_POISON", alloc::UNINIT_POISON as u64),
        ("LAYOUT_MAGIC", LAYOUT_MAGIC as u64),
        ("LAYOUT_VERSION", LAYOUT_VERSION as u64),
        ("LAYOUT_UNSAFE_FFI", LAYOUT_UNSAFE_FFI as u64),
        ("LAYOUT_DYNAMIC", LAYOUT_DYNAMIC as u64),
        ("LAYOUT_NO_EXTENT", LAYOUT_NO_EXTENT as u64),
        ("STEP_RELEASE", STEP_RELEASE as u64),
        ("STEP_FINALIZE", STEP_FINALIZE as u64),
        ("STEP_CLOSE_HANDLE", STEP_CLOSE_HANDLE as u64),
        ("STEP_TAG", STEP_TAG as u64),
        ("STEP_LOOP", STEP_LOOP as u64),
        ("STEP_COLUMN", STEP_COLUMN as u64),
        ("STEP_NONNULL", STEP_NONNULL as u64),
        ("STEP_LIVE_BIT", STEP_LIVE_BIT as u64),
        ("STEP_CURSOR_BITS", STEP_CURSOR_BITS as u64),
        ("WHOLE_VALUE", WHOLE_VALUE as u64),
        ("NO_OFFSET", NO_OFFSET),
        ("SEGMENT_SIZE", SEGMENT_SIZE as u64),
        ("REGION_POOL_IDLE_CAP", REGION_POOL_IDLE_CAP),
        ("CTX_DUP_MAX_OFFSET", CTX_DUP_MAX_OFFSET as u64),
        ("CTX_FREE_INLINE_OFFSET", CTX_FREE_INLINE_OFFSET as u64),
        ("CTX_IDLE_SLABS_OFFSET", CTX_IDLE_SLABS_OFFSET as u64),
        ("CTX_PAGES_OFFSET", CTX_PAGES_OFFSET as u64),
        ("POOL_COUNT", POOL_COUNT as u64),
        ("PAGE_FREE_OFFSET", PAGE_FREE_OFFSET as u64),
        ("PAGE_LIVE_OFFSET", PAGE_LIVE_OFFSET as u64),
        ("PAGE_LOCAL_FREE_OFFSET", PAGE_LOCAL_FREE_OFFSET as u64),
    ];
    for &(name, value) in expected {
        assert_eq!(header_constant(&header, name), value, "HIRSEL_RT_V2_{name}");
    }
    let classes: String = header
        .split("#define HIRSEL_RT_V2_CLASS_SIZES")
        .nth(1)
        .unwrap()
        .chars()
        .skip_while(|&c| c != '{')
        .take_while(|&c| c != '}')
        .collect();
    let classes: Vec<u32> = classes
        .replace(['{', '\\'], "")
        .split(',')
        .map(|item| item.trim().parse().unwrap())
        .collect();
    assert_eq!(classes, CLASS_SIZES);
}

/// `HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_<name>, <bytes>)` record sizes of the header.
fn header_size(header: &str, name: &str) -> usize {
    let key = format!("HIRSEL_RT_V2_ASSERT_SIZE(hirsel_rt_v2_{name}, ");
    let start = header
        .find(&key)
        .unwrap_or_else(|| panic!("no size assertion for {name}"))
        + key.len();
    let value = &header[start..start + header[start..].find(')').unwrap()];
    if value == "HIRSEL_RT_V2_HEADER_SIZE" {
        HEADER_SIZE
    } else {
        value.parse().unwrap()
    }
}

#[test]
fn record_layouts_match_the_header() {
    let header = crate_file("include/hirsel_rt_v2.h");
    let sizes = [
        ("header", size_of::<Header>()),
        ("step", size_of::<Step>()),
        ("layout", size_of::<Layout>()),
        ("cleanup", size_of::<Cleanup>()),
        ("region", size_of::<Region>()),
        ("builder", size_of::<Builder>()),
        ("config", size_of::<Config>()),
        ("stats", size_of::<Stats>()),
        ("audit", size_of::<Audit>()),
        ("failure", size_of::<FailureRecord>()),
    ];
    for (name, size) in sizes {
        assert_eq!(header_size(&header, name), size, "hirsel_rt_v2_{name}");
    }
    assert_eq!(
        [offset_of!(Header, count), offset_of!(Header, descriptor)],
        [0, 4]
    );
    assert_eq!(std::mem::align_of::<Header>(), 8);
    assert_eq!(
        offset_of!(Region, cursor) as u64,
        header_constant(&header, "REGION_CURSOR_OFFSET")
    );
    assert_eq!(
        offset_of!(Region, end) as u64,
        header_constant(&header, "REGION_END_OFFSET")
    );
    assert_eq!(offset_of!(Layout, finalizer), 24);
    assert_eq!(
        [
            offset_of!(Step, kind),
            offset_of!(Step, field),
            offset_of!(Step, variant),
            offset_of!(Step, span),
            offset_of!(Step, offset),
            offset_of!(Step, stride),
            offset_of!(Step, count),
            offset_of!(Step, count_offset),
            offset_of!(Step, layout),
        ],
        [0, 4, 8, 12, 16, 24, 32, 40, 48]
    );
    assert_eq!(offset_of!(Config, close_handle), 24);
    // LIR schema revision 7 appends the run's hash seed.
    assert_eq!(offset_of!(Config, hash_seed), 32);
    assert_eq!(size_of::<Config>(), 56);
    assert_eq!(offset_of!(Cleanup, value), 16);
}

fn command_output(command: &mut Command) -> String {
    let output = command
        .output()
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn compare_exports(exported: &BTreeSet<String>, declared: &BTreeSet<String>) -> Result<(), String> {
    let undeclared: Vec<_> = exported.difference(declared).collect();
    let missing: Vec<_> = declared.difference(exported).collect();
    if undeclared.is_empty() && missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "exported but undeclared: {undeclared:?}; declared but missing: {missing:?}"
        ))
    }
}

fn declared_functions(header: &str) -> BTreeSet<String> {
    // Preprocessing expands every scalar/conversion family. Preserve punctuation so
    // typedef names and function-pointer record fields cannot look like declarations.
    let mut tokens = Vec::new();
    let mut chars = header.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_alphanumeric() || c == '_' {
            let mut word = String::from(c);
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                word.push(chars.next().unwrap());
            }
            tokens.push(word);
        } else if !c.is_whitespace() {
            tokens.push(c.to_string());
        }
    }
    tokens
        .iter()
        .enumerate()
        .filter(|(i, token)| {
            token.starts_with("hirsel_rt_v2_")
                && (tokens.get(i + 1).is_some_and(|s| s == "(")
                    || (*i >= 3
                        && tokens[*i - 3] == "extern"
                        && tokens[*i - 2] == "const"
                        && tokens[*i - 1] == "hirsel_rt_v2_layout"
                        && tokens.get(i + 1).is_some_and(|s| s == ";")))
        })
        .map(|(_, token)| token.clone())
        .collect()
}

#[test]
fn export_comparison_rejects_both_directions() {
    let base = BTreeSet::from(["hirsel_rt_v2_present".to_string()]);
    let mut extra = base.clone();
    extra.insert("hirsel_rt_v2_extra".into());
    assert_eq!(compare_exports(&base, &base), Ok(()));
    assert!(
        compare_exports(&extra, &base)
            .unwrap_err()
            .contains("exported but undeclared: [\"hirsel_rt_v2_extra\"]")
    );
    assert!(
        compare_exports(&base, &extra)
            .unwrap_err()
            .contains("declared but missing: [\"hirsel_rt_v2_extra\"]")
    );
    assert_eq!(
        declared_functions(
            "typedef void (*hirsel_rt_v2_callback)(void); void hirsel_rt_v2_present (void);"
        ),
        base
    );
    assert_eq!(
        declared_functions(
            "typedef struct hirsel_rt_v2_layout hirsel_rt_v2_layout; \
             extern const hirsel_rt_v2_layout hirsel_rt_v2_bytes_descriptor;"
        ),
        BTreeSet::from(["hirsel_rt_v2_bytes_descriptor".to_string()])
    );
}

#[test]
fn every_export_is_declared_in_the_header_and_nothing_else_is() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let lock = std::fs::read_to_string(repo.join("tools/native-toolchain/llvm-21.lock")).unwrap();
    let locked = |key: &str| {
        lock.lines()
            .find_map(|line| line.strip_prefix(&format!("{key} ")))
            .unwrap()
    };
    // Same resolution as tools/native-toolchain/check: explicit environment prefix,
    // otherwise the lock's prefix; never PATH or an unpinned fallback.
    let prefix =
        PathBuf::from(std::env::var_os("LLVM_SYS_211_PREFIX").expect("LLVM_SYS_211_PREFIX"));
    let config = prefix.join("bin/llvm-config");
    assert_eq!(
        command_output(Command::new(&config).arg("--version")).trim(),
        locked("llvm_version")
    );
    let bindir = PathBuf::from(command_output(Command::new(&config).arg("--bindir")).trim());

    // Buck2 builds this checkout's production archive independently of the
    // diagnostic test harness. Do not invoke Cargo from a pool test action.
    let archive = if let Some(archives) = std::env::var_os("HIRSEL_RUNTIME_ARCHIVES") {
        PathBuf::from(archives).join("x86_64-unknown-linux-gnu/default/libhirsel_rt.a")
    } else {
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("abi-symbols");
        command_output(
            Command::new("cargo")
                .current_dir(repo)
                .args([
                    "build",
                    "--locked",
                    "--offline",
                    "-p",
                    "hirsel-rt",
                    "--lib",
                    "--target-dir",
                ])
                .arg(&target)
                .env_remove("CARGO_MAKEFLAGS")
                .env_remove("MAKEFLAGS"),
        );
        target.join("debug/libhirsel_rt.a")
    };
    assert!(
        archive.is_file(),
        "missing static archive: {}",
        archive.display()
    );
    // The separately built production archive must contain neither counter code nor
    // measurement environment parsing. Check private Rust symbols as well as C exports.
    let all_symbols = command_output(
        Command::new(bindir.join("llvm-nm"))
            .args(["--defined-only", "--demangle"])
            .arg(&archive),
    );
    assert!(!all_symbols.contains("hirsel_rt::measurement"));
    assert!(!all_symbols.contains("hirsel_rt_v2_measure_"));
    assert!(!all_symbols.contains("record_backing_allocation"));
    assert!(!all_symbols.contains("hirsel_rt::alloc::Heap::live"));
    assert!(!all_symbols.contains("hirsel_rt::context::RunCtx::exit_audit"));
    assert!(!all_symbols.contains("hirsel_rt_v2_ctx_stats"));
    let bytes = std::fs::read(&archive).unwrap();
    for forbidden in [
        b"HIRSEL_RT_MEASUREMENT".as_slice(),
        b"hirsel-measurement 1".as_slice(),
    ] {
        assert!(
            !bytes
                .windows(forbidden.len())
                .any(|window| window == forbidden)
        );
    }
    let symbols = command_output(
        Command::new(bindir.join("llvm-nm"))
            .args(["--defined-only", "--extern-only", "--format=posix"])
            .arg(archive),
    );
    assert!(
        !symbols
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .any(|symbol| symbol.starts_with("hirsel_rt_v1_")),
        "a retired ABI v1 export remains"
    );
    let exported: BTreeSet<_> = symbols
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|symbol| symbol.starts_with("hirsel_rt_v2_"))
        .map(str::to_string)
        .collect();
    // library.h includes hirsel_rt_v2.h, so this is the union of both interfaces.
    let header = command_output(
        Command::new(bindir.join("clang"))
            .args(["-E", "-P", "-x", "c"])
            .arg(repo.join("stdlib/native/library.h")),
    );
    let declared = declared_functions(&header);
    compare_exports(&exported, &declared).unwrap();
    // Scalar arithmetic and IEEE representation are emitted as instructions.
    for symbol in [
        "hirsel_rt_v2_bytes_descriptor",
        "hirsel_rt_v2_string_from_bytes",
        "hirsel_rt_v2_string_slice",
        "hirsel_rt_v2_unicode_scalars_open",
        "hirsel_rt_v2_unicode_scalars_next",
        "hirsel_rt_v2_scalar_positions_open",
        "hirsel_rt_v2_scalar_positions_next",
    ] {
        assert!(exported.contains(symbol), "{symbol} is not exported");
    }
    for family in [
        "_wrapping_add",
        "_checked_add",
        "_div_floor",
        "_try_narrow",
        "_min_number",
        "_to_i32",
        "_order",
        "_f32_bits",
        "_f64_bits",
        "_from_bits",
    ] {
        assert!(
            !exported.iter().any(|s| s.ends_with(family)),
            "a scalar runtime entry `*{family}` outlived its instruction"
        );
    }
}

#[test]
fn launch_rejects_another_abi_version_and_unknown_flags() {
    let mut ctx = ptr::null_mut();
    let config = Config {
        abi_version: ABI_VERSION + 1,
        ..Config::new()
    };
    assert_eq!(
        unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) },
        LAUNCH_ABI_MISMATCH
    );
    assert!(ctx.is_null());
    let config = Config {
        flags: 2,
        ..Config::new()
    };
    assert_eq!(
        unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) },
        LAUNCH_INVALID_CONFIG
    );
    let config = Config {
        flags: CONFIG_DIAGNOSTIC,
        ..Config::new()
    };
    assert_eq!(
        unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) },
        LAUNCH_OK
    );
    unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
    assert_eq!(hirsel_rt_v2_abi_version(), 5);
}

#[test]
fn a_rust_panic_aborts_at_the_c_boundary() {
    if in_child("a_rust_panic_aborts_at_the_c_boundary") {
        hirsel_rt_v2_diag_panic();
        // Unreachable: a panic in a non-unwinding extern "C" function aborts.
        std::process::exit(0);
    }
    assert_child_aborts(
        "a_rust_panic_aborts_at_the_c_boundary",
        "diagnostic panic probe",
    );
}
