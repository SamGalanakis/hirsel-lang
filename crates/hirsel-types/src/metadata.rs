use crate::*;
use hirsel_span::Span;
use hirsel_syntax as ast;
use std::collections::BTreeMap;

/// Resolver/manifest facts supplied by manifest indexing.
/// Missing visibility is conservative: a foreign type is not presumed public.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct DefinitionMetadata {
    pub public: bool,
    /// Transparent aliases expand with remapped type/size arguments.
    pub alias: bool,
}
/// One top-level name in source order across the manifest module, including duplicates.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct DeclarationSite {
    pub definition: ast::DefPath,
    pub name_span: Span,
}
/// A package's `unsafe ffi` manifest metadata (PROG-4).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnsafeFfi {
    /// The package's declared safe wrappers.
    pub wrappers: BTreeMap<ast::DefPath, ForeignWrapper>,
    /// Where its externs run (S-13).
    pub threads: ForeignThreads,
}
/// Metadata comes from the manifest and written foreign contract, never a body.
#[derive(Clone, Debug, Default)]
pub struct Metadata {
    pub definitions: BTreeMap<ast::DefPath, DefinitionMetadata>,
    /// All module declarations in source order. Empty uses the tree item order.
    pub declarations: Vec<DeclarationSite>,
    /// The selected packages marked `unsafe ffi`.
    pub unsafe_ffi: BTreeMap<ast::PackageId, UnsafeFfi>,
    /// Canonical library identities; matching a spelling is insufficient.
    pub unsafe_capability: Option<ast::DefPath>,
    pub result_type: Option<ast::DefPath>,
    pub tasks_capability: Option<ast::DefPath>,
    pub scope_capability: Option<ast::DefPath>,
    pub heap: Option<HeapRole>,
    pub nominal_role_cache: std::sync::OnceLock<std::sync::Arc<NominalRoles>>,
}
impl Metadata {
    /// Share one immutable registry-role selection across this metadata's headers.
    /// Public input fields remain authoritative if a caller edits them afterward.
    pub fn nominal_roles(&self) -> std::sync::Arc<NominalRoles> {
        let make = || NominalRoles {
            unsafe_capability: self.unsafe_capability.clone(),
            result_type: self.result_type.clone(),
            tasks_capability: self.tasks_capability.clone(),
            scope_capability: self.scope_capability.clone(),
            heap: self.heap.clone(),
        };
        let roles = self
            .nominal_role_cache
            .get_or_init(|| std::sync::Arc::new(make()));
        if roles.unsafe_capability == self.unsafe_capability
            && roles.result_type == self.result_type
            && roles.tasks_capability == self.tasks_capability
            && roles.scope_capability == self.scope_capability
            && roles.heap == self.heap
        {
            roles.clone()
        } else {
            std::sync::Arc::new(make())
        }
    }
}
/// Persistent semantic declaration metadata. Locations/build manifest maps stay outside.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct SignatureMetadata {
    /// Private body facts, never part of FunctionType or indirect-call evidence.
    pub inferred_unique: UniqueSummary,
    pub visibility: ast::Visibility,
    pub alias: bool,
    /// Explicit GEN-6 constant alias edge; consumers normalize terminal identity.
    pub constant_alias: Option<Box<ast::DefPath>>,
    /// Caller attribution is a declaration property, not part of a function value type.
    pub track_caller: bool,
    pub unsafe_ffi: bool,
    /// The trusted safe-wrapper declaration of the package manifest (PROG-4).
    pub wrapper: Option<ForeignWrapper>,
    pub roles: std::sync::Arc<NominalRoles>,
    /// Intrinsic declaration constraints, supplied by the declaring registry.
    /// Ordinary declarations have no schema constraints.
    pub schema: Vec<SchemaConstraint>,
    /// A member of a recursive type group (types naming each other through
    /// their bodies): its `hirsel_graph::scc_keys` key over the members' text,
    /// standing for the members it does not list as dependencies.
    pub recursive_group: Option<ast::ContentHash>,
    /// A function's implementation binders in declaration order (GEN-6). Each
    /// binder's type and size members are generic parameters of the function,
    /// supplied by the implementation a `with` clause names, never inferred.
    pub implementations: Vec<ImplementationBinder>,
}
/// `H: implementation Sig` (GEN-6): the binder's name, its terminal family and
/// the generic ordinal each type or size member denotes. A shared member
/// (`where H.K = A.K`) denotes the earlier binder's ordinal.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct ImplementationBinder {
    pub name: String,
    pub family: DefId,
    pub members: Vec<(String, u32)>,
}
impl Default for SignatureMetadata {
    fn default() -> Self {
        Self {
            inferred_unique: UniqueSummary::default(),
            visibility: ast::Visibility::Private,
            alias: false,
            constant_alias: None,
            track_caller: false,
            unsafe_ffi: false,
            wrapper: None,
            roles: std::sync::Arc::default(),
            schema: vec![],
            recursive_group: None,
            implementations: vec![],
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScalarDomain {
    Integer,
    Float,
    Scalar,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum SchemaConstraint {
    Bound {
        parameter: TypeParameterRef,
        bound: Bound,
        rule: hirsel_diag::RuleId,
    },
    Domain {
        parameter: TypeParameterRef,
        domain: ScalarDomain,
        rule: hirsel_diag::RuleId,
    },
    CompleteRangeFits {
        source: TypeParameterRef,
        destination: TypeParameterRef,
    },
    DestinationNoWider {
        source: TypeParameterRef,
        destination: TypeParameterRef,
    },
    PositiveSize(SizeParameterRef),
    NoallocDrop(TypeParameterRef),
}
/// Registry-selected identities; same-spelled user declarations do not satisfy these roles.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct NominalRoles {
    pub unsafe_capability: Option<ast::DefPath>,
    pub result_type: Option<ast::DefPath>,
    pub tasks_capability: Option<ast::DefPath>,
    pub scope_capability: Option<ast::DefPath>,
    pub heap: Option<HeapRole>,
}

/// Canonical Arena identity and the compiler-only operation producing the ambient heap.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct HeapRole {
    pub arena: ast::DefPath,
    pub operation: ast::DefPath,
}
impl Type {
    pub fn is_heap_target(&self, roles: &NominalRoles) -> bool {
        self.allocation_target == Some(AllocationTarget::Heap)
            && matches!(&self.kind, TypeKind::Named {definition, arguments}
                if arguments.is_empty() && roles.heap.as_ref().is_some_and(|h| &h.arena == definition.path()))
    }
}

/// Replay a declaration's intrinsic constraints against instantiated arguments.
/// Diagnostic identity comes from the declaration, never a library path lookup.
pub fn schema_failure(
    store: &TypeInterner,
    arguments: &[GenericArgument],
    constraints: &[SchemaConstraint],
    mut satisfies: impl FnMut(TypeId, Bound) -> bool,
) -> Option<(hirsel_diag::RuleId, &'static str)> {
    fn integer(kind: &TypeKind) -> Option<(u32, bool)> {
        use Primitive::*;
        match kind {
            TypeKind::Primitive(p) => Some(match p {
                U8 => (8, false),
                U16 => (16, false),
                U32 => (32, false),
                U64 => (64, false),
                U128 => (128, false),
                I8 => (8, true),
                I16 => (16, true),
                I32 => (32, true),
                I64 => (64, true),
                I128 => (128, true),
                _ => return None,
            }),
            _ => None,
        }
    }
    let ty = |p: TypeParameterRef| match (p.binder() == GenericBinder::Bound(0))
        .then(|| arguments.get(p.parameter().0 as usize))
        .flatten()
    {
        Some(GenericArgument::Type(t)) => store.get(*t).map(|t| &t.kind),
        _ => None,
    };
    let integer_shape = |p| ty(p).and_then(integer);
    let mut failures = vec![];
    for constraint in constraints {
        let failure = match constraint {
            SchemaConstraint::Bound {
                parameter,
                bound,
                rule,
            } => {
                match (parameter.binder() == GenericBinder::Bound(0))
                    .then(|| arguments.get(parameter.parameter().0 as usize))
                    .flatten()
                {
                    Some(GenericArgument::Type(ty)) if !satisfies(*ty, *bound) => Some((
                        rule.clone(),
                        "intrinsic type argument does not satisfy its required bound",
                    )),
                    _ => None,
                }
            }
            SchemaConstraint::Domain {
                parameter,
                domain,
                rule,
            } => {
                let valid = match domain {
                    ScalarDomain::Integer => integer_shape(*parameter).is_some(),
                    ScalarDomain::Float => ty(*parameter).is_none_or(|t| {
                        matches!(t, TypeKind::Primitive(Primitive::F32 | Primitive::F64))
                    }),
                    ScalarDomain::Scalar => ty(*parameter).is_none_or(|t| {
                        integer(t).is_some()
                            || matches!(
                                t,
                                TypeKind::Primitive(
                                    Primitive::F32 | Primitive::F64 | Primitive::Bool
                                )
                            )
                    }),
                };
                let message = match domain {
                    ScalarDomain::Integer => {
                        "integer operation requires valid sized integer arguments and conversion widths"
                    }
                    ScalarDomain::Float => "float operation requires f32 or f64",
                    ScalarDomain::Scalar => "operation requires an integer, float or bool scalar",
                };
                (!valid).then(|| (rule.clone(), message))
            }
            SchemaConstraint::CompleteRangeFits {
                source,
                destination,
            } => {
                let valid = match (integer_shape(*source), integer_shape(*destination)) {
                    (Some((from, signed)), Some((to, dest_signed))) => {
                        match (signed, dest_signed) {
                            (true, false) => false,
                            (false, true) => to > from,
                            _ => to >= from,
                        }
                    }
                    _ => true,
                };
                (!valid).then(|| (const { hirsel_diag::RuleId::of("SEM-1") },"integer operation requires valid sized integer arguments and conversion widths"))
            }
            SchemaConstraint::DestinationNoWider {
                source,
                destination,
            } => {
                let valid = match (integer_shape(*source), integer_shape(*destination)) {
                    (Some((from, _)), Some((to, _))) => to <= from,
                    _ => true,
                };
                (!valid).then(|| (const { hirsel_diag::RuleId::of("SEM-1") },"integer operation requires valid sized integer arguments and conversion widths"))
            }
            SchemaConstraint::PositiveSize(parameter) => matches!(
                (parameter.binder() == GenericBinder::Bound(0))
                    .then(|| arguments.get(parameter.parameter().0 as usize))
                    .flatten(),
                Some(GenericArgument::Size(Size::Constant(0)))
            )
            .then(|| {
                (
                    const { hirsel_diag::RuleId::of("MEM-8") },
                    "SIMD lane count must be positive",
                )
            }),
            _ => None,
        };
        if let Some(failure) = failure {
            failures.push(failure);
        }
    }
    failures.into_iter().min_by(|(a, _), (b, _)| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nominal_role_selection_shares_equal_headers_and_observes_metadata_edits() {
        let mut metadata = Metadata {
            result_type: Some(ast::DefPath::core("Result")),
            ..Default::default()
        };
        let first = metadata.nominal_roles();
        assert!(std::sync::Arc::ptr_eq(&first, &metadata.nominal_roles()));
        let mut changed = metadata.clone();
        changed.result_type = Some(ast::DefPath::core("Other"));
        assert_ne!(first, changed.nominal_roles());
        assert_eq!(first.result_type, metadata.result_type);
        metadata.result_type = None;
        assert_eq!(metadata.nominal_roles().result_type, None);
        assert_eq!(first.result_type, Some(ast::DefPath::core("Result")));
    }
}
