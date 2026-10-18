//! Pure graph transport. Signature lookup and rule diagnostics remain phase-owned.
use crate::*;
use hirsel_intern::work::{self, Counter};
use std::collections::{BTreeMap, BTreeSet};

/// Role of an edge in the complete structural type graph. Callable edges cross
/// one binder for value, type and size parameters, even on nongeneric callables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeEdgeRole {
    Component,
    TypeArgument,
    FunctionParameter,
    FunctionResult,
    FunctionError,
}
impl TypeEdgeRole {
    pub fn parameter_binders(self) -> u32 {
        match self {
            Self::Component | Self::TypeArgument => 0,
            Self::FunctionParameter | Self::FunctionResult | Self::FunctionError => 1,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypeGraphEdge {
    pub ty: TypeId,
    pub role: TypeEdgeRole,
}
impl TypeKind {
    /// Structural value parts in declaration order, excluding callable headers.
    /// Nominal/builtin type arguments are included even when phantom; consumers
    /// retain their own owned-payload, declaration and cycle policies.
    pub fn value_parts(&self) -> Vec<TypeId> {
        self.graph_edges()
            .into_iter()
            .filter(|e| e.role.parameter_binders() == 0)
            .map(|e| e.ty)
            .collect()
    }
    /// SEM-5 entry layout in declaration order: read-only key, then value.
    /// Consumers retain their own projection, use-mode and bound policies.
    pub fn map_entry_fields(&self) -> Option<[TypeId; 2]> {
        match self {
            Self::Builtin {
                kind: BuiltinType::Map | BuiltinType::SortedMap,
                arguments,
            } => match arguments.as_slice() {
                [GenericArgument::Type(key), GenericArgument::Type(value)] => Some([*key, *value]),
                _ => None,
            },
            _ => None,
        }
    }
    /// Complete graph view, in declaration order, including all callable edges.
    /// This does not describe owned payloads or separately executed closure bodies.
    /// Callers retain their own cycle, provenance and owned-payload traversal policies.
    pub fn graph_edges(&self) -> Vec<TypeGraphEdge> {
        use TypeEdgeRole as R;
        let edge = |ty, role| TypeGraphEdge { ty, role };
        match self {
            Self::Tuple(ts) => ts.iter().map(|t| edge(*t, R::Component)).collect(),
            Self::Box(t) => vec![edge(*t, R::Component)],
            Self::Record(fs) => fs.iter().map(|f| edge(f.ty, R::Component)).collect(),
            Self::Variant(vs) => vs
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| edge(f.1, R::Component)))
                .collect(),
            Self::Named { arguments, .. } | Self::Builtin { arguments, .. } => arguments
                .iter()
                .filter_map(|a| match a {
                    GenericArgument::Type(t) => Some(edge(*t, R::TypeArgument)),
                    GenericArgument::Size(_) => None,
                })
                .collect(),
            Self::Function(f) => f
                .parameters
                .iter()
                .map(|p| edge(p.ty, R::FunctionParameter))
                .chain([edge(f.result, R::FunctionResult)])
                .chain(f.error.map(|t| edge(t, R::FunctionError)))
                .collect(),
            Self::Primitive(_) | Self::Parameter(_) | Self::Error => vec![],
        }
    }
}
/// CT-3: a compile-time-only value type: a `TypeShape` or a core `Body`.
pub fn compile_time_only(kind: &TypeKind) -> bool {
    match kind {
        TypeKind::Builtin { kind, .. } => *kind == BuiltinType::TypeShape,
        TypeKind::Named { definition, .. } => definition.is_core("Body"),
        _ => false,
    }
}
/// Whether `ty` reaches a compile-time-only type through its complete graph,
/// type arguments and callable headers included; nominal representations are
/// their declarations' own.
pub fn mentions_compile_time_only(store: &TypeStore, ty: TypeId) -> bool {
    let mut seen = BTreeSet::new();
    let mut pending = vec![ty];
    while let Some(ty) = pending.pop() {
        if !seen.insert(ty) {
            continue;
        }
        let Some(row) = store.get(ty) else { continue };
        if compile_time_only(&row.kind) {
            return true;
        }
        pending.extend(row.kind.graph_edges().into_iter().map(|e| e.ty));
    }
    false
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphError {
    UnboundGeneric(GenericBinder),
    UnboundOrigin(OriginRoot),
    UnresolvedPlacement(TypeId),
    UnboundParameter(ParameterRef),
    MissingType(TypeId),
    StructuralCycle(TypeId),
    RecoveryType(TypeId),
    ArgumentKind(u32),
    MissingArgument(u32),
    MissingAuthority(u32),
}
/// Arguments belong to the destination store and replace the root declaration's
/// references. Nested callables retain their own references; enclosing references
/// cross the removed binder with relative depths adjusted.
/// Authority entries are required only when a replaced parameter occurs in an
/// authority summary; the caller obtains those summaries from declared signatures.
#[derive(Clone, Copy)]
pub struct Substitution<'a> {
    pub binder: Option<GenericBinder>,
    pub arguments: &'a [GenericArgument],
    pub authorities: &'a [AuthoritySummary],
    /// Nominal Result identity used to normalize derived callback error channels.
    pub roles: &'a NominalRoles,
}
impl Default for Substitution<'_> {
    fn default() -> Self {
        static ROLES: NominalRoles = NominalRoles {
            unsafe_capability: None,
            result_type: None,
            tasks_capability: None,
            scope_capability: None,
            heap: None,
        };
        Self {
            binder: None,
            arguments: &[],
            authorities: &[],
            roles: &ROLES,
        }
    }
}
/// The derived error channel is selected by nominal role, never by spelling.
pub fn result_error(store: &TypeStore, result: TypeId, roles: &NominalRoles) -> Option<TypeId> {
    match &store.get(result)?.kind {
        TypeKind::Named {
            definition,
            arguments,
        } if roles.result_type.as_ref() == Some(definition) => match arguments.as_slice() {
            [GenericArgument::Type(_), GenericArgument::Type(error)] => Some(*error),
            _ => None,
        },
        _ => None,
    }
}
/// Remove source value access modes from a generic type argument. Complete
/// actual rows stay in CheckedBody.calls; capture multiplicity/sendability remain
/// intrinsic to a callable type. Nested type arguments keep their own contracts.
/// `arena` says which relation places name arenas: a type argument transports
/// its actual's placement (PROV-4) but none of its lifetime.
pub fn generic_type_argument(
    store: &mut TypeStore,
    actual: TypeId,
    arena: impl Fn(&Origin) -> bool,
) -> Result<TypeId, GraphError> {
    let mut row = store
        .get(actual)
        .cloned()
        .ok_or(GraphError::MissingType(actual))?;
    // Root access belongs to the actual value, including when that value has
    // a symbolic type. Arena places and regions describe content and stay.
    // A forwarded abstract parameter carries its enclosing declaration's
    // relation contract, independently of concrete holder inference.
    if !matches!(row.kind, TypeKind::Parameter(_))
        || row.modes.relation.named_parameters().is_some()
        || row.sendability_requirement == SendabilityRequirement::Sendable
    {
        row.modes.relation = row.modes.relation.retain(|r| match r {
            Reach::Place(o) => arena(o),
            Reach::Region(_) => true,
            Reach::Environment => false,
        });
    }
    row.modes.uniqueness = Uniqueness::Aliased;
    row.sendability_requirement = SendabilityRequirement::None;
    // An inferred type actual describes representation and receiving modes,
    // not the current operand proof. Each argument proves crossing separately.
    if !matches!(
        row.kind,
        TypeKind::Function(_)
            | TypeKind::Builtin {
                kind: BuiltinType::Iter,
                ..
            }
    ) {
        row.modes.sendability = Sendability::NotSendable;
    }
    Ok(store.push(row))
}
/// Imports reachable rows, preserving modes/effects; no signature provider reads.
pub fn import_type(
    source: &TypeStore,
    root: TypeId,
    destination: &mut TypeStore,
) -> Result<TypeId, GraphError> {
    substitute_type(source, root, destination, Substitution::default())
}
/// Import a new root row whose children belong to the supplied source snapshot.
/// No source row is added or copied merely to introduce that root.
pub fn import_type_row(
    source: &TypeStore,
    row: &Type,
    destination: &mut TypeStore,
) -> Result<TypeId, GraphError> {
    let root = (TypeStore::fresh_id(source.len()), row.clone());
    Importer {
        source: Some(source),
        root_row: Some(&root),
        destination,
        substitution: Substitution::default(),
        memo: BTreeMap::new(),
        active: BTreeSet::new(),
    }
    .ty(root.0, 0)
}

/// Binder-aware import/substitution. Source rows are immutable; destination IDs
/// are never looked up in the source. Failure may leave unused destination rows.
pub fn substitute_type(
    source: &TypeStore,
    root: TypeId,
    destination: &mut TypeStore,
    substitution: Substitution<'_>,
) -> Result<TypeId, GraphError> {
    Importer {
        source: Some(source),
        root_row: None,
        destination,
        substitution,
        memo: BTreeMap::new(),
        active: BTreeSet::new(),
    }
    .ty(root, 0)
}
/// Substitute a rooted graph in its own store, appending immutable output rows.
pub fn substitute_type_in_place(
    store: &mut TypeStore,
    root: TypeId,
    substitution: Substitution<'_>,
) -> Result<TypeId, GraphError> {
    Importer {
        source: None,
        root_row: None,
        destination: store,
        substitution,
        memo: BTreeMap::new(),
        active: BTreeSet::new(),
    }
    .ty(root, 0)
}

struct Importer<'a> {
    source: Option<&'a TypeStore>,
    root_row: Option<&'a (TypeId, Type)>,
    destination: &'a mut TypeStore,
    substitution: Substitution<'a>,
    memo: BTreeMap<(TypeId, u32), TypeId>,
    active: BTreeSet<TypeId>,
}
fn raise_reference<P: Copy>(p: BinderRef<P>, amount: u32) -> BinderRef<P> {
    match p {
        BinderRef::Bound { depth, parameter } => BinderRef::Bound {
            depth: depth + amount,
            parameter,
        },
        p => p,
    }
}
impl Importer<'_> {
    fn reference<P: Copy>(&self, p: BinderRef<P>, nested: u32) -> (BinderRef<P>, bool) {
        let Some(target) = self.substitution.binder else {
            return (p, false);
        };
        match (p, target) {
            (BinderRef::Bound { depth, .. }, GenericBinder::Bound(target))
                if depth == target + nested =>
            {
                (p, true)
            }
            (BinderRef::Bound { depth, parameter }, GenericBinder::Bound(target))
                if depth > target + nested =>
            {
                (
                    BinderRef::Bound {
                        depth: depth - 1,
                        parameter,
                    },
                    false,
                )
            }
            (BinderRef::Body { scope, .. }, GenericBinder::Body(target)) if scope == target => {
                (p, true)
            }
            _ => (p, false),
        }
    }
    fn argument(&self, p: u32) -> Result<&GenericArgument, GraphError> {
        self.substitution
            .arguments
            .get(p as usize)
            .ok_or(GraphError::MissingArgument(p))
    }
    fn type_argument(&self, p: TypeParameterRef) -> Result<TypeId, GraphError> {
        match self.argument(p.parameter().0)? {
            GenericArgument::Type(t) => Ok(*t),
            _ => Err(GraphError::ArgumentKind(p.parameter().0)),
        }
    }
    fn drop_effect(
        &self,
        effect: DropAllocation,
        nested: u32,
    ) -> Result<DropAllocation, GraphError> {
        match effect {
            DropAllocation::DependsOn(ps) => {
                let mut effect = DropAllocation::NoAlloc;
                for p in ps {
                    let (p, selected) = self.reference(p, nested);
                    if !selected {
                        effect = effect.join(DropAllocation::DependsOn(vec![p].into()));
                        continue;
                    }
                    let id = self.type_argument(p)?;
                    effect = effect.join(
                        match self
                            .destination
                            .get(id)
                            .ok_or(GraphError::MissingType(id))?
                            .drop_allocation
                            .clone()
                        {
                            DropAllocation::DependsOn(ps) => DropAllocation::DependsOn(
                                ps.into_iter().map(|p| raise_reference(p, nested)).collect(),
                            ),
                            effect => effect,
                        },
                    );
                }
                Ok(effect)
            }
            effect => Ok(effect),
        }
    }
    fn sendability(&self, effect: Sendability, nested: u32) -> Result<Sendability, GraphError> {
        match effect {
            Sendability::DependsOn(ps) => {
                let mut depends = BTreeSet::new();
                for p in ps {
                    let (p, selected) = self.reference(p, nested);
                    if !selected {
                        depends.insert(p);
                        continue;
                    }
                    let id = self.type_argument(p)?;
                    match &self
                        .destination
                        .get(id)
                        .ok_or(GraphError::MissingType(id))?
                        .modes
                        .sendability
                    {
                        Sendability::NotSendable => return Ok(Sendability::NotSendable),
                        Sendability::Sendable => (),
                        Sendability::DependsOn(ps) => {
                            depends.extend(ps.iter().map(|p| raise_reference(*p, nested)))
                        }
                    }
                }
                Ok(if depends.is_empty() {
                    Sendability::Sendable
                } else {
                    Sendability::DependsOn(depends.into_iter().collect())
                })
            }
            effect => Ok(effect),
        }
    }
    fn authority(&self, summary: &mut AuthoritySummary, nested: u32) -> Result<(), GraphError> {
        let AuthoritySummary::Known(terms) = summary else {
            return Ok(());
        };
        let mut result = BTreeSet::new();
        for term in terms.iter() {
            if let AuthorityTerm::TypeParameter(p) = term {
                let (p, selected) = self.reference(*p, nested);
                if !selected {
                    result.insert(AuthorityTerm::TypeParameter(p));
                    continue;
                }
                self.type_argument(p)?;
                match self
                    .substitution
                    .authorities
                    .get(p.parameter().0 as usize)
                    .ok_or(GraphError::MissingAuthority(p.parameter().0))?
                {
                    AuthoritySummary::Unknown => {
                        *summary = AuthoritySummary::Unknown;
                        return Ok(());
                    }
                    AuthoritySummary::Known(terms) => {
                        result.extend(terms.iter().map(|term| match term {
                            AuthorityTerm::TypeParameter(p) => {
                                AuthorityTerm::TypeParameter(raise_reference(*p, nested))
                            }
                            AuthorityTerm::Parameter(p) => {
                                AuthorityTerm::Parameter(raise_reference(*p, nested))
                            }
                            term => term.clone(),
                        }))
                    }
                }
            } else {
                result.insert(term.clone());
            }
        }
        *summary = AuthoritySummary::Known(result.into_iter().collect());
        Ok(())
    }
    fn ty(&mut self, id: TypeId, nested: u32) -> Result<TypeId, GraphError> {
        let key = (id, nested);
        if let Some(id) = self.memo.get(&key) {
            return Ok(*id);
        }
        if !self.active.insert(id) {
            return Err(GraphError::StructuralCycle(id));
        }
        let mut t = if let Some((_, row)) = self.root_row.filter(|(root, _)| *root == id) {
            row.clone()
        } else {
            self.source
                .unwrap_or(&*self.destination)
                .get(id)
                .cloned()
                .ok_or(GraphError::MissingType(id))?
        };
        if let TypeKind::Parameter(p) = t.kind {
            let (p, selected) = self.reference(p, nested);
            t.kind = TypeKind::Parameter(p);
            if selected {
                let target = self.type_argument(p)?;
                let target = shift_type_binders(self.destination, target, nested)?;
                let mut replacement = self
                    .destination
                    .get(target)
                    .cloned()
                    .ok_or(GraphError::MissingType(target))?;
                // Arguments are type contracts, not complete source-value evidence.
                // Written refinements constrain those contracts. Call inference
                // separates root value access modes with generic_type_argument.
                if !t.modes.relation.is_global() {
                    replacement.modes.relation = t.modes.relation;
                }
                if t.modes.uniqueness == Uniqueness::Unique {
                    replacement.modes.uniqueness = Uniqueness::Unique;
                }
                if t.modes.multiplicity == Multiplicity::Once {
                    replacement.modes.multiplicity = Multiplicity::Once;
                }
                if t.sendability_requirement == SendabilityRequirement::Sendable {
                    replacement.sendability_requirement = SendabilityRequirement::Sendable;
                }
                if !t.brand.alternatives.is_empty() {
                    replacement.brand = t.brand;
                }
                let mut target = self.destination.push(replacement.clone());
                // Qualifiers written on the parameter occurrence say nothing
                // about plain bits. Nominal arguments need their declaration:
                // callers holding a lookup normalize the substituted graph.
                if structural_property(
                    self.destination,
                    target,
                    StructuralProperty::Plain,
                    &GenericDeclarations::default(),
                    &mut |_| Ok::<_, crate::QueryError>(None),
                )
                .unwrap()
                {
                    replacement.canonicalize_plain();
                    target = self.destination.push(replacement);
                }
                self.memo.insert(key, target);
                self.active.remove(&id);
                return Ok(target);
            }
        }
        if let Some(ErasedCaptures::Known(a)) = &mut t.erased_captures {
            self.authority(a, nested)?;
        }
        t.drop_allocation = self.drop_effect(t.drop_allocation, nested)?;
        t.modes.sendability = self.sendability(t.modes.sendability, nested)?;
        match &mut t.kind {
            TypeKind::Tuple(ts) => {
                for t in ts {
                    *t = self.ty(*t, nested)?;
                }
            }
            TypeKind::Box(t) => *t = self.ty(*t, nested)?,
            TypeKind::Record(fs) => {
                for f in fs {
                    f.ty = self.ty(f.ty, nested)?;
                }
            }
            TypeKind::Variant(vs) => {
                for v in vs {
                    for (_, t) in &mut v.fields {
                        *t = self.ty(*t, nested)?;
                    }
                }
            }
            TypeKind::Function(f) => {
                for p in &mut f.parameters {
                    p.ty = self.ty(p.ty, nested + 1)?;
                }
                f.result = self.ty(f.result, nested + 1)?;
                f.error = f.error.map(|t| self.ty(t, nested + 1)).transpose()?;
                if self.substitution.roles.result_type.is_some() {
                    f.error = result_error(self.destination, f.result, self.substitution.roles);
                }
                self.authority(&mut f.authority, nested + 1)?;
                if let Uses::Only(a) = &mut f.uses {
                    self.authority(a, nested + 1)?;
                }
            }
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                for a in arguments {
                    match a {
                        GenericArgument::Type(t) => *t = self.ty(*t, nested)?,
                        GenericArgument::Size(Size::Parameter(p)) => {
                            let (reference, selected) = self.reference(*p, nested);
                            if selected {
                                *a = match self.argument(p.parameter().0)? {
                                    GenericArgument::Size(size) => {
                                        let mut size = size.clone();
                                        if let Size::Parameter(BinderRef::Bound { depth, .. }) =
                                            &mut size
                                        {
                                            *depth += nested;
                                        }
                                        GenericArgument::Size(size)
                                    }
                                    _ => return Err(GraphError::ArgumentKind(p.parameter().0)),
                                };
                            } else {
                                *p = reference;
                            }
                        }
                        _ => (),
                    }
                }
            }
            TypeKind::Error => return Err(GraphError::RecoveryType(id)),
            _ => (),
        }
        let result = self.destination.push(t);
        self.memo.insert(key, result);
        self.active.remove(&id);
        Ok(result)
    }
}
/// Direction in which a nested row of a received value may promise more than
/// the row expected there (CLO-2, #58). A value with a stronger promise serves
/// wherever it is only read: as a tuple, record, variant or box part, as a
/// sequence element and as a callable's result or error. A callable's
/// parameter is supplied by its caller, so the direction turns round there
/// ("written unrestricted parameters do not meet restricted uses"), and a
/// `mut` parameter, which the callee writes back, keeps the exact contract.
/// A nominal type's arguments stay exact: its declaration may use them in
/// either direction.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Variance {
    Exact,
    Covariant,
    Contravariant,
}
impl Variance {
    pub fn reversed(self) -> Self {
        match self {
            Self::Exact => Self::Exact,
            Self::Covariant => Self::Contravariant,
            Self::Contravariant => Self::Covariant,
        }
    }
    /// One entry per `TypeKind::graph_edges` edge of `kind`, in its order.
    pub fn edges(self, kind: &TypeKind) -> Vec<Self> {
        match kind {
            TypeKind::Function(f) => f
                .parameters
                .iter()
                .map(|p| match p.passing {
                    hirsel_syntax::PassingMode::Mut => Self::Exact,
                    _ => self.reversed(),
                })
                .chain([self])
                .chain(f.error.map(|_| self))
                .collect(),
            TypeKind::Builtin {
                kind: BuiltinType::Array | BuiltinType::List | BuiltinType::Inline,
                ..
            } => vec![self; kind.graph_edges().len()],
            TypeKind::Named { .. } | TypeKind::Builtin { .. } => {
                vec![Self::Exact; kind.graph_edges().len()]
            }
            _ => vec![self; kind.graph_edges().len()],
        }
    }
}
/// Rooted structural equality of normalized types. Nominal identity is DefPath;
/// unreachable rows, interning order, and provenance do not participate.
/// Generic ordinals must be in corresponding binder scopes; this is not alias
/// expansion, subtyping, or proof search.
pub fn semantic_equal(
    a: &TypeStore,
    a_root: TypeId,
    b: &TypeStore,
    b_root: TypeId,
) -> Result<bool, GraphError> {
    semantic_equal_adjusted(a, a_root, None, b, b_root, None, false, None, false)
}

pub(crate) fn semantic_equal_adjusted(
    a: &TypeStore,
    a_root: TypeId,
    a_row: Option<&Type>,
    b: &TypeStore,
    b_root: TypeId,
    b_row: Option<&Type>,
    // A nested row of `b` may carry a brand where the row of `a` names none.
    forget_brands: bool,
    payloads: Option<&BTreeSet<TypeId>>,
    // `b` is a value received where `a` is expected: its nested rows may make
    // the stronger promise along `Variance` edges.
    received: bool,
) -> Result<bool, GraphError> {
    fn row<'a>(
        store: &'a TypeStore,
        id: TypeId,
        root: TypeId,
        adjusted: Option<&'a Type>,
    ) -> Result<&'a Type, GraphError> {
        if id == root {
            if let Some(row) = adjusted {
                return Ok(row);
            }
        }
        store.get(id).ok_or(GraphError::MissingType(id))
    }
    fn valid(store: &TypeStore, root: TypeId, adjusted: Option<&Type>) -> Result<(), GraphError> {
        let mut pending = vec![(root, false)];
        let mut active = BTreeSet::new();
        let mut done = BTreeSet::new();
        while let Some((id, exit)) = pending.pop() {
            if exit {
                active.remove(&id);
                done.insert(id);
                continue;
            }
            if done.contains(&id) {
                continue;
            }
            if !active.insert(id) {
                return Err(GraphError::StructuralCycle(id));
            }
            work::count(Counter::EqualityRows);
            let row = row(store, id, root, adjusted)?;
            if matches!(row.kind, TypeKind::Error) {
                return Err(GraphError::RecoveryType(id));
            }
            pending.push((id, true));
            pending.extend(
                row.kind
                    .graph_edges()
                    .into_iter()
                    .rev()
                    .map(|edge| (edge.ty, false)),
            );
        }
        Ok(())
    }
    // Compare row data independently of arena identity; edge correspondence is
    // checked by the worklist. This also retains canonical erased-capture policy.
    fn without_edges(row: &Type, placeholder: TypeId) -> Type {
        let mut row = row.clone();
        if let Some(ErasedCaptures::FromArguments(ps)) = &mut row.erased_captures {
            ps.sort();
            ps.dedup();
        }
        if row.erased_captures == Some(ErasedCaptures::Known(AuthoritySummary::Unknown)) {
            row.erased_captures = None;
        }
        match &mut row.kind {
            TypeKind::Tuple(ts) => ts.fill(placeholder),
            TypeKind::Box(t) => *t = placeholder,
            TypeKind::Record(fs) => fs.iter_mut().for_each(|f| f.ty = placeholder),
            TypeKind::Variant(vs) => vs
                .iter_mut()
                .flat_map(|v| &mut v.fields)
                .for_each(|f| f.1 = placeholder),
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                for argument in arguments {
                    if let GenericArgument::Type(t) = argument {
                        *t = placeholder;
                    }
                }
            }
            TypeKind::Function(f) => {
                for parameter in &mut f.parameters {
                    parameter.name = None;
                    parameter.ty = placeholder;
                }
                f.result = placeholder;
                if let Some(error) = &mut f.error {
                    *error = placeholder;
                }
            }
            _ => (),
        }
        row
    }
    // Both complete graphs must be valid even if their roots immediately differ.
    valid(a, a_root, a_row)?;
    valid(b, b_root, b_row)?;
    let root = if received {
        Variance::Covariant
    } else {
        Variance::Exact
    };
    // `header`: the row is a callable's parameter or result.
    let mut pending = vec![(a_root, b_root, payloads.is_some(), root, false)];
    let mut seen = BTreeSet::new();
    while let Some((left, right, view, variance, header)) = pending.pop() {
        if !seen.insert((left, right, view, variance, header)) {
            continue;
        }
        work::count(Counter::EqualityRows);
        let children_view = view && payloads.is_some_and(|ids| ids.contains(&right));
        let adjusted_root =
            left == a_root && right == b_root && (a_row.is_some() || b_row.is_some());
        let left = row(a, left, a_root, a_row)?;
        let right = row(b, right, b_root, b_row)?;
        let mut expected = without_edges(left, a_root);
        let mut actual = without_edges(right, a_root);
        if (a_row.is_some() || b_row.is_some())
            && variance == Variance::Exact
            && (!matches!(
                actual.kind,
                TypeKind::Function(_)
                    | TypeKind::Builtin {
                        kind: BuiltinType::Iter,
                        ..
                    }
            ) || expected.sendability_requirement == SendabilityRequirement::Sendable)
            && expected.sendability_requirement == actual.sendability_requirement
        {
            // Mutable/owned structural equality compares fixed receiving
            // contracts. Current content is checked separately by receipt flow.
            actual.modes.sendability = expected.modes.sendability.clone();
        }
        if forget_brands && left.brand.alternatives.is_empty() {
            actual.brand = Brand::default();
        }
        if view
            && left.modes.uniqueness == Uniqueness::Aliased
            && actual.modes.uniqueness == Uniqueness::Unique
        {
            actual.modes.uniqueness = Uniqueness::Aliased;
            if left.modes.sendability == Sendability::NotSendable {
                actual.modes.sendability = Sendability::NotSendable;
            }
        }
        // MODE-2: a callable's result may promise uniqueness its required
        // type does not, and a parameter may ask for less than it is given.
        // Only the header row's root weakens, as a received value's root does.
        if header && header_weakens_uniqueness(&expected, &actual, variance) {
            actual.modes.uniqueness = expected.modes.uniqueness;
        }
        let subsumed = if adjusted_root {
            true
        } else {
            match variance {
                Variance::Exact => true,
                Variance::Covariant => crate::properties::nested_promise(&expected, &mut actual),
                Variance::Contravariant => {
                    crate::properties::nested_promise(&actual, &mut expected)
                }
            }
        };
        if !subsumed || expected != actual {
            return Ok(false);
        }
        let children_header = matches!(left.kind, TypeKind::Function(_));
        pending.extend(
            left.kind
                .graph_edges()
                .into_iter()
                .zip(right.kind.graph_edges())
                .zip(variance.edges(&left.kind))
                .map(|((a, b), variance)| (a.ty, b.ty, children_view, variance, children_header)),
        );
    }
    Ok(true)
}

/// MODE-2 for a callable's header row read in `variance`: a covariant row
/// (a result) may be `unique` where `expected` is aliased, a contravariant row
/// (a borrowed or taken parameter) aliased where `expected` is `unique`.
/// A `mut` parameter is exact: the callee hands the value back.
pub fn header_weakens_uniqueness(expected: &Type, actual: &Type, variance: Variance) -> bool {
    match variance {
        Variance::Covariant => {
            expected.modes.uniqueness == Uniqueness::Aliased
                && actual.modes.uniqueness == Uniqueness::Unique
        }
        Variance::Contravariant => {
            expected.modes.uniqueness == Uniqueness::Unique
                && actual.modes.uniqueness == Uniqueness::Aliased
        }
        Variance::Exact => false,
    }
}

/// Proof expansions own only imported actual arguments and declaration rows.
/// Untouched input rows remain borrowed; every scratch graph is self-contained.
pub(crate) struct RootedScratch<'a> {
    source: &'a TypeStore,
    scratch: TypeStore,
}
impl<'a> RootedScratch<'a> {
    pub(crate) fn new(source: &'a TypeStore) -> Self {
        Self {
            source,
            scratch: TypeStore::default(),
        }
    }
    pub(crate) fn store(&self, id: TypeId) -> &TypeStore {
        if self.scratch.id(id.index()) == Some(id) {
            &self.scratch
        } else {
            self.source
        }
    }
    pub(crate) fn get(&self, id: TypeId) -> Option<&Type> {
        self.store(id).get(id)
    }
    pub(crate) fn equal(&self, a: TypeId, b: TypeId) -> bool {
        semantic_equal(self.store(a), a, self.store(b), b).unwrap_or(false)
    }
    pub(crate) fn expand(
        &mut self,
        source: &TypeStore,
        root: TypeId,
        arguments: &[GenericArgument],
        roles: &NominalRoles,
    ) -> Result<TypeId, GraphError> {
        let arguments = arguments
            .iter()
            .map(|argument| {
                Ok(match argument {
                    GenericArgument::Type(id) if self.source.id(id.index()) == Some(*id) => {
                        GenericArgument::Type(import_type(self.source, *id, &mut self.scratch)?)
                    }
                    argument => argument.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphError>>()?;
        substitute_type(
            source,
            root,
            &mut self.scratch,
            Substitution {
                binder: Some(GenericBinder::Bound(0)),
                arguments: &arguments,
                authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                roles,
            },
        )
    }
}

/// Visit origin-bearing rows without rebuilding them. Visits retain lexical
/// depth, declaration-order edges, and the rewriting walk's cycle/error policy.
pub fn visit_type_origins(
    store: &TypeStore,
    root: TypeId,
    visit: impl FnMut(&Origin, u32) -> Result<(), GraphError>,
) -> Result<(), GraphError> {
    visit_type_origins_where(store, root, |_| true, visit)
}

/// `visit_type_origins` restricted to the relation places `place` selects;
/// brands are always visited. A checker that tracks only arena identity
/// passes its arena predicate.
pub fn visit_type_origins_where(
    store: &TypeStore,
    root: TypeId,
    place: impl Fn(&Origin) -> bool,
    mut visit: impl FnMut(&Origin, u32) -> Result<(), GraphError>,
) -> Result<(), GraphError> {
    let mut pending = vec![(root, 0u32, false)];
    let mut active = BTreeSet::new();
    let mut done = BTreeSet::new();
    while let Some((id, depth, exit)) = pending.pop() {
        if exit {
            active.remove(&id);
            done.insert((id, depth));
            continue;
        }
        if done.contains(&(id, depth)) {
            continue;
        }
        if !active.insert(id) {
            return Err(GraphError::StructuralCycle(id));
        }
        let row = store.get(id).ok_or(GraphError::MissingType(id))?;
        hirsel_intern::work::count(hirsel_intern::work::Counter::ValidateOriginDependencyRows);
        for origin in &row.brand.alternatives {
            visit(origin, depth)?;
        }
        for reach in row.modes.relation.reaches() {
            if let Reach::Place(origin) = reach {
                if place(origin) {
                    visit(origin, depth)?;
                }
            }
        }
        pending.push((id, depth, true));
        pending.extend(
            row.kind
                .graph_edges()
                .into_iter()
                .rev()
                .map(|edge| (edge.ty, depth + edge.role.parameter_binders(), false)),
        );
    }
    Ok(())
}

/// H's rooted type-shape judgment. It retains constructors, nominal identity,
/// generic declarations/arguments, labels, passing conventions and layout flags.
/// Modes, origins, placement, cleanup, capture/allocator evidence and callable
/// authority/allocation/contracts are judged independently by later phases.
/// Complete semantic equality and receiving relations remain strict.
pub fn shape_equal(
    a: &TypeStore,
    a_root: TypeId,
    b: &TypeStore,
    b_root: TypeId,
) -> Result<bool, GraphError> {
    shape_equal_with_holes(a, a_root, b, b_root, |_| false)
}

/// Compare known type shape while allowing caller-designated inference holes.
/// Both complete graphs are still validated; recovery rows and cycles are errors.
/// This does not bind holes or change strict semantic/receiving comparisons.
pub fn shape_equal_with_holes(
    a: &TypeStore,
    a_root: TypeId,
    b: &TypeStore,
    b_root: TypeId,
    hole: impl Fn(&TypeKind) -> bool,
) -> Result<bool, GraphError> {
    fn valid(source: &TypeStore, root: TypeId) -> Result<(), GraphError> {
        let mut pending = vec![(root, false)];
        let mut active = BTreeSet::new();
        let mut done = BTreeSet::new();
        while let Some((id, exit)) = pending.pop() {
            if exit {
                active.remove(&id);
                done.insert(id);
                continue;
            }
            if done.contains(&id) {
                continue;
            }
            if !active.insert(id) {
                return Err(GraphError::StructuralCycle(id));
            }
            // Reject foreign IDs before the accessor's debug provenance assertion.
            if source.id(id.index()) != Some(id) {
                return Err(GraphError::MissingType(id));
            }
            let row = source.get(id).ok_or(GraphError::MissingType(id))?;
            pending.push((id, true));
            let mut edge = |t| pending.push((t, false));
            match &row.kind {
                TypeKind::Tuple(ts) => ts.iter().copied().for_each(edge),
                TypeKind::Record(fs) => fs.iter().for_each(|f| edge(f.ty)),
                TypeKind::Variant(vs) => vs.iter().flat_map(|v| &v.fields).for_each(|f| edge(f.1)),
                TypeKind::Box(t) => edge(*t),
                TypeKind::Function(f) => {
                    f.parameters.iter().for_each(|p| edge(p.ty));
                    edge(f.result);
                    if let Some(t) = f.error {
                        edge(t);
                    }
                }
                TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                    for argument in arguments {
                        if let GenericArgument::Type(t) = argument {
                            edge(*t);
                        }
                    }
                }
                TypeKind::Error => return Err(GraphError::RecoveryType(id)),
                TypeKind::Primitive(_) | TypeKind::Parameter(_) => (),
            }
        }
        Ok(())
    }
    fn arguments(
        a: &[GenericArgument],
        b: &[GenericArgument],
        pending: &mut Vec<(TypeId, TypeId)>,
    ) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(a, b)| match (a, b) {
                (GenericArgument::Type(a), GenericArgument::Type(b)) => {
                    pending.push((*a, *b));
                    true
                }
                (GenericArgument::Size(a), GenericArgument::Size(b)) => a == b,
                _ => false,
            })
    }
    valid(a, a_root)?;
    valid(b, b_root)?;
    let mut pending = vec![(a_root, b_root)];
    let mut seen = BTreeSet::new();
    while let Some((a_id, b_id)) = pending.pop() {
        if !seen.insert((a_id, b_id)) {
            continue;
        }
        let a = &a.get(a_id).ok_or(GraphError::MissingType(a_id))?.kind;
        let b = &b.get(b_id).ok_or(GraphError::MissingType(b_id))?.kind;
        if hole(a) || hole(b) {
            continue;
        }
        let equal = match (a, b) {
            (TypeKind::Primitive(a), TypeKind::Primitive(b)) => a == b,
            (TypeKind::Parameter(a), TypeKind::Parameter(b)) => a == b,
            (TypeKind::Tuple(a), TypeKind::Tuple(b)) => {
                pending.extend(a.iter().copied().zip(b.iter().copied()));
                a.len() == b.len()
            }
            (TypeKind::Box(a), TypeKind::Box(b)) => {
                pending.push((*a, *b));
                true
            }
            (
                TypeKind::Named {
                    definition: a,
                    arguments: aa,
                },
                TypeKind::Named {
                    definition: b,
                    arguments: ba,
                },
            ) => a == b && arguments(aa, ba, &mut pending),
            (
                TypeKind::Builtin {
                    kind: a,
                    arguments: aa,
                },
                TypeKind::Builtin {
                    kind: b,
                    arguments: ba,
                },
            ) => a == b && arguments(aa, ba, &mut pending),
            (TypeKind::Record(a), TypeKind::Record(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b).all(|(a, b)| {
                        pending.push((a.ty, b.ty));
                        a.name == b.name && a.growable == b.growable
                    })
            }
            (TypeKind::Variant(a), TypeKind::Variant(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b).all(|(a, b)| {
                        a.name == b.name
                            && a.fields.len() == b.fields.len()
                            && a.fields.iter().zip(&b.fields).all(|(a, b)| {
                                pending.push((a.1, b.1));
                                a.0 == b.0
                            })
                    })
            }
            (TypeKind::Function(a), TypeKind::Function(b)) => {
                if a.generics != b.generics || a.parameters.len() != b.parameters.len() {
                    return Ok(false);
                }
                pending.push((a.result, b.result));
                match (a.error, b.error) {
                    (Some(a), Some(b)) => pending.push((a, b)),
                    (None, None) => (),
                    _ => return Ok(false),
                }
                a.parameters.iter().zip(&b.parameters).all(|(a, b)| {
                    pending.push((a.ty, b.ty));
                    a.label == b.label && a.passing == b.passing
                })
            }
            _ => false,
        };
        if !equal {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    #[test]
    fn inference_holes_preserve_known_nested_shapes_and_graph_validation() {
        let mut store = TypeStore::default();
        let variable = TypeParameterRef::bound(TypeParamId(0));
        let hole = store.push(row(TypeKind::Parameter(variable)));
        let boolean = store.push(Type::boolean());
        let byte = store.push(row(TypeKind::Primitive(Primitive::U8)));
        let partial = store.push(row(TypeKind::Tuple(vec![hole, boolean].into())));
        let complete = store.push(row(TypeKind::Tuple(vec![byte, boolean].into())));
        let wrong_part = store.push(row(TypeKind::Tuple(vec![byte, byte].into())));
        let wrong_constructor = store.push(row(TypeKind::Box(complete)));
        let is_hole = |kind: &TypeKind| matches!(kind, TypeKind::Parameter(p) if *p == variable);
        assert!(!shape_equal(&store, partial, &store, complete).unwrap());
        assert!(shape_equal_with_holes(&store, partial, &store, complete, is_hole).unwrap());
        assert!(!shape_equal_with_holes(&store, partial, &store, wrong_part, is_hole).unwrap());
        assert!(
            !shape_equal_with_holes(&store, partial, &store, wrong_constructor, is_hole).unwrap()
        );
        let recovery = store.push(row(TypeKind::Error));
        assert!(matches!(
            shape_equal_with_holes(&store, hole, &store, recovery, is_hole),
            Err(GraphError::RecoveryType(_))
        ));
    }

    fn row(kind: TypeKind) -> Type {
        Type::with_kind(kind)
    }
    fn path(name: &str) -> hirsel_syntax::DefPath {
        hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: hirsel_syntax::PackageId {
                name: "fixture".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: name.into(),
        })
    }
    fn callable(t: TypeId) -> FunctionType {
        FunctionType {
            state_promises: Default::default(),
            generics: vec![
                GenericParameter::Type {
                    name: "T".into(),
                    bounds: vec![Bound::Data].into(),
                },
                GenericParameter::Size { name: "n".into() },
            ]
            .into(),
            parameters: vec![Parameter {
                name: Some("x".into()),
                label: Some("value".into()),
                passing: PassingMode::Borrow,
                ty: t,
            }]
            .into(),
            result: t,
            error: Some(t),
            allocation: Allocation::MayAllocate,
            authority: AuthoritySummary::Unknown,
            uses: Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        }
    }
    fn shapes(store: &mut TypeStore) -> Vec<TypeId> {
        let scalar = store.push(row(TypeKind::Primitive(Primitive::U8)));
        let parameter = store.push(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let kinds = vec![
            TypeKind::Primitive(Primitive::U8),
            TypeKind::Primitive(Primitive::U16),
            TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
            TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(1))),
            TypeKind::Tuple(vec![scalar, parameter].into()),
            TypeKind::Tuple(vec![scalar].into()),
            TypeKind::Box(scalar),
            TypeKind::Record(
                vec![Field {
                    name: "field".into(),
                    ty: scalar,
                    growable: false,
                }]
                .into(),
            ),
            TypeKind::Variant(
                vec![Variant {
                    name: "Case".into(),
                    fields: vec![(Some("field".into()), scalar)].into(),
                }]
                .into(),
            ),
            TypeKind::Named {
                definition: path("Owner"),
                arguments: vec![
                    GenericArgument::Type(parameter),
                    GenericArgument::Size(Size::Constant(2)),
                ]
                .into(),
            },
            TypeKind::Builtin {
                kind: BuiltinType::Inline,
                arguments: vec![
                    GenericArgument::Type(scalar),
                    GenericArgument::Size(Size::Parameter(SizeParameterRef::bound(SizeParamId(1)))),
                ]
                .into(),
            },
            TypeKind::Function(callable(parameter)),
        ];
        kinds
            .into_iter()
            .map(|kind| store.push(row(kind)))
            .collect()
    }
    #[test]
    fn shape_constructors_and_structural_children_remain_distinct() {
        let mut store = TypeStore::default();
        let roots = shapes(&mut store);
        for (i, a) in roots.iter().enumerate() {
            for (j, b) in roots.iter().enumerate() {
                assert_eq!(shape_equal(&store, *a, &store, *b).unwrap(), i == j);
            }
        }
    }
    #[test]
    fn callable_shapes_keep_binders_labels_passing_and_error_edges() {
        let mut store = TypeStore::default();
        let t = store.push(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let f = callable(t);
        let root = store.push(row(TypeKind::Function(f.clone())));
        for change in [
            |f: &mut FunctionType| f.parameters[0].name = Some("renamed".into()),
            |f: &mut FunctionType| f.allocation = Allocation::NoAlloc,
            |f: &mut FunctionType| f.authority = AuthoritySummary::inert(),
            |f: &mut FunctionType| f.uses = Uses::Only(AuthoritySummary::inert()),
            |f: &mut FunctionType| {
                f.contracts.push(Contract {
                    kind: hirsel_syntax::ContractKind::Requires,
                    ordinal: 0,
                    code_hash: hirsel_syntax::ContentHash::of_bytes(b"contract"),
                })
            },
            |f: &mut FunctionType| f.incomplete = true,
            |f: &mut FunctionType| {
                f.foreign = Some(ForeignContract {
                    abi: "c".into(),
                    blocking: true,
                    aliasing_contract: "alias".into(),
                })
            },
        ] {
            let mut changed = f.clone();
            change(&mut changed);
            let changed = store.push(row(TypeKind::Function(changed)));
            assert!(shape_equal(&store, root, &store, changed).unwrap());
        }
        for change in [
            |f: &mut FunctionType| f.parameters[0].label = Some("other".into()),
            |f: &mut FunctionType| f.parameters[0].passing = PassingMode::Take,
            |f: &mut FunctionType| f.generics.clear(),
            |f: &mut FunctionType| {
                f.generics[0] = GenericParameter::Type {
                    name: "U".into(),
                    bounds: vec![Bound::Data].into(),
                }
            },
            |f: &mut FunctionType| {
                f.generics[0] = GenericParameter::Type {
                    name: "T".into(),
                    bounds: vec![Bound::Key].into(),
                }
            },
            |f: &mut FunctionType| f.error = None,
        ] {
            let mut changed = f.clone();
            change(&mut changed);
            let changed = store.push(row(TypeKind::Function(changed)));
            assert!(!shape_equal(&store, root, &store, changed).unwrap());
        }
        let other = store.push(row(TypeKind::Primitive(Primitive::U8)));
        let inner = store.push(row(TypeKind::Function(callable(other))));
        let outer = store.push(row(TypeKind::Function(callable(root))));
        let changed = store.push(row(TypeKind::Function(callable(inner))));
        assert!(!shape_equal(&store, outer, &store, changed).unwrap());
    }
    #[test]
    fn shapes_keep_field_layout_nominal_arguments_and_generic_sizes() {
        let mut store = TypeStore::default();
        let t = store.push(row(TypeKind::Primitive(Primitive::U8)));
        let record = TypeKind::Record(
            vec![Field {
                name: "field".into(),
                ty: t,
                growable: false,
            }]
            .into(),
        );
        let root = store.push(row(record.clone()));
        for (name, growable) in [("other", false), ("field", true)] {
            let changed = store.push(row(TypeKind::Record(
                vec![Field {
                    name: name.into(),
                    ty: t,
                    growable,
                }]
                .into(),
            )));
            assert!(!shape_equal(&store, root, &store, changed).unwrap());
        }
        let nominal = store.push(row(TypeKind::Named {
            definition: path("Owner"),
            arguments: vec![
                GenericArgument::Type(t),
                GenericArgument::Size(Size::Constant(2)),
            ]
            .into(),
        }));
        for (name, size) in [
            ("Other", Size::Constant(2)),
            ("Owner", Size::Constant(3)),
            (
                "Owner",
                Size::Parameter(SizeParameterRef::bound(SizeParamId(0))),
            ),
        ] {
            let changed = store.push(row(TypeKind::Named {
                definition: path(name),
                arguments: vec![GenericArgument::Type(t), GenericArgument::Size(size)].into(),
            }));
            assert!(!shape_equal(&store, nominal, &store, changed).unwrap());
        }
        let mut other = TypeStore::default();
        let imported = import_type(&store, nominal, &mut other).unwrap();
        assert!(shape_equal(&store, nominal, &other, imported).unwrap());
    }
    #[test]
    fn rooted_shapes_reject_malformed_reachable_graphs_and_ignore_unrelated_rows() {
        let mut store = TypeStore::default();
        let root = store.push(row(TypeKind::Primitive(Primitive::U8)));
        store.push(row(TypeKind::Error));
        assert!(shape_equal(&store, root, &store, root).unwrap());
        let cycle = TypeStore::from_graph(1, |ids| vec![row(TypeKind::Box(ids[0]))]);
        assert!(matches!(
            shape_equal(&cycle, cycle.id(0).unwrap(), &cycle, cycle.id(0).unwrap()),
            Err(GraphError::StructuralCycle(_))
        ));
        assert!(matches!(
            shape_equal(&store, cycle.id(0).unwrap(), &store, root),
            Err(GraphError::MissingType(_))
        ));
        let recovery = store.push(row(TypeKind::Error));
        assert!(matches!(
            shape_equal(&store, recovery, &store, root),
            Err(GraphError::RecoveryType(_))
        ));
    }
}

#[cfg(test)]
mod value_parts_tests {
    use super::*;
    use crate::declarations::tests::{path, row};

    #[test]
    fn value_parts_follow_declaration_order_without_entering_callable_binders() {
        let mut store = TypeStore::default();
        let a = store.push(row(TypeKind::Primitive(Primitive::U8)));
        let b = store.push(row(TypeKind::Primitive(Primitive::Bytes)));
        let field = Field {
            name: "first".into(),
            ty: a,
            growable: false,
        };
        let cases = [
            (TypeKind::Tuple(vec![a, b].into()), vec![a, b]),
            (TypeKind::Record(vec![field].into()), vec![a]),
            (
                TypeKind::Variant(
                    vec![Variant {
                        name: "First".into(),
                        fields: vec![(None, a), (None, b)].into(),
                    }]
                    .into(),
                ),
                vec![a, b],
            ),
            (TypeKind::Box(a), vec![a]),
            (
                TypeKind::Named {
                    definition: path("Wrap"),
                    arguments: vec![
                        GenericArgument::Size(Size::Constant(4)),
                        GenericArgument::Type(a),
                        GenericArgument::Type(b),
                    ]
                    .into(),
                },
                vec![a, b],
            ),
            (
                TypeKind::Builtin {
                    kind: BuiltinType::Inline,
                    arguments: vec![
                        GenericArgument::Type(a),
                        GenericArgument::Size(Size::Constant(4)),
                    ]
                    .into(),
                },
                vec![a],
            ),
            (TypeKind::Primitive(Primitive::U8), vec![]),
            (
                TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
                vec![],
            ),
            (TypeKind::Error, vec![]),
        ];
        for (kind, expected) in cases {
            assert_eq!(kind.value_parts(), expected);
        }
        let callable = TypeKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: vec![Parameter {
                name: None,
                label: None,
                passing: PassingMode::Borrow,
                ty: a,
            }]
            .into(),
            result: b,
            error: Some(a),
            allocation: Allocation::MayAllocate,
            authority: AuthoritySummary::Unknown,
            uses: Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        });
        assert!(callable.value_parts().is_empty());
        assert_eq!(callable.graph_edges().len(), 3);
    }
}
