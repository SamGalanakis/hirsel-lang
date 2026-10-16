//! Linking emitted objects with C and the `hirsel_rt_v2` runtime archive, and running the
//! result: x86-64 natively, AArch64 under `qemu-aarch64-static`. These need the pinned clang,
//! the AArch64 cross toolchain and qemu, so only `tools/check-native.sh` runs their tests.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use hirsel_lir::layout_types::TargetSpec;

pub const RUNTIME_INCLUDE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../hirsel-rt/include");

pub fn llvm_prefix() -> PathBuf {
    std::env::var_os("LLVM_SYS_211_PREFIX")
        .expect("LLVM_SYS_211_PREFIX is set by .cargo/config.toml or the environment")
        .into()
}

pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn output(command: &mut Command) -> String {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("could not run {command:?}: {error}"));
    assert!(
        output.status.success(),
        "{command:?} failed with {}:\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn aarch64(target: &TargetSpec) -> bool {
    target.triple.starts_with("aarch64")
}

/// The release diagnostic `libhirsel_rt.a` for `target` with aborting panics, as
/// `crates/hirsel-rt/harness/run.sh` links it: Buck2's archive under
/// `HIRSEL_RUNTIME_ARCHIVES` (`<triple>/diagnostic/libhirsel_rt.a`), or otherwise built
/// once per test process with Cargo.
pub fn runtime_archive(target: &TargetSpec) -> PathBuf {
    if let Some(dir) = std::env::var_os("HIRSEL_RUNTIME_ARCHIVES") {
        let archive = Path::new(&dir)
            .join(&target.triple)
            .join("diagnostic/libhirsel_rt.a");
        assert!(
            archive.is_file(),
            "no runtime archive {}",
            archive.display()
        );
        return archive;
    }
    static BUILT: OnceLock<Mutex<Vec<(String, PathBuf)>>> = OnceLock::new();
    let built = BUILT.get_or_init(|| Mutex::new(Vec::new()));
    let mut built = built.lock().unwrap();
    if let Some((_, path)) = built.iter().find(|(t, _)| *t == target.triple) {
        return path.clone();
    }
    let target_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("runtime-cargo");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .args(["build", "--quiet", "--release", "-p", "hirsel-rt", "--lib"])
        .args(["--features", "diagnostic"])
        .arg("--manifest-path")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
        .arg("--target-dir")
        .arg(&target_dir)
        .env("CARGO_PROFILE_RELEASE_PANIC", "abort");
    let archive = if aarch64(target) {
        command.args(["--target", &target.triple]).env(
            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER",
            std::env::var_os("HIRSEL_AARCH64_CC").expect("HIRSEL_AARCH64_CC"),
        );
        target_dir
            .join(&target.triple)
            .join("release/libhirsel_rt.a")
    } else {
        target_dir.join("release/libhirsel_rt.a")
    };
    output(&mut command);
    built.push((target.triple.clone(), archive.clone()));
    archive
}

/// Links `objects` (written into `dir`), the C `sources` and the runtime archive into an
/// executable for `target`.
pub fn link(
    target: &TargetSpec,
    dir: &Path,
    objects: &[(&str, &[u8])],
    sources: &[PathBuf],
) -> PathBuf {
    let mut command = if aarch64(target) {
        Command::new(std::env::var_os("HIRSEL_AARCH64_CC").expect("HIRSEL_AARCH64_CC"))
    } else {
        let mut command = Command::new(llvm_prefix().join("bin/clang"));
        command.arg(format!(
            "--ld-path={}",
            llvm_prefix().join("bin/ld.lld").display()
        ));
        command
    };
    command
        .args([
            "-std=c11",
            "-DHIRSEL_RT_ENABLE_STATS",
            "-O1",
            "-Wall",
            "-Werror",
            "-Wno-unused-function",
        ])
        .arg("-I")
        .arg(RUNTIME_INCLUDE)
        .arg("-include")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/native/fixture.h"
        ))
        .arg("-I")
        .arg(dir);
    for source in sources {
        command.arg(source);
    }
    for (name, bytes) in objects {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        command.arg(path);
    }
    let executable = dir.join("program");
    command
        .arg(runtime_archive(target))
        .args([
            "-lgcc_s",
            "-lutil",
            "-lrt",
            "-lpthread",
            "-lm",
            "-ldl",
            "-o",
        ])
        .arg(&executable);
    output(&mut command);
    executable
}

/// Runs an executable for `target` and returns its standard output.
pub fn run(target: &TargetSpec, executable: &Path) -> String {
    if aarch64(target) {
        output(
            Command::new(std::env::var_os("HIRSEL_QEMU_AARCH64").expect("HIRSEL_QEMU_AARCH64"))
                .arg("-L")
                .arg(std::env::var_os("HIRSEL_AARCH64_SYSROOT").expect("HIRSEL_AARCH64_SYSROOT"))
                .arg(executable),
        )
    } else {
        output(&mut Command::new(executable))
    }
}
