use super::*;
use hirsel_engine::store::{Bytes, DirStore, EntryKey};
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
struct Missing<'a> {
    inner: &'a dyn ResultStore,
    missing: Reference,
}
impl ResultStore for Missing<'_> {
    fn get(&self, key: &EntryKey) -> Option<Bytes> {
        self.inner.get(key)
    }
    fn put(&self, key: &EntryKey, bytes: &[u8]) {
        self.inner.put(key, bytes);
    }
    fn get_content(&self, reference: Reference) -> Option<Bytes> {
        (reference != self.missing)
            .then(|| self.inner.get_content(reference))
            .flatten()
    }
    fn name(&self) -> &'static str {
        "missing-selected-platform-leaf"
    }
}

#[test]
#[ignore = "requires platform-closure-proof original selected tool packages"]
fn selected_virtual_inputs_keep_scripts_thin_members_and_absolute_dso_names() {
    let directory = tempfile::tempdir().unwrap();
    let store = DirStore::new(directory.path().join("products"));
    let llvm = input("LLVM_SYS_231_PREFIX");
    let linker = FrozenLinker::snapshot(&store, &input("HIRSEL_CAPTURE_LINKER")).unwrap();
    let floor = input("HIRSEL_GLIBC_FLOOR");
    let auditors = input("HIRSEL_AUDITOR_PACKAGE");
    let qemu = input("HIRSEL_QEMU_AARCH64");
    let limits = CaptureLimits {
        archive_bytes: 256 << 20,
        files: 2048,
    };
    let runtime_limits = RuntimeLimits {
        aliases: 128,
        name_bytes: 64 << 10,
    };
    for target in [
        super::super::super::TargetSpec::x86_64_unknown_linux_gnu(),
        super::super::super::TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let original = directory
            .path()
            .join(format!("original with spaces {}", target.triple));
        let emitted = directory.path().join(format!("emitted {}", target.triple));
        std::fs::create_dir_all(&original).unwrap();
        std::fs::create_dir_all(&emitted).unwrap();
        for entry in std::fs::read_dir(floor.join(&target.triple)).unwrap() {
            let entry = entry.unwrap();
            if entry.path().is_file() {
                std::fs::copy(entry.path(), original.join(entry.file_name())).unwrap();
            }
        }
        let arm = target.triple.starts_with("aarch64");
        let emulator = arm.then_some(qemu.as_path());
        let loader = original.join("ld-2.28.so");
        let auditor = original.join("auditor.so");
        std::fs::copy(auditors.join(format!("{}.so", target.triple)), &auditor).unwrap();
        let kit =
            FrozenLinkKit::snapshot(&store, &input("HIRSEL_LINK_KIT"), &target, linker, limits)
                .unwrap();
        let runtime = input("HIRSEL_RUNTIME_ARCHIVES")
            .join(&target.triple)
            .join("default/libhirsel_rt.a");
        let shared = original.join("without-soname.so");
        let leaf = object(
            &llvm,
            &original,
            &target.triple,
            "shared",
            if arm {
                ".globl shared\nshared:\n mov x0,#22\n ret\n"
            } else {
                ".globl shared\nshared:\n mov $22,%rax\n ret\n"
            },
        );
        run(Command::new(llvm.join("bin/ld.lld"))
            .args(["--shared", "--threads=1", "-o"])
            .arg(&shared)
            .arg(leaf));
        let member = original.join("member.o");
        let archive = original.join("thin.a");
        let script = original.join("selected.ld");
        let foreign = [script.clone()];
        std::fs::write(
            &script,
            format!(
                "INPUT(\"{}\" \"{}\")\n",
                archive.display(),
                shared.display()
            ),
        )
        .unwrap();
        let mut platforms: Vec<CapturedPlatform> = vec![];
        for value in [20, 21] {
            let text = if arm {
                format!(".globl thin\nthin:\n mov x0,#{value}\n ret\n")
            } else {
                format!(".globl thin\nthin:\n mov ${value},%rax\n ret\n")
            };
            object(&llvm, &original, &target.triple, "member", &text);
            if value == 20 {
                run(Command::new(llvm.join("bin/llvm-ar"))
                    .arg("crsT")
                    .arg(&archive)
                    .arg(&member));
            }
            let platform = CapturedPlatform::capture(
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
            .unwrap();
            assert!(platform.reads().iter().any(|read| read.path == member));
            if let Some(before) = platforms.first() {
                assert_ne!(before.identity(), platform.identity());
            }
            platforms.push(platform);
        }
        std::fs::remove_dir_all(&original).unwrap();
        let ir = format!(
            "target triple = \"{}\"\ndeclare i64 @thin()\ndeclare i64 @shared()\ndefine i32 @main() {{\n %a=call i64 @thin()\n %b=call i64 @shared()\n %sum=add i64 %a,%b\n %r=trunc i64 %sum to i32\n ret i32 %r\n}}\n",
            target.triple
        );
        let main = emitted.join("main.o");
        std::fs::write(
            &main,
            hirsel_llvm::compile_ir(&ir, &target, hirsel_lir::emission::OptLevel::None)
                .unwrap()
                .bytes,
        )
        .unwrap();
        let mut ids = Vec::new();
        for (index, platform) in platforms.iter().enumerate() {
            let plan = platform
                .plan_inputs(&store, &target, &emitted, std::slice::from_ref(&main), None)
                .unwrap();
            ids.push(plan.build_id());
            let captured = plan.capture(&store, platform, limits).unwrap();
            let image = emitted.join(format!("program-{index}"));
            captured.link(&store, &image).unwrap();
            platform
                .validate_image(
                    &store,
                    &target,
                    &image,
                    emulator,
                    Duration::from_secs(20),
                    runtime_limits,
                )
                .unwrap();
            let files = platform
                .runtime()
                .materialize(&store, &target, runtime_limits)
                .unwrap();
            let output = files.command(emulator, &image).output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(42 + index as i32),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let missing = platform
                .reads()
                .iter()
                .find(|read| read.path == member)
                .unwrap()
                .content;
            assert!(
                plan.capture(
                    &Missing {
                        inner: &store,
                        missing
                    },
                    platform,
                    limits
                )
                .is_err()
            );
        }
        assert_ne!(
            ids[0], ids[1],
            "an edited thin member must bind the final build ID too"
        );
        let extra = emitted.join("unselected.o");
        std::fs::copy(&main, &extra).unwrap();
        let injected = emitted.join("not-an-emitted-object.o");
        std::fs::write(&injected, format!("INPUT(\"{}\")\n", extra.display())).unwrap();
        let plan = platforms[0]
            .plan_inputs(&store, &target, &emitted, &[injected], None)
            .unwrap();
        assert!(
            plan.capture(&store, &platforms[0], limits)
                .err()
                .unwrap()
                .contains("outside selected inputs")
        );
    }
}
