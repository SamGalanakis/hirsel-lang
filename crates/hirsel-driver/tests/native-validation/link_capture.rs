use super::*;
use hirsel_engine::store::{DirStore, MemStore};

const LIMITS: CaptureLimits = CaptureLimits {
    archive_bytes: 4 * MAX_RECORD as u64,
    files: 32,
};

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = tar::Builder::new(Vec::new());
    for (name, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, name, *bytes).unwrap();
    }
    archive.into_inner().unwrap()
}

#[test]
fn reproducer_uses_canonical_names_and_bounded_content_records() {
    let store = MemStore::default();
    let bytes = vec![0x5a; MAX_RECORD + 17];
    let long = format!("inputs/tmp/{}/λ with spaces.o", "directory/".repeat(20));
    let tar = archive(&[
        ("inputs/version.txt", b"LLVM"),
        (&long, &bytes),
        ("inputs/response.txt", b"ignored argument serialization"),
        ("inputs/tmp/a.o", b"object"),
        (&long, &bytes),
    ]);
    let files = read_archive(&store, tar.as_slice(), LIMITS).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].name, "/tmp/a.o");
    assert_eq!(files[1].name, long.trim_start_matches("inputs"));
    assert_eq!(
        link_input_bytes(&store, files[1].bytes.clone().into()).unwrap(),
        bytes
    );
    assert_eq!(
        Reference::from(files[1].bytes.clone()),
        store_link_input(&store, &bytes).unwrap()
    );
}

#[test]
fn capture_refuses_truncation_conflicts_and_admission_overflow() {
    let store = MemStore::default();
    let tar = archive(&[
        ("inputs/response.txt", b""),
        ("inputs/version.txt", b"LLVM"),
        ("inputs/tmp/a.o", b"first"),
        ("inputs/tmp/a.o", b"changed"),
    ]);
    assert!(
        read_archive(&store, tar.as_slice(), LIMITS)
            .unwrap_err()
            .contains("changed during capture")
    );
    let tar = archive(&[
        ("inputs/response.txt", b""),
        ("inputs/version.txt", b"LLVM"),
        ("inputs/tmp/a.o", b"object"),
    ]);
    assert!(read_archive(&store, tar.as_slice(), CaptureLimits { files: 2, ..LIMITS }).is_err());
    assert!(
        read_archive(
            &store,
            tar.as_slice(),
            CaptureLimits {
                archive_bytes: 1,
                ..LIMITS
            }
        )
        .is_err()
    );
    // Three headers and the version's padded block precede the final payload.
    assert!(read_archive(&store, &tar[..2048 + 2], LIMITS).is_err());
    let missing = archive(&[
        ("inputs/version.txt", b"LLVM"),
        ("inputs/tmp/a.o", b"object"),
    ]);
    assert!(read_archive(&store, missing.as_slice(), LIMITS).is_err());
    let mut header = tar::Header::new_gnu();
    header.set_path("inputs/link").unwrap();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_link_name("/outside").unwrap();
    header.set_size(0);
    header.set_cksum();
    let mut builder = tar::Builder::new(Vec::new());
    builder.append(&header, &[][..]).unwrap();
    assert!(read_archive(&store, builder.into_inner().unwrap().as_slice(), LIMITS).is_err());
    let mut header = tar::Header::new_gnu();
    let traversal = b"inputs/../outside";
    header.as_mut_bytes()[..traversal.len()].copy_from_slice(traversal);
    header.set_size(0);
    header.set_cksum();
    let mut builder = tar::Builder::new(Vec::new());
    builder.append(&header, &[][..]).unwrap();
    assert!(read_archive(&store, builder.into_inner().unwrap().as_slice(), LIMITS).is_err());
}

#[test]
fn capture_descriptors_bind_invocation_and_require_the_entire_content_closure() {
    let store = MemStore::default();
    let bytes = store_link_input(&store, b"fixture only").unwrap();
    let tool = put_record(
        &store,
        TOOL,
        &ToolRecord {
            files: vec![
                FileRecord {
                    name: "bin/ld.lld".into(),
                    bytes: bytes.into(),
                },
                FileRecord {
                    name: "lib/ld-linux-x86-64.so.2".into(),
                    bytes: bytes.into(),
                },
            ],
        },
    )
    .unwrap();
    let mut record = CaptureRecord {
        tool: tool.into(),
        directory: "/original".into(),
        arguments: vec!["/original/a.o".into()],
        files: vec![FileRecord {
            name: "/original/a.o".into(),
            bytes: bytes.into(),
        }],
    };
    let first = put_record(&store, CAPTURE, &record).unwrap();
    CapturedLink::from_reference(&store, first).unwrap();
    record.arguments.push("--gc-sections".into());
    assert_ne!(put_record(&store, CAPTURE, &record).unwrap(), first);
    record.directory = "/other".into();
    assert_ne!(put_record(&store, CAPTURE, &record).unwrap(), first);
    record.files[0].bytes.output = ContentHash([0xdd; 32]);
    assert!(
        CapturedLink::from_reference(&store, put_record(&store, CAPTURE, &record).unwrap())
            .is_err()
    );
    for arguments in [
        vec!["@original.rsp".into()],
        vec!["--input-overlay=mutable.json".into()],
        vec!["-ooutput".into()],
        vec!["--remap-inputs-file=mutable.txt".into()],
    ] {
        assert!(check_arguments(&arguments).is_err());
    }
    assert!(check_arguments(&["--oformat=elf64-x86-64".into(), "-nostdlib".into()]).is_ok());
}

/// The dedicated linker-store-proof target supplies the packaged tool. This
/// test is not selected by ordinary compiler tests, which need no capture tool.
#[test]
#[ignore = "run through //tools/buck2/native-toolchain:linker-store-proof"]
fn packaged_capture_replays_changed_inputs_without_originals() {
    let package =
        PathBuf::from(std::env::var_os("HIRSEL_CAPTURE_LINKER").expect("packaged linker input"));
    let llvm = PathBuf::from(std::env::var_os("LLVM_SYS_231_PREFIX").expect("pinned LLVM input"));
    let scratch = tempfile::tempdir().unwrap();
    let source = scratch.path().join("source with spaces");
    std::fs::create_dir(&source).unwrap();
    let store_root = scratch.path().join("products");
    let store = DirStore::new(store_root.clone());
    let run = |command: &mut Command| {
        let result = command
            .env_clear()
            .env("LD_LIBRARY_PATH", llvm.join("lib"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        result
    };
    let assemble = |name: &str, text: &str| {
        let asm = source.join(format!("{name}.s"));
        let obj = source.join(format!("{name}.o"));
        std::fs::write(&asm, text).unwrap();
        run(Command::new(llvm.join("bin/llvm-mc"))
            .args(["-triple=x86_64-unknown-linux", "-filetype=obj"])
            .arg(&asm)
            .arg("-o")
            .arg(&obj));
        obj
    };
    let start =
        |exit| format!(".globl _start\n_start:\n mov $60, %rax\n mov ${exit}, %rdi\n syscall\n");
    let object = assemble("start", &start(42));
    let dep = assemble("dependency", ".globl dep\ndep:\n ret\n");
    let dep_archive = source.join("libdep.a");
    run(Command::new(llvm.join("bin/llvm-ar"))
        .arg("qcD")
        .arg(&dep_archive)
        .arg(dep));
    let member = assemble(
        "unused",
        &format!(
            ".globl unused\nunused:\n ret\n.section \".deplibs\",\"MS\",@llvm_dependent_libraries,1\n.asciz \"{}\"\n",
            dep_archive.display()
        ),
    );
    let thin = source.join("thin.a");
    run(Command::new(llvm.join("bin/llvm-ar"))
        .arg("qcTD")
        .arg(&thin)
        .arg(member));
    let child = source.join("child.ld");
    std::fs::write(
        &child,
        format!(
            "INPUT(\"{}\") GROUP(\"{}\")\n",
            object.display(),
            thin.display()
        ),
    )
    .unwrap();
    let script = source.join("main.ld");
    std::fs::write(&script, format!("INCLUDE \"{}\"\nSECTIONS {{ . = 0x200000; .probe : {{ \"{}\"(.text) }} .text : {{ *(.text*) }} }}\n", child.display(), object.display())).unwrap();
    let tool = FrozenLinker::snapshot(&store, &package).unwrap();
    let args = vec![
        "-nostdlib".into(),
        "--threads=1".into(),
        script.to_str().unwrap().into(),
    ];
    let first = tool.capture(&store, &source, &args, LIMITS).unwrap();
    assert!(
        first
            .record(&store)
            .unwrap()
            .files
            .iter()
            .any(|f| f.name == dep_archive.to_str().unwrap())
    );
    assemble("start", &start(43));
    let second = tool.capture(&store, &source, &args, LIMITS).unwrap();
    assert_ne!(first.reference(), second.reference());
    std::fs::remove_dir_all(&source).unwrap();
    let reopened = DirStore::new(store_root);
    for (capture, exit) in [(first, 42), (second, 43)] {
        let capture = CapturedLink::from_reference(&reopened, capture.reference()).unwrap();
        let output = scratch.path().join(format!("program-{exit}"));
        capture.link(&reopened, &output).unwrap();
        assert_eq!(Command::new(&output).status().unwrap().code(), Some(exit));
        let sections = run(Command::new(llvm.join("bin/llvm-readelf"))
            .arg("-S")
            .arg(&output));
        assert!(
            String::from_utf8(sections.stdout)
                .unwrap()
                .contains(".probe")
        );
    }
}

#[test]
#[ignore = "requires scratch-linker-proof selected linker inputs"]
fn large_tool_snapshots_and_replays_under_none_and_unwritable_with_original_ram_cap() {
    use hirsel_engine::{
        runtime::ProcessBudget,
        store::{NoneStore, RequestStore},
    };
    let package = PathBuf::from(std::env::var_os("HIRSEL_CAPTURE_LINKER").unwrap());
    let llvm = PathBuf::from(std::env::var_os("LLVM_SYS_231_PREFIX").unwrap());
    for unwritable in [false, true] {
        let scratch = tempfile::tempdir().unwrap();
        let blocked = scratch.path().join("not-a-directory");
        std::fs::write(&blocked, b"selected durable path cannot be written").unwrap();
        let selected: Arc<dyn ResultStore> = if unwritable {
            Arc::new(DirStore::new(blocked))
        } else {
            Arc::new(NoneStore)
        };
        let local = selected.local_product_store().is_some();
        let verifier = selected.verifier().is_some();
        let ram = 16 << 20;
        let budget = ProcessBudget::new(ram);
        let store = RequestStore::with_scratch(
            selected,
            budget.reserve(ram).unwrap(),
            scratch.path(),
            256 << 20,
        )
        .unwrap();
        let tool = FrozenLinker::snapshot(&store, &package).unwrap();
        assert_eq!(store.local_product_store().is_some(), local);
        assert_eq!(store.verifier().is_some(), verifier);
        assert!(store.retained_bytes() < ram);
        let source = scratch.path().join("original");
        std::fs::create_dir(&source).unwrap();
        let text = source.join("main.s");
        let object = source.join("main.o");
        std::fs::write(
            &text,
            ".globl _start\n_start:\n mov $60,%rax\n mov $42,%rdi\n syscall\n",
        )
        .unwrap();
        let compiled = Command::new(llvm.join("bin/llvm-mc"))
            .env_clear()
            .env("LD_LIBRARY_PATH", llvm.join("lib"))
            .args(["-triple=x86_64-unknown-linux", "-filetype=obj"])
            .arg(&text)
            .arg("-o")
            .arg(&object)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let captured = tool
            .capture(
                &store,
                &source,
                &[
                    "-nostdlib".into(),
                    "--threads=1".into(),
                    object.to_str().unwrap().into(),
                ],
                LIMITS,
            )
            .unwrap();
        std::fs::remove_dir_all(source).unwrap();
        let captured = CapturedLink::from_reference(&store, captured.reference()).unwrap();
        let output = scratch.path().join("program");
        captured.link(&store, &output).unwrap();
        assert_eq!(Command::new(output).status().unwrap().code(), Some(42));
        assert!(store.retained_bytes() <= ram);
        assert!(!store.exhausted());
        assert!(budget.reserve(1).is_err());
        drop(store);
        assert_eq!(budget.reserved(), 0);
    }
}
