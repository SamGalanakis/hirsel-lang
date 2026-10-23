//! Declared core@1 signatures. This module reads only the normative declaration
//! ledger, never executable operation bodies (LANG-6). Package identity: #36.
mod declarations;
mod products;
pub use declarations::nominal_roles;
mod dictionaries;
pub mod review;
#[cfg(test)]
mod tests;
use hirsel_syntax::{ContentHash, DefId, DefPath, FileContext, Path};
use hirsel_types::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

pub const SECTION22: &str = include_str!("section22.txt");
/// Deliberately absent; no guessed APIs for the still-open questions.
pub const OMITTED: &[(&str, u32)] = &[
    ("package discovery and manifest APIs", 4),
    ("raw-pointer APIs beyond PROG-4 externs and callbacks", 6),
    ("comptime step-budget API", 7),
    ("frame/arena/scratch charge and depth constants", 16),
];
#[derive(Clone)]
pub struct BuiltinRegistry {
    /// The interner the signatures' rows were built into.
    types: Arc<TypeInterner>,
    signatures: Arc<BTreeMap<DefId, OnceLock<SignatureRef>>>,
    records: Option<Arc<hirsel_product_codec::TypeRecords>>,
    references: Arc<BTreeMap<DefId, hirsel_artifact_store::Reference>>,
    revision: ContentHash,
}
/// Core declarations are ordinary products over a lazily loaded shared base.
struct Shared {
    base: Arc<TypeBase>,
    registry: BuiltinRegistry,
    variants: std::collections::BTreeSet<String>,
}
fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| {
        use bincode::Options;
        let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/core-products.index"));
        let index: products::CoreProducts = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(bytes.len() as u64)
            .reject_trailing_bytes()
            .deserialize(bytes)
            .expect("build-time core product index");
        let store = Arc::new(include!(concat!(env!("OUT_DIR"), "/core-products.rs")));
        let shared = hirsel_product_codec::SharedTypes::open(index.base, store)
            .expect("core type reference index");
        let base = shared.base.clone();
        let types = TypeInterner::new(Some(base.clone()));
        let records = shared.records(types.clone()).expect("core base records");
        let references: BTreeMap<_, _> = index.signatures.into_iter().collect();
        let signatures = references.keys().map(|&d| (d, OnceLock::new())).collect();
        let registry = BuiltinRegistry {
            types,
            signatures: Arc::new(signatures),
            revision: index.revision,
            records: Some(records),
            references: Arc::new(references),
        };
        Shared {
            base,
            registry,
            variants: index.variants,
        }
    })
}
/// Every variant name a core type declares: the resolver defers these names
/// to expected context.
pub fn variant_names() -> &'static std::collections::BTreeSet<String> {
    &shared().variants
}
/// The interner base holding the core rows [`BuiltinRegistry::shared`] names.
pub fn base() -> Arc<TypeBase> {
    shared().base.clone()
}
/// The interner holding [`BuiltinRegistry::shared`]'s rows.
pub fn shared_types() -> &'static Arc<TypeInterner> {
    &shared().registry.types
}
/// A session interner over the core rows.
pub fn session_types() -> Arc<TypeInterner> {
    TypeInterner::new(Some(base()))
}
impl Default for BuiltinRegistry {
    fn default() -> Self {
        Self::shared()
    }
}
impl BuiltinRegistry {
    /// The process-shared core signatures ([`BuiltinRegistry::shared`]).
    pub fn new() -> Self {
        Self::shared()
    }
    /// The core signatures, built into `types`.
    pub fn build(types: &Arc<TypeInterner>) -> Self {
        let signatures = declarations::build::registry(types);
        let revision = ContentHash::of_parts(
            "hirsel.core.registry.v9",
            signatures.values().map(|s| s.fingerprint.0.as_slice()),
        );
        Self {
            types: types.clone(),
            signatures: Arc::new(
                signatures
                    .into_iter()
                    .map(|(d, s)| (d, OnceLock::from(s)))
                    .collect(),
            ),
            records: None,
            references: Arc::new(BTreeMap::new()),
            revision,
        }
    }
    /// The core signatures over [`base`]: valid in every interner built over it.
    pub fn shared() -> Self {
        shared().registry.clone()
    }
    /// The interner holding the signatures' rows; every interner over
    /// [`base`] holds the shared registry's rows at the same ids.
    pub fn types(&self) -> &Arc<TypeInterner> {
        &self.types
    }
    pub fn get(&self, definition: impl Into<DefId>) -> Option<SignatureRef> {
        let d = definition.into();
        self.signatures
            .get(&d)
            .map(|slot| self.decode(d, slot).clone())
    }
    pub fn signatures(&self) -> impl Iterator<Item = &SignatureRef> {
        self.signatures
            .iter()
            .map(|(&d, slot)| self.decode(d, slot))
    }
    fn decode<'a>(&self, d: DefId, slot: &'a OnceLock<SignatureRef>) -> &'a SignatureRef {
        slot.get_or_init(|| {
            let records = self.records.as_ref().expect("lazy core records");
            let reference = *self.references.get(&d).expect("core product reference");
            let bytes = records
                .store
                .get_content(reference)
                .expect("checked core product");
            Arc::new(
                records
                    .decode_record(&bytes, |_, _| None)
                    .expect("core signature record"),
            )
        })
    }
    pub fn revision(&self) -> ContentHash {
        self.revision
    }
    /// Provider fallback only: the caller resolves locals/own-package definitions
    /// first. Explicitly versioned paths never fall back to another package.
    pub fn lookup_name(&self, path: &Path) -> Option<DefPath> {
        if path
            .package
            .as_ref()
            .is_some_and(|p| !p.package_is("core", "1"))
        {
            return None;
        }
        let name = path
            .segments
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(".");
        let path = hirsel_syntax::DefPath::core(&name);
        if path.0.modules.iter().any(|m| m.starts_with('$')) {
            return None;
        }
        self.signatures
            .contains_key(&DefId::intern(&path))
            .then_some(path)
    }
}
impl SignatureProvider for BuiltinRegistry {
    fn signature(&mut self, definition: DefId) -> Result<SignatureRef, QueryError> {
        self.get(definition).ok_or(QueryError::NotImplemented)
    }
    fn lookup(&mut self, _: &FileContext, path: &Path) -> LookupOutput {
        LookupOutput {
            manifest: self.revision,
            result: self.lookup_name(path).ok_or(QueryError::NotImplemented),
        }
    }
}
