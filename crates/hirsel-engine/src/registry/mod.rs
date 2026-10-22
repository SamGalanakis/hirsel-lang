//! Declared core@1 signatures. This module reads only the normative declaration
//! ledger, never executable operation bodies (LANG-6). Package identity: #36.
mod declarations;
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
    image: Option<hirsel_image::Image>,
    revision: ContentHash,
}
/// The compiler build freezes the core rows and declarations in an indexed
/// image. Its type rows form the base; signatures decode on their first lookup.
struct Shared {
    base: Arc<TypeBase>,
    registry: BuiltinRegistry,
    variants: std::collections::BTreeSet<String>,
}
fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| {
        let image = hirsel_image::Image::from_static(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/core.image"
        )))
        .expect("build-time core image");
        let base = image.base();
        let variants = image
            .blob(hirsel_image::FIRST_KIND + 1)
            .expect("core variant names")
            .expect("core variant section");
        let signatures = image
            .keys(hirsel_image::FIRST_KIND)
            .expect("core definitions")
            .into_iter()
            .map(|d| (d, OnceLock::new()))
            .collect();
        let registry = BuiltinRegistry {
            types: TypeInterner::new(Some(base.clone())),
            signatures: Arc::new(signatures),
            revision: image.key(),
            image: Some(image),
        };
        Shared {
            base,
            registry,
            variants,
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
            image: None,
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
            Arc::new(
                self.image
                    .as_ref()
                    .expect("lazy registry image")
                    .entry(hirsel_image::FIRST_KIND, d)
                    .expect("core signature")
                    .expect("core entry"),
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
