//! Declared core@1 signatures. This module reads only the normative declaration
//! ledger, never executable operation bodies (LANG-6). Package identity: #36.
mod build;
mod dictionaries;
mod heap;
mod loops;
pub mod review;
#[cfg(test)]
mod tests;
use hirsel_syntax::{ContentHash, DefPath, FileContext, Path};
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
        self.signatures.contains_key(&path).then_some(path)
    }
}
impl SignatureProvider for BuiltinRegistry {
    fn signature(&mut self, path: &DefPath) -> Result<SignatureRef, QueryError> {
        self.get(path).ok_or(QueryError::NotImplemented)
    }
    fn lookup(&mut self, _: &FileContext, path: &Path) -> LookupOutput {
        LookupOutput {
            manifest: self.revision,
            result: self.lookup_name(path).ok_or(QueryError::NotImplemented),
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
    let domains = match name {
        "int.widen" | "int.try_narrow" | "int.narrow" | "int.truncate" => {
            vec![
                (TypeParameterRef::bound(TypeParamId(0)), Integer),
                (TypeParameterRef::bound(TypeParamId(1)), Integer),
            ]
        }
        "int.wrapping_add" | "int.saturating_add" | "int.checked_add" | "int.wrapping_sub"
        | "int.saturating_sub" | "int.checked_sub" | "int.wrapping_mul" | "int.checked_mul"
        | "int.leading_zeros" | "int.trailing_zeros" | "int.count_ones" | "int.swap_bytes"
        | "int.reverse_bits" | "int.div_floor" | "int.mod" | "int.div_exact"
        | "int.unchecked_add" | "simd.add" => {
            vec![(TypeParameterRef::bound(TypeParamId(0)), Integer)]
        }
        "float.from_int" => vec![
            (TypeParameterRef::bound(TypeParamId(0)), Integer),
            (TypeParameterRef::bound(TypeParamId(1)), Float),
        ],
        "float.min" | "float.max" | "float.min_number" | "float.max_number" | "float.mul_add"
        | "float.sqrt" | "float.floor" | "float.ceil" | "float.trunc" | "float.round"
        | "float.abs" | "float.to_i32" => {
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
