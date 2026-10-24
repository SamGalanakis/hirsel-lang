//! glibc resolves DT_NEEDED using its existing loader; object 0.40 reads the
//! PT_DYNAMIC graph (including sectionless ELF). We snapshot the objects that
//! glibc actually opened, then require a second, closed resolution of the image.
//! This covers static/declared loading, not future arbitrary dlopen requests.
use super::*;
use object::read::elf::{Dyn as _, ProgramHeader as _};
use std::{collections::BTreeSet, io::Read, os::unix::fs::MetadataExt, time::Duration};

/// The selected loader's search policy and the caller's existing admission.
/// No host environment, ld.so.cache or implicit library-directory policy enters.
pub struct DiscoveryPolicy<'a> {
    pub library_path: &'a std::ffi::OsStr,
    pub emulator: Option<&'a Path>,
    pub timeout: Duration,
    pub events_bytes: u64,
    pub input_bytes: u64,
    pub runtime: RuntimeLimits,
}

/// A current source read. Include these exact refs in the request's readiness
/// dependencies; a stored image never excuses rereading changed runtime leaves.
pub struct RuntimeInput {
    pub path: PathBuf,
    pub file: Reference,
}

/// Complete DT_NEEDED resolution for this exact image under the selected policy.
/// This is a content property; it grants no native C or execution authority.
pub struct DiscoveredRuntime {
    pub image: Reference,
    pub runtime: FrozenRuntime,
    pub inputs: Vec<RuntimeInput>,
}

/// Discover against the actual linked image's location, preserving its ORIGIN.
/// The caller owns that image while this runs. Loader/auditor are already stored
/// inputs; dynamic libraries are strict current reads under the selected policy.
pub fn discover(
    store: &dyn ResultStore,
    target: &super::super::super::TargetSpec,
    image: &Path,
    loader: Reference,
    auditor: Reference,
    policy: DiscoveryPolicy<'_>,
) -> Result<DiscoveredRuntime, String> {
    if policy.events_bytes == 0 || policy.input_bytes == 0 || policy.timeout.is_zero() {
        return Err("runtime discovery has no admitted bounds".into());
    }
    let image = std::fs::canonicalize(image).map_err(error)?;
    let mut remaining = policy.input_bytes;
    let (image_ref, image_bytes) = snapshot(store, &image, &mut remaining)?;
    let scratch = tempfile::tempdir().map_err(error)?;
    let loader_path = scratch.path().join("loader");
    let auditor_path = scratch.path().join("auditor.so");
    materialize_link_input(store, loader, &loader_path, true)?;
    materialize_link_input(store, auditor, &auditor_path, false)?;
    let map = scratch.path().join("map");
    std::fs::write(&map, b"C").map_err(error)?;
    let events = scratch.path().join("events");
    let mut command = Command::new(policy.emulator.unwrap_or(&loader_path));
    if policy.emulator.is_some() {
        command.arg(&loader_path);
    }
    command
        .env_clear()
        .args(["--inhibit-cache", "--library-path"])
        .arg(policy.library_path)
        .arg("--audit")
        .arg(&auditor_path)
        .arg("--list")
        .arg(&image)
        .env("HIRSEL_RUNTIME_MAP", &map)
        .env("HIRSEL_RUNTIME_MAP_LENGTH", "1")
        .env("HIRSEL_RUNTIME_EVENTS", &events)
        .env(
            "HIRSEL_RUNTIME_EVENTS_LIMIT",
            policy.events_bytes.to_string(),
        );
    checked_output(&mut command, policy.timeout)?;
    let mut event_bytes = Vec::new();
    std::fs::File::open(events)
        .map_err(error)?
        .take(
            policy
                .events_bytes
                .checked_add(1)
                .ok_or("runtime event limit overflow")?,
        )
        .read_to_end(&mut event_bytes)
        .map_err(error)?;
    if event_bytes.len() as u64 > policy.events_bytes {
        return Err("runtime discovery events exceed admission".into());
    }
    let mut opened = BTreeSet::new();
    let mut requests = BTreeSet::new();
    for event in event_bytes.split_inclusive(|byte| *byte == 0) {
        if event.last() != Some(&0) || event.len() < 2 {
            return Err("malformed runtime discovery event".into());
        }
        let text = std::str::from_utf8(&event[1..event.len() - 1]).map_err(|e| e.to_string())?;
        match event[0] {
            b'S' => {
                requests.insert(text.to_string());
            }
            b'O' if text.is_empty() || text == "linux-vdso.so.1" || text == "linux-gate.so.1" => {}
            b'O' if Path::new(text) == image || Path::new(text) == loader_path => {}
            b'O' => {
                opened.insert(PathBuf::from(text));
            }
            _ => return Err("unknown runtime discovery event".into()),
        }
    }
    let mut inputs = Vec::new();
    let mut groups = BTreeMap::<(u64, u64), (Vec<String>, Reference)>::new();
    let mut names = BTreeMap::<String, BTreeSet<(u64, u64)>>::new();
    let mut needed = elf_names(&image_bytes)?.0;
    for path in opened {
        if !path.is_absolute() {
            return Err("runtime loader opened a nonabsolute library path".into());
        }
        let before = std::fs::metadata(&path).map_err(error)?;
        let identity = (before.dev(), before.ino());
        let (file, bytes) = snapshot(store, &path, &mut remaining)?;
        let after = std::fs::metadata(&path).map_err(error)?;
        if identity != (after.dev(), after.ino()) {
            return Err("runtime library identity changed during discovery".into());
        }
        let (dependencies, soname) = elf_names(&bytes)?;
        needed.extend(dependencies);
        let group = groups.entry(identity).or_insert_with(|| (Vec::new(), file));
        if group.1 != file {
            return Err("runtime library changed through an alias".into());
        }
        for name in [
            path.to_str().map(str::to_owned),
            path.file_name().and_then(|s| s.to_str()).map(str::to_owned),
            soname,
        ]
        .into_iter()
        .flatten()
        {
            names.entry(name).or_default().insert(identity);
        }
        inputs.push(RuntimeInput { path, file });
    }
    requests.extend(needed);
    let loader_bytes = link_input_bytes(store, loader)?;
    if let Some(soname) = elf_names(&loader_bytes)?.1
        && requests.contains(&soname)
    {
        let meta = std::fs::metadata(&loader_path).map_err(error)?;
        let identity = (meta.dev(), meta.ino());
        names.entry(soname).or_default().insert(identity);
        groups.insert(identity, (Vec::new(), loader));
    }
    // Expanded per-owner ORIGIN dependencies need a richer mapping than the
    // global audit aliases. Refuse them rather than choosing a different DSO.
    for request in requests {
        if request.contains('$') {
            return Err("runtime DT_NEEDED token requires owner-relative binding".into());
        }
        let candidates = names
            .get(&request)
            .ok_or_else(|| format!("unbound runtime request {request:?}"))?;
        if candidates.len() != 1 {
            return Err(format!(
                "runtime request {request:?} has ambiguous loaded identities"
            ));
        }
        groups
            .get_mut(candidates.first().unwrap())
            .unwrap()
            .0
            .push(request);
    }
    let libraries = groups
        .into_values()
        .map(|(aliases, file)| RuntimeLibrary { aliases, file });
    let runtime =
        FrozenRuntime::from_libraries(store, target, loader, auditor, libraries, policy.runtime)?;
    // Check the actual closed resolver, not merely membership in a name table.
    let files = runtime.materialize(store, target, policy.runtime)?;
    let closed_image = files.directory().join("image");
    materialize_link_input(store, image_ref, &closed_image, true)?;
    let command = files.command(policy.emulator, &closed_image);
    // command() ends with the image; --list must precede it for glibc.
    let arguments = command
        .get_args()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut closed = Command::new(command.get_program());
    closed.env_clear();
    for (name, value) in command.get_envs() {
        if let Some(value) = value {
            closed.env(name, value);
        }
    }
    closed
        .args(&arguments[..arguments.len() - 1])
        .args(["--inhibit-cache", "--list"])
        .arg(&closed_image);
    checked_output(&mut closed, policy.timeout)?;
    if super::super::super::file_digest::file(&image)
        .map_err(error)?
        .content
        != ContentHash::of_bytes(&image_bytes)
    {
        return Err("linked image changed during runtime discovery".into());
    }
    Ok(DiscoveredRuntime {
        image: image_ref,
        runtime,
        inputs,
    })
}

fn checked_output(command: &mut Command, timeout: Duration) -> Result<(), String> {
    let output = super::super::super::link::bounded(command, timeout).map_err(|e| e.to_string())?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(format!(
            "runtime resolution refused: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn snapshot(
    store: &dyn ResultStore,
    path: &Path,
    remaining: &mut u64,
) -> Result<(Reference, Vec<u8>), String> {
    let known = super::super::super::file_digest::file(path).map_err(error)?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(error)?
        .take(remaining.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(error)?;
    *remaining = remaining
        .checked_sub(bytes.len() as u64)
        .ok_or("runtime inputs exceed admitted bytes")?;
    if ContentHash::of_bytes(&bytes) != known.content
        || super::super::super::file_digest::file(path).map_err(error)? != known
    {
        return Err(format!(
            "runtime input changed while snapshotting {}",
            path.display()
        ));
    }
    Ok((store_link_input(store, &bytes)?, bytes))
}

/// Use object crate's PT_DYNAMIC reader, rather than sections (which may be stripped).
fn elf_names(bytes: &[u8]) -> Result<(Vec<String>, Option<String>), String> {
    let endian = object::LittleEndian;
    let header = object::elf::FileHeader64::<object::LittleEndian>::parse(bytes)
        .map_err(|e| e.to_string())?;
    let segments = header
        .program_headers(endian, bytes)
        .map_err(|e| e.to_string())?;
    let mut dynamic = None;
    for segment in segments {
        if let Some(entries) = segment.dynamic(endian, bytes).map_err(|e| e.to_string())? {
            if dynamic.replace(entries).is_some() {
                return Err("duplicate ELF dynamic segment".into());
            }
        }
    }
    let dynamic = dynamic.ok_or("runtime image has no dynamic segment")?;
    let address = dynamic
        .iter()
        .find(|entry| entry.d_tag(endian) == object::elf::DT_STRTAB)
        .ok_or("missing ELF dynamic strings")?
        .d_val(endian);
    let size = dynamic
        .iter()
        .find(|entry| entry.d_tag(endian) == object::elf::DT_STRSZ)
        .ok_or("missing ELF dynamic string size")?
        .d_val(endian);
    let mut strings = None;
    for segment in segments {
        if segment.p_type(endian) == object::elf::PT_LOAD {
            if let Some(data) = segment
                .data_range(endian, bytes, address, size)
                .map_err(|_| "invalid ELF dynamic strings")?
            {
                if strings.replace(data).is_some() {
                    return Err("ambiguous ELF dynamic string range".into());
                }
            }
        }
    }
    let data = strings.ok_or("unmapped ELF dynamic strings")?;
    let strings = object::read::StringTable::new(data, 0, data.len() as u64);
    let mut needed = Vec::new();
    let mut soname = None;
    for entry in dynamic {
        let tag = entry.d_tag(endian);
        if tag == object::elf::DT_NULL {
            break;
        }
        if tag == object::elf::DT_NEEDED || tag == object::elf::DT_SONAME {
            let text = entry.string(endian, strings).map_err(|e| e.to_string())?;
            let name = std::str::from_utf8(text)
                .map_err(|e| e.to_string())?
                .to_owned();
            if tag == object::elf::DT_NEEDED {
                needed.push(name);
            } else if soname.replace(name).is_some() {
                return Err("duplicate ELF SONAME".into());
            }
        }
    }
    Ok((needed, soname))
}
