//! The simulated disk's crash model, shared by every simulated `Dir` and the simulated
//! journal (core-library.md, Durability). A tree's durable state is what a crash keeps: each
//! directory's last synchronized listing and each file's last synchronized bytes. Writes,
//! creations, renames and removals change only the live tree until a sync covers them.
//! A crash rebuilds the tree from the durable state; the last unsynced write survives as a
//! prefix chosen by the crash seed when everything before it in its file is durable.
use std::collections::{BTreeMap, BTreeSet};

/// A directory entry: a directory, or a file by its identity (its inode).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Entry {
    Directory,
    File(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Write {
    file: u64,
    start: usize,
    bytes: Vec<u8>,
}

/// The durable state of one simulated tree.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Durable {
    /// Directory path ("" is the tree root) to its durable children by name.
    listings: BTreeMap<String, BTreeMap<String, Entry>>,
    /// File identity to its durable bytes; an identity absent here has none.
    data: BTreeMap<u64, Vec<u8>>,
    /// The most recent write since the last crash, while unsynced.
    last: Option<Write>,
}

/// The tree a crash leaves: its directories and its files with their identities and bytes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Crashed {
    pub directories: BTreeSet<String>,
    pub files: BTreeMap<String, (u64, Vec<u8>)>,
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(p, _)| p)
}
fn name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, n)| n)
}
fn join(directory: &str, name: &str) -> String {
    if directory.is_empty() {
        name.into()
    } else {
        format!("{directory}/{name}")
    }
}

/// The children of `directory` in a live tree of directory paths and file identities.
fn listing(
    directory: &str,
    directories: &BTreeSet<String>,
    files: &BTreeMap<String, u64>,
) -> BTreeMap<String, Entry> {
    let children = directories
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| (p, Entry::Directory))
        .chain(files.iter().map(|(p, id)| (p, Entry::File(*id))));
    children
        .filter(|(p, _)| parent(p) == directory)
        .map(|(p, entry)| (name(p).to_owned(), entry))
        .collect()
}

impl Durable {
    /// A tree whose every directory and file is durable, such as a fresh `test.dir`.
    pub fn of(directories: &BTreeSet<String>, files: &BTreeMap<String, (u64, Vec<u8>)>) -> Self {
        let identities = files.iter().map(|(p, (id, _))| (p.clone(), *id)).collect();
        let mut durable = Self::default();
        for directory in directories {
            durable.listings.insert(
                directory.clone(),
                listing(directory, directories, &identities),
            );
        }
        for (id, bytes) in files.values() {
            durable.data.insert(*id, bytes.clone());
        }
        durable
    }
    /// Note an unsynced write of `bytes` at offset `start` of file `file`.
    pub fn wrote(&mut self, file: u64, start: usize, bytes: &[u8]) {
        self.last = Some(Write {
            file,
            start,
            bytes: bytes.to_vec(),
        });
    }
    /// Make `file`'s current bytes, and the listing of every directory from `path`'s parent
    /// up to the tree root, durable. `path` is where the file is named in the live tree.
    pub fn sync(
        &mut self,
        file: u64,
        bytes: &[u8],
        path: &str,
        directories: &BTreeSet<String>,
        files: &BTreeMap<String, u64>,
    ) {
        self.data.insert(file, bytes.to_vec());
        if self.last.as_ref().is_some_and(|w| w.file == file) {
            self.last = None;
        }
        let mut directory = parent(path);
        loop {
            if directories.contains(directory) {
                self.listings
                    .insert(directory.to_owned(), listing(directory, directories, files));
            }
            if directory.is_empty() {
                break;
            }
            directory = parent(directory);
        }
    }
    /// The tree a crash leaves. `seed` chooses how much of the last unsynced write
    /// survives: none, a prefix or all of it, uniformly by byte count.
    pub fn crash(&self, seed: u64) -> Crashed {
        let torn = self.last.as_ref().and_then(|w| {
            let durable = self.data.get(&w.file).map_or(0, Vec::len);
            (durable == w.start).then(|| {
                let keep = (mix(seed) % (w.bytes.len() as u64 + 1)) as usize;
                (w.file, &w.bytes[..keep])
            })
        });
        let mut crashed = Crashed {
            directories: BTreeSet::from([String::new()]),
            files: BTreeMap::new(),
        };
        let mut pending = vec![String::new()];
        while let Some(directory) = pending.pop() {
            for (child, entry) in self.listings.get(&directory).into_iter().flatten() {
                let path = join(&directory, child);
                match entry {
                    Entry::Directory => {
                        crashed.directories.insert(path.clone());
                        pending.push(path);
                    }
                    Entry::File(id) => {
                        let mut bytes = self.data.get(id).cloned().unwrap_or_default();
                        if let Some((file, prefix)) = torn
                            && file == *id
                        {
                            bytes.extend_from_slice(prefix);
                        }
                        crashed.files.insert(path, (*id, bytes));
                    }
                }
            }
        }
        crashed
    }
    /// After a crash the surviving tree is durable as it stands.
    pub fn crashed(&mut self, seed: u64) -> Crashed {
        let crashed = self.crash(seed);
        *self = Self::of(&crashed.directories, &crashed.files);
        crashed
    }
}

/// SplitMix64's output function: a fixed, well-mixed choice from the seed.
fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(dirs: &[&str], files: &[(&str, u64)]) -> (BTreeSet<String>, BTreeMap<String, u64>) {
        (
            dirs.iter().map(|d| d.to_string()).collect(),
            files.iter().map(|(p, id)| (p.to_string(), *id)).collect(),
        )
    }

    #[test]
    fn unsynced_creations_vanish_and_synced_ones_survive() {
        let mut disk = Durable::of(&BTreeSet::from([String::new()]), &BTreeMap::new());
        let (dirs, files) = tree(&["", "log"], &[("log/1", 1), ("log/2", 2)]);
        disk.sync(1, b"one", "log/1", &dirs, &files);
        let crashed = disk.crash(0);
        assert_eq!(
            crashed.directories,
            BTreeSet::from(["".into(), "log".into()])
        );
        // The directory sync named log/2 too, but its bytes were never synced.
        assert_eq!(crashed.files["log/1"], (1, b"one".to_vec()));
        assert_eq!(crashed.files["log/2"], (2, vec![]));
    }

    #[test]
    fn a_sync_makes_its_whole_directory_chain_durable() {
        let mut disk = Durable::of(&BTreeSet::from([String::new()]), &BTreeMap::new());
        let (dirs, files) = tree(&["", "a", "a/b"], &[("a/b/f", 3)]);
        disk.sync(3, b"f", "a/b/f", &dirs, &files);
        let crashed = disk.crashed(1);
        assert_eq!(crashed.files["a/b/f"].1, b"f");
        assert_eq!(disk.crash(2), crashed);
    }
}
