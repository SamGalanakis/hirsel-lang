//! Runtime archives are immutable, content-addressed inputs to the linker.
use super::{NativeError, StageName, TargetSpec};
use hirsel_syntax::ContentHash;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

type Packages = Vec<(String, PathBuf)>;

fn dependencies(
    root: &Path,
    cargo: &std::ffi::OsStr,
    target: &TargetSpec,
) -> Result<Packages, NativeError> {
    let bytes = version(
        Command::new(cargo)
            .current_dir(root)
            .args([
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--filter-platform",
                &target.triple,
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
            format!("{command:?}: {}", String::from_utf8_lossy(&output.stderr)),
        ));
    }
    Ok(output.stdout)
}

fn context(
    root: &Path,
    cargo: &std::ffi::OsStr,
    target: &TargetSpec,
    prefix: &Path,
) -> Result<Vec<Vec<u8>>, NativeError> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let mut parts = vec![
        b"cargo build --release -p hirsel-rt --lib; panic=abort".to_vec(),
        format!("{target:?}").into_bytes(),
        version(Command::new(cargo).arg("--version"))?,
        version(Command::new(rustc).arg("-vV"))?,
        version(Command::new(prefix.join("bin/clang")).arg("--version"))?,
    ];
    if target.triple.starts_with("aarch64") {
        let compiler = super::link::environment_path("HIRSEL_AARCH64_CC")?;
        parts.push(compiler.as_os_str().as_encoded_bytes().to_vec());
        parts.push(version(Command::new(compiler).arg("--version"))?);
    }
    let mut flags: Vec<_> = std::env::vars_os()
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
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cargo")));
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

fn private_directory(cache: &Path) -> Result<File, NativeError> {
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

fn private_file(path: &Path) -> std::io::Result<File> {
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

fn digest(mut file: File) -> std::io::Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verified(archive: &Path, receipt: &Path) -> bool {
    let check = || -> std::io::Result<bool> {
        // A receipt belongs to this input key by its filename in the private
        // directory. Read at most one digest so a corrupt receipt cannot grow RAM.
        let mut expected = String::new();
        private_file(receipt)?
            .take(65)
            .read_to_string(&mut expected)?;
        if expected.len() != 64 {
            return Ok(false);
        }
        Ok(digest(private_file(archive)?)? == expected)
    };
    check().unwrap_or(false)
}

fn temporary_file(directory: &Path, key: &str) -> Result<(PathBuf, File), NativeError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!("{key}.{}.{serial}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io(e)),
        }
    }
}

fn publish(
    cache: &Path,
    key: &str,
    build: impl FnOnce(&Path) -> Result<PathBuf, NativeError>,
) -> Result<PathBuf, NativeError> {
    let directory = private_directory(cache)?;
    // Pin operations to the checked directory, including through publication.
    use std::os::fd::AsRawFd;
    let pinned = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let archive = pinned.join(format!("{key}.a"));
    let receipt = pinned.join(format!("{key}.sha256"));
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
    if verified(&archive, &receipt) {
        return Ok(cache.join(format!("{key}.a")));
    }
    let built = build(&cache.join("build").join(key))?;
    let (temporary, mut output) = temporary_file(&pinned, key)?;
    let mut digest_temporary = None;
    let result = (|| {
        let mut input = File::open(built).map_err(io)?;
        std::io::copy(&mut input, &mut output).map_err(io)?;
        output.sync_all().map_err(io)?;
        let hash = digest(File::open(&temporary).map_err(io)?).map_err(io)?;
        let (path, mut receipt_file) = temporary_file(&pinned, key)?;
        digest_temporary = Some(path);
        receipt_file.write_all(hash.as_bytes()).map_err(io)?;
        receipt_file.sync_all().map_err(io)?;
        fs::rename(&temporary, &archive).map_err(io)?;
        fs::rename(digest_temporary.as_ref().unwrap(), &receipt).map_err(io)?;
        directory.sync_all().map_err(io)?;
        // Readers hold the same lock. A crash between renames leaves a missing
        // or mismatched receipt, which forces rebuilding rather than reuse.
        Ok(cache.join(format!("{key}.a")))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
        if let Some(path) = digest_temporary {
            let _ = fs::remove_file(path);
        }
    }
    result
}

/// The bytes of the private file `name` in `cache`, at most `limit` of them; `None`
/// when it is missing, foreign, larger than `limit` or the directory is not private.
pub(super) fn read_private(cache: &Path, name: &str, limit: u64) -> Option<Vec<u8>> {
    use std::os::fd::AsRawFd;
    let directory = private_directory(cache).ok()?;
    let pinned = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let mut bytes = Vec::new();
    private_file(&pinned.join(name))
        .ok()?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

/// Publish `bytes` as the private file `name` in `cache`, replacing it atomically.
pub(super) fn write_private(cache: &Path, name: &str, bytes: &[u8]) -> Result<(), NativeError> {
    use std::os::fd::AsRawFd;
    let directory = private_directory(cache)?;
    let pinned = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let (temporary, mut file) = temporary_file(&pinned, name)?;
    let result = (|| {
        file.write_all(bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        fs::rename(&temporary, pinned.join(name)).map_err(io)?;
        directory.sync_all().map_err(io)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub(super) fn archive(
    cache: &Path,
    target: &TargetSpec,
    prefix: &Path,
    runtime_build: super::link::RuntimeBuild,
    build: impl FnOnce(&Path) -> Result<PathBuf, NativeError>,
) -> Result<PathBuf, NativeError> {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut context = context(root, &cargo, target, prefix)?;
    context.push(format!("runtime features: {}", runtime_build.features()).into_bytes());
    let packages = dependencies(root, &cargo, target)?;
    let key = input_key(root, &packages, &context)?;
    publish(cache, &key, |build_dir| {
        let built = build(build_dir)?;
        if input_key(root, &packages, &context)? != key {
            return Err(NativeError::new(
                StageName::Link,
                "runtime inputs changed during the build; retry",
            ));
        }
        Ok(built)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const CRATES: &[&str] = &["hirsel-rt", "hirsel-kernels"];
    #[test]
    fn distinct_runtime_inputs_publish_distinct_archives_and_identical_trees_share() {
        let root =
            std::env::temp_dir().join(format!("hirsel-runtime-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
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
            publish(&cache, key, |build_dir| {
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
        publish(&cache, &first, |_| {
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
        use std::sync::mpsc;
        let cache =
            std::env::temp_dir().join(format!("hirsel-runtime-publication-{}", std::process::id()));
        let _ = fs::remove_dir_all(&cache);
        let (started, ready) = mpsc::channel();
        let (release, proceed) = mpsc::channel();
        let first_cache = cache.clone();
        let first = std::thread::spawn(move || {
            publish(&first_cache, "same-inputs", |dir| {
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
        let second_cache = cache.clone();
        let second = std::thread::spawn(move || {
            publish(&second_cache, "same-inputs", |_| {
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
    fn test_cache(name: &str) -> PathBuf {
        let cache =
            std::env::temp_dir().join(format!("hirsel-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&cache);
        private_directory(&cache).unwrap();
        cache
    }

    fn build_bytes(cache: &Path, key: &str, bytes: &[u8]) -> PathBuf {
        publish(cache, key, |dir| {
            fs::create_dir_all(dir).map_err(io)?;
            let built = dir.join("runtime.a");
            fs::write(&built, bytes).map_err(io)?;
            Ok(built)
        })
        .unwrap()
    }

    #[test]
    fn planted_archive_without_a_receipt_is_rebuilt() {
        let cache = test_cache("planted");
        fs::write(cache.join("key.a"), b"planted archive").unwrap();
        let archive = build_bytes(&cache, "key", b"trusted build");
        assert_eq!(fs::read(&archive).unwrap(), b"trusted build");
        publish(&cache, "key", |_| {
            panic!("verified archive should be reused")
        })
        .unwrap();
        fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn truncated_or_changed_archives_and_receipts_are_rebuilt() {
        let cache = test_cache("corrupt");
        let archive = build_bytes(&cache, "key", b"complete archive");
        fs::write(&archive, b"complete").unwrap();
        build_bytes(&cache, "key", b"complete archive");
        assert_eq!(fs::read(&archive).unwrap(), b"complete archive");
        fs::write(&archive, b"planted contents").unwrap();
        build_bytes(&cache, "key", b"complete archive");
        fs::write(cache.join("key.sha256"), "bad receipt").unwrap();
        build_bytes(&cache, "key", b"complete archive");
        publish(&cache, "key", |_| {
            panic!("verified archive should be reused")
        })
        .unwrap();
        fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn private_directory_checks_owner_and_mode_on_every_use() {
        use std::os::unix::fs::PermissionsExt;
        let cache = test_cache("permissions");
        let meta = fs::metadata(&cache).unwrap();
        assert_eq!(meta.mode() & 0o7777, 0o700);
        check_directory(&meta, meta.uid()).unwrap();
        // Exercise the exact production owner check without requiring root to chown.
        assert!(check_directory(&meta, meta.uid().wrapping_add(1)).is_err());
        build_bytes(&cache, "key", b"archive");
        for mode in [0o755, 0o770, 0o777, 0o1700] {
            fs::set_permissions(&cache, fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                publish(&cache, "key", |_| panic!(
                    "insecure directory must be refused"
                ))
                .is_err()
            );
        }
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o700)).unwrap();
        publish(&cache, "key", |_| panic!("private directory should reuse")).unwrap();
        fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn foreign_archive_permissions_and_symlinks_force_rebuilding() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let cache = test_cache("foreign");
        let archive = build_bytes(&cache, "key", b"trusted build");
        fs::set_permissions(&archive, fs::Permissions::from_mode(0o666)).unwrap();
        build_bytes(&cache, "key", b"trusted build");
        fs::remove_file(&archive).unwrap();
        let outside = cache.join("foreign.a");
        fs::write(&outside, b"foreign archive").unwrap();
        symlink(&outside, &archive).unwrap();
        build_bytes(&cache, "key", b"trusted build");
        assert_eq!(fs::read(&archive).unwrap(), b"trusted build");
        assert_eq!(fs::read(outside).unwrap(), b"foreign archive");
        let alias = cache.with_extension("alias");
        symlink(&cache, &alias).unwrap();
        assert!(publish(&alias, "key", |_| panic!("symlinked cache must be refused")).is_err());
        fs::remove_file(alias).unwrap();
        fs::remove_dir_all(cache).unwrap();
    }
}
