//! Front query namespace in the shared artifact store. Hits retain mapped
//! ranges; no second payload allocation is made before checking query reads.
use super::{EntryKey, ResultStore};
use std::fs;
use std::path::{Path, PathBuf};

/// The store format version in the directory name.
pub const FORMAT: u32 = 14;
/// The default size cap.
pub const DEFAULT_CAP: u64 = 2 << 30;

#[derive(Debug)]
pub struct DirStore {
    root: PathBuf,
    physical: hirsel_artifact_store::Store,
    read_only: Vec<hirsel_artifact_store::Store>,
    cap: u64,
    verifier: std::sync::Arc<hirsel_validate::VerifierCache>,
}

/// What a collection found and removed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GcReport {
    pub entries: u64,
    pub bytes: u64,
    pub removed: u64,
    pub removed_bytes: u64,
}

impl DirStore {
    pub fn new(root: PathBuf) -> Self {
        let physical = hirsel_artifact_store::Store::rebuildable(root.clone());
        physical.set_cache_cap(DEFAULT_CAP);
        Self {
            verifier: std::sync::Arc::new(hirsel_validate::VerifierCache::directory_store(
                physical.clone(),
            )),
            physical,
            root,
            read_only: vec![],
            cap: DEFAULT_CAP,
        }
    }
    /// Read installed ordinary products with the same checked format. Writes,
    /// collection and verifier associations remain in this store's own root.
    pub fn with_read_only_root(mut self, root: PathBuf) -> Self {
        if root != self.root && !self.read_only.iter().any(|store| store.root() == root) {
            self.read_only.push(hirsel_artifact_store::Store::new(root));
        }
        self
    }
    pub fn with_cap(mut self, cap: u64) -> Self {
        self.cap = cap;
        self.physical.set_cache_cap(cap);
        self
    }
    /// `$XDG_CACHE_HOME/hirsel/results-v<N>`, else `$HOME/.cache/...`.
    pub fn default_location() -> Option<Self> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|v| !v.is_empty())
                    .map(|h| PathBuf::from(h).join(".cache"))
            })?;
        // Like Go's os.UserCacheDir: an absent or relative location is not
        // authority to write a cache into the current working directory.
        if !base.is_absolute() {
            return None;
        }
        Some(
            Self::new(base.join("hirsel").join(format!("results-v{FORMAT}")))
                .with_installed_products(),
        )
    }
    /// Compose environment-selected local storage with installed ordinary
    /// products. Explicit library providers remain exactly caller configured.
    pub(super) fn with_installed_products(mut self) -> Self {
        let installed = std::env::var_os("HIRSEL_PRODUCTS")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .map(|executable| executable.with_file_name("hirsel-std.products"))
            });
        if let Some(installed) = installed {
            self = self.with_read_only_root(installed);
        }
        self
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Collect immutable outputs and action roots with bounded scratch.
    pub fn gc(&self, cap: u64) -> GcReport {
        let report = self.physical.collect(cap);
        GcReport {
            entries: report.entries,
            bytes: report.bytes,
            removed: report.removed,
            removed_bytes: report.removed_bytes,
        }
    }
    pub fn cap(&self) -> u64 {
        self.cap
    }
    fn publish(&self, key: &EntryKey, action: super::Action, value: &[u8]) {
        use hirsel_artifact_store::{Address, Domain};
        let store = &self.physical;
        let _batch = store.batch();
        let write = || -> std::io::Result<()> {
            let reference = store.put(
                Domain {
                    kind: key.2 as u32 + 256,
                    schema: FORMAT,
                },
                value,
            )?;
            store.associate(Address(key.0), action, reference)
        };
        let _ = write();
    }
}

impl ResultStore for DirStore {
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        Some(self.physical.clone())
    }
    fn lookup_product(&self, address: super::Address) -> Option<super::Association> {
        std::iter::once(&self.physical)
            .chain(&self.read_only)
            .find_map(|store| store.lookup(address))
    }
    fn associate_product(
        &self,
        address: super::Address,
        action: super::Action,
        reference: super::Reference,
    ) {
        let store = &self.physical;
        // Shared immutable roots may supply ordinary content, but local writes
        // and GC remain in the writable root. A missing reference is a miss.
        if self.get_content(reference).is_some() {
            let _ = store.associate(address, action, reference);
        }
    }
    fn verifier(&self) -> Option<std::sync::Arc<hirsel_validate::VerifierCache>> {
        Some(self.verifier.clone())
    }
    fn get_content(&self, reference: super::Reference) -> Option<super::Bytes> {
        for (index, root) in std::iter::once(&self.physical)
            .chain(&self.read_only)
            .enumerate()
        {
            let store = root;
            if let Some(bytes) = store.get(reference) {
                if index == 0 {
                    store.touch(
                        hirsel_artifact_store::Address(reference.output.0),
                        reference,
                    );
                }
                return Some(bytes);
            }
        }
        None
    }
    fn put_content(&self, domain: super::Domain, bytes: &[u8]) -> Option<super::Reference> {
        let reference = self.physical.put(domain, bytes).ok()?;
        Some(reference)
    }
    fn get(&self, key: &EntryKey) -> Option<super::Bytes> {
        for (index, root) in std::iter::once(&self.physical)
            .chain(&self.read_only)
            .enumerate()
        {
            let store = root;
            let Some(opened) = store.lookup_open(hirsel_artifact_store::Address(key.0)) else {
                if index == 0
                    && let Some(path) =
                        store.association_path(hirsel_artifact_store::Address(key.0))
                {
                    let _ = fs::remove_file(path);
                }
                continue;
            };
            let previous = opened.association;
            // Query code validates prior input/output identities before decoding.
            if previous.reference.domain
                != (hirsel_artifact_store::Domain {
                    kind: key.2 as u32 + 256,
                    schema: FORMAT,
                })
            {
                continue;
            }
            let Some(bytes) = opened.get() else {
                if index == 0 {
                    let _ = fs::remove_file(store.output_path(previous.reference));
                }
                continue;
            };
            if index == 0 {
                store.touch(hirsel_artifact_store::Address(key.0), previous.reference);
            }
            return Some(bytes);
        }
        None
    }
    fn put(&self, key: &EntryKey, value: &[u8]) {
        self.publish(
            key,
            super::Action(hirsel_syntax::ContentHash::of_parts(
                "hirsel.result.raw-action.v1",
                [key.0.0.as_slice(), key.1.0.as_slice()],
            )),
            value,
        );
    }
    fn put_query(
        &self,
        kind: crate::QueryKind,
        key: &EntryKey,
        action: super::Action,
        value: &[u8],
    ) {
        assert_eq!(kind, key.2);
        self.publish(key, action, value);
    }
    fn name(&self) -> &'static str {
        "dir@1"
    }
}
