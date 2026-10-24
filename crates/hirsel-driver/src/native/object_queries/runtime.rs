//! Ordinary-store transport for an explicitly selected dynamic runtime closure.
//! glibc's public audit interface performs loading; object 0.40.0's ELF reader
//! checks the target header. No ELF/script parser or validation authority lives
//! here. Closure derivation is a separate producer, not inferred from aliases.
use super::*;
use object::read::elf::FileHeader as _;
use std::{collections::BTreeMap, path::PathBuf, process::Command};

const RUNTIME: Domain = Domain {
    kind: 0x4f00000d,
    schema: 1,
};

/// Caller-owned admission bounds. These are not production budget defaults.
#[derive(Clone, Copy)]
pub struct RuntimeLimits {
    pub aliases: usize,
    pub name_bytes: usize,
}
#[derive(Clone)]
struct Alias {
    request: String,
    file: WireRef,
}
struct RuntimeRecord {
    target: ContentHash,
    loader: WireRef,
    auditor: WireRef,
    aliases: Vec<Alias>,
}
snapshot_struct!(Alias { request, file });
snapshot_struct!(RuntimeRecord {
    target,
    loader,
    auditor,
    aliases
});

/// Content only. In particular this is not proof of a complete foreign closure
/// or permission to select a Link hit without validating current dependencies.
#[derive(Clone, Copy)]
pub struct FrozenRuntime(Reference);
impl FrozenRuntime {
    pub fn from_settled(
        store: &dyn ResultStore,
        target: &super::super::TargetSpec,
        loader: Reference,
        auditor: Reference,
        aliases: impl IntoIterator<Item = (String, Reference)>,
        limits: RuntimeLimits,
    ) -> Result<Self, String> {
        let mut bindings = BTreeMap::new();
        let mut names = 0usize;
        for (request, file) in aliases {
            names = names
                .checked_add(request.len())
                .ok_or("runtime alias size overflow")?;
            if request.is_empty()
                || request.contains('\0')
                || names > limits.name_bytes
                || bindings.len() >= limits.aliases
                || bindings.insert(request, file).is_some()
            {
                return Err("runtime aliases exceed admission or repeat a request".into());
            }
        }
        let record = RuntimeRecord {
            target: target.digest(),
            loader: loader.into(),
            auditor: auditor.into(),
            aliases: bindings
                .into_iter()
                .map(|(request, file)| Alias {
                    request,
                    file: file.into(),
                })
                .collect(),
        };
        check(store, &record, target, limits)?;
        Ok(Self(put_record(store, RUNTIME, &record)?))
    }
    pub fn reference(self) -> Reference {
        self.0
    }
    pub fn from_reference(
        store: &dyn ResultStore,
        reference: Reference,
        target: &super::super::TargetSpec,
        limits: RuntimeLimits,
    ) -> Result<Self, String> {
        let runtime = Self(reference);
        runtime.validate(store, target, limits)?;
        Ok(runtime)
    }
    /// Call on hits too: cached executable bytes cannot hide missing runtime
    /// libraries, their loader or their closed-resolution implementation.
    pub fn validate(
        self,
        store: &dyn ResultStore,
        target: &super::super::TargetSpec,
        limits: RuntimeLimits,
    ) -> Result<(), String> {
        check(store, &get_record(store, RUNTIME, self.0)?, target, limits)
    }
    pub fn materialize(
        self,
        store: &dyn ResultStore,
        target: &super::super::TargetSpec,
        limits: RuntimeLimits,
    ) -> Result<RuntimeFiles, String> {
        let record: RuntimeRecord = get_record(store, RUNTIME, self.0)?;
        check(store, &record, target, limits)?;
        let directory = tempfile::tempdir().map_err(error)?;
        let loader = directory.path().join("loader");
        let auditor = directory.path().join("auditor.so");
        materialize_link_input(store, record.loader.into(), &loader, true)?;
        materialize_link_input(store, record.auditor.into(), &auditor, false)?;
        let mut files = BTreeMap::<ContentHash, (Reference, PathBuf)>::new();
        let mut map = vec![b'R'];
        for alias in record.aliases {
            let reference: Reference = alias.file.into();
            let index = files.len();
            let file = files
                .entry(reference.output.0)
                .or_insert_with(|| (reference, directory.path().join(format!("lib-{index}"))));
            if file.0 != reference {
                return Err("conflicting runtime content reference".into());
            }
            map.extend_from_slice(alias.request.as_bytes());
            map.push(0);
            map.extend_from_slice(
                file.1
                    .to_str()
                    .ok_or("non-UTF8 runtime directory")?
                    .as_bytes(),
            );
            map.push(0);
        }
        for (_, (reference, path)) in files {
            materialize_link_input(store, reference, &path, false)?;
        }
        let map_path = directory.path().join("map");
        std::fs::write(&map_path, &map).map_err(error)?;
        Ok(RuntimeFiles {
            directory,
            loader,
            auditor,
            map: map_path,
            map_bytes: map.len(),
        })
    }
}
fn error(error: std::io::Error) -> String {
    error.to_string()
}
fn check(
    store: &dyn ResultStore,
    record: &RuntimeRecord,
    target: &super::super::TargetSpec,
    limits: RuntimeLimits,
) -> Result<(), String> {
    if record.target != target.digest()
        || record.aliases.len() > limits.aliases
        || record
            .aliases
            .windows(2)
            .any(|pair| pair[0].request >= pair[1].request)
    {
        return Err("runtime target or alias selection differs".into());
    }
    let mut names = 0usize;
    for alias in &record.aliases {
        names = names
            .checked_add(alias.request.len())
            .ok_or("runtime alias size overflow")?;
        if alias.request.is_empty() || alias.request.contains('\0') || names > limits.name_bytes {
            return Err("runtime alias exceeds admission or contains NUL".into());
        }
    }
    let machine = match target.triple.as_str() {
        "x86_64-unknown-linux-gnu" => object::elf::EM_X86_64,
        "aarch64-unknown-linux-gnu" => object::elf::EM_AARCH64,
        _ => return Err("runtime auditing requires a supported ELF Linux target".into()),
    };
    for reference in std::iter::once(record.loader.clone())
        .chain(std::iter::once(record.auditor.clone()))
        .chain(record.aliases.iter().map(|alias| alias.file.clone()))
    {
        let chunks = blob_chunks(store, reference.into())?;
        let first = chunks.first().ok_or("empty runtime ELF input")?;
        let header = object::elf::FileHeader64::<object::LittleEndian>::parse(first.as_ref())
            .map_err(|error| error.to_string())?;
        if !header.is_little_endian()
            || header.e_machine(object::LittleEndian) != machine
            || header.e_type(object::LittleEndian) != object::elf::ET_DYN
        {
            return Err("runtime ELF input differs from selected target or dynamic kind".into());
        }
    }
    Ok(())
}

/// Retain this owner until the child exits. Physical paths exist only for this
/// materialization; the stored runtime has logical request names and contents.
pub struct RuntimeFiles {
    directory: tempfile::TempDir,
    loader: PathBuf,
    auditor: PathBuf,
    map: PathBuf,
    map_bytes: usize,
}
impl RuntimeFiles {
    /// Build a launch command over the captured loader. The normal native
    /// launch owner still supplies grants, arguments, replay and observations.
    /// A target emulator, if needed, is supplied explicitly by that owner.
    pub fn command(&self, emulator: Option<&Path>, image: &Path) -> Command {
        let mut command = Command::new(emulator.unwrap_or(&self.loader));
        if emulator.is_some() {
            command.arg(&self.loader);
        }
        command
            .env_clear()
            .arg("--audit")
            .arg(&self.auditor)
            .arg(image)
            .env("HIRSEL_RUNTIME_MAP", &self.map)
            .env("HIRSEL_RUNTIME_MAP_LENGTH", self.map_bytes.to_string());
        command
    }
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }
}

#[cfg(test)]
#[path = "../../../tests/native-validation/runtime_products.rs"]
mod tests;
