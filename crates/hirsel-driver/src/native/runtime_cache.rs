//! Runtime archives and derivations are ordinary products in the selected store.
//! Files here are materializations only; no archive/receipt cache grants reuse.
use super::{NativeError, StageName, TargetSpec};
use hirsel_artifact_store::{Action, Address, Domain, Reference};
use hirsel_engine::store::ResultStore;
use hirsel_syntax::ContentHash;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

type Packages = Vec<(String, PathBuf)>;

fn dependencies(
    root: &Path,
    cargo: &OsStr,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<Packages, NativeError> {
    let bytes = version(
        Command::new(cargo)
            .env_clear()
            .envs(environment)
            .current_dir(root)
            .args([
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--manifest-path",
            ])
            .arg(root.join("Cargo.toml")),
    )?;
    let metadata: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        NativeError::new(StageName::Link, format!("runtime dependency metadata: {e}"))
    })?;
    let packages: BTreeMap<_, _> = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p))
        .collect();
    let nodes: BTreeMap<_, _> = metadata["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| (n["id"].as_str().unwrap(), n))
        .collect();
    let start = packages
        .iter()
        .find(|(_, p)| p["name"] == "hirsel-rt")
        .unwrap()
        .0;
    let mut pending = vec![*start];
    let mut seen = BTreeSet::new();
    let mut local = Vec::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let package = packages[id];
        if package["source"].is_null() {
            let name = format!(
                "{}@{}",
                package["name"].as_str().unwrap(),
                package["version"].as_str().unwrap()
            );
            let manifest = Path::new(package["manifest_path"].as_str().unwrap());
            local.push((name, manifest.parent().unwrap().to_path_buf()));
        }
        for dep in nodes[id]["deps"].as_array().unwrap() {
            if dep["dep_kinds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|k| k["kind"] != "dev")
            {
                pending.push(dep["pkg"].as_str().unwrap());
            }
        }
    }
    local.sort();
    Ok(local)
}

fn io(error: std::io::Error) -> NativeError {
    NativeError::new(StageName::Link, format!("runtime cache: {error}"))
}

fn input_key(root: &Path, packages: &Packages, context: &[Vec<u8>]) -> Result<String, NativeError> {
    let mut paths = vec![root.join("Cargo.toml"), root.join("Cargo.lock")];
    paths.push(root.join("crates/hirsel-rt/include/hirsel_rt_v2.h"));
    for optional in [
        ".cargo/config",
        ".cargo/config.toml",
        "rust-toolchain",
        "rust-toolchain.toml",
    ] {
        let path = root.join(optional);
        if path.exists() {
            paths.push(path);
        }
    }
    paths.sort();
    let mut parts = context.to_vec();
    for path in paths {
        parts.push(
            path.strip_prefix(root)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec(),
        );
        parts.push(fs::read(path).map_err(io)?);
    }
    // Registry sources are pinned by Cargo.lock. Hash each local dependency's
    // full package input tree, including headers and data used by build scripts.
    // Cargo package directories have no build output here (target-dir is external).
    for (name, dir) in packages {
        let mut paths = Vec::new();
        package_sources(dir, &mut paths)?;
        paths.sort();
        parts.push(name.as_bytes().to_vec());
        for path in paths {
            parts.push(
                path.strip_prefix(dir)
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
            );
            parts.push(fs::read(path).map_err(io)?);
        }
    }
    let hash = ContentHash::of_parts("hirsel.native-runtime.v1", parts.iter().map(Vec::as_slice));
    Ok(hash.0.iter().map(|b| format!("{b:02x}")).collect())
}

fn package_sources(path: &Path, paths: &mut Vec<PathBuf>) -> Result<(), NativeError> {
    for entry in fs::read_dir(path).map_err(io)? {
        let entry = entry.map_err(io)?;
        if matches!(entry.file_name().to_str(), Some("target" | ".git")) {
            continue;
        }
        if entry.file_type().map_err(io)?.is_dir() {
            package_sources(&entry.path(), paths)?;
        } else {
            paths.push(entry.path());
        }
    }
    Ok(())
}

fn version(command: &mut Command) -> Result<Vec<u8>, NativeError> {
    let output = command.output().map_err(io)?;
    if !output.status.success() {
        return Err(NativeError::new(
            StageName::Link,
            format!(
                "{:?} {:?} exited {}: {}",
                command.get_program(),
                command.get_args().collect::<Vec<_>>(),
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }
    Ok(output.stdout)
}

fn context(
    environment: &BTreeMap<OsString, OsString>,
    target: &TargetSpec,
    prefix: &Path,
) -> Result<Vec<Vec<u8>>, NativeError> {
    let cargo = environment
        .get(OsStr::new("CARGO"))
        .map_or(OsStr::new("cargo"), OsString::as_os_str);
    let rustc = environment
        .get(OsStr::new("RUSTC"))
        .map_or(OsStr::new("rustc"), OsString::as_os_str);
    let probe = |command: &mut Command| version(command.env_clear().envs(environment));
    let mut parts = vec![
        b"cargo build --release -p hirsel-rt --lib; panic=abort".to_vec(),
        format!("{target:?}").into_bytes(),
        probe(Command::new(cargo).arg("--version"))?,
        probe(Command::new(rustc).arg("-vV"))?,
        probe(Command::new(prefix.join("bin/clang")).arg("--version"))?,
    ];
    if target.triple.starts_with("aarch64") {
        let compiler = super::link::configured_path(
            "HIRSEL_AARCH64_CC",
            environment.get(OsStr::new("HIRSEL_AARCH64_CC")).cloned(),
        )?;
        parts.push(compiler.as_os_str().as_encoded_bytes().to_vec());
        parts.push(probe(Command::new(compiler).arg("--version"))?);
    }
    Ok(parts)
}

fn configuration(
    root: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<Vec<Vec<u8>>, NativeError> {
    let mut parts = Vec::new();
    let mut flags: Vec<_> = environment
        .iter()
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            (name.starts_with("CARGO_PROFILE_")
                || name.starts_with("CARGO_TARGET_")
                    && (name.ends_with("_RUSTFLAGS") || name.ends_with("_LINKER"))
                || matches!(
                    name.as_ref(),
                    "RUSTFLAGS"
                        | "CARGO_ENCODED_RUSTFLAGS"
                        | "RUSTC"
                        | "RUSTC_WRAPPER"
                        | "RUSTC_WORKSPACE_WRAPPER"
                        | "CARGO_BUILD_RUSTFLAGS"
                        | "CARGO_BUILD_RUSTC"
                        | "CARGO_BUILD_RUSTC_WRAPPER"
                        | "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"
                        | "CC"
                        | "CFLAGS"
                        | "AR"
                        | "LDFLAGS"
                ))
                && name != "CARGO_PROFILE_RELEASE_PANIC"
        })
        .collect();
    flags.sort();
    for (name, value) in flags {
        parts.push(name.as_encoded_bytes().to_vec());
        parts.push(value.as_encoded_bytes().to_vec());
    }
    let cargo_home = environment
        .get(OsStr::new("CARGO_HOME"))
        .map(PathBuf::from)
        .or_else(|| {
            environment
                .get(OsStr::new("HOME"))
                .map(|p| PathBuf::from(p).join(".cargo"))
        });
    if let Some(home) = cargo_home {
        for name in ["config", "config.toml"] {
            let path = home.join(name);
            if path.exists() {
                parts.push(name.as_bytes().to_vec());
                parts.push(fs::read(path).map_err(io)?);
            }
        }
    }
    for parent in root.canonicalize().map_err(io)?.ancestors().skip(1) {
        for name in [".cargo/config", ".cargo/config.toml"] {
            let path = parent.join(name);
            if path.exists() {
                parts.push(name.as_bytes().to_vec());
                parts.push(fs::read(path).map_err(io)?);
            }
        }
    }
    Ok(parts)
}

/// The default is user-specific even when TMPDIR is shared. Existing directories
/// are never chmodded: ownership and privacy must already be trustworthy.
pub(super) fn default_directory() -> PathBuf {
    // SAFETY: geteuid has no pointer arguments or side effects.
    std::env::temp_dir().join(format!("hirsel-native-runtime-{}", unsafe {
        libc::geteuid()
    }))
}

pub(super) fn private_directory(cache: &Path) -> Result<File, NativeError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(cache).map_err(io)?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(cache)
        .map_err(io)?;
    let meta = directory.metadata().map_err(io)?;
    // SAFETY: geteuid has no pointer arguments or side effects.
    check_directory(&meta, unsafe { libc::geteuid() })?;
    Ok(directory)
}

fn check_directory(meta: &fs::Metadata, owner: u32) -> Result<(), NativeError> {
    if !meta.is_dir() || meta.uid() != owner || meta.mode() & 0o7777 != 0o700 {
        return Err(NativeError::new(
            StageName::Link,
            "runtime cache directory must be owned by the current user with mode 0700",
        ));
    }
    Ok(())
}

pub(super) fn private_file(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let meta = file.metadata()?;
    // SAFETY: geteuid has no pointer arguments or side effects.
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o7777 != 0o600
        || meta.nlink() != 1
    {
        return Err(std::io::Error::other("foreign or non-private cache file"));
    }
    Ok(file)
}

const RUNTIME: Domain = Domain {
    kind: 0x4f00_0006,
    schema: 1,
};

fn identity(key: &str) -> (Address, Action) {
    let address = Address(ContentHash::of_parts(
        "hirsel.runtime-product-address.v1",
        [key.as_bytes()],
    ));
    let action = Action(ContentHash::of_parts(
        "hirsel.runtime-product-action.v1",
        [key.as_bytes()],
    ));
    (address, action)
}

fn selected(store: &dyn ResultStore, key: &str) -> Option<Reference> {
    let (address, action) = identity(key);
    let association = store.lookup_product(address)?;
    if association.action != action || association.reference.domain != RUNTIME {
        return None;
    }
    let bytes = store.get_content(association.reference)?;
    if RUNTIME.reference(&bytes) != Some(association.reference) {
        return None;
    }
    let (stored_key, archive): (String, Reference) = serde_json::from_slice(&bytes).ok()?;
    (stored_key == key).then_some(archive)
}

fn restore(
    store: &dyn ResultStore,
    reference: Reference,
    destination: &Path,
) -> Result<(), NativeError> {
    // A materialization may not alias or chmod a foreign file. Only the checked
    // private regular file is eligible for inode-preserving byte comparison.
    if destination.symlink_metadata().is_ok() && private_file(destination).is_err() {
        fs::remove_file(destination).map_err(io)?;
    }
    super::object_queries::materialize_link_input(store, reference, destination, false)
        .map_err(|e| NativeError::new(StageName::Link, e))?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o600)).map_err(io)
}

fn publish(
    store: &dyn ResultStore,
    cache: &Path,
    key: &str,
    build: impl FnOnce(&Path) -> Result<PathBuf, NativeError>,
) -> Result<PathBuf, NativeError> {
    let directory = private_directory(cache)?;
    // Pin operations to the checked directory, including through publication.
    use std::os::fd::AsRawFd;
    let pinned = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let archive = pinned.join(format!("{key}.a"));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(pinned.join(format!("{key}.lock")))
        .map_err(io)?;
    let meta = lock.metadata().map_err(io)?;
    // SAFETY: geteuid has no pointer arguments or side effects.
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o7777 != 0o600
        || meta.nlink() != 1
    {
        return Err(NativeError::new(
            StageName::Link,
            "foreign runtime cache lock",
        ));
    }
    // OS locks disappear on process exit; a failed builder cannot strand a lock.
    lock.lock().map_err(io)?;
    if let Some(reference) = selected(store, key)
        && restore(store, reference, &archive).is_ok()
    {
        return Ok(cache.join(format!("{key}.a")));
    }
    // Different requests/processes may compute the same product. The file lock
    // coalesces this synchronous Cargo adapter only; the store owns all reuse.
    // Unique scratch prevents failed builds from exposing partial archives.
    let scratch = super::executable::Scratch::new()?;
    let built = build(&scratch.0)?;
    let reference =
        super::object_queries::store_link_input_reader(store, File::open(built).map_err(io)?)
            .map_err(|e| NativeError::new(StageName::Link, e))?;
    restore(store, reference, &archive)?;
    let bytes = serde_json::to_vec(&(key, reference))
        .map_err(|e| NativeError::new(StageName::Link, e.to_string()))?;
    let record = store
        .put_content(RUNTIME, &bytes)
        .ok_or_else(|| NativeError::new(StageName::Link, "runtime product publication failed"))?;
    if RUNTIME.reference(&bytes) != Some(record) {
        return Err(NativeError::new(
            StageName::Link,
            "runtime product identity differs",
        ));
    }
    let (address, action) = identity(key);
    store.associate_product(address, action, record);
    if store
        .lookup_product(address)
        .is_none_or(|row| row.action != action || row.reference != record)
    {
        return Err(NativeError::new(
            StageName::Link,
            "runtime product association failed",
        ));
    }
    Ok(cache.join(format!("{key}.a")))
}

/// An ordinary derivation of the exact archive bytes and transformation inputs.
/// The returned private file is only a materialization of that stored product.
pub(super) fn derived(
    store: &dyn ResultStore,
    cache: &Path,
    archive: &Path,
    part: &str,
    tools: ContentHash,
    build: impl FnOnce(&Path) -> Result<PathBuf, NativeError>,
) -> Result<PathBuf, NativeError> {
    let key = archive
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| NativeError::new(StageName::Link, "a runtime archive has no key"))?;
    // Bind the actual consumed archive, not the spelling of its pathname.
    let input =
        super::object_queries::store_link_input_reader(store, File::open(archive).map_err(io)?)
            .map_err(|e| NativeError::new(StageName::Link, e))?;
    let encoded = serde_json::to_vec(&(key, part, tools, input))
        .map_err(|e| NativeError::new(StageName::Link, e.to_string()))?;
    let hash = ContentHash::of_parts("hirsel.runtime-derived.v1", [encoded.as_slice()]);
    let derived_key: String = hash.0.iter().map(|b| format!("{b:02x}")).collect();
    publish(store, cache, &derived_key, |scratch| {
        fs::create_dir_all(scratch).map_err(io)?;
        build(scratch)
    })
}

/// Frozen Cargo-fallback inputs. Retain one per resident session; create a new
/// snapshot to observe source/configuration or toolchain changes. Buck archives
/// bypass this snapshot entirely.
pub struct RuntimeInputs {
    root: PathBuf,
    environment: BTreeMap<OsString, OsString>,
    source_key: String,
    keys: Mutex<BTreeMap<(String, super::RuntimeBuild, PathBuf), String>>,
}

impl RuntimeInputs {
    pub fn capture() -> Result<Self, NativeError> {
        Self::capture_at(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
            std::env::vars_os().collect(),
        )
    }

    fn capture_at(
        root: &Path,
        environment: BTreeMap<OsString, OsString>,
    ) -> Result<Self, NativeError> {
        let root = root.to_path_buf();
        let cargo = environment
            .get(OsStr::new("CARGO"))
            .map_or(OsStr::new("cargo"), OsString::as_os_str);
        // Include the dependency closure for every target in the same snapshot.
        let packages = dependencies(&root, cargo, &environment)?;
        let source_key = input_key(&root, &packages, &configuration(&root, &environment)?)?;
        Ok(Self {
            root,
            environment,
            source_key,
            keys: Mutex::new(BTreeMap::new()),
        })
    }

    fn key(
        &self,
        target: &TargetSpec,
        prefix: &Path,
        runtime_build: super::RuntimeBuild,
    ) -> Result<String, NativeError> {
        let identity = (format!("{target:?}"), runtime_build, prefix.to_path_buf());
        let mut keys = self.keys.lock().unwrap();
        if let Some(key) = keys.get(&identity) {
            return Ok(key.clone());
        }
        let mut parts = context(&self.environment, target, prefix)?;
        parts.push(self.source_key.as_bytes().to_vec());
        parts.push(runtime_build.features().into_bytes());
        let hash =
            ContentHash::of_parts("hirsel.native-runtime.v2", parts.iter().map(Vec::as_slice));
        let key: String = hash.0.iter().map(|b| format!("{b:02x}")).collect();
        keys.insert(identity, key.clone());
        Ok(key)
    }
}

pub(super) fn archive(
    store: &dyn ResultStore,
    inputs: Option<&RuntimeInputs>,
    cache: &Path,
    target: &TargetSpec,
    prefix: &Path,
    runtime_build: super::link::RuntimeBuild,
    build: impl FnOnce(&Path, &BTreeMap<OsString, OsString>) -> Result<PathBuf, NativeError>,
) -> Result<PathBuf, NativeError> {
    let captured;
    let inputs = match inputs {
        Some(inputs) => inputs,
        None => {
            captured = RuntimeInputs::capture()?;
            &captured
        }
    };
    let key = inputs.key(target, prefix, runtime_build)?;
    publish(store, cache, &key, |build_dir| {
        let built = build(build_dir, &inputs.environment)?;
        // Never publish a build from mutable live inputs under a frozen key.
        // Tool/config probes are intentionally paid only on a cache miss.
        if RuntimeInputs::capture_at(&inputs.root, inputs.environment.clone())?.key(
            target,
            prefix,
            runtime_build,
        )? != key
        {
            return Err(NativeError::new(
                StageName::Link,
                "runtime inputs changed during the build; start a new session",
            ));
        }
        Ok(built)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_engine::store::MemStore;
    const CRATES: &[&str] = &["hirsel-rt", "hirsel-kernels"];

    #[test]
    fn frozen_inputs_memoize_probes_and_keys_but_refuse_changed_builds() {
        use std::os::unix::fs::PermissionsExt;
        let store = MemStore::default();
        let root_scratch = tempfile::Builder::new()
            .prefix("hirsel-runtime-snapshot-")
            .tempdir()
            .unwrap();
        let root = root_scratch.path().to_path_buf();
        let package = root.join("crates/hirsel-rt");
        fs::create_dir_all(package.join("include")).unwrap();
        fs::create_dir_all(package.join("src")).unwrap();
        fs::write(root.join("Cargo.toml"), "workspace").unwrap();
        fs::write(root.join("Cargo.lock"), "locked").unwrap();
        fs::write(package.join("Cargo.toml"), "runtime").unwrap();
        fs::write(package.join("include/hirsel_rt_v2.h"), "ABI").unwrap();
        let source = package.join("src/lib.rs");
        fs::write(&source, "first").unwrap();
        let metadata = serde_json::json!({
            "packages": [{"id": "rt", "name": "hirsel-rt", "version": "1", "source": null, "manifest_path": package.join("Cargo.toml")}],
            "resolve": {"nodes": [{"id": "rt", "deps": []}]}
        });
        let metadata_path = root.join("metadata.json");
        fs::write(&metadata_path, metadata.to_string()).unwrap();
        let log = root.join("probes");
        let prefix = root.join("llvm");
        fs::create_dir_all(prefix.join("bin")).unwrap();
        let script = format!(
            "#!/bin/sh\necho probe >> '{}'\nif [ \"$1\" = metadata ]; then /bin/cat '{}'; else echo tool-v1; fi\n",
            log.display(),
            metadata_path.display()
        );
        for tool in [
            root.join("cargo"),
            root.join("rustc"),
            prefix.join("bin/clang"),
        ] {
            fs::write(&tool, &script).unwrap();
            fs::set_permissions(tool, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let environment: BTreeMap<OsString, OsString> = [
            ("CARGO".into(), root.join("cargo").into_os_string()),
            ("RUSTC".into(), root.join("rustc").into_os_string()),
        ]
        .into();
        let inputs = RuntimeInputs::capture_at(&root, environment.clone()).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let variant = super::super::RuntimeBuild::default();
        let key = inputs.key(&target, &prefix, variant).unwrap();
        let probes = fs::read(&log).unwrap();
        assert_eq!(inputs.key(&target, &prefix, variant).unwrap(), key);
        assert_eq!(
            fs::read(&log).unwrap(),
            probes,
            "a warm key must run no tool"
        );
        let diagnostic = super::super::RuntimeBuild {
            diagnostic: true,
            ..variant
        };
        assert_ne!(inputs.key(&target, &prefix, diagnostic).unwrap(), key);
        let cache = root.join("cache");
        let built = archive(
            &store,
            Some(&inputs),
            &cache,
            &target,
            &prefix,
            variant,
            |dir, _| {
                fs::create_dir_all(dir).map_err(io)?;
                let archive = dir.join("rt.a");
                fs::write(&archive, b"first archive").map_err(io)?;
                Ok(archive)
            },
        )
        .unwrap();
        fs::write(&source, "second").unwrap();
        let probes = fs::read(&log).unwrap();
        assert_eq!(
            archive(
                &store,
                Some(&inputs),
                &cache,
                &target,
                &prefix,
                variant,
                |_, _| panic!("frozen archive must be reused")
            )
            .unwrap(),
            built
        );
        assert_eq!(
            fs::read(&log).unwrap(),
            probes,
            "a warm archive must run no tool or metadata"
        );
        let changed = RuntimeInputs::capture_at(&root, environment).unwrap();
        assert_ne!(changed.key(&target, &prefix, variant).unwrap(), key);
        let error = archive(
            &store,
            Some(&changed),
            &cache,
            &target,
            &prefix,
            variant,
            |dir, _| {
                fs::create_dir_all(dir).map_err(io)?;
                let archive = dir.join("rt.a");
                fs::write(&archive, b"changed build").map_err(io)?;
                fs::write(&source, "third").map_err(io)?;
                Ok(archive)
            },
        )
        .unwrap_err();
        assert!(
            error.message.contains("inputs changed during the build"),
            "{error}"
        );
        assert!(
            !cache
                .join(format!(
                    "{}.a",
                    changed.key(&target, &prefix, variant).unwrap()
                ))
                .exists()
        );
        fs::remove_dir_all(root).unwrap();
        let error = version(
            Command::new("/bin/sh")
                .args(["-c", "exit 7"])
                .env("HIRSEL_TEST_SECRET", "snapshot-secret-value"),
        )
        .unwrap_err();
        assert!(!error.message.contains("snapshot-secret-value"), "{error}");
        assert!(error.message.contains("exit status: 7"), "{error}");
    }

    #[test]
    fn distinct_runtime_inputs_publish_distinct_archives_and_identical_trees_share() {
        let store = MemStore::default();
        let root_scratch = tempfile::Builder::new()
            .prefix("hirsel-runtime-cache-")
            .tempdir()
            .unwrap();
        let root = root_scratch.path().to_path_buf();
        let tree = root.join("tree");
        let copy = root.join("other-checkout");
        for tree in [&tree, &copy] {
            fs::create_dir_all(tree).unwrap();
            fs::write(tree.join("Cargo.toml"), "workspace").unwrap();
            fs::write(tree.join("Cargo.lock"), "locked").unwrap();
            for name in CRATES {
                let dir = tree.join("crates").join(name);
                fs::create_dir_all(dir.join("src")).unwrap();
                fs::write(dir.join("Cargo.toml"), name).unwrap();
                fs::write(dir.join("src/lib.rs"), "first").unwrap();
            }
            fs::create_dir_all(tree.join("crates/hirsel-rt/include")).unwrap();
            fs::write(tree.join("crates/hirsel-rt/include/hirsel_rt_v2.h"), "ABI").unwrap();
        }
        let context = vec![b"target/toolchain/build flags".to_vec()];
        let packages = |tree: &Path| {
            CRATES
                .iter()
                .map(|n| (n.to_string(), tree.join("crates").join(n)))
                .collect()
        };
        let local = packages(&tree);
        let first = input_key(&tree, &local, &context).unwrap();
        assert_eq!(first, input_key(&copy, &packages(&copy), &context).unwrap());
        let cache = root.join("cache");
        let publish_bytes = |key: &str, bytes: &[u8]| {
            publish(&store, &cache, key, |build_dir| {
                fs::create_dir_all(build_dir).map_err(io)?;
                let artifact = build_dir.join("runtime.a");
                fs::write(&artifact, bytes).map_err(io)?;
                Ok(artifact)
            })
            .unwrap()
        };
        let a = publish_bytes(&first, b"first archive");
        fs::write(tree.join("crates/hirsel-rt/src/lib.rs"), "second").unwrap();
        let second = input_key(&tree, &local, &context).unwrap();
        assert_ne!(first, second);
        let b = publish_bytes(&second, b"second archive");
        assert_ne!(a, b);
        assert_eq!(fs::read(a).unwrap(), b"first archive");
        assert_eq!(fs::read(b).unwrap(), b"second archive");
        publish(&store, &cache, &first, |_| {
            panic!("an immutable archive must be reused")
        })
        .unwrap();
        assert_ne!(
            second,
            input_key(
                &tree,
                &local,
                &[b"different flags/toolchain/target".to_vec()]
            )
            .unwrap()
        );
        fs::write(
            tree.join("crates/hirsel-rt/include/hirsel_rt_v2.h"),
            "new ABI",
        )
        .unwrap();
        assert_ne!(second, input_key(&tree, &local, &context).unwrap());
        let header_key = input_key(&tree, &local, &context).unwrap();
        fs::write(
            tree.join("crates/hirsel-kernels/src/lib.rs"),
            "changed dependency",
        )
        .unwrap();
        assert_ne!(header_key, input_key(&tree, &local, &context).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_builders_publish_one_complete_archive() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Arc, mpsc};
        let store = Arc::new(MemStore::default());
        let cache_scratch = tempfile::Builder::new()
            .prefix("hirsel-runtime-publication-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let cache = cache_scratch.path().to_path_buf();
        let (started, ready) = mpsc::channel();
        let (release, proceed) = mpsc::channel();
        let first_store = store.clone();
        let first_cache = cache.clone();
        let first = std::thread::spawn(move || {
            publish(first_store.as_ref(), &first_cache, "same-inputs", |dir| {
                fs::create_dir_all(dir).map_err(io)?;
                let artifact = dir.join("runtime.a");
                let mut file = File::create(&artifact).map_err(io)?;
                file.write_all(b"first half;").map_err(io)?;
                started.send(()).unwrap();
                proceed.recv().unwrap();
                file.write_all(b"second half").map_err(io)?;
                Ok(artifact)
            })
            .unwrap()
        });
        ready.recv().unwrap();
        assert!(!cache.join("same-inputs.a").exists());
        let second_store = store.clone();
        let second_cache = cache.clone();
        let second = std::thread::spawn(move || {
            publish(second_store.as_ref(), &second_cache, "same-inputs", |_| {
                panic!("one builder per key")
            })
            .unwrap()
        });
        release.send(()).unwrap();
        let a = first.join().unwrap();
        assert_eq!(a, second.join().unwrap());
        assert_eq!(fs::read(a).unwrap(), b"first half;second half");
        fs::remove_dir_all(cache).unwrap();
    }
    fn test_cache(name: &str) -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt;
        let cache_scratch = tempfile::Builder::new()
            .prefix(&format!("hirsel-cache-{name}-"))
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let cache = cache_scratch.path().to_path_buf();
        private_directory(&cache).unwrap();
        cache_scratch
    }

    fn build_bytes(store: &dyn ResultStore, cache: &Path, key: &str, bytes: &[u8]) -> PathBuf {
        publish(store, cache, key, |dir| {
            fs::create_dir_all(dir).map_err(io)?;
            let built = dir.join("runtime.a");
            fs::write(&built, bytes).map_err(io)?;
            Ok(built)
        })
        .unwrap()
    }

    #[test]
    fn planted_archive_without_a_receipt_is_rebuilt() {
        let store = MemStore::default();
        let cache_scratch = test_cache("planted");
        let cache = cache_scratch.path().to_path_buf();
        fs::write(cache.join("key.a"), b"planted archive").unwrap();
        let archive = build_bytes(&store, &cache, "key", b"trusted build");
        assert_eq!(fs::read(&archive).unwrap(), b"trusted build");
        publish(&store, &cache, "key", |_| {
            panic!("verified archive should be reused")
        })
        .unwrap();
        fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn truncated_or_changed_archives_and_receipts_are_rebuilt() {
        let store = MemStore::default();
        let cache_scratch = test_cache("corrupt");
        let cache = cache_scratch.path().to_path_buf();
        let archive = build_bytes(&store, &cache, "key", b"complete archive");
        fs::write(&archive, b"complete").unwrap();
        build_bytes(&store, &cache, "key", b"complete archive");
        assert_eq!(fs::read(&archive).unwrap(), b"complete archive");
        fs::write(&archive, b"planted contents").unwrap();
        build_bytes(&store, &cache, "key", b"complete archive");
        // A truncated ordinary descriptor forces recomputation. No filesystem
        // receipt may select a runtime product anymore.
        let (address, action) = identity("key");
        let broken = store.put_content(RUNTIME, b"truncated descriptor").unwrap();
        store.associate_product(address, action, broken);
        assert!(selected(&store, "key").is_none());
        build_bytes(&store, &cache, "key", b"complete archive");
        publish(&store, &cache, "key", |_| {
            panic!("verified archive should be reused")
        })
        .unwrap();
        fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn private_directory_checks_owner_and_mode_on_every_use() {
        use std::os::unix::fs::PermissionsExt;
        let store = MemStore::default();
        let cache_scratch = test_cache("permissions");
        let cache = cache_scratch.path().to_path_buf();
        let meta = fs::metadata(&cache).unwrap();
        assert_eq!(meta.mode() & 0o7777, 0o700);
        check_directory(&meta, meta.uid()).unwrap();
        // Exercise the exact production owner check without requiring root to chown.
        assert!(check_directory(&meta, meta.uid().wrapping_add(1)).is_err());
        build_bytes(&store, &cache, "key", b"archive");
        for mode in [0o755, 0o770, 0o777, 0o1700] {
            fs::set_permissions(&cache, fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                publish(&store, &cache, "key", |_| panic!(
                    "insecure directory must be refused"
                ))
                .is_err()
            );
        }
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o700)).unwrap();
        publish(&store, &cache, "key", |_| {
            panic!("private directory should reuse")
        })
        .unwrap();
        fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn foreign_archive_permissions_and_symlinks_force_rebuilding() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let store = MemStore::default();
        let cache_scratch = test_cache("foreign");
        let cache = cache_scratch.path().to_path_buf();
        let archive = build_bytes(&store, &cache, "key", b"trusted build");
        fs::set_permissions(&archive, fs::Permissions::from_mode(0o666)).unwrap();
        build_bytes(&store, &cache, "key", b"trusted build");
        fs::remove_file(&archive).unwrap();
        let outside = cache.join("foreign.a");
        fs::write(&outside, b"foreign archive").unwrap();
        symlink(&outside, &archive).unwrap();
        build_bytes(&store, &cache, "key", b"trusted build");
        assert_eq!(fs::read(&archive).unwrap(), b"trusted build");
        assert_eq!(fs::read(outside).unwrap(), b"foreign archive");
        let alias = cache.with_extension("alias");
        symlink(&cache, &alias).unwrap();
        assert!(
            publish(&store, &alias, "key", |_| panic!(
                "symlinked cache must be refused"
            ))
            .is_err()
        );
        fs::remove_file(alias).unwrap();
        fs::remove_dir_all(cache).unwrap();
    }
    #[test]
    fn ordinary_runtime_products_restore_missing_files_and_reopen_in_another_directory() {
        let store_root = tempfile::tempdir().unwrap();
        let cache = test_cache("ordinary");
        let next = test_cache("ordinary-next");
        let store = hirsel_engine::store::DirStore::new(store_root.path().join("products"));
        let archive = build_bytes(&store, cache.path(), "key", b"runtime bytes");
        assert!(!cache.path().join("key.sha256").exists());
        fs::remove_file(&archive).unwrap();
        let reopened = hirsel_engine::store::DirStore::new(store_root.path().join("products"));
        for directory in [cache.path(), next.path()] {
            let restored = publish(&reopened, directory, "key", |_| {
                panic!("ordinary product must hit")
            })
            .unwrap();
            assert_eq!(fs::read(restored).unwrap(), b"runtime bytes");
        }
        let fresh = MemStore::default();
        let mut computed = false;
        publish(&fresh, cache.path(), "key", |dir| {
            computed = true;
            let output = dir.join("new.a");
            fs::write(&output, b"new selected provider").map_err(io)?;
            Ok(output)
        })
        .unwrap();
        assert!(
            computed,
            "a materialized .a cannot bypass the selected provider"
        );
    }

    #[test]
    fn runtime_derivations_bind_archive_bytes_and_tool_identity() {
        let store = MemStore::default();
        let cache = test_cache("derived-products");
        let archive = cache.path().join("input.a");
        fs::write(&archive, b"one").unwrap();
        let transform = ContentHash::of_bytes(b"tool version one");
        let build = |dir: &Path| {
            let output = dir.join("derived.a");
            fs::write(&output, fs::read(&archive).map_err(io)?).map_err(io)?;
            Ok(output)
        };
        let first = derived(&store, cache.path(), &archive, "strip", transform, build).unwrap();
        assert_eq!(
            derived(
                &store,
                cache.path(),
                &archive,
                "strip",
                transform,
                |_| panic!("same inputs must hit")
            )
            .unwrap(),
            first
        );
        fs::write(&archive, b"two").unwrap();
        let second = derived(&store, cache.path(), &archive, "strip", transform, build).unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), b"one");
        assert_eq!(fs::read(&second).unwrap(), b"two");
        let third = derived(
            &store,
            cache.path(),
            &archive,
            "strip",
            ContentHash::of_bytes(b"tool version two"),
            build,
        )
        .unwrap();
        assert_ne!(second, third);
    }

    #[test]
    fn failed_or_unadmitted_runtime_products_never_publish_a_selected_output() {
        let cache = test_cache("failed-products");
        let store = MemStore::default();
        assert!(
            publish(&store, cache.path(), "key", |dir| {
                fs::write(dir.join("partial.a"), b"partial").map_err(io)?;
                Err(NativeError::new(StageName::Link, "injected build crash"))
            })
            .is_err()
        );
        assert!(store.lookup_product(identity("key").0).is_none());
        assert!(!cache.path().join("key.a").exists());
        let budget = hirsel_engine::runtime::ProcessBudget::new(0);
        let denied = hirsel_engine::store::RequestStore::new(
            std::sync::Arc::new(hirsel_engine::store::NoneStore),
            budget.reserve(0).unwrap(),
        );
        assert!(
            publish(&denied, cache.path(), "key", |dir| {
                let output = dir.join("runtime.a");
                fs::write(&output, b"unadmitted").map_err(io)?;
                Ok(output)
            })
            .is_err()
        );
        assert!(denied.exhausted());
        assert!(denied.lookup_product(identity("key").0).is_none());
        assert!(!cache.path().join("key.a").exists());
    }
}
