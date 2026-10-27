use super::*;
use hirsel_engine::store::{Bytes, DirStore, EntryKey};

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        command,
        String::from_utf8_lossy(&output.stderr)
    );
}
fn assemble(llvm: &Path, root: &Path, target: &str, name: &str, source: &str) -> PathBuf {
    let text = root.join(format!("{name}.s"));
    let object = root.join(format!("{name}.o"));
    std::fs::write(&text, source).unwrap();
    run(Command::new(llvm.join("bin/llvm-mc"))
        .arg(format!("-triple={target}"))
        .args(["-filetype=obj", "-o"])
        .arg(&object)
        .arg(&text));
    object
}
fn store_file(store: &dyn ResultStore, path: &Path) -> Reference {
    store_link_input_reader(store, std::fs::File::open(path).unwrap()).unwrap()
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
        "runtime-missing-content"
    }
}

#[test]
#[ignore = "requires runtime-store-proof loader/auditor inputs"]
fn actual_runtime_products_reopen_changed_leaf_versions_and_refuse_missing_closure() {
    let llvm = PathBuf::from(std::env::var_os("LLVM_SYS_231_PREFIX").unwrap());
    let auditors = PathBuf::from(std::env::var_os("HIRSEL_AUDITOR_PACKAGE").unwrap());
    let floor = PathBuf::from(std::env::var_os("HIRSEL_GLIBC_FLOOR").unwrap());
    let qemu = PathBuf::from(std::env::var_os("HIRSEL_QEMU_AARCH64").unwrap());
    for target in [
        super::super::super::TargetSpec::x86_64_unknown_linux_gnu(),
        super::super::super::TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let store_root = directory.path().join("products");
        let store = DirStore::new(store_root.clone());
        let original = directory.path().join("original with spaces\nand newline");
        std::fs::create_dir_all(&original).unwrap();
        let loader_path = original.join("loader");
        let auditor_path = original.join("auditor.so");
        std::fs::copy(floor.join(&target.triple).join("ld-2.28.so"), &loader_path).unwrap();
        std::fs::copy(
            auditors.join(format!("{}.so", target.triple)),
            &auditor_path,
        )
        .unwrap();
        let loader = store_file(&store, &loader_path);
        let auditor = store_file(&store, &auditor_path);
        let arm = target.triple.starts_with("aarch64");
        let leaf_path = original.join("libleaf.so");
        let top_path = original.join("top.so");
        let limits = RuntimeLimits {
            aliases: 2,
            name_bytes: 4096,
        };
        let mut versions = Vec::new();
        let mut image = None;
        let mut leaf_input = None;
        for value in [42, 43] {
            let source = if arm {
                format!(".globl leaf\nleaf:\n mov x0,#{value}\n ret\n")
            } else {
                format!(".globl leaf\nleaf:\n mov ${value},%rax\n ret\n")
            };
            let object = assemble(&llvm, &original, &target.triple, "leaf", &source);
            run(Command::new(llvm.join("bin/ld.lld"))
                .args(["-shared", "--threads=1", "--soname=libleaf.so", "-o"])
                .arg(&leaf_path)
                .arg(&object));
            if image.is_none() {
                let object = assemble(
                    &llvm,
                    &original,
                    &target.triple,
                    "top",
                    if arm {
                        ".globl top\ntop:\n b leaf\n"
                    } else {
                        ".globl top\ntop:\n jmp leaf@PLT\n"
                    },
                );
                run(Command::new(llvm.join("bin/ld.lld"))
                    .args(["-shared", "--threads=1", "-rpath", "$ORIGIN", "-o"])
                    .arg(&top_path)
                    .arg(&object)
                    .arg(&leaf_path));
                let object = assemble(
                    &llvm,
                    &original,
                    &target.triple,
                    "start",
                    if arm {
                        ".globl _start\n_start:\n bl top\n mov x8,#93\n svc #0\n"
                    } else {
                        ".globl _start\n_start:\n call top@PLT\n mov %rax,%rdi\n mov $60,%rax\n syscall\n"
                    },
                );
                let executable = original.join("program");
                run(Command::new(llvm.join("bin/ld.lld"))
                    .args(["--threads=1", "--allow-shlib-undefined", "-o"])
                    .arg(&executable)
                    .arg(&object)
                    .arg(&top_path));
                image = Some(store_file(&store, &executable));
            }
            let leaf = store_file(&store, &leaf_path);
            leaf_input = Some(leaf);
            let aliases = vec![
                (
                    top_path.to_str().unwrap().to_string(),
                    store_file(&store, &top_path),
                ),
                ("libleaf.so".into(), leaf),
            ];
            let runtime = FrozenRuntime::from_settled(
                &store,
                &target,
                loader,
                auditor,
                aliases.clone(),
                limits,
            )
            .unwrap();
            assert!(
                FrozenRuntime::from_settled(
                    &store,
                    &target,
                    loader,
                    auditor,
                    aliases.clone(),
                    RuntimeLimits {
                        aliases: 1,
                        ..limits
                    }
                )
                .is_err()
            );
            assert!(
                FrozenRuntime::from_settled(
                    &store,
                    &target,
                    loader,
                    auditor,
                    [aliases[0].clone(), aliases[0].clone()],
                    limits
                )
                .is_err()
            );
            assert!(
                FrozenRuntime::from_settled(
                    &store,
                    &target,
                    loader,
                    auditor,
                    [("nul\0name".into(), leaf)],
                    limits
                )
                .is_err()
            );
            versions.push(runtime.reference());
        }
        assert_ne!(
            versions[0], versions[1],
            "same-path leaf content belongs to runtime identity even when image bytes stay equal"
        );
        std::fs::remove_dir_all(original).unwrap();
        drop(store);
        let store = DirStore::new(store_root);
        let executable = directory.path().join("restored-image");
        materialize_link_input(&store, image.unwrap(), &executable, true).unwrap();
        let last = versions[1];
        for (reference, expected) in versions.into_iter().zip([42, 43]) {
            let runtime =
                FrozenRuntime::from_reference(&store, reference, &target, limits).unwrap();
            let files = runtime.materialize(&store, &target, limits).unwrap();
            let output = files
                .command(arm.then_some(qemu.as_path()), &executable)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let runtime = FrozenRuntime::from_reference(&store, last, &target, limits).unwrap();
        // A valid cached executable does not excuse a missing selected library.
        // Retain the actual final descriptor instead of asking an ambient path.
        let missing = Missing {
            inner: &store,
            missing: leaf_input.unwrap(),
        };
        assert!(runtime.validate(&missing, &target, limits).is_err());
    }
}

#[test]
#[ignore = "requires runtime-identity-case-proof loader/auditor inputs"]
fn equal_bytes_keep_distinct_loaded_libraries_and_shared_aliases() {
    let llvm = PathBuf::from(std::env::var_os("LLVM_SYS_231_PREFIX").unwrap());
    let auditors = PathBuf::from(std::env::var_os("HIRSEL_AUDITOR_PACKAGE").unwrap());
    let floor = PathBuf::from(std::env::var_os("HIRSEL_GLIBC_FLOOR").unwrap());
    let target = super::super::super::TargetSpec::x86_64_unknown_linux_gnu();
    let directory = tempfile::tempdir().unwrap();
    let store = DirStore::new(directory.path().join("products"));
    let original = directory.path().join("original");
    std::fs::create_dir(&original).unwrap();
    let object = assemble(
        &llvm,
        &original,
        &target.triple,
        "initializer",
        ".text\n.Linit:\n mov $1,%rax\n mov $1,%rdi\n lea .Lmessage(%rip),%rsi\n mov $1,%rdx\n syscall\n ret\n.section .rodata\n.Lmessage: .byte 88\n.section .init_array,\"aw\",@init_array\n.quad .Linit\n",
    );
    let one = original.join("one.so");
    let two = original.join("two.so");
    run(Command::new(llvm.join("bin/ld.lld"))
        .args(["-shared", "--threads=1", "-o"])
        .arg(&one)
        .arg(&object));
    std::fs::copy(&one, &two).unwrap();
    let object = assemble(
        &llvm,
        &original,
        &target.triple,
        "start",
        ".globl _start\n_start:\n xor %edi,%edi\n mov $60,%eax\n syscall\n",
    );
    let image = directory.path().join("program");
    run(Command::new(llvm.join("bin/ld.lld"))
        .args(["--threads=1", "--no-as-needed", "-o"])
        .arg(&image)
        .arg(&object)
        .arg(&one)
        .arg(&two));
    let loader_path = floor.join(&target.triple).join("ld-2.28.so");
    let baseline = Command::new(&loader_path).arg(&image).output().unwrap();
    assert!(baseline.status.success());
    assert_eq!(
        baseline.stdout, b"XX",
        "distinct equal-byte DSOs initialize twice"
    );
    let loader = store_file(&store, &loader_path);
    let auditor = store_file(&store, &auditors.join(format!("{}.so", target.triple)));
    let file = store_file(&store, &one);
    assert_eq!(file, store_file(&store, &two));
    let a = one.to_str().unwrap().to_string();
    let b = two.to_str().unwrap().to_string();
    let limits = RuntimeLimits {
        aliases: 2,
        name_bytes: 4096,
    };
    let distinct = FrozenRuntime::from_libraries(
        &store,
        &target,
        loader,
        auditor,
        [
            RuntimeLibrary {
                aliases: vec![a.clone()],
                file,
            },
            RuntimeLibrary {
                aliases: vec![b.clone()],
                file,
            },
        ],
        limits,
    )
    .unwrap();
    std::fs::remove_file(&two).unwrap();
    std::fs::hard_link(&one, &two).unwrap();
    let baseline = Command::new(&loader_path).arg(&image).output().unwrap();
    assert_eq!(
        baseline.stdout, b"X",
        "two original aliases share one initialization"
    );
    let shared = FrozenRuntime::from_libraries(
        &store,
        &target,
        loader,
        auditor,
        [RuntimeLibrary {
            aliases: vec![a, b],
            file,
        }],
        limits,
    )
    .unwrap();
    assert_ne!(shared.reference(), distinct.reference());
    std::fs::remove_dir_all(original).unwrap();
    for (runtime, expected) in [(distinct, b"XX".as_slice()), (shared, b"X".as_slice())] {
        let files = runtime.materialize(&store, &target, limits).unwrap();
        let output = files.command(None, &image).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected);
    }
}

#[test]
#[ignore = "requires runtime-discovery-proof loader/auditor inputs"]
fn actual_loader_discovers_transitive_runtime_inputs_and_closes_replay() {
    use discovery::{DiscoveryPolicy, discover};
    let llvm = PathBuf::from(std::env::var_os("LLVM_SYS_231_PREFIX").unwrap());
    let auditors = PathBuf::from(std::env::var_os("HIRSEL_AUDITOR_PACKAGE").unwrap());
    let floor = PathBuf::from(std::env::var_os("HIRSEL_GLIBC_FLOOR").unwrap());
    let qemu = PathBuf::from(std::env::var_os("HIRSEL_QEMU_AARCH64").unwrap());
    for target in [
        super::super::super::TargetSpec::x86_64_unknown_linux_gnu(),
        super::super::super::TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original with spaces\nand newline");
        std::fs::create_dir_all(&original).unwrap();
        let store_root = directory.path().join("products");
        let store = DirStore::new(store_root.clone());
        let loader = store_file(&store, &floor.join(&target.triple).join("ld-2.28.so"));
        let auditor = store_file(&store, &auditors.join(format!("{}.so", target.triple)));
        let arm = target.triple.starts_with("aarch64");
        let leaf = original.join("libleaf.so");
        let top = original.join("top.so");
        let image = original.join("program");
        let limits = RuntimeLimits {
            aliases: 16,
            name_bytes: 8192,
        };
        let mut versions = Vec::new();
        for value in [42, 43] {
            let object = assemble(
                &llvm,
                &original,
                &target.triple,
                "leaf",
                &if arm {
                    format!(".globl leaf\nleaf:\n mov x0,#{value}\n ret\n")
                } else {
                    format!(".globl leaf\nleaf:\n mov ${value},%rax\n ret\n")
                },
            );
            run(Command::new(llvm.join("bin/ld.lld"))
                .args(["-shared", "--threads=1", "--soname=libleaf.so", "-o"])
                .arg(&leaf)
                .arg(object));
            if value == 42 {
                let object = assemble(
                    &llvm,
                    &original,
                    &target.triple,
                    "top",
                    if arm {
                        ".globl top\ntop:\n b leaf\n"
                    } else {
                        ".globl top\ntop:\n jmp leaf@PLT\n"
                    },
                );
                run(Command::new(llvm.join("bin/ld.lld"))
                    .args(["-shared", "--threads=1", "-rpath", "$ORIGIN", "-o"])
                    .arg(&top)
                    .arg(object)
                    .arg(&leaf));
                let object = assemble(
                    &llvm,
                    &original,
                    &target.triple,
                    "start",
                    if arm {
                        ".globl _start\n_start:\n bl top\n mov x8,#93\n svc #0\n"
                    } else {
                        ".globl _start\n_start:\n call top@PLT\n mov %rax,%rdi\n mov $60,%rax\n syscall\n"
                    },
                );
                run(Command::new(llvm.join("bin/ld.lld"))
                    .args(["--threads=1", "--allow-shlib-undefined", "-o"])
                    .arg(&image)
                    .arg(object)
                    .arg(&top));
            }
            let result = discover(
                &store,
                &target,
                &image,
                loader,
                auditor,
                DiscoveryPolicy {
                    library_path: original.as_os_str(),
                    emulator: arm.then_some(qemu.as_path()),
                    timeout: std::time::Duration::from_secs(20),
                    events_bytes: 16 << 10,
                    input_bytes: 16 << 20,
                    runtime: limits,
                },
            )
            .unwrap();
            assert_eq!(result.inputs.len(), 2);
            assert!(
                result
                    .inputs
                    .iter()
                    .any(|input| input.path == leaf && input.file == store_file(&store, &leaf))
            );
            versions.push((result.image, result.runtime.reference()));
        }
        assert_eq!(versions[0].0, versions[1].0);
        assert_ne!(versions[0].1, versions[1].1);
        std::fs::rename(&leaf, original.join("removed-leaf")).unwrap();
        assert!(
            discover(
                &store,
                &target,
                &image,
                loader,
                auditor,
                DiscoveryPolicy {
                    library_path: original.as_os_str(),
                    emulator: arm.then_some(qemu.as_path()),
                    timeout: std::time::Duration::from_secs(20),
                    events_bytes: 16 << 10,
                    input_bytes: 16 << 20,
                    runtime: limits,
                }
            )
            .is_err(),
            "missing current transitive leaf must refuse"
        );
        std::fs::remove_dir_all(original).unwrap();
        drop(store);
        let store = DirStore::new(store_root);
        for ((image, runtime), expected) in versions.into_iter().zip([42, 43]) {
            let runtime = FrozenRuntime::from_reference(&store, runtime, &target, limits).unwrap();
            let files = runtime.materialize(&store, &target, limits).unwrap();
            let executable = files.directory().join("program");
            materialize_link_input(&store, image, &executable, true).unwrap();
            let output = files
                .command(arm.then_some(qemu.as_path()), &executable)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
