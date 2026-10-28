//! The link kit and the one link line every executable and program library is
//! linked with.
//!
//! The kit (`tools/buck2/native-toolchain/link_kit.py`, `HIRSEL_LINK_KIT`) holds
//! every input of a link except the program, the runtime and foreign libraries:
//! per target the glibc 2.28 stub DSOs in the loader's search order, glibc
//! 2.28's startup objects and nonshared archive, LLVM's crtbegin/crtend and a
//! static LLVM libunwind. Its `manifest.json` names them and the `digest` file
//! is the manifest's SHA-256, the kit's identity. No host compiler driver, crt,
//! libgcc or sysroot is on the line, so a link reads only the kit, the pinned
//! `ld.lld`, the program's objects, the runtime archive and foreign inputs.
//!
//! The line is a pure function of the kit, the target, the output kind and the
//! foreign inputs: `ld.lld` runs with a cleared environment, one thread, and a
//! literal build ID hashed from everything that reaches the output.
use super::stages::{NativeError, StageName};
use hirsel_syntax::ContentHash;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

/// One target's part of the kit, as paths in link order.
#[derive(Debug)]
struct Target {
    interpreter: String,
    options: Vec<String>,
    start: Vec<PathBuf>,
    unwind: PathBuf,
    libraries: Vec<PathBuf>,
    nonshared: PathBuf,
    end: Vec<PathBuf>,
    static_link: Option<Static>,
}

#[derive(Debug)]
struct Static {
    start: Vec<PathBuf>,
    libraries: Vec<PathBuf>,
    personality: PathBuf,
}

/// A loaded kit: its identity and each target's inputs.
#[derive(Debug)]
pub(crate) struct Kit {
    root: PathBuf,
    digest: String,
    targets: BTreeMap<String, Target>,
}

fn failed(message: impl Into<String>) -> NativeError {
    NativeError::new(StageName::Link, message)
}

impl Kit {
    /// Read and check the currently selected kit manifest.
    pub(crate) fn load(root: &Path) -> Result<Arc<Kit>, NativeError> {
        Self::read(root).map(Arc::new)
    }

    fn read(root: &Path) -> Result<Self, NativeError> {
        let read = |name: &str| {
            std::fs::read(root.join(name))
                .map_err(|e| failed(format!("the link kit {}: {name}: {e}", root.display())))
        };
        let manifest = read("manifest.json")?;
        let digest = String::from_utf8_lossy(&read("digest")?).trim().to_string();
        Self::from_manifest(root, &manifest, digest)
    }

    pub(crate) fn from_manifest(
        root: &Path,
        manifest: &[u8],
        digest: String,
    ) -> Result<Self, NativeError> {
        let actual: String = {
            use sha2::Digest;
            sha2::Sha256::digest(&manifest)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        if digest != actual {
            return Err(failed(format!(
                "the link kit {} has digest {digest} but its manifest hashes to {actual}",
                root.display()
            )));
        }
        let manifest: serde_json::Value = serde_json::from_slice(&manifest)
            .map_err(|e| failed(format!("the link kit manifest: {e}")))?;
        let mut targets = BTreeMap::new();
        for (triple, target) in manifest["targets"].as_object().into_iter().flatten() {
            let directory = root.join(triple);
            let text = |key: &str| target[key].as_str().map(str::to_string);
            let list = |key: &str| -> Option<Vec<String>> {
                target[key]
                    .as_array()?
                    .iter()
                    .map(|v| v.as_str().map(str::to_string))
                    .collect()
            };
            let paths = |key: &str| -> Option<Vec<PathBuf>> {
                Some(list(key)?.iter().map(|p| directory.join(p)).collect())
            };
            let parsed = (|| {
                Some(Target {
                    interpreter: text("interpreter")?,
                    options: list("options")?,
                    start: paths("start")?,
                    unwind: directory.join(text("unwind")?),
                    libraries: paths("libraries")?,
                    nonshared: directory.join(text("nonshared")?),
                    end: paths("end")?,
                    static_link: if target.get("static").is_some() {
                        let input = &target["static"];
                        let paths = |key: &str| -> Option<Vec<PathBuf>> {
                            input[key]
                                .as_array()?
                                .iter()
                                .map(|value| Some(directory.join(value.as_str()?)))
                                .collect()
                        };
                        Some(Static {
                            start: paths("start")?,
                            libraries: paths("libraries")?,
                            personality: directory.join(input["personality"].as_str()?),
                        })
                    } else {
                        None
                    },
                })
            })()
            .ok_or_else(|| failed(format!("the link kit manifest's {triple} is malformed")))?;
            targets.insert(triple.clone(), parsed);
        }
        Ok(Self {
            root: root.to_path_buf(),
            digest,
            targets,
        })
    }

    /// Replace the manifest-only identity with the normalized captured input
    /// closure when planning with a frozen kit. Legacy kit lines keep their ID.
    pub(crate) fn bind_input_identity(&mut self, identity: ContentHash) {
        self.digest = identity
            .0
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
    }

    /// Exact files the existing executable link line reads for one target.
    /// Ordering remains in the manifest; the snapshot itself is a file set.
    pub(crate) fn link_files(&self, triple: &str) -> Result<Vec<PathBuf>, NativeError> {
        let target = self.target(triple)?;
        let mut files = vec![self.root.join("manifest.json"), self.root.join("digest")];
        files.extend(target.start.iter().cloned());
        files.push(target.unwind.clone());
        files.extend(target.libraries.iter().cloned());
        files.push(target.nonshared.clone());
        files.extend(target.end.iter().cloned());
        if let Some(link) = &target.static_link {
            files.extend(link.start.iter().cloned());
            files.extend(link.libraries.iter().cloned());
            files.push(link.personality.clone());
        }
        files.sort();
        files.dedup();
        Ok(files)
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn has_static(&self, triple: &str) -> Result<bool, NativeError> {
        Ok(self.target(triple)?.static_link.is_some())
    }

    pub(crate) fn executable_kind(
        &self,
        triple: &str,
        static_foreign: bool,
        loader: Option<(PathBuf, PathBuf)>,
    ) -> Result<Kind, NativeError> {
        if loader.is_none() && static_foreign && self.has_static(triple)? {
            Ok(Kind::StaticExecutable)
        } else {
            Ok(Kind::Executable { loader })
        }
    }

    /// `triple`'s stub DSOs, in the loader's search order.
    pub(crate) fn libraries(&self, triple: &str) -> Result<&[PathBuf], NativeError> {
        Ok(&self.target(triple)?.libraries)
    }

    fn target(&self, triple: &str) -> Result<&Target, NativeError> {
        self.targets
            .get(triple)
            .ok_or_else(|| failed(format!("the link kit has no {triple}")))
    }

    /// The kit's stub DSO for foreign library `name` (`c` for `libc.so.6`), if
    /// it has one.
    pub(crate) fn library(&self, triple: &str, name: &str) -> Option<PathBuf> {
        let prefix = format!("lib{name}.so.");
        self.target(triple).ok()?.libraries.iter().find_map(|path| {
            let file = path.file_name()?.to_str()?;
            file.starts_with(&prefix).then(|| path.clone())
        })
    }
}

/// Archives and ELF relocatables need no runtime loader. Linker scripts keep
/// dynamic resolution because their selected leaves can include shared libraries.
pub(crate) fn static_inputs(paths: &[PathBuf]) -> Result<bool, NativeError> {
    use std::io::Read;
    for path in paths {
        let mut magic = [0; 64];
        let mut file = std::fs::File::open(path).map_err(|e| failed(e.to_string()))?;
        let count = file.read(&mut magic).map_err(|e| failed(e.to_string()))?;
        if !static_input(&magic[..count]) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn static_input(bytes: &[u8]) -> bool {
    use object::read::elf::FileHeader as _;
    bytes.starts_with(b"!<arch>\n")
        || bytes.starts_with(b"!<thin>\n")
        || object::elf::FileHeader64::<object::LittleEndian>::parse(bytes).is_ok_and(|header| {
            header.is_little_endian() && header.e_type(object::LittleEndian) == object::elf::ET_REL
        })
}

/// What a link writes.
#[derive(Clone, Debug)]
pub(crate) enum Kind {
    /// A static PIE with the kit's archive closure and compiler-rt personality.
    StaticExecutable,
    /// A position-independent executable, loaded by the target's canonical
    /// interpreter or, for a checking tool's launch, by `loader` with
    /// `library_path` as its run path.
    Executable { loader: Option<(PathBuf, PathBuf)> },
    /// A program image a runner or host loads: units and launcher objects only,
    /// bound to themselves (`-Bsymbolic`), with the runtime left to the process
    /// that loads it and only `script`'s symbols exported.
    Image { script: PathBuf },
    /// An executable exporting `exports` (each also an undefined root, so its
    /// archive member is linked) to the images it loads. With `main`, that
    /// runtime function is its `main` and no object defines one.
    Host {
        exports: Vec<String>,
        main: Option<String>,
    },
}

/// One link: the program's objects in shipped order, the runtime archive (or,
/// for a program library, the runtime's compiler builtins), and the foreign
/// inputs, already resolved to files.
pub(crate) struct Link<'a> {
    pub kit: &'a Kit,
    pub triple: &'a str,
    pub kind: Kind,
    pub objects: &'a [PathBuf],
    pub runtime: Option<&'a Path>,
    /// The kit's static unwinder, which the runtime archive's backtrace and
    /// unwind imports need.
    pub unwind: bool,
    pub foreign: &'a [PathBuf],
}

fn input_identity(path: &Path) -> Result<ContentHash, NativeError> {
    super::file_digest::file(path)
        .map(|hash| hash.content)
        .map_err(|e| failed(format!("{}: {e}", path.display())))
}

impl Link<'_> {
    /// The `ld.lld` arguments, without the output.
    fn arguments(&self) -> Result<Vec<OsString>, NativeError> {
        let target = self.kit.target(self.triple)?;
        let mut args: Vec<OsString> = ["-nostdlib", "--threads=1", "-z", "now", "-z", "relro"]
            .into_iter()
            .chain(["-z", "noexecstack", "--eh-frame-hdr", "--hash-style=gnu"])
            .map(OsString::from)
            .collect();
        args.extend(target.options.iter().map(OsString::from));
        let executable = !matches!(self.kind, Kind::Image { .. });
        let static_link = if matches!(self.kind, Kind::StaticExecutable) {
            Some(
                target
                    .static_link
                    .as_ref()
                    .ok_or_else(|| failed("the selected link kit has no static archive closure"))?,
            )
        } else {
            None
        };
        match &self.kind {
            Kind::StaticExecutable => {
                args.extend(
                    ["-static", "-pie", "--no-dynamic-linker", "-z", "text"]
                        .into_iter()
                        .map(OsString::from),
                );
            }
            Kind::Executable { loader: None } | Kind::Host { .. } => {
                args.push("-pie".into());
                args.push(format!("--dynamic-linker={}", target.interpreter).into());
            }
            Kind::Executable {
                loader: Some((loader, library_path)),
            } => {
                args.push("-pie".into());
                let mut option = OsString::from("--dynamic-linker=");
                option.push(loader);
                args.push(option);
                args.push("-rpath".into());
                args.push(library_path.into());
            }
            Kind::Image { script } => {
                args.extend(["-shared".into(), "-Bsymbolic".into()]);
                let mut option = OsString::from("--version-script=");
                option.push(script);
                args.push(option);
            }
        }
        if let Kind::Host { exports, main } = &self.kind {
            // The images resolve their runtime imports against the host's one runtime.
            for export in exports {
                args.push(format!("--undefined={export}").into());
                args.push(format!("--export-dynamic-symbol={export}").into());
            }
            if let Some(main) = main {
                args.push(format!("--undefined={main}").into());
                args.push(format!("--defsym=main={main}").into());
            }
        }
        if executable {
            args.extend(
                static_link
                    .map_or(&target.start, |link| &link.start)
                    .iter()
                    .map(Into::into),
            );
        }
        args.extend(self.objects.iter().map(Into::into));
        if static_link.is_some() {
            args.push("--start-group".into());
        }
        args.extend(self.runtime.map(Into::into));
        if self.unwind {
            args.push((&target.unwind).into());
        }
        args.extend(self.foreign.iter().map(Into::into));
        if let Some(link) = static_link {
            args.push((&link.personality).into());
            args.extend(link.libraries.iter().map(Into::into));
            args.push("--end-group".into());
        } else {
            args.push("--as-needed".into());
            args.extend(target.libraries.iter().map(Into::into));
        }
        if executable {
            if static_link.is_none() {
                args.push((&target.nonshared).into());
            }
            args.extend(target.end.iter().map(Into::into));
        }
        args.push("--gc-sections".into());
        Ok(args)
    }

    /// The build ID: every input that reaches the output, by content (the kit's
    /// files by its digest), and the line with input paths replaced by those
    /// contents, so it never depends on a checkout, cache or scratch directory.
    fn build_id(
        &self,
        args: &[OsString],
        input: &mut impl FnMut(&Path) -> Result<ContentHash, NativeError>,
    ) -> Result<[u8; 20], NativeError> {
        // The pinned ld.lld is the loaded libLLVM's release (`hirsel_llvm::PINNED_LLVM`).
        let mut parts: Vec<Vec<u8>> = vec![
            self.kit.digest.as_bytes().to_vec(),
            format!("ld.lld {}", hirsel_llvm::PINNED_LLVM).into_bytes(),
        ];
        let named: BTreeMap<&Path, ContentHash> = {
            let mut named = BTreeMap::new();
            for path in self
                .objects
                .iter()
                .chain(self.foreign)
                .map(PathBuf::as_path)
                .chain(self.runtime)
                .chain(match &self.kind {
                    Kind::Image { script } => Some(script.as_path()),
                    _ => None,
                })
            {
                if let std::collections::btree_map::Entry::Vacant(entry) = named.entry(path) {
                    entry.insert(input(path)?);
                }
            }
            named
        };
        for arg in args {
            let path = Path::new(arg);
            let arg = arg.as_bytes();
            let script = arg
                .strip_prefix(b"--version-script=")
                .map(|script| Path::new(std::ffi::OsStr::from_bytes(script)));
            let bytes = match named.get(script.unwrap_or(path)) {
                Some(hash) => [b"content:" as &[u8], &hash.0].concat(),
                None => match path.strip_prefix(&self.kit.root) {
                    Ok(inside) => [b"kit:" as &[u8], inside.as_os_str().as_bytes()].concat(),
                    Err(_) => arg.to_vec(),
                },
            };
            parts.push(bytes);
        }
        let hash = ContentHash::of_parts("hirsel.build-id.v1", parts.iter().map(Vec::as_slice));
        Ok(hash.0[..20].try_into().unwrap())
    }

    /// Shared invocation for live commands and stored-tool capture. The output
    /// destination and executable tool are deliberately supplied by the consumer.
    pub(crate) fn invocation(&self) -> Result<(Vec<OsString>, [u8; 20]), NativeError> {
        self.invocation_with(&mut input_identity)
    }

    /// The same link line with identities supplied by an immutable selected
    /// platform. This never reopens its runtime/foreign source installation.
    pub(crate) fn invocation_with(
        &self,
        input: &mut impl FnMut(&Path) -> Result<ContentHash, NativeError>,
    ) -> Result<(Vec<OsString>, [u8; 20]), NativeError> {
        let mut args = self.arguments()?;
        let id = self.build_id(&args, input)?;
        let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
        args.push(format!("--build-id=0x{hex}").into());
        Ok((args, id))
    }

    /// The `lld` command writing `output`, and the build ID it carries.
    pub(crate) fn command(
        &self,
        lld: &Path,
        output: &Path,
    ) -> Result<(Command, [u8; 20]), NativeError> {
        let (args, id) = self.invocation()?;
        let mut command = Command::new(lld);
        command.env_clear().args(args).arg("-o").arg(output);
        // The pinned tree's ld.lld loads its own libLLVM.
        if let Some(tree) = lld.parent().and_then(Path::parent) {
            command.env("LD_LIBRARY_PATH", tree.join("lib"));
        }
        Ok((command, id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_rechecks_changed_and_corrupt_manifests_at_the_same_path() {
        fn write(root: &Path, option: &str) {
            use sha2::Digest;
            let bytes = serde_json::to_vec(&serde_json::json!({
                "targets": { "test": {
                    "interpreter": "/loader", "options": [option],
                    "start": [], "unwind": "unwind.a", "libraries": [],
                    "nonshared": "nonshared.a", "end": []
                }}
            }))
            .unwrap();
            let digest = sha2::Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            std::fs::write(root.join("manifest.json"), bytes).unwrap();
            std::fs::write(root.join("digest"), digest).unwrap();
        }
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "first");
        let first = Kit::load(root.path()).unwrap();
        assert_eq!(first.target("test").unwrap().options, ["first"]);
        write(root.path(), "second");
        let second = Kit::load(root.path()).unwrap();
        assert_ne!(first.digest, second.digest);
        assert_eq!(second.target("test").unwrap().options, ["second"]);
        std::fs::write(
            root.path().join("manifest.json"),
            b"corrupt current manifest",
        )
        .unwrap();
        assert!(Kit::load(root.path()).is_err());
    }
    use hirsel_lir::emission::OptLevel;
    use hirsel_lir::layout_types::TargetSpec;

    fn input(name: &str) -> PathBuf {
        PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} is a test input")))
    }

    /// A `main` that keeps the runtime's launch entry, so the link takes in the
    /// runtime's closure, the unwinder and the stubs.
    fn object(target: &TargetSpec, dir: &Path) -> PathBuf {
        let ir = format!(
            "target triple = \"{}\"\ndeclare void @hirsel_rt_v2_launch()\n\
             @keep = global ptr @hirsel_rt_v2_launch\n\
             define i32 @main() {{\n ret i32 0\n}}\n",
            target.triple
        );
        let compiled = hirsel_llvm::compile_ir(&ir, target, OptLevel::None).unwrap();
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("main.o");
        std::fs::write(&path, compiled.bytes).unwrap();
        path
    }

    fn link(target: &TargetSpec, dir: &Path, extra: &[&str]) -> (Vec<u8>, [u8; 20]) {
        let kit = Kit::load(&input("HIRSEL_LINK_KIT")).unwrap();
        let runtime = input("HIRSEL_RUNTIME_ARCHIVES")
            .join(&target.triple)
            .join("default/libhirsel_rt.a");
        let objects = [object(target, dir)];
        let link = Link {
            kit: &kit,
            triple: &target.triple,
            kind: kit.executable_kind(&target.triple, true, None).unwrap(),
            objects: &objects,
            runtime: Some(&runtime),
            unwind: true,
            foreign: &[],
        };
        let output = dir.join("program");
        let lld = input("LLVM_SYS_231_PREFIX").join("bin/ld.lld");
        let (mut command, id) = link.command(&lld, &output).unwrap();
        let ran = command.args(extra).output().unwrap();
        assert!(ran.status.success(), "{ran:?}");
        (std::fs::read(output).unwrap(), id)
    }

    #[test]
    fn kit_links_depend_on_neither_linker_threads_nor_directories() {
        let root_scratch = tempfile::Builder::new()
            .prefix("hirsel-kit-")
            .tempdir()
            .unwrap();
        let root = root_scratch.path().to_path_buf();
        for target in [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ] {
            let (pinned, id) = link(&target, &root.join("a"), &[]);
            // A later `--threads` overrides the pinned one.
            let (threaded, _) = link(&target, &root.join("b/deeper"), &["--threads=8"]);
            assert!(
                pinned == threaded,
                "{}: thread count changed the bytes",
                target.triple
            );
            let (again, again_id) = link(&target, &root.join("c"), &[]);
            assert_eq!((again == pinned, again_id), (true, id), "{}", target.triple);
            let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
            assert!(
                pinned.windows(20).any(|w| w == id),
                "{}: build ID {hex} is not in the output",
                target.triple
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn static_kit_bootstraps_threads_tls_and_retained_runtime_on_both_targets() {
        let root = tempfile::tempdir().unwrap();
        let kit = Kit::load(&input("HIRSEL_LINK_KIT")).unwrap();
        let llvm = input("LLVM_SYS_231_PREFIX");
        let source = root.path().join("main.c");
        std::fs::write(
            &source,
            r#"
extern void hirsel_rt_v2_launch(void);
static void (*volatile keep)(void) = hirsel_rt_v2_launch;
extern int pthread_create(unsigned long *, const void *, void *(*)(void *), void *);
extern int pthread_join(unsigned long, void **);
extern int getaddrinfo(const char *, const char *, const void *, void **);
extern void freeaddrinfo(void *);
static __thread unsigned value;
static void *worker(void *argument) {
    value = (unsigned)(unsigned long)argument;
    return (void *)(unsigned long)value;
}
int main(void) {
    unsigned long thread;
    void *result = (void *)0;
    value = 17;
    if (pthread_create(&thread, (void *)0, worker, (void *)42) != 0) return 2;
    if (pthread_join(thread, &result) != 0) return 3;
    void *addresses = (void *)0;
    if (getaddrinfo("localhost", "80", (void *)0, &addresses) != 0) return 5;
    if (addresses == (void *)0) return 6;
    freeaddrinfo(addresses);
    return value == 17 && result == (void *)42 && keep != (void *)0 ? 0 : 4;
}
"#,
        )
        .unwrap();
        for target in [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ] {
            let object = root.path().join(format!("{}.o", target.triple));
            let compiled = Command::new(llvm.join("bin/clang"))
                .env_clear()
                .env("LD_LIBRARY_PATH", llvm.join("lib"))
                .arg(format!("--target={}", target.triple))
                .args(["-O2", "-fPIC", "-nostdlibinc", "-c"])
                .arg(&source)
                .arg("-o")
                .arg(&object)
                .output()
                .unwrap();
            assert!(compiled.status.success(), "{compiled:?}");
            let runtime = input("HIRSEL_RUNTIME_ARCHIVES")
                .join(&target.triple)
                .join("default/libhirsel_rt.a");
            let objects = [object];
            let link = Link {
                kit: &kit,
                triple: &target.triple,
                kind: kit.executable_kind(&target.triple, true, None).unwrap(),
                objects: &objects,
                runtime: Some(&runtime),
                unwind: true,
                foreign: &[],
            };
            let executable = root.path().join(&target.triple);
            let (mut command, _) = link.command(&llvm.join("bin/ld.lld"), &executable).unwrap();
            let linked = command.output().unwrap();
            assert!(linked.status.success(), "{}: {linked:?}", target.triple);
            assert!(
                super::super::object_queries::runtime::is_static_executable(&executable).unwrap()
            );
            let mut command = if target.triple.starts_with("aarch64") {
                let mut command = Command::new(input("HIRSEL_QEMU_AARCH64"));
                command.arg(&executable);
                command
            } else {
                Command::new(&executable)
            };
            command.env_clear();
            let ran =
                super::super::link::bounded(&mut command, std::time::Duration::from_secs(120))
                    .unwrap();
            assert!(ran.status.success(), "{}: {ran:?}", target.triple);
        }
    }

    #[test]
    #[ignore = "requires the executable-plan-proof packaged linker inputs"]
    fn stored_linker_replays_shared_executable_plans_for_both_targets() {
        use crate::native::object_queries::capture::{CaptureLimits, CapturedLink, FrozenLinker};
        use crate::native::{ExecutableLinkInputs, plan_executable_inputs};
        use hirsel_engine::store::MemStore;
        let store = MemStore::default();
        let linker = FrozenLinker::snapshot(&store, &input("HIRSEL_CAPTURE_LINKER")).unwrap();
        let root = tempfile::tempdir().unwrap();
        for target in [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ] {
            let original = root.path().join(&target.triple);
            let (expected, id) = link(&target, &original, &[]);
            let runtime = input("HIRSEL_RUNTIME_ARCHIVES")
                .join(&target.triple)
                .join("default/libhirsel_rt.a");
            let plan = plan_executable_inputs(
                &target,
                &original,
                &[original.join("main.o")],
                ExecutableLinkInputs {
                    kit: &input("HIRSEL_LINK_KIT"),
                    runtime: &runtime,
                    foreign: &[],
                    loader: None,
                },
            )
            .unwrap();
            assert_eq!(plan.build_id, id);
            assert!(!plan.arguments.iter().any(|arg| arg == "-o"));
            // The selected package and Ubuntu's LLD differ in .comment producer
            // text. Keep the stock kit-line oracle for every allocated section,
            // then compare replay byte-for-byte with this exact selected tool.
            let package = input("HIRSEL_CAPTURE_LINKER");
            let direct = original.join("selected-tool-output");
            let output = Command::new(package.join("lib/ld-linux-x86-64.so.2"))
                .env_clear()
                .arg("--library-path")
                .arg(package.join("lib"))
                .arg(package.join("bin/ld.lld"))
                .args(&plan.arguments)
                .arg("-o")
                .arg(&direct)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let selected = std::fs::read(&direct).unwrap();
            {
                use object::{Object as _, ObjectSection as _};
                let stock = object::File::parse(expected.as_slice()).unwrap();
                let actual = object::File::parse(selected.as_slice()).unwrap();
                for section in stock.sections() {
                    if matches!(section.flags(), object::SectionFlags::Elf { sh_flags, .. } if sh_flags.contains(object::elf::SHF_ALLOC))
                    {
                        let other = actual.section_by_name(section.name().unwrap()).unwrap();
                        assert!(
                            section.data().unwrap() == other.data().unwrap(),
                            "selected tool changed allocated section {}",
                            section.name().unwrap()
                        );
                    }
                }
            }
            let expected = selected;
            let captured = plan
                .capture(
                    &store,
                    linker,
                    CaptureLimits {
                        archive_bytes: 64 * 1024 * 1024,
                        files: 256,
                    },
                )
                .unwrap();
            std::fs::remove_dir_all(&original).unwrap();
            let captured = CapturedLink::from_reference(&store, captured.reference()).unwrap();
            let output = root.path().join("replayed");
            captured.link(&store, &output).unwrap();
            assert_eq!(
                std::fs::read(output).unwrap(),
                expected,
                "{}",
                target.triple
            );
        }
    }

    #[test]
    #[ignore = "requires frozen-kit-proof packaged linker inputs"]
    fn frozen_kit_binds_transitive_leaf_edits_and_plans_without_originals() {
        use crate::native::object_queries::capture::{CaptureLimits, FrozenLinkKit, FrozenLinker};
        use crate::native::{FrozenExecutableLinkInputs, plan_frozen_executable_inputs};
        use hirsel_engine::store::MemStore;
        use sha2::Digest;
        let store = MemStore::default();
        let linker = FrozenLinker::snapshot(&store, &input("HIRSEL_CAPTURE_LINKER")).unwrap();
        let llvm = input("LLVM_SYS_231_PREFIX");
        let root = tempfile::tempdir().unwrap();
        let limits = CaptureLimits {
            archive_bytes: 16 << 20,
            files: 64,
        };
        for target in [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ] {
            let kit = root.path().join(&target.triple);
            let files = kit.join(&target.triple);
            std::fs::create_dir_all(&files).unwrap();
            let assemble = |name: &str, source: &str| {
                let text = files.join(format!("{name}.s"));
                let object = files.join(format!("{name}.o"));
                std::fs::write(&text, source).unwrap();
                let output = Command::new(llvm.join("bin/llvm-mc"))
                    .arg(format!("-triple={}", target.triple))
                    .args(["-filetype=obj", "-o"])
                    .arg(&object)
                    .arg(text)
                    .env_clear()
                    .env("LD_LIBRARY_PATH", llvm.join("lib"))
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                object
            };
            let arm = target.triple.starts_with("aarch64");
            assemble(
                "start",
                if arm {
                    ".globl _start\n_start:\n bl leaf\n mov x8,#93\n svc #0\n"
                } else {
                    ".globl _start\n_start:\n call leaf\n mov %rax,%rdi\n mov $60,%rax\n syscall\n"
                },
            );
            std::fs::write(files.join("empty.a"), b"!<arch>\n").unwrap();
            std::fs::write(files.join("script.ld"), b"INPUT(leaf.o)\n").unwrap();
            let manifest = serde_json::to_vec(&serde_json::json!({ "targets": { (target.triple.clone()): {
                "interpreter": "/unused/loader", "options": [], "start": ["start.o"],
                "unwind": "empty.a", "libraries": ["script.ld"], "nonshared": "empty.a", "end": []
            }}}))
            .unwrap();
            std::fs::write(kit.join("manifest.json"), &manifest).unwrap();
            std::fs::write(
                kit.join("digest"),
                format!("{:x}", sha2::Sha256::digest(&manifest)),
            )
            .unwrap();
            let mut versions = Vec::new();
            for value in [42, 43] {
                assemble(
                    "leaf",
                    &if arm {
                        format!(".globl leaf\nleaf:\n mov x0,#{value}\n ret\n")
                    } else {
                        format!(".globl leaf\nleaf:\n mov ${value},%rax\n ret\n")
                    },
                );
                versions
                    .push(FrozenLinkKit::snapshot(&store, &kit, &target, linker, limits).unwrap());
            }
            assert_ne!(
                versions[0].reference(),
                versions[1].reference(),
                "manifest is unchanged; actual transitive bytes bind the hit identity"
            );
            // The same bytes at another kit root must retain their build ID.
            let relocated = root.path().join(format!("relocated-{}", target.triple));
            std::fs::create_dir_all(relocated.join(&target.triple)).unwrap();
            for name in ["manifest.json", "digest"] {
                std::fs::copy(kit.join(name), relocated.join(name)).unwrap();
            }
            for entry in std::fs::read_dir(&files).unwrap() {
                let entry = entry.unwrap();
                std::fs::copy(
                    entry.path(),
                    relocated.join(&target.triple).join(entry.file_name()),
                )
                .unwrap();
            }
            versions.push(
                FrozenLinkKit::snapshot(&store, &relocated, &target, linker, limits).unwrap(),
            );
            let planned = root.path().join(format!("planned-{}", target.triple));
            std::fs::create_dir_all(&planned).unwrap();
            let runtime = planned.join("runtime.a");
            std::fs::write(&runtime, b"!<arch>\n").unwrap();
            std::fs::remove_dir_all(&kit).unwrap();
            std::fs::remove_dir_all(&relocated).unwrap();
            let mut images = Vec::new();
            let mut identities = Vec::new();
            for kit in versions {
                let kit = FrozenLinkKit::from_reference(&store, kit.reference(), &target).unwrap();
                let plan = plan_frozen_executable_inputs(
                    &target,
                    &planned,
                    &[],
                    FrozenExecutableLinkInputs {
                        store: &store,
                        kit,
                        runtime: &runtime,
                        foreign: &[],
                        loader: None,
                    },
                )
                .unwrap();
                identities.push(plan.build_id);
                let captured = plan.capture(&store, linker, limits).unwrap();
                let output = planned.join("program");
                captured.link(&store, &output).unwrap();
                images.push(std::fs::read(output).unwrap());
            }
            assert_ne!(
                images[0], images[1],
                "different frozen leaf code must reach each image"
            );
            assert_ne!(
                identities[0], identities[1],
                "transitive leaf bytes belong to the build ID"
            );
            assert_eq!(
                identities[1], identities[2],
                "kit relocation is not an emitted input"
            );
            assert!(
                images[1] == images[2],
                "relocated frozen kits must link identical bytes"
            );
        }
    }

    /// The kit carries no compiler builtins beyond the runtime's own
    /// `compiler_builtins`: every member of every archive variant, linked whole,
    /// resolves against the kit alone.
    #[test]
    fn whole_runtime_archives_need_no_builtins_beyond_their_own() {
        let root_scratch = tempfile::Builder::new()
            .prefix("hirsel-kit-whole-")
            .tempdir()
            .unwrap();
        let root = root_scratch.path().to_path_buf();
        let kit = Kit::load(&input("HIRSEL_LINK_KIT")).unwrap();
        let lld = input("LLVM_SYS_231_PREFIX").join("bin/ld.lld");
        for target in [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ] {
            let objects = [object(&target, &root.join(&target.triple))];
            let variants = input("HIRSEL_RUNTIME_ARCHIVES").join(&target.triple);
            let mut linked = 0;
            for entry in std::fs::read_dir(&variants).unwrap() {
                let runtime = entry.unwrap().path().join("libhirsel_rt.a");
                if !runtime.is_file() {
                    continue;
                }
                let link = Link {
                    kit: &kit,
                    triple: &target.triple,
                    kind: Kind::Executable { loader: None },
                    objects: &objects,
                    runtime: Some(&runtime),
                    unwind: true,
                    foreign: &[],
                };
                let output = root.join(&target.triple).join("program");
                let (command, _) = link.command(&lld, &output).unwrap();
                let mut whole = Command::new(command.get_program());
                whole.env_clear();
                for (name, value) in command.get_envs() {
                    if let Some(value) = value {
                        whole.env(name, value);
                    }
                }
                for argument in command.get_args() {
                    if argument == runtime.as_os_str() {
                        whole.args([
                            "--whole-archive".as_ref(),
                            argument,
                            "--no-whole-archive".as_ref(),
                        ]);
                    } else {
                        whole.arg(argument);
                    }
                }
                let ran = whole.output().unwrap();
                assert!(
                    ran.status.success(),
                    "{}: {}",
                    runtime.display(),
                    String::from_utf8_lossy(&ran.stderr)
                );
                linked += 1;
            }
            assert!(
                linked > 0,
                "no runtime archives under {}",
                variants.display()
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
