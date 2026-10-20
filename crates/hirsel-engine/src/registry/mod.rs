//! Declared core@1 signatures. This module reads only the normative declaration
//! ledger, never executable operation bodies (LANG-6). Package identity: #36.
mod build;
mod dictionaries;
mod heap;
mod loops;
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
pub fn nominal_roles() -> NominalRoles {
    NominalRoles {
        unsafe_capability: Some(hirsel_syntax::DefPath::core("Unsafe")),
        result_type: Some(hirsel_syntax::DefPath::core("Result")),
        tasks_capability: Some(hirsel_syntax::DefPath::core("Tasks")),
        scope_capability: Some(hirsel_syntax::DefPath::core("Scope")),
        heap: Some(HeapRole {
            arena: hirsel_syntax::DefPath::core("Arena"),
            operation: hirsel_syntax::DefPath::core("$intrinsic.heap"),
        }),
    }
}
#[derive(Clone)]
pub struct BuiltinRegistry {
    /// The interner the signatures' rows were built into.
    types: Arc<TypeInterner>,
    signatures: Arc<BTreeMap<DefId, SignatureRef>>,
    revision: ContentHash,
}
/// The process's core rows: the registry built once into a scratch interner,
/// frozen as an interner base at the same positions, so its signatures name
/// rows of every interner built over [`base`].
struct Shared {
    base: Arc<TypeBase>,
    registry: BuiltinRegistry,
    variants: std::collections::BTreeSet<String>,
}
fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| {
        let scratch = TypeInterner::new(None);
        let registry = BuiltinRegistry::build(&scratch);
        let mut variants = std::collections::BTreeSet::new();
        let base = TypeBase::build(scratch.len(), |ids| {
            ids.iter()
                .map(|id| {
                    let row = scratch.row(*id);
                    if let hirsel_types::TypeKind::Variant(cases) = &row.kind {
                        variants.extend(cases.iter().map(|v| v.name.as_str().to_string()));
                    }
                    row.clone()
                })
                .collect()
        });
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
        let signatures = build::registry(types);
        let revision = ContentHash::of_parts(
            "hirsel.core.registry.v9",
            signatures.values().map(|s| s.fingerprint.0.as_slice()),
        );
        Self {
            types: types.clone(),
            signatures: Arc::new(signatures),
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
        self.signatures.get(&definition.into()).cloned()
    }
    pub fn signatures(&self) -> impl Iterator<Item = &SignatureRef> {
        self.signatures.values()
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
fn publish(
    t: &TypeInterner,
    mut signature: Signature,
    signatures: &mut BTreeMap<DefId, SignatureRef>,
) {
    signature.fingerprint = hirsel_types::encoding::fingerprint(t, &signature, &[])
        .expect("well-formed core signature");
    assert!(
        signatures
            .insert(signature.definition, Arc::new(signature))
            .is_none(),
        "duplicate core declaration"
    );
}

/// This is the only ledger of intrinsic scalar constraints. Every domain is
/// checked through the signature on calls and type annotations.
fn schema(name: &str) -> Vec<SchemaConstraint> {
    use ScalarDomain::*;
    use SchemaConstraint::*;
    let domains = match name {
        "int.widen" | "int.try_narrow" | "int.narrow" | "int.truncate" => {
            vec![
                (TypeParameterRef::bound(TypeParamId(0)), Integer),
                (TypeParameterRef::bound(TypeParamId(1)), Integer),
            ]
        }
        "int.wrapping_add" | "int.saturating_add" | "int.checked_add" | "int.div_floor"
        | "int.mod" | "int.div_exact" | "int.unchecked_add" | "simd.add" => {
            vec![(TypeParameterRef::bound(TypeParamId(0)), Integer)]
        }
        "float.from_int" => vec![
            (TypeParameterRef::bound(TypeParamId(0)), Integer),
            (TypeParameterRef::bound(TypeParamId(1)), Float),
        ],
        "float.min" | "float.max" | "float.min_number" | "float.max_number" | "float.mul_add"
        | "float.total_order" | "float.to_i32" => {
            vec![(TypeParameterRef::bound(TypeParamId(0)), Float)]
        }
        "atomic.create" | "atomic.load" | "atomic.store" | "Atomic" | "simd.splat" | "Simd" => {
            vec![(TypeParameterRef::bound(TypeParamId(0)), Scalar)]
        }
        _ => return vec![],
    };
    let mut constraints = vec![];
    for (parameter, domain) in domains {
        constraints.push(Domain {
            parameter,
            domain,
            rule: match domain {
                Integer => const { hirsel_diag::RuleId::of("SEM-1") },
                Float if name == "float.from_int" => const { hirsel_diag::RuleId::of("SEM-1") },
                Float => const { hirsel_diag::RuleId::of("SEM-2") },
                Scalar if name.starts_with("simd.") || name == "Simd" => {
                    const { hirsel_diag::RuleId::of("MEM-8") }
                }
                Scalar => const { hirsel_diag::RuleId::of("CONC-7") },
            },
        });
        constraints.push(NoallocDrop(parameter));
    }
    match name {
        "int.widen" => constraints.push(CompleteRangeFits {
            source: TypeParameterRef::bound(TypeParamId(0)),
            destination: TypeParameterRef::bound(TypeParamId(1)),
        }),
        "int.truncate" => constraints.push(DestinationNoWider {
            source: TypeParameterRef::bound(TypeParamId(0)),
            destination: TypeParameterRef::bound(TypeParamId(1)),
        }),
        "simd.splat" | "simd.add" | "Simd" => {
            constraints.push(PositiveSize(SizeParameterRef::bound(SizeParamId(1))))
        }
        _ => (),
    }
    constraints
}
