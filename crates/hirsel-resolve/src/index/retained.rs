//! Immutable manifest-index backing. Rust alloc BTree node layouts, as used
//! by hirsel-types/generics.rs, bound even a retained empty root. Shared rows
//! are charged per occurrence so inspection needs no identity-set allocation.
use super::*;
use std::alloc::Layout;

#[derive(Default)]
struct Bytes(u64);
impl Bytes {
    fn add(&mut self, n: usize) {
        self.0 = self.0.saturating_add(n as u64);
    }
    fn arc<T: ?Sized>(&mut self, value: &T) {
        self.add(
            Layout::new::<[usize; 2]>()
                .extend(Layout::for_value(value))
                .expect("existing Arc allocation")
                .0
                .pad_to_align()
                .size(),
        );
    }
    fn tree<K, V>(&mut self, n: usize) {
        let parts = [
            Layout::new::<usize>(),
            Layout::new::<u16>(),
            Layout::new::<u16>(),
            Layout::new::<[K; 11]>(),
            Layout::new::<[V; 11]>(),
            Layout::new::<[usize; 12]>(),
        ];
        let alignment = parts.iter().map(Layout::align).max().unwrap();
        let node = parts.iter().fold(alignment - 1, |n, l| {
            n.saturating_add(l.size()).saturating_add(l.align() - 1)
        });
        self.add((n / 5 + 1).saturating_mul(node));
    }
    fn package(&mut self, value: &PackageId) {
        self.add(value.name.capacity());
        self.add(value.version.capacity());
    }
    fn strings(&mut self, values: &Vec<String>) {
        self.add(values.capacity().saturating_mul(size_of::<String>()));
        for value in values {
            self.add(value.capacity());
        }
    }
    fn path(&mut self, value: &DefPath) {
        self.package(&value.0.package);
        self.strings(&value.0.modules);
        self.add(value.0.name.capacity());
    }
}
impl ManifestIndex {
    /// Owned and shared metadata payload retained by this index. Excludes the
    /// inline index and session-interned DefIds; includes all shared Arc backing
    /// conservatively, without source lookup or temporary allocation.
    pub fn retained_payload_bytes(&self) -> u64 {
        let mut bytes = Bytes::default();
        bytes.package(&self.package);
        bytes.arc(&*self.entries);
        bytes.add(
            self.entries
                .capacity()
                .saturating_mul(size_of::<(DefId, DefinitionEntry)>())
                .saturating_mul(2),
        );
        // DefMap uses hashbrown: bucket count may exceed usable capacity.
        // Two entries per usable slot plus controls covers its <=7/8 load.
        if self.entries.capacity() != 0 {
            bytes.add(self.entries.capacity().saturating_mul(2).saturating_add(16));
        }
        for value in self.entries.values() {
            bytes.path(&value.definition);
        }
        bytes.arc(&*self.heads);
        bytes.tree::<String, BTreeSet<(PackageId, Vec<String>)>>(self.heads.len());
        for (head, values) in &*self.heads {
            bytes.add(head.capacity());
            bytes.tree::<(PackageId, Vec<String>), ()>(values.len());
            for (package, modules) in values {
                bytes.package(package);
                bytes.strings(modules);
            }
        }
        bytes.arc(&*self.module_heads);
        bytes.tree::<(PackageId, Vec<String>), BTreeSet<String>>(self.module_heads.len());
        for ((package, modules), values) in &*self.module_heads {
            bytes.package(package);
            bytes.strings(modules);
            bytes.tree::<String, ()>(values.len());
            for value in values {
                bytes.add(value.capacity());
            }
        }
        bytes.0
    }
}
