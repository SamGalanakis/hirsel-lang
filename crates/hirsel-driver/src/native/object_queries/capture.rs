//! LLVM reproducer inputs in the ordinary product store.
//!
//! LLVM 23 lld/ELF/InputFiles.cpp (readFile), Driver.cpp (getArchiveMembers)
//! and Support/TarWriter.cpp own input discovery and archive production.
//! tar 0.4.46 src/archive.rs:86 and src/entry.rs:279 decode the archive;
//! no linker-script or tar grammar is implemented here.
use super::*;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, PathBuf};
use std::process::{Command, Output as ProcessOutput};

const TOOL: Domain = Domain {
    kind: 0x4f00000b,
    schema: 1,
};
const CAPTURE: Domain = Domain {
    kind: 0x4f00000c,
    schema: 1,
};

/// Bounds supplied by the caller's admission policy, including tar metadata.
#[derive(Clone, Copy)]
pub struct CaptureLimits {
    pub archive_bytes: u64,
    pub files: usize,
}

/// A stored packaged linker, including its loader and complete DSO directory.
/// Its mutable installation is never used by capture or replay.
#[derive(Clone, Copy)]
pub struct FrozenLinker(Reference);

/// One bound invocation and the bytes LLVM read while discovering its inputs.
/// This is content, not a native-validation capability.
#[derive(Clone, Copy)]
pub struct CapturedLink(Reference);

#[derive(Clone, Debug)]
struct FileRecord {
    name: String,
    bytes: WireRef,
}
struct ToolRecord {
    files: Vec<FileRecord>,
}
struct CaptureRecord {
    tool: WireRef,
    directory: String,
    arguments: Vec<String>,
    files: Vec<FileRecord>,
}
snapshot_struct!(FileRecord { name, bytes });
snapshot_struct!(ToolRecord { files });
snapshot_struct!(CaptureRecord {
    tool,
    directory,
    arguments,
    files
});

impl FrozenLinker {
    /// `root` is the pinned native-toolchain:linker package, not the target kit.
    pub fn snapshot(store: &dyn ResultStore, root: &Path) -> Result<Self, String> {
        let mut names = vec!["bin/ld.lld".to_string()];
        for entry in std::fs::read_dir(root.join("lib")).map_err(error)? {
            let entry = entry.map_err(error)?;
            if !entry.file_type().map_err(error)?.is_file() {
                return Err("packaged linker library is not a regular file".into());
            }
            names.push(format!(
                "lib/{}",
                entry
                    .file_name()
                    .to_str()
                    .ok_or("non-UTF8 linker library")?
            ));
        }
        names.sort();
        let files = names
            .into_iter()
            .map(|name| {
                let bytes = store_link_input_reader(
                    store,
                    std::fs::File::open(root.join(&name)).map_err(error)?,
                )?;
                Ok(FileRecord {
                    name,
                    bytes: bytes.into(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        check_tool(&files)?;
        Ok(Self(put_record(store, TOOL, &ToolRecord { files })?))
    }

    pub fn reference(self) -> Reference {
        self.0
    }

    /// Capture runs the stored tool against the original inputs once. Actual
    /// linking later runs the same tool with a closed LLVM VFS of these bytes.
    /// Arguments omit the output; capture/replay control options are reserved.
    /// Host object/archive/script links are supported. External LTO configuration
    /// and pass plugins require their own input admission and are refused here.
    pub fn capture(
        self,
        store: &dyn ResultStore,
        directory: &Path,
        arguments: &[String],
        limits: CaptureLimits,
    ) -> Result<CapturedLink, String> {
        self.capture_fixed(store, directory, arguments, limits, &[])
    }

    fn capture_fixed(
        self,
        store: &dyn ResultStore,
        directory: &Path,
        arguments: &[String],
        limits: CaptureLimits,
        fixed: &[FileRecord],
    ) -> Result<CapturedLink, String> {
        check_arguments(arguments)?;
        let directory = std::fs::canonicalize(directory).map_err(error)?;
        let directory = directory
            .to_str()
            .ok_or("non-UTF8 linker working directory")?
            .to_string();
        let scratch = tempfile::tempdir().map_err(error)?;
        let tool = self.materialize(store, &scratch.path().join("tool"))?;
        let archive = scratch.path().join("inputs.tar");
        let mut command = tool.command();
        command
            .current_dir(&directory)
            .args(["--whole-archive", "--allow-multiple-definition"])
            // Inventory every archive member. Positional archive policy belongs
            // to the actual link, whose original argument order is retained.
            .args(arguments.iter().filter(|arg| *arg != "--no-whole-archive"))
            .arg("--capture-inputs-only")
            .arg(format!("--reproduce={}", archive.display()))
            .arg("-o")
            .arg(scratch.path().join("unused-output"));
        if !fixed.is_empty() {
            let mut roots = Vec::new();
            for (index, file) in fixed.iter().enumerate() {
                let local = scratch.path().join("fixed").join(index.to_string());
                materialize_link_input(store, file.bytes.clone().into(), &local, false)?;
                roots.push(serde_json::json!({"type": "file", "name": file.name, "external-contents": local}));
            }
            let overlay = scratch.path().join("fixed.json");
            std::fs::write(
                &overlay,
                serde_json::to_vec(&serde_json::json!({
                    "version": 0, "use-external-names": false, "fallthrough": true, "roots": roots
                }))
                .map_err(error)?,
            )
            .map_err(error)?;
            command.arg(format!("--input-overlay={}", overlay.display()));
        }
        success(command.output().map_err(error)?)?;
        let file = std::fs::File::open(archive).map_err(error)?;
        if file.metadata().map_err(error)?.len() > limits.archive_bytes {
            return Err("link input archive exceeds admitted bytes".into());
        }
        let files = read_archive(store, file.take(limits.archive_bytes), limits)?;
        Ok(CapturedLink(put_record(
            store,
            CAPTURE,
            &CaptureRecord {
                tool: self.0.into(),
                directory,
                arguments: arguments.to_vec(),
                files,
            },
        )?))
    }

    fn materialize(self, store: &dyn ResultStore, root: &Path) -> Result<LocalTool, String> {
        let record: ToolRecord = get_record(store, TOOL, self.0)?;
        check_tool(&record.files)?;
        for file in record.files {
            materialize_link_input(store, file.bytes.into(), &root.join(file.name), true)?;
        }
        Ok(LocalTool(root.to_path_buf()))
    }
}

impl CapturedLink {
    pub(crate) fn inputs(
        self,
        store: &dyn ResultStore,
    ) -> Result<Vec<(PathBuf, Reference)>, String> {
        Ok(self
            .record(store)?
            .files
            .into_iter()
            .map(|file| (PathBuf::from(file.name), file.bytes.into()))
            .collect())
    }

    /// Identity of the discovered file namespace, excluding the probe's
    /// temporary working directory and argument spelling.
    pub(crate) fn inputs_identity(self, store: &dyn ResultStore) -> Result<ContentHash, String> {
        let files = self.record(store)?.files;
        Ok(hirsel_codec::digest(
            "hirsel.captured-link-inputs.v1",
            "Inputs",
            &files,
        ))
    }

    /// Exact bytes the selected linker read at an absolute input path. This
    /// associates selected platform inputs with the captured probe, not with
    /// another read of a mutable installation.
    pub(crate) fn input(
        self,
        store: &dyn ResultStore,
        path: &Path,
    ) -> Result<Option<Reference>, String> {
        Ok(self
            .record(store)?
            .files
            .into_iter()
            .find(|file| Path::new(&file.name) == path)
            .map(|file| file.bytes.into()))
    }

    pub fn reference(self) -> Reference {
        self.0
    }

    pub fn from_reference(store: &dyn ResultStore, reference: Reference) -> Result<Self, String> {
        let result = Self(reference);
        result.validate(store)?;
        Ok(result)
    }

    /// Call before selecting a Link hit as well as on misses. A descriptor must
    /// not hide missing tool, archive or script content behind a cached image.
    pub fn validate(self, store: &dyn ResultStore) -> Result<(), String> {
        let record = self.record(store)?;
        let tool: ToolRecord = get_record(store, TOOL, record.tool.into())?;
        check_tool(&tool.files)?;
        for file in tool.files.iter().chain(&record.files) {
            validate_link_input(store, file.bytes.clone().into())?;
        }
        Ok(())
    }

    /// No original input pathname is opened. The output destination is the only
    /// invocation parameter supplied after capture.
    pub fn link(self, store: &dyn ResultStore, output: &Path) -> Result<(), String> {
        let record = self.record(store)?;
        let scratch = tempfile::tempdir().map_err(error)?;
        let tool =
            FrozenLinker(record.tool.into()).materialize(store, &scratch.path().join("tool"))?;
        let mut roots = vec![serde_json::json!({
            "type": "directory", "name": record.directory, "contents": []
        })];
        for (index, file) in record.files.into_iter().enumerate() {
            let local = scratch.path().join("files").join(index.to_string());
            materialize_link_input(store, file.bytes.into(), &local, false)?;
            roots.push(
                serde_json::json!({"type": "file", "name": file.name, "external-contents": local}),
            );
        }
        let overlay = scratch.path().join("inputs.json");
        std::fs::write(
            &overlay,
            serde_json::to_vec(&serde_json::json!({
                "version": 0, "use-external-names": false, "fallthrough": false, "roots": roots
            }))
            .map_err(error)?,
        )
        .map_err(error)?;
        let output = if output.is_absolute() {
            output.to_path_buf()
        } else {
            std::env::current_dir().map_err(error)?.join(output)
        };
        let mut command = tool.command();
        command
            .current_dir(scratch.path())
            .args(record.arguments)
            .arg(format!("--input-overlay={}", overlay.display()))
            .arg(format!("--input-working-directory={}", record.directory))
            .arg("-o")
            .arg(&output);
        success(command.output().map_err(error)?)?;
        if !std::fs::metadata(output).map_err(error)?.is_file() {
            return Err("linker produced no regular output".into());
        }
        Ok(())
    }

    fn record(self, store: &dyn ResultStore) -> Result<CaptureRecord, String> {
        let record: CaptureRecord = get_record(store, CAPTURE, self.0)?;
        check_arguments(&record.arguments)?;
        if !Path::new(&record.directory).is_absolute() || record.directory.contains('\0') {
            return Err("invalid captured working directory".into());
        }
        check_order(&record.files)?;
        for file in &record.files {
            if !Path::new(&file.name).is_absolute() || file.name.contains('\0') {
                return Err("invalid captured input name".into());
            }
        }
        Ok(record)
    }
}

const KIT: Domain = Domain {
    kind: 0x4f00000f,
    schema: 2,
};
struct KitRecord {
    root: String,
    triple: String,
    manifest: WireRef,
    digest: String,
    capture: WireRef,
}
snapshot_struct!(KitRecord {
    root,
    triple,
    manifest,
    digest,
    capture
});

/// The selected kit manifest and all startup/archive/script bytes actually read
/// by the stored LLVM linker, captured before program emission. Bind reference()
/// into linker_options before selecting a Link hit.
#[derive(Clone, Copy)]
pub struct FrozenLinkKit(Reference);
impl FrozenLinkKit {
    pub fn snapshot(
        store: &dyn ResultStore,
        root: &Path,
        target: &super::super::TargetSpec,
        linker: FrozenLinker,
        limits: CaptureLimits,
    ) -> Result<Self, String> {
        use std::os::unix::ffi::OsStrExt as _;
        let root = std::fs::canonicalize(root).map_err(error)?;
        let mut manifest = Vec::new();
        std::fs::File::open(root.join("manifest.json"))
            .map_err(error)?
            .take(
                limits
                    .archive_bytes
                    .checked_add(1)
                    .ok_or("kit manifest admission overflow")?,
            )
            .read_to_end(&mut manifest)
            .map_err(error)?;
        if manifest.len() as u64 > limits.archive_bytes {
            return Err("kit manifest exceeds admission".into());
        }
        let digest = std::fs::read_to_string(root.join("digest"))
            .map_err(error)?
            .trim()
            .to_owned();
        // Parse exactly the bytes transported to later planning.
        let parsed = super::super::kit::Kit::from_manifest(&root, &manifest, digest.clone())
            .map_err(|e| e.to_string())?;
        let link = super::super::kit::Link {
            kit: &parsed,
            triple: &target.triple,
            kind: super::super::kit::Kind::Executable { loader: None },
            objects: &[],
            runtime: None,
            startup_order: None,
            unwind: true,
            foreign: &[],
        };
        let (arguments, _) = link.invocation().map_err(|e| e.to_string())?;
        let arguments = arguments
            .iter()
            .map(|arg| {
                std::str::from_utf8(arg.as_bytes())
                    .map(str::to_owned)
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut capture = linker.capture(store, &root, &arguments, limits)?;
        if parsed
            .has_static(&target.triple)
            .map_err(|e| e.to_string())?
        {
            let static_link = super::super::kit::Link {
                kind: super::super::kit::Kind::StaticExecutable,
                ..link
            };
            let (arguments, _) = static_link.invocation().map_err(|e| e.to_string())?;
            let arguments = arguments
                .iter()
                .map(|arg| {
                    arg.to_str()
                        .map(str::to_owned)
                        .ok_or("non-UTF8 static link argument")
                })
                .collect::<Result<Vec<_>, _>>()?;
            let archives = linker.capture(store, &root, &arguments, limits)?;
            let mut combined = capture.record(store)?;
            let mut files = BTreeMap::new();
            for file in combined
                .files
                .into_iter()
                .chain(archives.record(store)?.files)
            {
                if let Some(previous) = files.insert(file.name, file.bytes.clone())
                    && Reference::from(previous) != Reference::from(file.bytes)
                {
                    return Err("kit input changed between archive and DSO capture".into());
                }
            }
            if files.len() > limits.files {
                return Err("combined kit closure exceeds admitted file count".into());
            }
            let mut bytes = 0u64;
            for reference in files.values() {
                bytes = bytes
                    .checked_add(blob_record(store, reference.clone().into())?.len)
                    .ok_or("combined kit closure size overflow")?;
            }
            if bytes > limits.archive_bytes {
                return Err("combined kit closure exceeds admitted bytes".into());
            }
            combined.files = files
                .into_iter()
                .map(|(name, bytes)| FileRecord { name, bytes })
                .collect();
            capture = CapturedLink(put_record(store, CAPTURE, &combined)?);
        }
        let record = KitRecord {
            root: root.to_str().ok_or("non-UTF8 kit root")?.into(),
            triple: target.triple.clone(),
            manifest: store_link_input(store, &manifest)?.into(),
            digest,
            capture: capture.reference().into(),
        };
        let result = Self(put_record(store, KIT, &record)?);
        result.validate(store, target)?;
        Ok(result)
    }
    pub fn reference(self) -> Reference {
        self.0
    }
    pub fn from_reference(
        store: &dyn ResultStore,
        reference: Reference,
        target: &super::super::TargetSpec,
    ) -> Result<Self, String> {
        let result = Self(reference);
        result.validate(store, target)?;
        Ok(result)
    }
    pub fn validate(
        self,
        store: &dyn ResultStore,
        target: &super::super::TargetSpec,
    ) -> Result<(), String> {
        self.load(store, target).map(|_| ())
    }
    pub(crate) fn load(
        self,
        store: &dyn ResultStore,
        target: &super::super::TargetSpec,
    ) -> Result<super::super::kit::Kit, String> {
        let record: KitRecord = get_record(store, KIT, self.0)?;
        if record.triple != target.triple || !Path::new(&record.root).is_absolute() {
            return Err("frozen kit target or root differs".into());
        }
        let manifest = link_input_bytes(store, record.manifest.into())?;
        let mut kit = super::super::kit::Kit::from_manifest(
            Path::new(&record.root),
            &manifest,
            record.digest.clone(),
        )
        .map_err(|e| e.to_string())?;
        let captured = CapturedLink::from_reference(store, record.capture.into())?;
        let captured = captured.record(store)?;
        let files = captured.files;
        let relative = files
            .iter()
            .map(|file| {
                let name = Path::new(&file.name)
                    .strip_prefix(&record.root)
                    .unwrap_or_else(|_| Path::new(&file.name));
                (
                    name.to_string_lossy().into_owned(),
                    Reference::from(file.bytes.clone()),
                )
            })
            .collect::<Vec<_>>();
        let encoding = serde_json::to_vec(&(
            record.triple,
            record.digest,
            Reference::from(captured.tool),
            relative,
        ))
        .map_err(|error| error.to_string())?;
        kit.bind_input_identity(ContentHash::of_parts(
            "hirsel.frozen-kit-build-id.v1",
            [encoding.as_slice()],
        ));
        // Every manifest leaf must be supplied by the closed captured file set.
        for path in kit.link_files(&target.triple).map_err(|e| e.to_string())? {
            if path == Path::new(&record.root).join("manifest.json")
                || path == Path::new(&record.root).join("digest")
            {
                continue;
            }
            if !files.iter().any(|file| Path::new(&file.name) == path) {
                return Err(format!("missing frozen kit leaf {}", path.display()));
            }
        }
        Ok(kit)
    }
    pub(crate) fn capture_selected(
        self,
        store: &dyn ResultStore,
        linker: FrozenLinker,
        selected: CapturedLink,
        plan: &super::super::ExecutableLinkPlan,
        objects: &[PathBuf],
        limits: CaptureLimits,
    ) -> Result<CapturedLink, String> {
        let record: KitRecord = get_record(store, KIT, self.0)?;
        let base = CapturedLink::from_reference(store, record.capture.into())?.record(store)?;
        let selected = selected.record(store)?;
        if Reference::from(base.tool) != linker.reference()
            || Reference::from(selected.tool) != linker.reference()
        {
            return Err("selected platform names another frozen linker".into());
        }
        let mut fixed = BTreeMap::new();
        for file in base.files.into_iter().chain(selected.files) {
            let content: Reference = file.bytes.into();
            if let Some(previous) = fixed.insert(file.name, content)
                && previous != content
            {
                return Err("conflicting selected platform file names".into());
            }
        }
        let files = fixed
            .iter()
            .map(|(name, content)| FileRecord {
                name: name.clone(),
                bytes: (*content).into(),
            })
            .collect::<Vec<_>>();
        let arguments = plan
            .arguments
            .iter()
            .map(|arg| {
                arg.to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "non-UTF8 link argument".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let captured = linker.capture_fixed(store, &plan.directory, &arguments, limits, &files)?;
        for file in captured.record(store)?.files {
            match fixed.get(&file.name) {
                Some(expected) if *expected == Reference::from(file.bytes) => {}
                None if objects.iter().any(|path| path == Path::new(&file.name)) => {}
                _ => {
                    return Err(format!(
                        "final link read outside selected inputs: {}",
                        file.name
                    ));
                }
            }
        }
        Ok(captured)
    }

    pub(crate) fn capture(
        self,
        store: &dyn ResultStore,
        linker: FrozenLinker,
        directory: &Path,
        arguments: &[String],
        limits: CaptureLimits,
    ) -> Result<CapturedLink, String> {
        let record: KitRecord = get_record(store, KIT, self.0)?;
        let base = CapturedLink::from_reference(store, record.capture.into())?.record(store)?;
        if Reference::from(base.tool) != linker.reference() {
            return Err("frozen kit selected another linker".into());
        }
        let captured = linker.capture_fixed(store, directory, arguments, limits, &base.files)?;
        let actual = captured.record(store)?;
        for file in &actual.files {
            if let Some(expected) = base
                .files
                .iter()
                .find(|expected| expected.name == file.name)
            {
                if Reference::from(expected.bytes.clone()) != Reference::from(file.bytes.clone()) {
                    return Err("frozen kit input changed during final capture".into());
                }
            } else if Path::new(&file.name).starts_with(&record.root) {
                return Err("final link read an uncaptured kit input".into());
            }
        }
        Ok(captured)
    }
}

struct LocalTool(PathBuf);
impl LocalTool {
    fn command(&self) -> Command {
        // The package contains this exact loader and its resolved tool DSO
        // closure. Target-library loading is a separate host responsibility.
        let mut command = Command::new(self.0.join("lib/ld-linux-x86-64.so.2"));
        command
            .env_clear()
            .arg("--library-path")
            .arg(self.0.join("lib"))
            .arg(self.0.join("bin/ld.lld"));
        command
    }
}

fn success(output: ProcessOutput) -> Result<(), String> {
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "captured linker {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn check_order(files: &[FileRecord]) -> Result<(), String> {
    if files.windows(2).any(|pair| pair[0].name >= pair[1].name) {
        return Err("captured files are not sorted unique names".into());
    }
    Ok(())
}

fn check_tool(files: &[FileRecord]) -> Result<(), String> {
    check_order(files)?;
    if !files.iter().any(|f| f.name == "bin/ld.lld")
        || !files.iter().any(|f| f.name == "lib/ld-linux-x86-64.so.2")
    {
        return Err("packaged ELF linker lacks its entry or loader".into());
    }
    for file in files {
        let Some((parent, name)) = file.name.split_once('/') else {
            return Err("invalid packaged linker name".into());
        };
        if !matches!(parent, "bin" | "lib")
            || name.is_empty()
            || name.contains('/')
            || matches!(name, "." | "..")
            || name.contains('\0')
        {
            return Err("invalid packaged linker name".into());
        }
    }
    Ok(())
}

fn check_arguments(arguments: &[String]) -> Result<(), String> {
    for arg in arguments {
        if arg.contains('\0')
            || arg.starts_with('@')
            || arg.starts_with("--lto-")
            || arg.starts_with("--thinlto-")
            || arg == "-o"
            || arg.starts_with("-o") && !arg.starts_with("--")
            || [
                "--output",
                "--reproduce",
                "--input-overlay",
                "--input-working-directory",
                "--capture-inputs-only",
                "--dependency-file",
                "--chroot",
                "--remap-inputs",
                "--remap-inputs-file",
                "--load-pass-plugin",
                "--plugin",
                "-plugin",
                "--plugin-opt",
                "-plugin-opt",
                "--mllvm",
                "-mllvm",
            ]
            .iter()
            .any(|prefix| arg == prefix || arg.starts_with(&format!("{prefix}=")))
        {
            return Err("reserved captured-link argument".into());
        }
    }
    Ok(())
}

fn read_archive(
    store: &dyn ResultStore,
    reader: impl Read,
    limits: CaptureLimits,
) -> Result<Vec<FileRecord>, String> {
    let mut archive = tar::Archive::new(reader);
    let mut files = BTreeMap::new();
    let mut response = false;
    let mut version = false;
    let mut count = 0usize;
    for entry in archive.entries().map_err(error)? {
        let mut entry = entry.map_err(error)?;
        count = count.checked_add(1).ok_or("link input count overflow")?;
        if count > limits.files {
            return Err("link input count exceeds admission".into());
        }
        if !entry.header().entry_type().is_file() {
            return Err("non-file in LLVM reproducer".into());
        }
        let path = entry.path().map_err(error)?;
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err("noncanonical LLVM reproducer path".into());
        }
        let path = path.to_str().ok_or("non-UTF8 LLVM reproducer path")?;
        let path = path
            .strip_prefix("inputs/")
            .ok_or("unexpected LLVM reproducer root")?;
        match path {
            "response.txt" | "version.txt" => {
                let seen = if path == "response.txt" {
                    &mut response
                } else {
                    &mut version
                };
                if *seen {
                    return Err("duplicate LLVM reproducer metadata".into());
                }
                *seen = true;
                let size = entry.size();
                if std::io::copy(&mut entry, &mut std::io::sink()).map_err(error)? != size {
                    return Err("truncated LLVM reproducer metadata".into());
                }
            }
            "" => return Err("empty LLVM reproducer path".into()),
            _ => {
                let path = format!("/{path}");
                let size = entry.size();
                if size > limits.archive_bytes {
                    return Err("link input exceeds admitted bytes".into());
                }
                let mut counted = Counted {
                    reader: &mut entry,
                    bytes: 0,
                };
                let bytes = store_link_input_reader(store, &mut counted)?;
                if counted.bytes != size {
                    return Err("truncated LLVM reproducer input".into());
                }
                if let Some(old) = files.insert(path, bytes)
                    && old != bytes
                {
                    return Err("LLVM input changed during capture".into());
                }
            }
        }
    }
    if !response || !version || files.is_empty() {
        return Err("incomplete LLVM reproducer".into());
    }
    Ok(files
        .into_iter()
        .map(|(name, bytes)| FileRecord {
            name,
            bytes: bytes.into(),
        })
        .collect())
}

struct Counted<R> {
    reader: R,
    bytes: u64,
}
impl<R: Read> Read for Counted<R> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let count = self.reader.read(output)?;
        self.bytes += count as u64;
        Ok(count)
    }
}

#[cfg(test)]
#[path = "../../../tests/native-validation/link_capture.rs"]
mod tests;
