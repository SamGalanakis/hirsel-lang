//! Front query namespace in the shared artifact store. Hits retain mapped
//! ranges; no second payload allocation is made before checking query reads.
use super::{EntryKey, ResultStore};
use std::fs;
use std::path::{Path, PathBuf};

/// The store format version in the directory name.
pub const FORMAT: u32 = 15;
/// The default size cap.
pub const DEFAULT_CAP: u64 = 2 << 30;

#[derive(Debug)]
pub struct DirStore {
    root: PathBuf,
    physical: Option<hirsel_artifact_store::Store>,
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
            physical: Some(physical),
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
        if let Some(physical) = &self.physical {
            physical.set_cache_cap(cap);
        }
        self
    }
    /// `$XDG_CACHE_HOME/hirsel/results-v<N>`, else `$HOME/.cache/...`.
    pub fn default_location() -> Option<Self> {
        Some(
            Self::new(
                Self::cache_base()?
                    .join("hirsel")
                    .join(format!("results-v{FORMAT}")),
            )
            .with_installed_products(),
        )
    }
    fn cache_base() -> Option<PathBuf> {
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
        Some(base)
    }
    /// V's own local associations are independent of query-result writes.
    /// Neither a run's result directory nor an installed product root selects
    /// this directory. Only V receives the writable handle.
    fn installed_verifier(installed: &Path) -> std::sync::Arc<hirsel_validate::VerifierCache> {
        Self::cache_base()
            .and_then(|base| Self::local_verifier(&base, installed))
            .map(std::sync::Arc::new)
            .unwrap_or_default()
    }
    fn local_verifier(base: &Path, installed: &Path) -> Option<hirsel_validate::VerifierCache> {
        // Canonicalize the existing cache base before appending the private
        // namespace. Refuse ambiguous/unavailable locations rather than letting
        // product selection point at the verifier's trusted associations.
        let base = base.canonicalize().ok()?;
        let installed = installed.canonicalize().ok()?;
        let root = base.join("hirsel").join("validator-v2");
        let root = root.canonicalize().unwrap_or(root);
        if root.starts_with(&installed) || installed.starts_with(&root) {
            return None;
        }
        // A symlink on the existing namespace parent also needs resolving.
        let parent = root.parent()?;
        if parent.exists() && parent.canonicalize().ok()? != parent {
            return None;
        }
        let physical = hirsel_artifact_store::Store::rebuildable(root);
        physical.set_cache_cap(DEFAULT_CAP);
        Some(hirsel_validate::VerifierCache::directory_store(physical))
    }
    /// Installed ordinary products are independent of writable-cache policy.
    fn installed_root() -> Option<PathBuf> {
        std::env::var_os("HIRSEL_PRODUCTS")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .map(|executable| executable.with_file_name("hirsel-std.products"))
            })
    }
    pub(super) fn installed_only() -> Option<Self> {
        let root = Self::installed_root().filter(|root| root.is_dir())?;
        let verifier = Self::installed_verifier(&root);
        Some(Self {
            read_only: vec![hirsel_artifact_store::Store::new(root.clone())],
            root,
            physical: None,
            cap: 0,
            verifier,
        })
    }
    /// Compose environment-selected local storage with installed ordinary
    /// products. Explicit library providers remain exactly caller configured.
    pub(super) fn with_installed_products(mut self) -> Self {
        if let Some(installed) = Self::installed_root() {
            self = self.with_read_only_root(installed);
        }
        self
    }
    fn roots(&self) -> impl Iterator<Item = (bool, &hirsel_artifact_store::Store)> {
        self.physical
            .iter()
            .map(|root| (true, root))
            .chain(self.read_only.iter().map(|root| (false, root)))
    }
    fn publication_store(&self) -> Option<&hirsel_artifact_store::Store> {
        let store = self.physical.as_ref()?;
        // A batch can retain unpublished bytes before its eventual disk flush.
        // An impossible directory (notably ENOTDIR) must remain a cache miss,
        // rather than serving those buffered writes as successful local hits.
        fs::create_dir_all(&self.root).ok()?;
        Some(store)
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Collect immutable outputs and action roots with bounded scratch.
    pub fn gc(&self, cap: u64) -> GcReport {
        let Some(physical) = &self.physical else {
            return GcReport::default();
        };
        let report = physical.collect(cap);
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
        let Some(store) = self.publication_store() else {
            return;
        };
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
    fn for_operation(&self) -> Option<std::sync::Arc<dyn ResultStore>> {
        let physical = self.physical.as_ref().map(|store| store.for_operation());
        Some(std::sync::Arc::new(Self {
            root: self.root.clone(),
            verifier: self.verifier.for_operation(physical.as_ref()),
            physical,
            read_only: self
                .read_only
                .iter()
                .map(|store| store.for_operation())
                .collect(),
            cap: self.cap,
        }))
    }
    fn read_scope(&self) -> Option<Box<dyn Send + Sync>> {
        if self.read_only.is_empty() {
            return None;
        }
        Some(Box::new(
            self.read_only
                .iter()
                .map(|store| store.batch())
                .collect::<Vec<_>>(),
        ))
    }
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        self.physical.clone()
    }
    fn lookup_product(&self, address: super::Address) -> Option<super::Association> {
        // A front QueryId shares its logical address with its variant row.
        // Keep the product index disjoint, as the byte providers do.
        let address = super::Address(super::product_association_key(address).0);
        self.roots().find_map(|(_, store)| store.lookup(address))
    }
    fn associate_product(
        &self,
        address: super::Address,
        action: super::Action,
        reference: super::Reference,
    ) {
        let Some(store) = self.publication_store() else {
            return;
        };
        // Shared immutable roots may supply ordinary content, but local writes
        // and GC remain in the writable root. A missing reference is a miss.
        if self.get_content(reference).is_some() {
            let address = super::Address(super::product_association_key(address).0);
            let _ = store.associate(address, action, reference);
        }
    }
    fn verifier(&self) -> Option<std::sync::Arc<hirsel_validate::VerifierCache>> {
        Some(self.verifier.clone())
    }
    fn get_content(&self, reference: super::Reference) -> Option<super::Bytes> {
        for (local, store) in self.roots() {
            if let Some(bytes) = store.get(reference) {
                if local {
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
        let reference = self.publication_store()?.put(domain, bytes).ok()?;
        Some(reference)
    }
    fn get(&self, key: &EntryKey) -> Option<super::Bytes> {
        for (local, store) in self.roots() {
            let Some(opened) = store.lookup_open(hirsel_artifact_store::Address(key.0)) else {
                if local
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
                if local {
                    let _ = fs::remove_file(store.output_path(previous.reference));
                }
                continue;
            };
            if local {
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
        if self.physical.is_some() {
            "dir@1"
        } else {
            "none@1"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Domain, ResultStore};

    #[test]
    fn query_variants_and_product_associations_survive_each_others_publication_and_reopen() {
        use crate::store::{Action, Address, Entry, VariantRef};
        for kind in [crate::QueryKind::Modes, crate::QueryKind::Lower] {
            let directory = tempfile::tempdir().unwrap();
            let store = DirStore::new(directory.path().to_owned());
            let key = EntryKey::new(
                Default::default(),
                &hirsel_syntax::DefPath::core("separate-query-and-product"),
                Default::default(),
                kind,
                crate::LoweringPolicy::Reachable,
            );
            let variant =
                VariantRef::publish(&store, &key, crate::Reads::empty(), b"body").unwrap();
            let mut entry = Entry::default();
            entry.insert(variant.clone());
            store.put(&key, &entry.encode_for(&key));
            let address = Address(key.0);
            let action = Action(hirsel_syntax::ContentHash::of_bytes(b"selected product"));
            store.associate_product(address, action, variant.output);
            drop(store);
            let reopened = DirStore::new(directory.path().to_owned());
            assert_eq!(Entry::load(&reopened, &key), entry);
            assert_eq!(
                reopened.lookup_product(address).unwrap().reference,
                variant.output
            );
            // Rewriting a variant row must preserve the selected association too.
            reopened.put(&key, &entry.encode_for(&key));
            let product = reopened.lookup_product(address).unwrap();
            assert_eq!(product.action, action);
            assert_eq!(product.reference, variant.output);
            assert_eq!(&*reopened.get_content(product.reference).unwrap(), b"body");
            let installed = DirStore::new(directory.path().join("local"))
                .with_read_only_root(directory.path().to_owned());
            assert_eq!(Entry::load(&installed, &key), entry);
            assert_eq!(installed.lookup_product(address), Some(product));
        }
    }

    #[test]
    fn installed_only_verifier_is_host_local_and_never_writes_query_results() {
        let directory = tempfile::tempdir().unwrap();
        let installed = directory.path().join("installed");
        let cache = directory.path().join("cache");
        fs::create_dir_all(&installed).unwrap();
        fs::create_dir_all(&cache).unwrap();
        let root = cache.join("hirsel/validator-v2");
        let verifier = DirStore::local_verifier(&cache, &installed).unwrap();
        let reader = DirStore {
            root: installed.clone(),
            physical: None,
            read_only: vec![hirsel_artifact_store::Store::new(installed.clone())],
            cap: 0,
            verifier: std::sync::Arc::new(verifier),
        };
        // Initialize the installed store's ordinary read lock before observing
        // query writes; lock creation is independent of query publication.
        assert!(
            reader
                .get(&EntryKey(
                    Default::default(),
                    Default::default(),
                    crate::QueryKind::Summary
                ))
                .is_none()
        );
        let installed_before: Vec<_> = fs::read_dir(&installed)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let key = EntryKey::new(
            Default::default(),
            &hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
                package: crate::default_context().package,
                modules: vec![],
                name: "host-local".into(),
            }),
            Default::default(),
            crate::QueryKind::Summary,
            crate::LoweringPolicy::Reachable,
        );
        reader.put(&key, b"must not persist");
        assert!(reader.get(&key).is_none());
        assert!(reader.local_product_store().is_none());
        assert!(
            reader
                .put_content(
                    Domain {
                        kind: 42,
                        schema: 1
                    },
                    b"no query content"
                )
                .is_none()
        );
        assert!(!root.exists(), "only V may publish in its namespace");
        let installed_after: Vec<_> = fs::read_dir(&installed)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(installed_after, installed_before);
        fs::create_dir_all(&root).unwrap();
        assert!(DirStore::local_verifier(&cache, &root).is_none());
        assert!(DirStore::local_verifier(&cache, &cache).is_none());
        fs::create_dir_all(root.join("program-inputs")).unwrap();
        assert!(DirStore::local_verifier(&cache, &root.join("program-inputs")).is_none());
        #[cfg(unix)]
        {
            let alias = directory.path().join("product-alias");
            std::os::unix::fs::symlink(&root, &alias).unwrap();
            assert!(DirStore::local_verifier(&cache, &alias).is_none());
        }
    }

    #[test]
    fn installed_read_scope_reuses_regions_refreshes_and_rejects_damage() {
        for forwarded in 0..3 {
            let directory = std::env::temp_dir().join(format!(
                "hirsel-installed-read-scope-{}-{:?}-{forwarded}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&directory);
            let installed = directory.join("installed");
            let local = directory.join("unused-local");
            let writer = hirsel_artifact_store::Store::new(installed.clone());
            let domain = Domain {
                kind: 0x4e0000fa,
                schema: 1,
            };
            let reference = writer.put(domain, b"installed bytes").unwrap();
            let reader = DirStore::new(local.clone()).with_read_only_root(installed);
            let statistics = reader.read_only[0].enable_statistics();
            let reader: std::sync::Arc<dyn ResultStore> = std::sync::Arc::new(reader);
            let budget = crate::runtime::ProcessBudget::new(4096);
            let reader: std::sync::Arc<dyn ResultStore> = if forwarded == 2 {
                std::sync::Arc::new(
                    crate::store::RequestStore::with_scratch(
                        reader,
                        budget.reserve(4096).unwrap(),
                        &directory,
                        4096,
                    )
                    .unwrap(),
                )
            } else if forwarded == 1 {
                std::sync::Arc::new(crate::store::RequestStore::new(
                    reader,
                    budget.reserve(4096).unwrap(),
                ))
            } else {
                reader
            };
            let scope = reader.read_scope();
            assert_eq!(&*reader.get_content(reference).unwrap(), b"installed bytes");
            let before = statistics.snapshot();
            for _ in 0..8 {
                assert_eq!(&*reader.get_content(reference).unwrap(), b"installed bytes");
            }
            let after = statistics.snapshot();
            for counter in ["IndexLoadAttempts", "IndexStamps", "SegmentOpens"] {
                assert_eq!(
                    after.counters[counter], before.counters[counter],
                    "{counter}"
                );
            }
            assert_eq!(after.counters["RegionCacheHits"], 8);
            let later = domain.reference(b"next operation").unwrap();
            assert!(reader.get_content(later).is_none());
            assert_eq!(writer.put(domain, b"next operation").unwrap(), later);
            assert!(
                reader.get_content(later).is_none(),
                "absence stays pinned within an operation"
            );
            drop(scope);
            let scope = reader.read_scope();
            assert_eq!(&*reader.get_content(later).unwrap(), b"next operation");
            drop(scope);
            fs::write(writer.output_path(reference), b"damaged installed pack").unwrap();
            let scope = reader.read_scope();
            assert!(
                reader.get_content(reference).is_none(),
                "new operation must reject damaged backing"
            );
            drop(scope);
            assert!(
                !local.exists(),
                "read scope creates no writable provider state"
            );
            drop(reader);
            assert_eq!(budget.reserved(), 0);
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn request_publication_scope_coalesces_cache_index_updates() {
        let directory = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(DirStore::new(directory.path().join("products")));
        let statistics = store.physical.as_ref().unwrap().enable_statistics();
        let budget = crate::runtime::ProcessBudget::new(64 << 10);
        let request = crate::store::RequestStore::with_scratch(
            store.clone(),
            budget.reserve(64 << 10).unwrap(),
            directory.path(),
            64 << 10,
        )
        .unwrap();
        let scope = request.publication_scope().unwrap();
        let before = statistics.snapshot();
        let domain = Domain {
            kind: 0x4e0000fb,
            schema: 1,
        };
        let references: Vec<_> = (0u64..16)
            .map(|n| request.put_content(domain, &n.to_le_bytes()).unwrap())
            .collect();
        for reference in &references {
            assert!(request.get_content(*reference).is_some());
        }
        let pending = statistics.snapshot();
        assert_eq!(
            pending.counters["IndexPublicationAttempts"],
            before.counters["IndexPublicationAttempts"]
        );
        drop(scope);
        let committed = statistics.snapshot();
        assert!(
            committed.counters["IndexPublicationAttempts"]
                > pending.counters["IndexPublicationAttempts"]
        );
        let reopened = DirStore::new(directory.path().join("products"));
        for (n, reference) in references.iter().enumerate() {
            assert_eq!(
                &*reopened.get_content(*reference).unwrap(),
                &(n as u64).to_le_bytes()
            );
        }
    }
}
