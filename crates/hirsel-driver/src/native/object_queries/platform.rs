//! Select immutable link inputs and their declared DT_NEEDED runtime before
//! selecting an S4 Link key. LLVM owns the probe ELF; glibc owns resolution.
//! Arbitrary future dlopen is outside this capability's declared-load scope.
use super::{capture::*, runtime::*, *};
use sha2::{Digest, Sha256};
use std::{ffi::OsStr, fs::File, io::Read, path::PathBuf, time::Duration};

/// Filename-sensitive ordered linker input. Contents are ordinary-store data.
pub struct NamedLinkInput {
    pub name: String,
    pub content: Reference,
}
/// A current immutable input observation; callers must retain these as current
/// dependency reads on both hits and misses.
pub struct PlatformRead {
    pub path: PathBuf,
    pub content: Reference,
}
/// Explicit selected runtime policy; no host environment or ld.so.cache lookup.
pub struct RuntimeSelection<'a> {
    pub loader: &'a Path,
    pub auditor: &'a Path,
    pub library_path: &'a OsStr,
    pub emulator: Option<&'a Path>,
    pub timeout: Duration,
    pub events_bytes: u64,
    pub limits: RuntimeLimits,
}
pub struct PlatformCaptureInputs<'a> {
    pub linker: FrozenLinker,
    pub kit: FrozenLinkKit,
    pub runtime_archive: &'a Path,
    pub foreign: &'a [PathBuf],
    pub runtime: RuntimeSelection<'a>,
    pub limits: CaptureLimits,
}
/// No public constructor can assert runtime closure. capture() uses a real
/// target ELF probe over every selected kit DSO and ordered foreign input, then
/// requires the existing closed glibc resolver to admit that exact graph.
/// The final linked image must additionally pass validate_image() before use.
pub struct CapturedPlatform {
    linker: FrozenLinker,
    kit: FrozenLinkKit,
    runtime_archive: NamedLinkInput,
    foreign: Vec<NamedLinkInput>,
    runtime: FrozenRuntime,
    target: ContentHash,
    identity: ContentHash,
    reads: Vec<PlatformRead>,
}
struct SelectedInput {
    name: String,
    content: WireRef,
}
struct PlatformIdentity {
    target: ContentHash,
    linker: WireRef,
    kit: WireRef,
    runtime_archive: SelectedInput,
    foreign: Vec<SelectedInput>,
    runtime: WireRef,
}
snapshot_struct!(SelectedInput { name, content });
snapshot_struct!(PlatformIdentity {
    target,
    linker,
    kit,
    runtime_archive,
    foreign,
    runtime
});

impl CapturedPlatform {
    pub fn capture(
        store: &dyn ResultStore,
        target: &super::super::TargetSpec,
        inputs: PlatformCaptureInputs<'_>,
    ) -> Result<Self, String> {
        if inputs
            .foreign
            .len()
            .checked_add(3)
            .ok_or("platform input count overflow")?
            > inputs.limits.files
            || inputs.limits.archive_bytes == 0
        {
            return Err("selected platform inputs exceed admission".into());
        }
        let kit = inputs.kit.load(store, target)?;
        let mut remaining = inputs.limits.archive_bytes;
        let mut reads = Vec::new();
        let mut select = |path: &Path| -> Result<NamedLinkInput, String> {
            let path = std::fs::canonicalize(path).map_err(error)?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("selected link input lacks a UTF-8 filename")?
                .to_owned();
            let content = snapshot(store, &path, &mut remaining)?;
            reads.push(PlatformRead { path, content });
            Ok(NamedLinkInput { name, content })
        };
        let runtime_archive = select(inputs.runtime_archive)?;
        let foreign = inputs
            .foreign
            .iter()
            .map(|path| select(path))
            .collect::<Result<Vec<_>, _>>()?;
        // Keep the original selected absolute names: an ELF without SONAME
        // binds its pathname, which must remain stable across Link requests.
        let foreign_paths = reads
            .iter()
            .skip(1)
            .map(|read| read.path.clone())
            .collect::<Vec<_>>();
        let loader_path = std::fs::canonicalize(inputs.runtime.loader).map_err(error)?;
        let auditor_path = std::fs::canonicalize(inputs.runtime.auditor).map_err(error)?;
        let loader = snapshot(store, &loader_path, &mut remaining)?;
        let auditor = snapshot(store, &auditor_path, &mut remaining)?;
        reads.push(PlatformRead {
            path: loader_path,
            content: loader,
        });
        reads.push(PlatformRead {
            path: auditor_path,
            content: auditor,
        });
        let scratch = tempfile::tempdir().map_err(error)?;
        let machine = match target.triple.as_str() {
            "x86_64-unknown-linux-gnu" => "elf_x86_64",
            "aarch64-unknown-linux-gnu" => "aarch64linux",
            _ => return Err("selected runtime capture requires a supported ELF target".into()),
        };
        let mut arguments = vec![
            "-m".into(),
            machine.into(),
            "--shared".into(),
            "--threads=1".into(),
            "--build-id=none".into(),
            "--no-as-needed".into(),
            "--allow-shlib-undefined".into(),
        ];
        for directory in std::env::split_paths(inputs.runtime.library_path) {
            if !directory.is_absolute() {
                return Err("runtime library search directories must be absolute".into());
            }
            arguments.push(format!(
                "-L{}",
                directory
                    .to_str()
                    .ok_or("non-UTF8 runtime search directory")?
            ));
        }
        for library in kit
            .libraries(&target.triple)
            .map_err(|error| error.to_string())?
        {
            let name = library
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("kit DSO lacks a UTF-8 filename")?;
            arguments.push(format!("-l:{name}"));
        }
        for path in &foreign_paths {
            arguments.push(path.to_str().ok_or("non-UTF8 foreign input")?.to_owned());
        }
        let probe = inputs
            .linker
            .capture(store, scratch.path(), &arguments, inputs.limits)?;
        for (path, selected) in foreign_paths.iter().zip(&foreign) {
            if probe.input(store, path)? != Some(selected.content) {
                return Err("selected foreign input changed during runtime capture".into());
            }
        }
        let image = scratch.path().join("runtime-probe.so");
        probe.link(store, &image)?;
        let discovered = discovery::discover(
            store,
            target,
            &image,
            loader,
            auditor,
            discovery::DiscoveryPolicy {
                library_path: inputs.runtime.library_path,
                emulator: inputs.runtime.emulator,
                timeout: inputs.runtime.timeout,
                events_bytes: inputs.runtime.events_bytes,
                input_bytes: remaining,
                runtime: inputs.runtime.limits,
            },
        )?;
        reads.extend(discovered.inputs.into_iter().map(|input| PlatformRead {
            path: input.path,
            content: input.file,
        }));
        let mut current = std::collections::BTreeMap::new();
        for read in reads {
            if let Some(previous) = current.insert(read.path, read.content)
                && previous != read.content
            {
                return Err(
                    "selected platform input changed between link and runtime resolution".into(),
                );
            }
        }
        let reads = current
            .into_iter()
            .map(|(path, content)| PlatformRead { path, content })
            .collect();
        let selected = |input: &NamedLinkInput| SelectedInput {
            name: input.name.clone(),
            content: input.content.into(),
        };
        let identity = hirsel_codec::digest(
            "hirsel.selected-link-platform.v1",
            "Platform",
            &PlatformIdentity {
                target: target.digest(),
                linker: inputs.linker.reference().into(),
                kit: inputs.kit.reference().into(),
                runtime_archive: selected(&runtime_archive),
                foreign: foreign.iter().map(selected).collect(),
                runtime: discovered.runtime.reference().into(),
            },
        );
        Ok(Self {
            linker: inputs.linker,
            kit: inputs.kit,
            runtime_archive,
            foreign,
            runtime: discovered.runtime,
            target: target.digest(),
            identity,
            reads,
        })
    }
    /// Owned object, vector capacities, filenames and current-input path buffers.
    /// Stored tool/runtime bytes remain charged by the selected provider; an
    /// outer Arc/header belongs to its caller's existing owner reservation.
    pub fn retained_metadata_bytes(&self) -> u64 {
        let mut bytes = std::mem::size_of::<Self>();
        let mut add = |amount| bytes = bytes.saturating_add(amount);
        add(self.runtime_archive.name.capacity());
        add(self
            .foreign
            .capacity()
            .saturating_mul(std::mem::size_of::<NamedLinkInput>()));
        for input in &self.foreign {
            add(input.name.capacity());
        }
        add(self
            .reads
            .capacity()
            .saturating_mul(std::mem::size_of::<PlatformRead>()));
        for read in &self.reads {
            add(read.path.capacity());
        }
        u64::try_from(bytes).unwrap_or(u64::MAX)
    }

    pub fn linker(&self) -> FrozenLinker {
        self.linker
    }
    pub fn kit(&self) -> FrozenLinkKit {
        self.kit
    }
    pub fn runtime_archive(&self) -> &NamedLinkInput {
        &self.runtime_archive
    }
    pub fn foreign(&self) -> &[NamedLinkInput] {
        &self.foreign
    }
    pub fn runtime(&self) -> FrozenRuntime {
        self.runtime
    }
    pub fn identity(&self) -> ContentHash {
        self.identity
    }
    pub fn reads(&self) -> &[PlatformRead] {
        &self.reads
    }
    /// A pre-key probe does not excuse checking the actual result. A cached
    /// executable also must resolve using only this already selected runtime.
    pub fn validate_image(
        &self,
        store: &dyn ResultStore,
        target: &super::super::TargetSpec,
        image: &Path,
        emulator: Option<&Path>,
        timeout: Duration,
        limits: RuntimeLimits,
    ) -> Result<(), String> {
        if self.target != target.digest() {
            return Err("selected platform target differs".into());
        }
        let files = self.runtime.materialize(store, target, limits)?;
        let command = files.command(emulator, image);
        let arguments = command
            .get_args()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let mut closed = std::process::Command::new(command.get_program());
        closed.env_clear();
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                closed.env(key, value);
            }
        }
        closed
            .args(&arguments[..arguments.len() - 1])
            .args(["--inhibit-cache", "--list"])
            .arg(image);
        let output =
            super::super::link::bounded(&mut closed, timeout).map_err(|error| error.to_string())?;
        if !output.status.success() || !output.stderr.is_empty() {
            return Err(format!(
                "selected runtime refuses linked image: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }
}
fn error(error: std::io::Error) -> String {
    error.to_string()
}
fn snapshot(
    store: &dyn ResultStore,
    path: &Path,
    remaining: &mut u64,
) -> Result<Reference, String> {
    let mut file = File::open(path).map_err(error)?;
    let length = file.metadata().map_err(error)?.len();
    if length > *remaining {
        return Err("selected platform input bytes exceed admission".into());
    }
    let known = super::super::file_digest::file(path).map_err(error)?;
    let mut reader = HashedReader {
        source: file.by_ref().take(length.saturating_add(1)),
        hash: Sha256::new(),
        bytes: 0,
    };
    let content = store_link_input_reader(store, &mut reader)?;
    if reader.bytes != length
        || <[u8; 32]>::from(reader.hash.finalize()) != known.raw
        || super::super::file_digest::file(path).map_err(error)? != known
    {
        return Err(format!(
            "selected platform input changed during capture: {}",
            path.display()
        ));
    }
    *remaining -= length;
    Ok(content)
}
struct HashedReader<R> {
    source: R,
    hash: Sha256,
    bytes: u64,
}
impl<R: Read> Read for HashedReader<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let count = self.source.read(out)?;
        self.hash.update(&out[..count]);
        self.bytes += count as u64;
        Ok(count)
    }
}

#[cfg(test)]
#[path = "../../../tests/native-validation/platform_capture.rs"]
mod tests;
