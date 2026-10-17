//! Declared core@1 signatures. This module reads only the normative declaration
//! ledger, never executable operation bodies (LANG-6). Package identity: #36.
mod build;
mod dictionaries;
mod heap;
mod loops;
pub mod review;
#[cfg(test)]
mod tests;
use hirsel_intern::Interner;
use hirsel_syntax::{ContentHash, DefPath, FileContext, ItemPath, PackageId, Path};
use hirsel_types::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

pub const SECTION22: &str = include_str!("section22.txt");
/// Deliberately absent; no guessed APIs for the still-open questions.
pub const OMITTED: &[(&str, u32)] = &[
    ("package discovery and manifest APIs", 4),
    ("FFI wrappers, callbacks and raw-pointer APIs", 6),
    ("comptime step-budget API", 7),
    ("frame/arena/scratch charge and depth constants", 16),
];
/// Stable identity agreed with the milestone owner; see spec issue #36.
pub fn core_path(name: &str) -> DefPath {
    let mut parts: Vec<_> = name.split('.').map(str::to_owned).collect();
    let name = parts.pop().expect("core name");
    DefPath(ItemPath {
        package: PackageId {
            name: "core".into(),
            version: "1".into(),
        },
        modules: parts,
        name,
    })
}
pub fn nominal_roles() -> NominalRoles {
    NominalRoles {
        unsafe_capability: Some(core_path("Unsafe")),
        result_type: Some(core_path("Result")),
        tasks_capability: Some(core_path("Tasks")),
        scope_capability: Some(core_path("Scope")),
        heap: Some(HeapRole {
            arena: core_path("Arena"),
            operation: core_path("$intrinsic.heap"),
        }),
    }
}
#[derive(Clone)]
pub struct BuiltinRegistry {
    signatures: Arc<BTreeMap<DefPath, SignatureRef>>,
    revision: ContentHash,
}
impl Default for BuiltinRegistry {
    fn default() -> Self {
        Self::new()
    }
}
impl BuiltinRegistry {
    pub fn new() -> Self {
        static REGISTRY: OnceLock<BuiltinRegistry> = OnceLock::new();
        REGISTRY
            .get_or_init(|| {
                let signatures = build::registry();
                let revision = ContentHash::of_parts(
                    "hirsel.core.registry.v9",
                    signatures.values().map(|s| s.fingerprint.0.as_slice()),
                );
                Self {
                    signatures: Arc::new(signatures),
                    revision,
                }
            })
            .clone()
    }
    pub fn get(&self, path: &DefPath) -> Option<SignatureRef> {
        self.signatures.get(path).cloned()
    }
    pub fn signatures(&self) -> impl Iterator<Item = &SignatureRef> {
        self.signatures.values()
    }
    pub fn revision(&self) -> ContentHash {
        self.revision
    }
    /// Provider fallback only: the caller resolves locals/own-package definitions
    /// first. Explicitly versioned paths never fall back to another package.
    pub fn lookup_name(&self, path: &Path, symbols: &Interner) -> Option<DefPath> {
        if path
            .package
            .as_ref()
            .is_some_and(|p| p.name != "core" || p.version != "1")
        {
            return None;
        }
        let name = path
            .segments
            .iter()
            .map(|s| symbols.resolve(*s))
            .collect::<Option<Vec<_>>>()?
            .join(".");
        let path = core_path(&name);
        if path.0.modules.iter().any(|m| m.starts_with('$')) {
            return None;
        }
        self.signatures.contains_key(&path).then_some(path)
    }
}
impl SignatureProvider for BuiltinRegistry {
    fn signature(&mut self, path: &DefPath) -> Result<SignatureRef, QueryError> {
        self.get(path).ok_or(QueryError::NotImplemented)
    }
    fn lookup(&mut self, _: &FileContext, path: &Path, symbols: &Interner) -> LookupOutput {
        LookupOutput {
            manifest: self.revision,
            result: self
                .lookup_name(path, symbols)
                .ok_or(QueryError::NotImplemented),
        }
    }
}
fn publish(mut signature: Signature, signatures: &mut BTreeMap<DefPath, SignatureRef>) {
    signature.fingerprint =
        hirsel_types::encoding::fingerprint(&signature, &[]).expect("well-formed core signature");
    assert!(
        signatures
            .insert(signature.definition.clone(), Arc::new(signature))
            .is_none(),
        "duplicate core declaration"
    );
}

/// This is the only ledger of intrinsic scalar constraints. Every domain is
/// checked through the signature on calls and type annotations.
fn schema(name: &str) -> Vec<SchemaConstraint> {
    use ScalarDomain::*;
    use SchemaConstraint::*;
    // #48: the published borrowed key cannot be retained when noncopyable.
    // Keep the section-22 ledger verbatim until take-k versus copyable is ruled.
    if name == "sorted_map.insert" {
        return vec![Bound {
            parameter: TypeParameterRef::bound(TypeParamId(0)),
            bound: hirsel_types::Bound::Copyable,
            rule: const { hirsel_diag::RuleId::of("GEN-1") },
        }];
    }
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
