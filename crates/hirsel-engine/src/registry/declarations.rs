//! The declaration builder, shared by the compiler build and explicit registry builds.
use hirsel_syntax::{ContentHash, DefId};
use hirsel_types::*;
use std::{collections::BTreeMap, sync::Arc};
#[path = "build.rs"]
pub(super) mod build;
#[path = "dictionary_declarations.rs"]
pub(super) mod dictionaries;
#[path = "heap.rs"]
mod heap;
#[path = "loops.rs"]
mod loops;
#[path = "text.rs"]
mod text;
pub const SECTION22: &str = include_str!("section22.txt");
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
pub(super) fn schema(name: &str) -> Vec<SchemaConstraint> {
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
