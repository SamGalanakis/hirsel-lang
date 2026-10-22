//! Content identities for link inputs. Metadata is a reuse hint, never an identity.
//!
//! Like Git's stat cache and racy-index check (statinfo.c; Documentation/technical/
//! racy-git.adoc), include ctime/device and refuse recent timestamps. The private,
//! user-owned receipts are disposable, not portable artifacts. A miss reads all
//! bytes. Both existing SHA-256 encodings are accumulated in that one read so
//! native-plan fingerprints and build IDs keep their original byte encodings.
use hirsel_syntax::ContentHash;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Digests {
    pub raw: [u8; 32],
    pub content: ContentHash,
}

// dev, inode, mode, size, mtime seconds/nanos, ctime seconds/nanos.
type Stamp = [u64; 8];
type Receipt = (u32, Vec<u8>, Stamp, [u8; 32], [u8; 32]);

fn stamp(meta: &std::fs::Metadata) -> Stamp {
    [
        meta.dev(),
        meta.ino(),
        meta.mode() as u64,
        meta.size(),
        meta.mtime() as u64,
        meta.mtime_nsec() as u64,
        meta.ctime() as u64,
        meta.ctime_nsec() as u64,
    ]
}

fn stable(state: &Stamp, now: SystemTime) -> bool {
    // Two seconds covers coarse timestamp clocks, as in a racy Git index.
    // Future timestamps (including a clock rollback) never become cache hits.
    let cutoff = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(2);
    state[4] < cutoff && state[6] < cutoff
}

fn read_digests(file: &mut File, size: u64) -> std::io::Result<Digests> {
    let mut raw = Sha256::new();
    let mut content = Sha256::new();
    let domain = b"hirsel.text.v1";
    content.update((domain.len() as u64).to_le_bytes());
    content.update(domain);
    content.update(size.to_le_bytes());
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        raw.update(&buffer[..count]);
        content.update(&buffer[..count]);
    }
    Ok(Digests {
        raw: raw.finalize().into(),
        content: ContentHash(content.finalize().into()),
    })
}

pub(super) fn file(path: &Path) -> std::io::Result<Digests> {
    let cache = std::env::var_os("HIRSEL_NATIVE_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(super::runtime_cache::default_directory)
        .join("file-digests-v1");
    cached(path, &cache, SystemTime::now())
}

static MEMORY: Mutex<BTreeMap<PathBuf, (Stamp, Digests)>> = Mutex::new(BTreeMap::new());

fn cached(path: &Path, cache: &Path, started: SystemTime) -> std::io::Result<Digests> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut source = File::open(&path)?;
    let meta = source.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::other("a link input must be a regular file"));
    }
    let state = stamp(&meta);
    let reusable = stable(&state, started);
    if reusable
        && let Some((known, value)) = MEMORY.lock().unwrap().get(&path)
        && *known == state
    {
        return Ok(*value);
    }
    // Cache failures only disable reuse. In particular, never follow a receipt
    // symlink or accept a foreign, shared, malformed or differently keyed file.
    let private = super::runtime_cache::private_directory(cache).is_ok();
    let name = format!("{:x}", Sha256::digest(path.as_os_str().as_bytes()));
    let receipt = cache.join(name);
    if reusable
        && private
        && let Ok(file) = super::runtime_cache::private_file(&receipt)
        && let Ok((1, known_path, known, raw, content)) =
            serde_json::from_reader::<_, Receipt>(file.take(4096))
        && known_path == path.as_os_str().as_bytes()
        && known == state
    {
        let value = Digests {
            raw,
            content: ContentHash(content),
        };
        MEMORY.lock().unwrap().insert(path, (state, value));
        return Ok(value);
    }
    let (state, value) = verified_digests(&path, &mut source, || {})?;
    if stable(&state, started) {
        MEMORY.lock().unwrap().insert(path.clone(), (state, value));
        if private {
            let record: Receipt = (
                1,
                path.as_os_str().as_bytes().to_vec(),
                state,
                value.raw,
                value.content.0,
            );
            // Atomic replacement: parallel writers publish the same verified state;
            // a delayed old writer can only cause a subsequent metadata miss.
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let temporary = cache.join(format!(
                ".{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let publish = || -> std::io::Result<()> {
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&temporary)?;
                output.write_all(&serde_json::to_vec(&record)?)?;
                std::fs::rename(&temporary, &receipt)
            };
            let _ = publish();
            let _ = std::fs::remove_file(temporary);
        }
    }
    Ok(value)
}

fn verified_digests(
    path: &Path,
    source: &mut File,
    mut after_read: impl FnMut(),
) -> std::io::Result<(Stamp, Digests)> {
    // CAS materialization can add/remove hard links to an immutable input,
    // changing ctime during a read. Keep ctime in every cache/verification
    // check, but retry a full read before treating metadata churn as failure.
    for attempt in 0..3 {
        let state = stamp(&source.metadata()?);
        source.rewind()?;
        let value = read_digests(source, state[3])?;
        after_read();
        match verify_state(path, source, &state) {
            Ok(()) => return Ok((state, value)),
            Err(error) if attempt == 2 => return Err(error),
            Err(_) => {}
        }
    }
    unreachable!()
}

fn verify_state(path: &Path, source: &File, state: &Stamp) -> std::io::Result<()> {
    let opened = stamp(&source.metadata()?);
    let named = stamp(&std::fs::metadata(path)?);
    if opened != *state || named != *state {
        return Err(std::io::Error::other(format!(
            "a link input changed while hashing: {state:?} -> opened {opened:?}, named {named:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::FileTimes;
    use std::os::unix::fs::symlink;
    use std::time::Duration;

    fn fixture(name: &str) -> (PathBuf, PathBuf, SystemTime) {
        let root =
            std::env::temp_dir().join(format!("hirsel-digests-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("input");
        std::fs::write(&source, b"abcd").unwrap();
        // Advance only the test's clock; production always uses SystemTime::now.
        (
            source,
            root.join("cache"),
            SystemTime::now() + Duration::from_secs(10),
        )
    }

    fn forget(path: &Path) {
        MEMORY.lock().unwrap().remove(path);
    }

    fn expected(bytes: &[u8]) -> Digests {
        Digests {
            raw: Sha256::digest(bytes).into(),
            content: ContentHash::of_bytes(bytes),
        }
    }

    #[test]
    fn raw_and_framed_hash_vectors_and_disk_reuse() {
        let (path, cache, now) = fixture("vectors");
        for bytes in [b"".as_slice(), b"abc", &vec![0xab; 131073]] {
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(cached(&path, &cache, now).unwrap(), expected(bytes));
            forget(&path);
            assert_eq!(cached(&path, &cache, now).unwrap(), expected(bytes));
        }
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn replacement_during_a_read_is_refused_with_opened_and_named_states() {
        let (path, _, _) = fixture("during-read");
        let opened = File::open(&path).unwrap();
        let before = stamp(&opened.metadata().unwrap());
        let replacement = path.with_file_name("replacement");
        std::fs::write(&replacement, b"new!").unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        let error = verify_state(&path, &opened, &before)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("opened") && error.contains("named"),
            "{error}"
        );
        // The old descriptor remains valid, but the link would open a new inode.
        assert_eq!(opened.metadata().unwrap().ino(), before[1]);
        assert_eq!(opened.metadata().unwrap().size(), before[3]);
        assert_ne!(stamp(&std::fs::metadata(&path).unwrap()), before);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn hard_link_publication_during_hashing_retries_without_changing_identity() {
        let (path, _, _) = fixture("hard-link");
        let mut opened = File::open(&path).unwrap();
        let original = stamp(&opened.metadata().unwrap());
        let alias = path.with_file_name("published");
        let mut reads = 0;
        let (state, value) = verified_digests(&path, &mut opened, || {
            reads += 1;
            if reads == 1 {
                // Pool actions link immutable CAS inputs into independent trees.
                // Publishing another name changes ctime, without changing bytes.
                std::fs::hard_link(&path, &alias).unwrap();
                std::fs::remove_file(&alias).unwrap();
            }
        })
        .unwrap();
        assert_eq!(reads, 2);
        assert_eq!(value, expected(b"abcd"));
        assert_eq!(state, stamp(&std::fs::metadata(&path).unwrap()));
        assert_eq!(state[..6], original[..6]);
        assert_ne!(state[6..], original[6..]);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn bytes_changed_during_hashing_are_read_again_even_with_restored_mtime() {
        let (path, _, _) = fixture("during-write");
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut opened = File::open(&path).unwrap();
        let mut reads = 0;
        let (_, value) = verified_digests(&path, &mut opened, || {
            reads += 1;
            if reads == 1 {
                std::fs::write(&path, b"efgh").unwrap();
                File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_times(FileTimes::new().set_modified(modified))
                    .unwrap();
            }
        })
        .unwrap();
        assert_eq!(reads, 2);
        assert_eq!(value, expected(b"efgh"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn persistent_changes_and_path_replacement_fail_after_bounded_retries() {
        for replacement in [false, true] {
            let (path, _, _) = fixture(if replacement {
                "retry-replace"
            } else {
                "retry-write"
            });
            let mut opened = File::open(&path).unwrap();
            let mut reads = 0;
            let result = verified_digests(&path, &mut opened, || {
                reads += 1;
                if replacement {
                    let next = path.with_file_name("replacement");
                    std::fs::write(&next, b"efgh").unwrap();
                    std::fs::rename(&next, &path).unwrap();
                } else {
                    std::fs::write(&path, vec![0; reads]).unwrap();
                }
            });
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("changed while hashing")
            );
            assert_eq!(reads, 3);
            std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn changed_bytes_with_equal_size_and_restored_mtime_miss() {
        let (path, cache, now) = fixture("mtime");
        let before = std::fs::metadata(&path).unwrap();
        assert_eq!(cached(&path, &cache, now).unwrap(), expected(b"abcd"));
        std::fs::write(&path, b"efgh").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(before.modified().unwrap()))
            .unwrap();
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        assert_eq!(before.len(), after.len());
        assert_ne!(stamp(&before), stamp(&after));
        assert_eq!(cached(&path, &cache, now).unwrap(), expected(b"efgh"));
        forget(&path);
        assert_eq!(cached(&path, &cache, now).unwrap(), expected(b"efgh"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn same_path_replacement_and_symlink_retarget_miss() {
        let (path, cache, now) = fixture("replacement");
        let original = std::fs::metadata(&path).unwrap();
        cached(&path, &cache, now).unwrap();
        let replacement = path.with_file_name("replacement");
        std::fs::write(&replacement, b"efgh").unwrap();
        File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(FileTimes::new().set_modified(original.modified().unwrap()))
            .unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert_ne!(std::fs::metadata(&path).unwrap().ino(), original.ino());
        assert_eq!(cached(&path, &cache, now).unwrap(), expected(b"efgh"));
        let alias = path.with_file_name("alias");
        symlink(&path, &alias).unwrap();
        assert_eq!(cached(&alias, &cache, now).unwrap(), expected(b"efgh"));
        std::fs::write(&replacement, b"ijkl").unwrap();
        std::fs::remove_file(&alias).unwrap();
        symlink(&replacement, &alias).unwrap();
        assert_eq!(cached(&alias, &cache, now).unwrap(), expected(b"ijkl"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn inode_reuse_requires_ctime_and_device_and_recent_states_are_never_saved() {
        let (path, cache, now) = fixture("racy");
        let state = stamp(&std::fs::metadata(&path).unwrap());
        assert!(!stable(&state, SystemTime::now()));
        assert!(!stable(&state, UNIX_EPOCH));
        assert_eq!(
            cached(&path, &cache, SystemTime::now()).unwrap(),
            expected(b"abcd")
        );
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
        assert!(!MEMORY.lock().unwrap().contains_key(&path));
        cached(&path, &cache, now).unwrap();
        for slot in [0, 6, 7] {
            // Deterministic inode-reuse control: same inode, path, size, mtime,
            // but another device or change time, including nanoseconds.
            let mut old = state;
            old[slot] = old[slot].wrapping_add(1);
            MEMORY
                .lock()
                .unwrap()
                .insert(path.clone(), (old, expected(b"fake")));
            assert_eq!(cached(&path, &cache, now).unwrap(), expected(b"abcd"));
        }
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn invalid_receipts_and_unavailable_cache_fall_back_to_full_read() {
        let (path, cache, now) = fixture("corrupt");
        cached(&path, &cache, now).unwrap();
        let receipt = std::fs::read_dir(&cache)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(&receipt, b"truncated").unwrap();
        forget(&path);
        assert_eq!(cached(&path, &cache, now).unwrap(), expected(b"abcd"));
        std::fs::remove_file(&receipt).unwrap();
        symlink(&path, &receipt).unwrap();
        forget(&path);
        assert_eq!(cached(&path, &cache, now).unwrap(), expected(b"abcd"));
        assert_eq!(std::fs::read(&path).unwrap(), b"abcd");
        forget(&path);
        assert_eq!(cached(&path, &path, now).unwrap(), expected(b"abcd"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
