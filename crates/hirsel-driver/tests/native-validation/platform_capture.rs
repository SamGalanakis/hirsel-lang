use super::*;
use hirsel_engine::store::DirStore;
use std::process::Command;

fn input(name: &str) -> PathBuf {
    std::env::var_os(name).unwrap().into()
}
fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn object(llvm: &Path, directory: &Path, target: &str, name: &str, text: &str) -> PathBuf {
    let source = directory.join(format!("{name}.s"));
    let output = directory.join(format!("{name}.o"));
    std::fs::write(&source, text).unwrap();
    run(Command::new(llvm.join("bin/llvm-mc"))
        .arg(format!("-triple={target}"))
        .args(["-filetype=obj", "-o"])
        .arg(&output)
        .arg(source));
    output
}
#[test]
#[ignore = "requires platform-capture-proof selected linker, kit, runtime and loaders"]
fn selected_platform_closes_runtime_before_keys_and_replays_without_originals() {
    let directory = tempfile::tempdir().unwrap();
    let store = DirStore::new(directory.path().join("products"));
    let llvm = input("LLVM_SYS_231_PREFIX");
    let package = input("HIRSEL_CAPTURE_LINKER");
    let linker = FrozenLinker::snapshot(&store, &package).unwrap();
    let auditors = input("HIRSEL_AUDITOR_PACKAGE");
    let floor = input("HIRSEL_GLIBC_FLOOR");
    let qemu = input("HIRSEL_QEMU_AARCH64");
    let limits = CaptureLimits {
        archive_bytes: 256 << 20,
        files: 1024,
    };
    for target in [
        super::super::super::TargetSpec::x86_64_unknown_linux_gnu(),
        super::super::super::TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let original = directory.path().join(&target.triple);
        std::fs::create_dir_all(&original).unwrap();
        for entry in std::fs::read_dir(floor.join(&target.triple)).unwrap() {
            let entry = entry.unwrap();
            if entry.path().is_file() {
                std::fs::copy(entry.path(), original.join(entry.file_name())).unwrap();
            }
        }
        let loader = original.join("ld-2.28.so");
        let auditor = original.join("audit.so");
        std::fs::copy(auditors.join(format!("{}.so", target.triple)), &auditor).unwrap();
        let kit =
            FrozenLinkKit::snapshot(&store, &input("HIRSEL_LINK_KIT"), &target, linker, limits)
                .unwrap();
        let runtime = input("HIRSEL_RUNTIME_ARCHIVES")
            .join(&target.triple)
            .join("default/libhirsel_rt.a");
        let foreign = [original.join("libselected.so")];
        let arm = target.triple.starts_with("aarch64");
        let emulator = arm.then_some(qemu.as_path());
        let runtime_limits = RuntimeLimits {
            aliases: 128,
            name_bytes: 64 << 10,
        };
        let mut versions: Vec<CapturedPlatform> = Vec::new();
        let image = directory.path().join(format!("{}-program", target.triple));
        for value in [42, 43] {
            let text = if arm {
                format!(".globl selected\nselected:\n mov x0,#{value}\n ret\n")
            } else {
                format!(".globl selected\nselected:\n mov ${value},%rax\n ret\n")
            };
            let leaf = object(&llvm, &original, &target.triple, "leaf", &text);
            run(Command::new(llvm.join("bin/ld.lld"))
                .args(["--shared", "--threads=1", "--soname=libselected.so", "-o"])
                .arg(&foreign[0])
                .arg(leaf));
            let select = || {
                CapturedPlatform::capture(
                    &store,
                    &target,
                    PlatformCaptureInputs {
                        linker,
                        kit,
                        runtime_archive: &runtime,
                        foreign: &foreign,
                        runtime: RuntimeSelection {
                            loader: &loader,
                            auditor: &auditor,
                            library_path: original.as_os_str(),
                            emulator,
                            timeout: Duration::from_secs(20),
                            events_bytes: 64 << 10,
                            limits: runtime_limits,
                        },
                        limits,
                    },
                )
                .unwrap()
            };
            let selected = select();
            assert!(selected.reads().iter().any(
                |read| read.path == foreign[0] && read.content == selected.foreign()[0].content
            ));
            assert_eq!(
                selected.identity(),
                select().identity(),
                "fresh input reads must preserve the platform key"
            );
            if value == 42 {
                let start = object(
                    &llvm,
                    &original,
                    &target.triple,
                    "start",
                    if arm {
                        ".globl _start\n_start:\n bl selected\n mov x8,#93\n svc #0\n"
                    } else {
                        ".globl _start\n_start:\n call selected@PLT\n mov %rax,%rdi\n mov $60,%rax\n syscall\n"
                    },
                );
                run(Command::new(llvm.join("bin/ld.lld"))
                    .args(["--threads=1", "-o"])
                    .arg(&image)
                    .arg(start)
                    .arg(&foreign[0]));
            } else {
                assert_ne!(versions[0].identity(), selected.identity());
            }
            selected
                .validate_image(
                    &store,
                    &target,
                    &image,
                    emulator,
                    Duration::from_secs(20),
                    runtime_limits,
                )
                .unwrap();
            versions.push(selected);
        }
        std::fs::remove_dir_all(&original).unwrap();
        for (selected, expected) in versions.iter().zip([42, 43]) {
            selected
                .validate_image(
                    &store,
                    &target,
                    &image,
                    emulator,
                    Duration::from_secs(20),
                    runtime_limits,
                )
                .unwrap();
            let files = selected
                .runtime()
                .materialize(&store, &target, runtime_limits)
                .unwrap();
            let output = files.command(emulator, &image).output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
