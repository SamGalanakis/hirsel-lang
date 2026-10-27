//! A shipped executable is a pure function of its program and the toolchain:
//! `hirsel build` gives the same bytes (including embedded DWARF with `--debug`)
//! on a repeat and from 1, 2 or 4 builds running at once,
//! on both targets, each build in its own scratch directory.
use hirsel_driver::native::{self, Backend, BuildProfile, TargetSpec};
use hirsel_engine::with_compiler_stack;
use object::read::elf::{FileHeader, Sym};
use object::{LittleEndian, elf};
use std::path::{Path, PathBuf};

const PROGRAM: &str = "type Shape = Circle(u64) | Square(u64)

fn area(item: Shape) -> u64 {
  match item {
    Circle(r) => 3u64 * r * r,
    Square(s) => s * s,
  }
}

pub fn main(rt: Io) -> Result[(), GrantError] {
  let stdout = io.stdout(rt)?
  let shapes: List[Shape] = [Circle(2u64), Square(3u64)]
  let mut total = 0u64
  for item in shapes {
    total += area(item)
  }
  let _ = out.print(stdout, \"shapes\\n\")
  Ok(())
}
";

/// The executable's bytes and, for a debug build, its debug file's name and bytes.
type Built = (Vec<u8>, Option<(String, Vec<u8>)>);

fn build(source: &Path, dir: &Path, target: &TargetSpec, profile: BuildProfile) -> Built {
    std::fs::create_dir_all(dir).unwrap();
    let executable = dir.join("app");
    let source = source.to_path_buf();
    let target = target.clone();
    let built = with_compiler_stack(|| {
        native::build_path_native_backend_with_validation(
            &source,
            &executable,
            target,
            profile,
            Backend::Llvm,
            None,
            false,
            hirsel_engine::Verify::On,
            hirsel_driver::native::Validation::EveryStage,
        )
    })
    .unwrap();
    assert!(built.is_ok(), "{built:?}");
    let debug = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|e| e == "debug"))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&path).unwrap(),
            )
        });
    assert!(
        debug.is_none(),
        "debug information belongs in the executable"
    );
    let bytes = std::fs::read(&executable).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    (bytes, debug)
}

#[test]
fn shipped_links_are_identical_on_repeat_and_under_concurrent_builds_on_both_targets() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("link-kit-determinism");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("main.hir");
    std::fs::write(&source, PROGRAM).unwrap();
    crate::image_fixture::core_manifest(&source, &root.join("main.hir.json"));
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        for profile in [BuildProfile::Default, BuildProfile::Debug] {
            let name = format!("{}-{profile:?}", target.triple);
            let reference = build(&source, &root.join(&name).join("first"), &target, profile);
            let again = build(&source, &root.join(&name).join("again"), &target, profile);
            assert!(again == reference, "{name}: a repeated build differs");
            for jobs in [1, 2, 4] {
                let builds: Vec<Built> = std::thread::scope(|scope| {
                    let handles: Vec<_> = (0..jobs)
                        .map(|job| {
                            let dir = root.join(&name).join(format!("jobs-{jobs}-{job}"));
                            let (source, target) = (&source, &target);
                            scope.spawn(move || build(source, &dir, target, profile))
                        })
                        .collect();
                    handles.into_iter().map(|h| h.join().unwrap()).collect()
                });
                for built in builds {
                    assert!(built == reference, "{name}: a build among {jobs} differs");
                }
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

/// A program that reaches TLS: only its TLS entry names the runtime's TLS engine.
const TLS_PROGRAM: &str = "pub fn main(rt: Io) -> Result[(), NetError] {
  let n = match io.net(rt) {
    Ok(n) => n,
    Error(_) => return Ok(()),
  }
  let plain = net.connect(n, \"localhost\", 443u16, Unbounded())?
  let _secure = tls.connect(n, take plain, \"localhost\", WebPki())?
  Ok(())
}
";

/// A network entry reaches the socket provider without linking TLS.
const NET_PROGRAM: &str = "pub fn main(rt: Io) -> Result[(), NetError] {
  let n = match io.net(rt) {
    Ok(n) => n,
    Error(_) => return Ok(()),
  }
  let _plain = net.connect(n, \"localhost\", 443u16, Unbounded())?
  Ok(())
}
";

const PROCESS_PROGRAM: &str = "pub fn main(rt: Io) -> Result[(), ProcessError] {
  let p = match io.process(rt) {
    Ok(p) => p,
    Error(_) => return Ok(()),
  }
  let command = Command(program: path.of(\"/bin/true\"), args: [], dir: None(), env: None(), own_group: false)
  let _ = proc.run(p, command, b\"\", 64u64, None())?
  Ok(())
}
";

#[test]
fn an_executable_links_each_host_provider_only_when_its_program_reaches_it() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("link-kit-providers");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    // By name: ring's primitives, rustls's client handshake and the compiled root set
    // (TLS), the resolver and sockets (network), and process spawning.
    let providers = |bytes: &[u8]| {
        // Read ELF symbol tables as native::runner does. setsockopt is also
        // used by managed_cores' control socket, so the network marker names
        // its provider constructors rather than that shared libc dependency.
        // Local bindings select provider; concurrent bindings select send_provider.
        let header = elf::FileHeader64::<LittleEndian>::parse(bytes).unwrap();
        let endian = header.endian().unwrap();
        let sections = header.sections(endian, bytes).unwrap();
        let mut names = Vec::new();
        for kind in [elf::SHT_SYMTAB, elf::SHT_DYNSYM] {
            let symbols = sections.symbols(endian, bytes, kind).unwrap();
            for symbol in symbols.iter() {
                names.push(symbol.name(endian, symbols.strings()).unwrap());
            }
        }
        let symbol = |needle: &[u8]| {
            names
                .iter()
                .any(|name| name.windows(needle.len()).any(|w| w == needle))
        };
        [
            symbol(b"ring_core_"),
            symbol(b"ClientConnection"),
            bytes.windows(b"ISRG Root".len()).any(|w| w == b"ISRG Root"),
            symbol(b"getaddrinfo"),
            symbol(b"net_host8provider") || symbol(b"net_host13send_provider"),
            symbol(b"posix_spawnp"),
        ]
    };
    for (name, program, expected) in [
        ("plain", PROGRAM, [false; 6]),
        ("net", NET_PROGRAM, [false, false, false, true, true, false]),
        ("tls", TLS_PROGRAM, [true, true, true, true, true, false]),
        (
            "process",
            PROCESS_PROGRAM,
            [false, false, false, false, false, true],
        ),
    ] {
        let source = root.join(format!("{name}.hir"));
        std::fs::write(&source, program).unwrap();
        let (bytes, _) = build(&source, &root.join(name), &target, BuildProfile::Default);
        assert_eq!(providers(&bytes), expected, "{name}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
