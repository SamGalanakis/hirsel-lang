//! Independent model of a simulated tree's crash (core-library.md, Durability): each
//! directory's last synchronized listing and each file's last synchronized bytes survive,
//! and the last unsynced write keeps a prefix chosen by the crash seed.
use super::{Io, IoGrant, io_error, io_ok, io_portable};
use hirsel_heapcheck::{OpOutcome, OpValue};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Weak};

/// A durable child: a directory, or a file by its identity serial.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Child {
    Directory,
    File(u64),
}

#[derive(Clone, Debug, Default)]
pub(super) struct Durable {
    listings: BTreeMap<String, BTreeMap<String, Child>>,
    data: BTreeMap<u64, Vec<u8>>,
    /// The last unsynced write: identity serial, offset and bytes.
    last: Option<(u64, usize, Vec<u8>)>,
}

/// File identities as stable serials; each Weak keeps its identity's address unique.
#[derive(Clone, Debug, Default)]
pub(super) struct Serials(Vec<(Weak<()>, u64)>);
impl Serials {
    fn of(&mut self, identity: &Arc<()>) -> u64 {
        if let Some((_, serial)) = self
            .0
            .iter()
            .find(|(weak, _)| std::ptr::eq(weak.as_ptr(), Arc::as_ptr(identity)))
        {
            return *serial;
        }
        let serial = self.0.len() as u64 + 1;
        self.0.push((Arc::downgrade(identity), serial));
        serial
    }
    fn identity(&mut self, serial: u64) -> Arc<()> {
        if let Some(identity) = self
            .0
            .iter()
            .find(|(_, s)| *s == serial)
            .and_then(|(weak, _)| weak.upgrade())
        {
            return identity;
        }
        let identity = Arc::new(());
        self.0.retain(|(_, s)| *s != serial);
        self.0.push((Arc::downgrade(&identity), serial));
        identity
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(p, _)| p)
}

impl Io {
    fn listing(&mut self, tree: u64, directory: &str) -> BTreeMap<String, Child> {
        let (directories, files) = &self.trees[&tree];
        let mut listing = BTreeMap::new();
        for path in directories {
            if !path.is_empty() && parent(path) == directory {
                let name = path.rsplit_once('/').map_or(path.as_str(), |(_, n)| n);
                listing.insert(name.to_owned(), Child::Directory);
            }
        }
        let named: Vec<_> = files
            .keys()
            .filter(|p| parent(p) == directory)
            .cloned()
            .collect();
        for path in named {
            let identity = self.files[&(tree, path.clone())].clone();
            let name = path.rsplit_once('/').map_or(path.as_str(), |(_, n)| n);
            listing.insert(name.to_owned(), Child::File(self.serials.of(&identity)));
        }
        listing
    }
    /// A `test.dir` tree starts durable.
    pub(super) fn durable_start(&mut self, tree: u64) {
        let mut durable = Durable::default();
        for directory in self.trees[&tree].0.clone() {
            let listing = self.listing(tree, &directory);
            durable.listings.insert(directory, listing);
        }
        for (path, bytes) in self.trees[&tree].1.clone() {
            let identity = self.files[&(tree, path)].clone();
            durable.data.insert(self.serials.of(&identity), bytes);
        }
        self.durable.insert(tree, durable);
    }
    pub(super) fn durable_write(
        &mut self,
        tree: u64,
        identity: &Arc<()>,
        start: usize,
        bytes: &[u8],
    ) {
        let serial = self.serials.of(identity);
        if let Some(durable) = self.durable.get_mut(&tree)
            && !bytes.is_empty()
        {
            durable.last = Some((serial, start, bytes.to_vec()));
        }
    }
    /// The file's bytes and the listings from `path`'s parent up to the tree root.
    pub(super) fn durable_sync(&mut self, tree: u64, identity: &Arc<()>, bytes: &[u8], path: &str) {
        if !self.durable.contains_key(&tree) {
            return;
        }
        let serial = self.serials.of(identity);
        let mut listings = vec![];
        let mut directory = parent(path).to_owned();
        loop {
            if self.trees[&tree].0.contains(&directory) {
                listings.push((directory.clone(), self.listing(tree, &directory)));
            }
            if directory.is_empty() {
                break;
            }
            directory = parent(&directory).to_owned();
        }
        let durable = self.durable.get_mut(&tree).unwrap();
        durable.data.insert(serial, bytes.to_vec());
        if durable.last.as_ref().is_some_and(|(s, ..)| *s == serial) {
            durable.last = None;
        }
        durable.listings.extend(listings);
    }
    /// `test.crash(t, d, seed)`.
    pub(super) fn crash(&mut self, directory: u64, seed: u64) -> Option<OpOutcome> {
        let Some(IoGrant::Dir { tree, .. }) = self.grants.get(&directory) else {
            return Some(OpOutcome::Fail("CAP-2".into()));
        };
        let tree = *tree;
        let Some(durable) = self.durable.get(&tree).cloned() else {
            return Some(OpOutcome::Fail("LANG-9".into()));
        };
        let first = hirsel_rt::io::RandomSource::SplitMix64(seed).draw(8).ok()?;
        let m = u64::from_le_bytes(first.try_into().ok()?);
        let torn = durable.last.as_ref().and_then(|(serial, start, bytes)| {
            (durable.data.get(serial).map_or(0, Vec::len) == *start).then(|| {
                (
                    *serial,
                    bytes[..(m % (bytes.len() as u64 + 1)) as usize].to_vec(),
                )
            })
        });
        let mut directories = BTreeSet::from([String::new()]);
        let mut files = BTreeMap::new();
        let mut identities = BTreeMap::new();
        let mut pending = vec![String::new()];
        while let Some(directory) = pending.pop() {
            for (name, child) in durable.listings.get(&directory).into_iter().flatten() {
                let path = if directory.is_empty() {
                    name.clone()
                } else {
                    format!("{directory}/{name}")
                };
                match child {
                    Child::Directory => {
                        directories.insert(path.clone());
                        pending.push(path);
                    }
                    Child::File(serial) => {
                        let mut bytes = durable.data.get(serial).cloned().unwrap_or_default();
                        if let Some((file, prefix)) = &torn
                            && file == serial
                        {
                            bytes.extend_from_slice(prefix);
                        }
                        identities.insert(path.clone(), self.serials.identity(*serial));
                        files.insert(path, bytes);
                    }
                }
            }
        }
        self.permissions
            .retain(|(t, p), _| *t != tree || directories.contains(p) || files.contains_key(p));
        self.files.retain(|(t, _), _| *t != tree);
        for (path, identity) in identities {
            self.files.insert((tree, path), identity);
        }
        self.trees.insert(tree, (directories, files));
        let stale: Vec<u64> = self
            .grants
            .iter()
            .filter_map(|(id, grant)| match grant {
                IoGrant::File { tree: t, .. } | IoGrant::Atomic { tree: t, .. } if *t == tree => {
                    Some(*id)
                }
                IoGrant::Mapped { directory, .. }
                    if matches!(self.grants.get(directory), Some(IoGrant::Dir { tree: t, .. }) if *t == tree) =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect();
        self.stale.extend(stale);
        self.durable_start(tree);
        Some(OpOutcome::Value(OpValue::Unit))
    }
    /// `file.sync(d, mut f)`.
    pub(super) fn sync(&mut self, directory: u64, file: u64) -> Option<OpOutcome> {
        let Some(IoGrant::Dir { tree, .. }) = self.grants.get(&directory).cloned() else {
            return Some(OpOutcome::Fail("CAP-2".into()));
        };
        let Some(IoGrant::File {
            tree: owner_tree,
            directory: owner,
            identity,
            path,
            ..
        }) = self.grants.get(&file).cloned()
        else {
            return Some(OpOutcome::Fail("CAP-2".into()));
        };
        if owner_tree != tree || owner != directory {
            return Some(OpOutcome::Fail("CAP-2".into()));
        }
        if self.stale.contains(&file) {
            return Some(OpOutcome::Value(io_error(1)));
        }
        let bytes = self
            .files
            .iter()
            .find(|((t, _), id)| *t == tree && Arc::ptr_eq(id, &identity))
            .and_then(|((_, p), _)| self.trees[&tree].1.get(p))
            .cloned()
            .unwrap_or_default();
        let open = self.physical_path(directory, &path).unwrap_or_default();
        self.durable_sync(tree, &identity, &bytes, &open);
        Some(OpOutcome::Value(io_ok(OpValue::Unit)))
    }
    /// `file.put_new(d, p, take data, mode)` at physical `path`.
    pub(super) fn put_new(&mut self, tree: u64, path: &str, data: &[u8], durable: bool) -> OpValue {
        let (directories, files) = &self.trees[&tree];
        if directories.contains(path) {
            return io_portable(5);
        }
        if !directories.contains(parent(path)) {
            return io_portable(if files.contains_key(parent(path)) {
                7
            } else {
                3
            });
        }
        if let Some(existing) = files.get(path) {
            if existing != data {
                return io_portable(5);
            }
            if durable {
                let identity = self.files[&(tree, path.to_owned())].clone();
                let bytes = existing.clone();
                self.durable_sync(tree, &identity, &bytes, path);
            }
            return io_ok(OpValue::Unit);
        }
        self.trees
            .get_mut(&tree)
            .unwrap()
            .1
            .insert(path.to_owned(), data.to_vec());
        self.permissions.remove(&(tree, path.to_owned()));
        let identity = Arc::new(());
        self.files.insert((tree, path.to_owned()), identity.clone());
        if durable {
            self.durable_sync(tree, &identity, data, path);
        } else {
            self.durable_write(tree, &identity, 0, data);
        }
        io_ok(OpValue::Unit)
    }
}
