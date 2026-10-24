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
use std::sync::{Arc, Mutex};

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
    /// The kit at `root`, read once per process.
    pub(crate) fn load(root: &Path) -> Result<Arc<Kit>, NativeError> {
        static KITS: Mutex<BTreeMap<PathBuf, Arc<Kit>>> = Mutex::new(BTreeMap::new());
        if let Some(kit) = KITS.lock().unwrap().get(root) {
            return Ok(kit.clone());
        }
        let kit = Arc::new(Self::read(root)?);
        KITS.lock().unwrap().insert(root.to_path_buf(), kit.clone());
        Ok(kit)
    }

    fn read(root: &Path) -> Result<Self, NativeError> {
        let read = |name: &str| {
            std::fs::read(root.join(name))
                .map_err(|e| failed(format!("the link kit {}: {name}: {e}", root.display())))
        };
        let manifest = read("manifest.json")?;
        let digest = String::from_utf8_lossy(&read("digest")?).trim().to_string();
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
        files.sort();
        files.dedup();
        Ok(files)
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
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

/// What a link writes.
#[derive(Clone, Debug)]
pub(crate) enum Kind {
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
        match &self.kind {
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
            args.extend(target.start.iter().map(Into::into));
        }
        args.extend(self.objects.iter().map(Into::into));
        args.extend(self.runtime.map(Into::into));
        if self.unwind {
            args.push((&target.unwind).into());
        }
        args.extend(self.foreign.iter().map(Into::into));
        args.push("--as-needed".into());
        args.extend(target.libraries.iter().map(Into::into));
        if executable {
            args.push((&target.nonshared).into());
            args.extend(target.end.iter().map(Into::into));
        }
        args.push("--gc-sections".into());
        Ok(args)
    }

    /// The build ID: every input that reaches the output, by content (the kit's
    /// files by its digest), and the line with input paths replaced by those
    /// contents, so it never depends on a checkout, cache or scratch directory.
    fn build_id(&self, args: &[OsString]) -> Result<[u8; 20], NativeError> {
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
                    entry.insert(input_identity(path)?);
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

    /// The `lld` command writing `output`, and the build ID it carries.
    pub(crate) fn command(
        &self,
        lld: &Path,
        output: &Path,
    ) -> Result<(Command, [u8; 20]), NativeError> {
        let mut args = self.arguments()?;
        let id = self.build_id(&args)?;
        let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
        args.push(format!("--build-id=0x{hex}").into());
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
            kind: Kind::Executable { loader: None },
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
