use super::*;
use hirsel_syntax::{BinaryOp as B, PassingMode, UnaryOp as U};
use hirsel_type_analysis::cleanup::{CleanupLeaf, cleanup_summary};
use hirsel_types::*;

/// How a place receives a value, which decides what its nested rows may forget.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) enum Reception {
    /// A readonly argument or a whole taken value: nothing writes it back.
    View,
    /// A copy whose source stays live beside the receiving place.
    Holder,
    /// A `mut` argument, written back through the parameter.
    HandBack,
}

pub(super) type Relations = crate::hashed::HashMap<
    (TypeId, TypeId, TypeRelation, Reception),
    Result<bool, hirsel_types::QueryError>,
>;

pub(super) fn declaration_tables(
    t: &TypeInterner,
    definition: &Definition,
    signatures: &Reader<'_>,
) -> Option<Vec<GenericDeclarations>> {
    let empty: std::sync::Arc<[GenericParameter]> = [].into();
    let mut scopes: Vec<_> = definition
        .bodies
        .iter()
        .map(|b| (b.parameter_scope, None, empty.clone()))
        .collect();
    if let Some(signature) = signatures.get(definition.path) {
        let parameters: std::sync::Arc<[GenericParameter]> = signature.generics().into();
        scopes[definition.entry.0 as usize].2 = parameters.clone();
        for (_, id) in &definition.contracts {
            scopes[id.0 as usize].2 = parameters.clone();
        }
    }
    for (parent_id, parent) in definition.bodies.iter().enumerate() {
        for instruction in parent.blocks.iter().flat_map(|b| &b.instructions) {
            work::count(Counter::ValidateDeclarationRows);
            let InstructionKind::Assign { destination, value } = instruction.kind else {
                continue;
            };
            let Rvalue::Closure { body: child, .. } = parent.rvalues[value.0 as usize] else {
                continue;
            };
            if let TypeKind::Function(function) =
                &t.get(parent.places[destination.0 as usize].ty)?.kind
            {
                scopes[child.0 as usize].1 = Some(parent_id);
                scopes[child.0 as usize].2 = function.generics.clone().into();
            }
        }
    }
    GenericDeclarations::lexical(scopes)
}

// Additional data selectors strengthen an immutable entry. The independent
// source flow checker proves every such claim from the body, including exits
// and captures. Ordinary borrowed parameters acquire no contract implicitly.
fn scoped_parameters_valid(
    types: &TypeInterner,
    function: &hirsel_types::FunctionType,
    claimed: &[ScopedParameter],
    expected: &[ScopedParameter],
) -> bool {
    claimed.windows(2).all(|w| w[0] < w[1])
        && expected.iter().all(|p| claimed.contains(p))
        && claimed.iter().all(|p| {
            expected.contains(p)
                || p.fields.is_empty()
                    && function
                        .parameters
                        .get(p.parameter as usize)
                        .is_some_and(|a| {
                            a.passing == PassingMode::Borrow
                                && !matches!(&types.row(a.ty).kind, TypeKind::Function(_))
                        })
        })
}

pub(super) fn operands(r: &Rvalue) -> Vec<&Operand> {
    match r {
        Rvalue::Use(o) | Rvalue::Unary { value: o, .. } => {
            vec![o]
        }
        Rvalue::Binary { left, right, .. } => vec![left, right],
        Rvalue::Construct {
            constructor: Constructor::SortedMap { by },
            fields,
            ..
        } => std::iter::once(by)
            .chain(fields.iter().map(|f| &f.value))
            .collect(),
        Rvalue::Construct { fields, .. } => fields.iter().map(|f| &f.value).collect(),
        Rvalue::Closure { captures, .. } => captures.iter().collect(),
        _ => vec![],
    }
}

pub(super) fn signature_valid(t: &TypeInterner, s: &Signature) -> bool {
    if let SignatureKind::Const { generics, .. } = &s.kind {
        if generics
            .iter()
            .any(|parameter| !matches!(parameter, GenericParameter::Size { .. }))
        {
            return false;
        }
    }
    let rows = hirsel_types::closure(t, hirsel_types::signature_types(t, s));
    if !graph_valid(t, &rows)
        || rows
            .iter()
            .map(|&id| t.row(id))
            .any(|row| row.allocation_target.is_some() && !row.is_heap_target(&s.metadata.roles))
    {
        return false;
    }
    if let SignatureKind::Function(f) = &s.kind {
        if !value_parameters_valid(t, f) {
            return false;
        }
    }
    let declarations = GenericDeclarations::signature(s.generics());
    if let SignatureKind::Function(f) = &s.kind {
        if !generic_authority_valid(&f.authority, &declarations)
            || matches!(&f.uses, Uses::Only(a) if !generic_authority_valid(a, &declarations))
        {
            return false;
        }
    }
    if let SignatureKind::Type(t) = &s.kind {
        if !generic_authority_valid(&t.authority, &declarations)
            || t.implementation_authority
                .as_ref()
                .is_some_and(|h| !generic_authority_valid(h, &declarations))
        {
            return false;
        }
    }
    if !s.metadata.schema.iter().all(|c| match c {
        SchemaConstraint::Bound { parameter, .. }
        | SchemaConstraint::Domain { parameter, .. }
        | SchemaConstraint::NoallocDrop(parameter) => declarations.get(*parameter).is_some(),
        SchemaConstraint::CompleteRangeFits {
            source,
            destination,
        }
        | SchemaConstraint::DestinationNoWider {
            source,
            destination,
        } => declarations.get(*source).is_some() && declarations.get(*destination).is_some(),
        SchemaConstraint::PositiveSize(p) => declarations.get(*p).is_some(),
    }) {
        return false;
    }
    let roots = match &s.kind {
        SignatureKind::Function(f) => f
            .parameters
            .iter()
            .map(|p| p.ty)
            .chain([f.result])
            .chain(f.error)
            .collect(),
        SignatureKind::Type(t) => t.representation.into_iter().collect::<Vec<_>>(),
        SignatureKind::Const { ty: t, .. } => vec![*t],
        SignatureKind::Implementation(i) => i.types(),
        SignatureKind::Family(_) => vec![],
    };
    roots
        .into_iter()
        .all(|root| t.contains(root) && generic_references_valid(t, root, &declarations))
}

fn value_parameters_valid(store: &TypeInterner, f: &FunctionType) -> bool {
    let promises_valid = |f: &FunctionType| {
        let mut seen = BTreeSet::new();
        f.state_promises.iter().all(|p| {
            seen.insert(p.selector.clone())
                && (p.advance_only || p.preserve_captures || p.preserve_unique)
                && (!p.preserve_unique || p.selector.fields.is_empty())
                && matches!(p.selector.parameter, ParameterRef::Bound { depth: 0, parameter } if f.parameters.get(parameter.0 as usize).is_some_and(|q| q.passing == PassingMode::Mut
                    || q.passing == PassingMode::Take && p.preserve_unique && !p.preserve_captures && !p.advance_only))
        })
    };
    if !promises_valid(f) {
        return false;
    }
    fn valid(p: ParameterRef, scopes: &[usize]) -> bool {
        match p {
            ParameterRef::Bound { depth, parameter } => scopes
                .get(depth as usize)
                .is_some_and(|n| (parameter.0 as usize) < *n),
            _ => false,
        }
    }
    fn authority(a: &AuthoritySummary, scopes: &[usize]) -> bool {
        match a {
            AuthoritySummary::Known(ts) => ts.iter().all(|t| match t {
                AuthorityTerm::Parameter(p) => valid(*p, scopes),
                _ => true,
            }),
            _ => true,
        }
    }
    if !authority(&f.authority, &[f.parameters.len()])
        || matches!(&f.uses, Uses::Only(a) if !authority(a,&[f.parameters.len()]))
    {
        return false;
    }
    let mut pending: Vec<_> = f
        .parameters
        .iter()
        .map(|p| p.ty)
        .chain([f.result])
        .chain(f.error)
        .map(|t| (t, vec![f.parameters.len()]))
        .collect();
    let mut seen = HashSet::default();
    while let Some((t, mut scopes)) = pending.pop() {
        if !seen.insert((t, scopes.clone())) {
            continue;
        }
        let Some(row) = store.get(t) else {
            return false;
        };
        let origin_valid = |o: &Origin| match o.root {
            OriginRoot::Parameter(p) | OriginRoot::ArgumentBrand(p) => valid(p, &scopes),
            _ => true,
        };
        if !row.brand.alternatives.iter().all(origin_valid)
            || row
                .modes
                .relation
                .reaches()
                .iter()
                .any(|r| matches!(r, Reach::Place(o) if !origin_valid(o)))
        {
            return false;
        }
        if row
            .erased_captures
            .as_ref()
            .is_some_and(|a| !authority(&a.authority(), &scopes))
        {
            return false;
        }
        if let TypeKind::Function(f) = &row.kind {
            if !promises_valid(f) {
                return false;
            }
            scopes.insert(0, f.parameters.len());
            if !authority(&f.authority, &scopes)
                || matches!(&f.uses, Uses::Only(a) if !authority(a,&scopes))
            {
                return false;
            }
        }
        pending.extend(
            row.kind
                .graph_edges()
                .into_iter()
                .map(|edge| (edge.ty, scopes.clone())),
        );
    }
    true
}

pub(super) fn integer(p: Primitive) -> bool {
    !matches!(
        p,
        Primitive::Bool
            | Primitive::F32
            | Primitive::F64
            | Primitive::Char
            | Primitive::String
            | Primitive::Bytes
            | Primitive::Never
    )
}
fn numeric(p: Primitive) -> bool {
    integer(p) || matches!(p, Primitive::F32 | Primitive::F64)
}
/// Whether a constant's bits are a value of its scalar type: an integer fits
/// its width and a char is a Unicode scalar value (LANG-7).
pub(super) fn constant_valid(c: &Constant, kind: &TypeKind) -> bool {
    match (c, kind) {
        (Constant::Unit, TypeKind::Tuple(ts)) => ts.is_empty(),
        (Constant::Bool(_), TypeKind::Primitive(Primitive::Bool))
        | (Constant::F32(_), TypeKind::Primitive(Primitive::F32))
        | (Constant::F64(_), TypeKind::Primitive(Primitive::F64))
        | (Constant::String(_), TypeKind::Primitive(Primitive::String))
        | (Constant::Bytes(_), TypeKind::Primitive(Primitive::Bytes)) => true,
        (Constant::Integer(bits), TypeKind::Primitive(Primitive::Char)) => {
            u32::try_from(*bits).ok().and_then(char::from_u32).is_some()
        }
        (Constant::Integer(bits), TypeKind::Primitive(p)) if integer(*p) => {
            let width = match p {
                Primitive::U8 | Primitive::I8 => 8,
                Primitive::U16 | Primitive::I16 => 16,
                Primitive::U32 | Primitive::I32 => 32,
                Primitive::U64 | Primitive::I64 => 64,
                _ => 128,
            };
            width == 128 || *bits < (1u128 << width)
        }
        _ => false,
    }
}

impl<'a> Checker<'a> {
    pub(super) fn types(&mut self) {
        self.arena_scope_entry();
        self.match_tables();
        let declarations = self.generic_declarations();
        if self
            .body
            .locals
            .iter()
            .map(|l| l.ty)
            .chain(self.body.places.iter().map(|p| p.ty))
            .chain([self.body.result])
            .any(|t| !generic_references_valid(self.t, t, &declarations))
        {
            self.error(ViolationKind::InvalidTypeGraph);
            return;
        }
        self.reachability();
        self.owner_type();
        self.nominal_bounds();
        self.prepare_content_inputs();
        for (i, p) in self.body.places.iter().enumerate() {
            let store = self.t;
            let expected = self.projection_type(store, p, |depth, field| {
                let binding = p.binding.as_ref()?;
                let base = p.projection.len().checked_sub(binding.fields.len())?;
                let relative = depth.checked_sub(base)?;
                let mut parent = binding.clone();
                parent.fields.truncate(relative);
                parent.fields.push(field);
                Some(parent)
            });
            let expected = expected.map(|root| self.canonical_plain(store, root));
            let current = self.global_root_projection(store, p);
            let current = self.receiving_root(p.root).then_some(current).flatten();
            let actual = p.ty;
            let physical = store.get(actual).unwrap().modes.clone();
            let actual = self.canonical_plain(store, actual);
            let actual = if !p.projection.is_empty() {
                let mut row = store.get(actual).unwrap().clone();
                row.modes.relation = physical.relation;
                row.modes.uniqueness = physical.uniqueness;
                store.intern(row)
            } else {
                actual
            };
            let valid = expected.is_some_and(|expected| {
                let actual = self.comparison_evidence(store, expected, actual);
                let mut row = store.get(actual).unwrap().clone();
                if !matches!(
                    row.kind,
                    TypeKind::Function(_)
                        | TypeKind::Builtin {
                            kind: BuiltinType::Iter,
                            ..
                        }
                ) {
                    row.modes.sendability = store.get(expected).unwrap().modes.sendability.clone();
                }
                let actual = store.intern(row);
                self.t.semantic_eq(expected, actual)
                    || current.is_some_and(|current| self.t.semantic_eq(current, p.ty))
                    || (!p.projection.is_empty()
                        && (Self::has_origins(&store, expected) || Self::has_origins(self.t, p.ty)))
            });
            if !valid {
                self.error(ViolationKind::InvalidProjection(PlaceId(i as u32)));
            }
        }
        let receipt_paths = (0..self.body.places.len())
            .map(|i| self.receipt_path(PlaceId(i as u32)))
            .collect();
        let _ = self.scratch.receipt_paths[self.site.body.0 as usize].set(receipt_paths);
        for (b, block) in self.body.blocks.iter().enumerate() {
            self.site.block = Some(BlockId(b as u32));
            for (i, instruction) in block.instructions.iter().enumerate() {
                self.site.instruction = Some(i as u32);
                match &instruction.kind {
                    InstructionKind::Assign { destination, value } => {
                        self.rvalue_type(*destination, &self.body.rvalues[value.0 as usize])
                    }
                    InstructionKind::Snapshot { destination, value } => {
                        self.same(
                            self.place_type(*destination),
                            self.place_type(value.place),
                            "snapshot",
                        );
                        if super::flow::needs_release(self, self.place_type(*destination), self.t) {
                            self.error(ViolationKind::TypeMismatch {
                                entity: "nonplain snapshot",
                            });
                        }
                    }
                    InstructionKind::Check { condition, .. } => {
                        self.boolean(condition, "check condition")
                    }
                    _ => (),
                }
            }
            self.site.instruction = None;
            match &block.terminator {
                Terminator::Match { table, from } => self.match_start(*table, *from),
                Terminator::Branch { condition, .. } => self.boolean(condition, "branch condition"),
                Terminator::Switch {
                    value,
                    arms,
                    otherwise,
                } => {
                    let store = self.t;
                    let kind = self.layout(store, self.place_type(value.place));
                    match kind {
                        Some(TypeKind::Variant(vs)) => {
                            if arms.iter().any(|a| a.0 as usize >= vs.len())
                                || (matches!(
                                    self.body.blocks[otherwise.0 as usize].terminator,
                                    Terminator::Unreachable
                                ) && (0..vs.len())
                                    .any(|v| !arms.iter().any(|a| a.0 as usize == v)))
                            {
                                self.error(ViolationKind::MatchCoverage);
                            }
                        }
                        Some(TypeKind::Primitive(Primitive::Bool)) => {
                            if arms.iter().any(|a| a.0 > 1)
                                || (matches!(
                                    self.body.blocks[otherwise.0 as usize].terminator,
                                    Terminator::Unreachable
                                ) && (0..2).any(|v| !arms.iter().any(|a| a.0 == v)))
                            {
                                self.error(ViolationKind::MatchCoverage);
                            }
                        }
                        Some(TypeKind::Primitive(p)) if integer(p) => (),
                        _ => self.error(ViolationKind::TypeMismatch {
                            entity: "switch scrutinee",
                        }),
                    }
                }
                Terminator::Call {
                    callee,
                    arguments,
                    returns,
                    ..
                } => self.call_type(callee, arguments, returns.map(|r| r.destination), None),
                Terminator::Return { value, .. } => {
                    if self.retains_scoped_capability(self.place_type(value.place)) {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "scoped capability escape",
                        });
                    }
                    let store = self.t;
                    let (expected, unknown_join) = self.return_type(store);
                    // A conservative source join accepts a more precise borrowed
                    // placement. CFG source inclusion independently proves that
                    // it cannot outlive any selected source; owned returns still
                    // use Value compatibility and cannot erase arena placement.
                    if unknown_join {
                        let expected = receiving_storage(store, expected);
                        let actual = self.copied_type(store, value, self.place_type(value.place));
                        if !self.compatible_with_context(
                            store,
                            expected,
                            actual,
                            TypeRelation::Parameter,
                            &self.generic_declarations(),
                        ) {
                            self.error(ViolationKind::TypeMismatch { entity: "return" });
                        }
                    } else {
                        self.storage_value_type_in(
                            store,
                            expected,
                            self.place_type(value.place),
                            value,
                            "return",
                        )
                    }
                }
                _ => (),
            }
        }
    }

    fn reachability(&mut self) {
        let mut seen = vec![false; self.body.blocks.len()];
        let mut pending = vec![self.body.entry];
        while let Some(b) = pending.pop() {
            if std::mem::replace(&mut seen[b.0 as usize], true) {
                continue;
            }
            pending.extend(
                self.body
                    .successors(&self.body.blocks[b.0 as usize].terminator),
            );
        }
        for (i, block) in self.body.blocks.iter().enumerate() {
            // Empty dead sentinels are valid defaults after exhaustive switches.
            if !seen[i]
                && !(block.instructions.is_empty()
                    && matches!(block.terminator, Terminator::Unreachable))
            {
                self.error(ViolationKind::UnreachableBlock(BlockId(i as u32)));
            }
        }
        let mut used = vec![false; self.body.rvalues.len()];
        for block in &self.body.blocks {
            for i in &block.instructions {
                if let InstructionKind::Assign { value, .. } = i.kind {
                    used[value.0 as usize] = true;
                }
            }
        }
        for (i, used) in used.iter().enumerate() {
            if !used {
                self.error(ViolationKind::UnreferencedRvalue(RvalueId(i as u32)));
            }
        }
        if self.site.body == self.definition.entry {
            // Each executable closure body has one construction convention.
            // Flow cannot choose parameter modes from one producer and capture
            // ownership from another, even if their slot types are identical.
            let mut producers = BTreeSet::from([self.definition.entry.0]);
            for owner in &self.definition.bodies {
                for instruction in owner.blocks.iter().flat_map(|block| &block.instructions) {
                    if let InstructionKind::Assign { value, .. } = instruction.kind {
                        if let Rvalue::Closure { body, .. } = owner.rvalues[value.0 as usize] {
                            if !producers.insert(body.0) {
                                self.error(ViolationKind::Duplicate {
                                    entity: "closure body",
                                    index: body.0,
                                });
                            }
                        }
                    }
                }
            }
            for (_, clause) in &self.definition.contracts {
                if *clause == self.definition.entry || !producers.contains(&clause.0) {
                    self.error(ViolationKind::WrongKind {
                        entity: "contract producer",
                        index: clause.0,
                    });
                }
            }
            let mut bodies = vec![false; self.definition.bodies.len()];
            let mut pending: Vec<_> = self
                .definition
                .contracts
                .iter()
                .map(|c| c.1)
                .chain([self.definition.entry])
                .collect();
            while let Some(b) = pending.pop() {
                if std::mem::replace(&mut bodies[b.0 as usize], true) {
                    continue;
                }
                for block in &self.definition.bodies[b.0 as usize].blocks {
                    for i in &block.instructions {
                        if let InstructionKind::Assign { value, .. } = i.kind {
                            if let Rvalue::Closure { body, .. } =
                                self.definition.bodies[b.0 as usize].rvalues[value.0 as usize]
                            {
                                pending.push(body);
                            }
                        }
                    }
                }
            }
            for (i, used) in bodies.iter().enumerate() {
                if !used {
                    self.error(ViolationKind::UnreferencedBody(BodyId(i as u32)));
                }
            }
        }
    }

    /// Reconstruct declarations independently from the owner signature and closure
    /// definitions. Values carry their binder, so captures and aliases need no
    /// additional namespace transport.
    pub(super) fn generic_declarations(&self) -> &'a GenericDeclarations {
        &self.declarations[self.site.body.0 as usize]
    }

    fn size_parameter(&mut self, parameter: SizeParameterRef) {
        if !matches!(
            self.generic_declarations().get(parameter),
            Some(GenericParameter::Size { .. })
        ) {
            self.error(ViolationKind::InvalidId {
                arena: "size parameter",
                index: parameter.parameter().0,
            });
        }
    }

    pub(super) fn return_type(&self, store: &TypeInterner) -> (TypeId, bool) {
        let declared = store.get(self.body.result).unwrap().modes.relation.clone();
        // The body's own Arena parameters are where its result's blocks live
        // (PROV-4); the remaining parameters are its lifetime sources.
        let arena_parameter = |p: &ParameterRef| {
            let ordinal = match p {
                ParameterRef::Bound {
                    depth: 0,
                    parameter,
                } => parameter.0,
                ParameterRef::Body { scope, parameter } if *scope == self.body.parameter_scope => {
                    parameter.0
                }
                _ => return false,
            };
            self.body
                .parameters
                .get(ordinal as usize)
                .is_some_and(|l| self.arena_type(self.body.locals[l.0 as usize].ty))
        };
        let dependent = declared
            .retain(|r| {
                !matches!(r, Reach::Place(Origin { root: OriginRoot::Parameter(p), fields })
                    if fields.is_empty() && arena_parameter(p))
            })
            .named_parameters()
            .is_some();
        let mut actuals = BTreeMap::new();
        let mut origins = BTreeMap::new();
        let mut captures = BTreeMap::new();
        for (i, local) in self.body.parameters.iter().enumerate() {
            let ty = self.body.locals[local.0 as usize].ty;
            let row = store.get(ty).unwrap();
            let properties = CaptureProperties::from_type(
                row,
                self.authority_with_generics(store, ty, &self.generic_declarations()),
                row.modes.sendability.clone(),
            );
            for parameter in [
                ParameterRef::bound(ParamId(i as u32)),
                ParameterRef::body(self.body.parameter_scope, ParamId(i as u32)),
            ] {
                actuals.insert(parameter, ty);
                captures.insert(parameter.into(), properties.clone());
                let identity = Origin {
                    root: OriginRoot::Parameter(ParameterRef::body(
                        self.body.parameter_scope,
                        ParamId(i as u32),
                    )),
                    fields: vec![].into(),
                };
                origins.insert(
                    parameter,
                    OriginArgument {
                        identity: identity.clone(),
                        brand: row.brand.clone(),
                        relation: if self.arena_type(ty) {
                            Relation::place(identity)
                        } else {
                            row.modes.relation.clone()
                        },
                    },
                );
            }
        }
        let expected = instantiate_parameter_sources(store, self.body.result, &origins, &actuals)
            .expect("validated parameter graph");
        // The declared creation envelope is instantiated from the incoming
        // parameters here; executable flow separately proves its input set.
        let expected = instantiate_parameter_captures(store, expected, &captures)
            .expect("validated capture selector graph");
        let mut expected = store.get(expected).unwrap().clone();
        // A dependent result's sources are bounded by flow; its placement is
        // where every source agrees, else unknown (PROV-4).
        let placement = self.placement(&expected.modes.relation);
        let unknown_join = dependent && placement.reaches().len() != 1;
        if dependent {
            expected.modes.relation = if unknown_join {
                Relation::environment()
            } else {
                Relation::environment().union(&placement)
            };
        }
        let expected = store.intern(expected);
        let expected = if dependent {
            self.derive_sendability(store, expected)
        } else {
            expected
        };
        (expected, unknown_join)
    }

    fn owner_type(&mut self) {
        if self.site.body != self.definition.entry {
            return;
        }
        let Some(signature) = self.signatures.get(self.definition.path) else {
            if !self.body.parameters.is_empty() {
                self.error(ViolationKind::MissingSignature(
                    self.definition.path.path().clone(),
                ));
            }
            return;
        };
        if let SignatureKind::Type(t) = &signature.kind {
            if !t.resource {
                self.error(ViolationKind::InvalidSignature(
                    signature.definition.path().clone(),
                ));
            } else if self.body.parameters.len() != 1 {
                self.error(ViolationKind::Arity {
                    entity: "finalizer parameters",
                    expected: 1,
                    actual: self.body.parameters.len(),
                });
            } else {
                let self_ty = self.body.locals[self.body.parameters[0].0 as usize].ty;
                let valid = match &self.t.get(self_ty).unwrap().kind {
                    TypeKind::Named { definition, arguments } => *definition == signature.definition
                        && arguments.len() == t.generics.len()
                        && arguments.iter().zip(&t.generics).enumerate().all(|(i, (a, p))| match (a, p) {
                            (GenericArgument::Type(id), GenericParameter::Type { .. }) => matches!(
                                self.t.get(*id).unwrap().kind, TypeKind::Parameter(p) if p.binder() == GenericBinder::Body(self.body.parameter_scope) && p.parameter().0 == i as u32),
                            (GenericArgument::Size(Size::Parameter(p)), GenericParameter::Size { .. }) => p.binder() == GenericBinder::Body(self.body.parameter_scope) && p.parameter().0 == i as u32,
                            _ => false,
                        }),
                    _ => false,
                };
                if !valid {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "finalizer self",
                    });
                }
            }
            if !matches!(&self.t.get(self.body.result).unwrap().kind, TypeKind::Tuple(ts) if ts.is_empty())
            {
                self.error(ViolationKind::TypeMismatch {
                    entity: "finalizer result",
                });
            }
            return;
        }
        if let SignatureKind::Const { ty: result, .. } = &signature.kind {
            if !self.body.parameters.is_empty() {
                self.error(ViolationKind::Arity {
                    entity: "constant parameters",
                    expected: 0,
                    actual: self.body.parameters.len(),
                });
            }
            self.cross_type(self.t, *result, self.body.result, &[], "constant result");
            return;
        }
        let SignatureKind::Function(f) = &signature.kind else {
            self.error(ViolationKind::InvalidSignature(
                signature.definition.path().clone(),
            ));
            return;
        };
        if !f.generics.is_empty() {
            // Generic bodies retain declaration-scope parameters; import without
            // substitution here. Concrete calls substitute in the caller store.
        }
        let expected: Vec<_> = f
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.passing != PassingMode::Take
                    && matches!(
                        self.t.get(p.ty).map(|t| &t.kind),
                        Some(TypeKind::Function(_))
                    )
            })
            .map(|(i, _)| ScopedParameter {
                parameter: i as u32,
                fields: Vec::new(),
            })
            .collect();
        if !scoped_parameters_valid(self.t, f, &self.body.scoped_parameters, &expected) {
            self.error(ViolationKind::TypeMismatch {
                entity: "scoped parameter contract",
            });
        }
        if f.parameters.len() != self.body.parameters.len() {
            self.error(ViolationKind::Arity {
                entity: "body parameters",
                expected: f.parameters.len(),
                actual: self.body.parameters.len(),
            });
            return;
        }
        for (p, local) in f.parameters.iter().zip(&self.body.parameters) {
            self.cross_type(
                self.t,
                p.ty,
                self.body.locals[local.0 as usize].ty,
                &[],
                "body parameter",
            );
        }
        self.cross_type(self.t, f.result, self.body.result, &[], "body result");
    }

    pub(super) fn place_type(&self, place: PlaceId) -> TypeId {
        self.body.places[place.0 as usize].ty
    }
    fn same(&mut self, a: TypeId, b: TypeId, entity: &'static str) {
        if !self.t.semantic_eq(a, b) {
            self.error(ViolationKind::TypeMismatch { entity });
        }
    }
    /// Arithmetic reads scalar bits without requiring an operand's identity.
    /// Its result must independently have the same complete type with no brand.
    fn arithmetic_type(&mut self, left: TypeId, right: TypeId, result: TypeId) {
        let store = self.t;
        let mut left_row = store.get(left).unwrap().clone();
        let mut right_row = store.get(right).unwrap().clone();
        left_row.brand = Brand::default();
        right_row.brand = Brand::default();
        let left = store.intern(left_row);
        let right = store.intern(right_row);
        if !self.t.semantic_eq(left, right) {
            self.error(ViolationKind::TypeMismatch {
                entity: "binary operands",
            });
        }
        if !self.t.semantic_eq(result, left) {
            self.error(ViolationKind::TypeMismatch {
                entity: "binary result",
            });
        }
    }
    /// A comparison borrows both operands and reads their contents, so no
    /// root evidence about either holder takes part: uniqueness, nested
    /// `unique` parts included, is set aside, differing relations, arena
    /// placement included, meet at a common borrowed root (a read is the same
    /// wherever the content lives), and brands, nested ones included, are
    /// identity evidence rather than operand type. Cleanup and every other
    /// nested qualifier remain exact. Sendability is derived again for a row
    /// whose holder evidence changed, never for one compared as published.
    fn comparison_type(&mut self, left: TypeId, right: TypeId) {
        let store = self.t;
        let rows = [left, right].map(|t| store.get(t).unwrap().clone());
        let relations = rows[0].modes.relation != rows[1].modes.relation;
        let [left, right] = rows.map(|mut row| {
            let derive = row.modes.uniqueness == Uniqueness::Unique
                || (relations && row.modes.relation != Relation::environment());
            row.modes.uniqueness = Uniqueness::Aliased;
            if relations {
                row.modes.relation = Relation::environment();
            }
            let root = store.intern(row);
            // A graph that cannot be rewritten is compared as published.
            let root = hirsel_types::erase_brands(store, root).unwrap_or(root);
            let aliased = hirsel_types::erase_uniqueness(store, root).unwrap_or(root);
            let derive = derive || aliased != root;
            let root = aliased;
            let root = if derive {
                self.derive_sendability(store, root)
            } else {
                root
            };
            hirsel_types::erase_comparison_crossing(store, root).unwrap_or(root)
        });
        if !self.t.semantic_eq(left, right) {
            self.error(ViolationKind::TypeMismatch {
                entity: "binary operands",
            });
        }
    }
    /// MODE-2/7 allow an inferred value to satisfy a weaker storage or result
    /// promise. Root value evidence weakens; of the nested rows only callable
    /// promises in reading positions do (CLO-2).
    fn value_type_in(
        &mut self,
        source: &TypeInterner,
        expected: TypeId,
        actual: TypeId,
        entity: &'static str,
    ) {
        if !self.compatible(source, expected, actual, TypeRelation::Value) {
            self.error(ViolationKind::TypeMismatch { entity });
        }
    }
    /// Reconstruct inactive payload arguments from executable producers. A
    /// phantom generic argument may acquire a receiving brand only when every
    /// incoming value selects a variant with no branded field. Never trust the
    /// operand row as evidence of which variant is active.
    fn readonly_capture_sources(
        &self,
        body_id: BodyId,
        root: hirsel_tir::LocalId,
        producers: &[super::variant_brands::Producer],
    ) -> Option<Vec<(BodyId, PlaceId)>> {
        // Exactly the initial capture, with no later write or mutable loan.
        if !matches!(producers, [super::variant_brands::Producer::Unknown]) {
            return None;
        }
        let body = &self.definition.bodies[body_id.0 as usize];
        let ordinal = body
            .locals
            .iter()
            .enumerate()
            .filter(|(_, local)| local.storage == Storage::Capture)
            .position(|(index, _)| index == root.0 as usize)?;
        let inputs = self.scratch.definition_inputs(self.t, self.definition);
        let sites = &inputs.producers[body_id.0 as usize].captures;
        if sites.is_empty() {
            return None;
        }
        sites
            .iter()
            .map(|(parent, value)| {
                let Rvalue::Closure { captures, .. } =
                    &self.definition.bodies[parent.0 as usize].rvalues[value.0 as usize]
                else {
                    return None;
                };
                let capture = captures.get(ordinal)?;
                (capture.usage != UseKind::Mut).then_some((*parent, capture.place))
            })
            .collect()
    }

    pub(super) fn local_callable_bodies(
        &self,
        body: BodyId,
        place: PlaceId,
    ) -> Option<Vec<BodyId>> {
        let mut pending = vec![(body, place)];
        let mut seen = BTreeSet::new();
        let mut callees = vec![];
        while let Some((body_id, place)) = pending.pop() {
            let body = &self.definition.bodies[body_id.0 as usize];
            let access = &body.places[place.0 as usize];
            if !access.projection.is_empty() {
                return None;
            }
            if !seen.insert((body_id.0, access.root.0)) {
                continue;
            }
            let producers = self.scratch.variant_inputs[body_id.0 as usize]
                .get_or_init(|| super::variant_brands::Inputs::new(body))
                .roots[access.root.0 as usize]
                .clone();
            if body.locals[access.root.0 as usize].storage == Storage::Capture {
                pending.extend(self.readonly_capture_sources(body_id, access.root, &producers)?);
                continue;
            }
            if producers.is_empty() {
                return None;
            }
            for producer in producers {
                match producer {
                    super::variant_brands::Producer::Transfer(place) => {
                        pending.push((body_id, place))
                    }
                    super::variant_brands::Producer::Closure(body) => callees.push(body),
                    _ => return None,
                }
            }
        }
        (!callees.is_empty()).then_some(callees)
    }

    pub(super) fn brandless_variant_operand(
        &mut self,
        store: &TypeInterner,
        place: PlaceId,
        expected: TypeId,
    ) -> bool {
        self.brandless_variant_from(store, self.site.body, place, expected)
    }

    pub(super) fn brandless_variant_from(
        &mut self,
        store: &TypeInterner,
        body: BodyId,
        place: PlaceId,
        expected: TypeId,
    ) -> bool {
        let mut pending = vec![(body, place)];
        let mut seen = BTreeSet::new();
        let mut constructed = false;
        while let Some((body_id, place)) = pending.pop() {
            let body = &self.definition.bodies[body_id.0 as usize];
            let access = &body.places[place.0 as usize];
            if !access.projection.is_empty() {
                return false;
            }
            let root = access.root;
            if !seen.insert((body_id.0, root.0)) {
                continue;
            }
            let producers = self.scratch.variant_inputs[body_id.0 as usize]
                .get_or_init(|| super::variant_brands::Inputs::new(body))
                .roots[root.0 as usize]
                .clone();
            if producers.is_empty() {
                return false;
            }
            if body.locals[root.0 as usize].storage == Storage::Capture {
                let Some(sources) = self.readonly_capture_sources(body_id, root, &producers) else {
                    return false;
                };
                pending.extend(sources);
                continue;
            }
            for producer in producers {
                match producer {
                    super::variant_brands::Producer::Transfer(place) => {
                        pending.push((body_id, place))
                    }
                    super::variant_brands::Producer::Call { callable } => {
                        let Some(callees) = self.local_callable_bodies(body_id, callable) else {
                            return false;
                        };
                        for callee in callees {
                            let body = &self.definition.bodies[callee.0 as usize];
                            let mut returns = false;
                            for block in &body.blocks {
                                if let Terminator::Return { value, .. } = &block.terminator {
                                    pending.push((callee, value.place));
                                    returns = true;
                                }
                            }
                            if !returns {
                                return false;
                            }
                        }
                    }
                    super::variant_brands::Producer::Variant {
                        definition,
                        ordinal: variant,
                        fields,
                    } => {
                        if !matches!(&store.get(expected).unwrap().kind, TypeKind::Named { definition: owner, .. } if *owner.path() == definition)
                        {
                            return false;
                        }
                        let Some(TypeKind::Variant(variants)) = self.layout(store, expected) else {
                            return false;
                        };
                        let Some(selected) = variants.get(variant as usize) else {
                            return false;
                        };
                        if selected.fields.len() != fields {
                            return false;
                        }
                        let mut todo: Vec<_> = selected.fields.iter().map(|f| f.1).collect();
                        let mut types = HashSet::default();
                        while let Some(ty) = todo.pop() {
                            if !types.insert(ty) {
                                continue;
                            }
                            let row = store.get(ty).unwrap();
                            if !row.brand.alternatives.is_empty() {
                                return false;
                            }
                            todo.extend(row.kind.value_parts());
                        }
                        constructed = true;
                    }
                    _ => return false,
                }
            }
        }
        constructed
    }
    fn operand_value_type_in(
        &mut self,
        store: &TypeInterner,
        expected: TypeId,
        actual: TypeId,
        value: &Operand,
        entity: &'static str,
    ) {
        // Matching written contracts need no second proof in this shape
        // comparison. Executable transfers check current owned-part evidence.
        let (expected, actual) =
            matching_crossing_contracts(store, expected, actual, &mut HashMap::default());
        let actual = self.comparison_evidence(store, expected, actual);
        let mut receiving = store.get(expected).unwrap().clone();
        receiving.sendability_requirement = hirsel_types::SendabilityRequirement::None;
        if !matches!(
            receiving.kind,
            TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                }
        ) {
            // Executable mode replay checks current root evidence before
            // construction/extraction. Static typing checks the remaining shape.
            receiving.modes.sendability = Sendability::NotSendable;
        }
        let expected = store.intern(receiving);
        let actual = self.copied_type(store, value, actual);
        if self.received_type(store, expected, actual, value.usage) {
            return;
        }
        if store.get(expected).unwrap().brand == store.get(actual).unwrap().brand
            && matches!((&store.get(expected).unwrap().kind, &store.get(actual).unwrap().kind),
                (TypeKind::Named { definition: a, .. }, TypeKind::Named { definition: b, .. }) if a == b)
            && self.brandless_variant_operand(store, value.place, expected)
        {
            let erased_expected = hirsel_types::erase_brands(store, expected).unwrap_or(expected);
            let erased_actual = hirsel_types::erase_brands(store, actual).unwrap_or(actual);
            if self.received_type(store, erased_expected, erased_actual, value.usage) {
                return;
            }
        }
        // MODE-1: content is as global as what it currently holds. A row
        // local to an environment below the root of an owned value is received
        // by Global storage when flow proves the value's content Global, as
        // a call argument's is; typing records the debt, flow settles it.
        if let (Some(block), UseKind::Copy | UseKind::Take) = (self.site.block, value.usage) {
            // The root keeps its own reception rule; only rows below it
            // are acquired here.
            let acquired = self.acquired_global_type(store, actual);
            let mut acquired = store.get(acquired).unwrap().clone();
            acquired.modes = store.get(actual).unwrap().modes.clone();
            let acquired = store.intern(acquired);
            if !(self.t.semantic_eq(acquired, actual)
                || !self.t.valid(acquired)
                || !self.t.valid(actual))
                && self.received_type(store, expected, acquired, value.usage)
            {
                self.scratch.acquisitions[self.site.body.0 as usize]
                    .borrow_mut()
                    .entry((block.0, self.site.instruction))
                    .or_default()
                    .push((value.place, entity));
                return;
            }
        }
        self.error(ViolationKind::TypeMismatch { entity });
    }
    fn resolved_cleanup(&self, ty: TypeId) -> DropAllocation {
        let resolved = instantiate_cleanup_bounds(self.t, ty, self.generic_declarations())
            .unwrap_or_else(|error| self.proof_failure(error, ty));
        self.t.get(resolved).unwrap().drop_allocation.clone()
    }
    pub(super) fn received_type(
        &self,
        store: &TypeInterner,
        expected: TypeId,
        actual: TypeId,
        usage: UseKind,
    ) -> bool {
        self.related(
            store,
            expected,
            actual,
            TypeRelation::Value,
            if usage == UseKind::Take {
                Reception::View
            } else {
                Reception::Holder
            },
            &self.generic_declarations(),
        )
    }
    /// A whole slot of this body declared local without a placement holds a
    /// value whose placement is unknown: the join of values that live in
    /// different places. It accepts a scoped placement as a borrowed parameter
    /// does (PROV-4); flow then bounds that content by this frame. A
    /// constructor field is judged at its root.
    pub(super) fn scoped_into_unplaced(&self, destination: PlaceId, value: &Operand) -> bool {
        let place = &self.body.places[destination.0 as usize];
        self.unplaced_holder(destination)
            && self.unplaced_row(place.ty, self.place_type(value.place))
    }
    /// The same relation for a whole value, at its root or any part: an
    /// aggregate whose part joined differently placed content holds that part
    /// unplaced (PROV-4 at every depth).
    pub(super) fn scoped_parts_into_unplaced(&self, destination: PlaceId, value: &Operand) -> bool {
        let place = &self.body.places[destination.0 as usize];
        self.unplaced_holder(destination)
            && self.unplaced_parts(place.ty, self.place_type(value.place))
    }
    fn unplaced_row(&self, holder: TypeId, value: TypeId) -> bool {
        let types = self.t;
        matches!((types.get(holder), types.get(value)), (Some(h), Some(v))
            if !self.placed(&h.modes.relation)
                && self.lifetime(&h.modes.relation).has_environment()
                && self.placed(&v.modes.relation))
    }
    fn unplaced_parts(&self, holder: TypeId, value: TypeId) -> bool {
        if self.unplaced_row(holder, value) {
            return true;
        }
        let types = self.t;
        let (Some(h), Some(v)) = (types.get(holder), types.get(value)) else {
            return false;
        };
        let (hs, vs) = (h.kind.value_parts(), v.kind.value_parts());
        hs.len() == vs.len() && hs.iter().zip(&vs).any(|(h, v)| self.unplaced_parts(*h, *v))
    }
    /// A whole slot of this body: its unplaced local rows receive an owned
    /// value's content as the slot's root does (`hirsel_types::adopt_local`).
    fn unplaced_holder(&self, destination: PlaceId) -> bool {
        let place = &self.body.places[destination.0 as usize];
        place.projection.is_empty()
            && matches!(
                self.body.locals[place.root.0 as usize].storage,
                Storage::Local | Storage::Temporary
            )
    }
    fn storage_value_type_in(
        &mut self,
        store: &TypeInterner,
        expected: TypeId,
        actual: TypeId,
        value: &Operand,
        entity: &'static str,
    ) {
        let (expected, actual) =
            matching_crossing_contracts(store, expected, actual, &mut HashMap::default());
        let actual = self.comparison_evidence(store, expected, actual);
        let receiving = receiving_storage(store, expected);
        let mut receiving = store.get(receiving).unwrap().clone();
        let access = &self.body.places[value.place.0 as usize];
        let owner = store
            .get(self.body.locals[access.root.0 as usize].ty)
            .unwrap();
        let acquisition = if access.projection.is_empty() {
            owner.modes.relation.has_environment() && self.receiving_root(access.root)
        } else {
            self.content_proof(None, value.place).acquirable
                && self
                    .projection_type(store, access, |_, _| None)
                    .is_some_and(|physical| {
                        store
                            .get(physical)
                            .unwrap()
                            .modes
                            .relation
                            .has_environment()
                    })
        };
        if self.lifetime(&receiving.modes.relation).is_global()
            && store.get(actual).unwrap().modes.relation.has_environment()
            && matches!(value.usage, UseKind::Copy | UseKind::Take)
            && acquisition
        {
            // Flow independently proves current Global content.
            receiving.modes.relation =
                Relation::environment().union(&self.placement(&receiving.modes.relation));
        }
        let expected = store.intern(receiving);
        self.operand_value_type_in(store, expected, actual, value, entity);
    }
    /// A deep content proof permits acquisition through Global receiving storage.
    /// Traverse data payloads, never callable contracts or Shared contents.
    fn acquired_global_type(&self, store: &TypeInterner, root: TypeId) -> TypeId {
        fn visit(
            checker: &Checker<'_>,
            store: &TypeInterner,
            ty: TypeId,
            seen: &mut HashMap<TypeId, TypeId>,
            active: &mut HashSet<TypeId>,
        ) -> TypeId {
            if let Some(&result) = seen.get(&ty) {
                return result;
            }
            if !active.insert(ty) {
                return ty;
            }
            let mut row = store.get(ty).unwrap().clone();
            match &mut row.kind {
                TypeKind::Box(part) => *part = visit(checker, store, *part, seen, active),
                TypeKind::Tuple(parts) => {
                    *parts = parts
                        .iter()
                        .map(|p| visit(checker, store, *p, seen, active))
                        .collect::<Vec<_>>()
                        .into()
                }
                TypeKind::Record(fields) => {
                    *fields = fields
                        .iter()
                        .cloned()
                        .map(|mut f| {
                            f.ty = visit(checker, store, f.ty, seen, active);
                            f
                        })
                        .collect::<Vec<_>>()
                        .into()
                }
                TypeKind::Variant(variants) => {
                    *variants = variants
                        .iter()
                        .cloned()
                        .map(|mut v| {
                            v.fields = v
                                .fields
                                .iter()
                                .map(|(name, part)| {
                                    (name.clone(), visit(checker, store, *part, seen, active))
                                })
                                .collect::<Vec<_>>()
                                .into();
                            v
                        })
                        .collect::<Vec<_>>()
                        .into()
                }
                TypeKind::Named { arguments, .. }
                | TypeKind::Builtin {
                    kind:
                        BuiltinType::Array
                        | BuiltinType::List
                        | BuiltinType::Inline
                        | BuiltinType::Map
                        | BuiltinType::SortedMap
                        | BuiltinType::Set
                        | BuiltinType::Simd,
                    arguments,
                } => {
                    *arguments = arguments
                        .iter()
                        .map(|a| match a {
                            GenericArgument::Type(part) => {
                                GenericArgument::Type(visit(checker, store, *part, seen, active))
                            }
                            _ => a.clone(),
                        })
                        .collect::<Vec<_>>()
                        .into();
                }
                _ => (),
            }
            let relocalized = !checker.lifetime(&row.modes.relation).is_global();
            row.modes.relation = checker.placement(&row.modes.relation);
            let result = if row == *store.get(ty).unwrap() {
                ty
            } else {
                store.intern(row)
            };
            let result = if relocalized {
                checker.derive_sendability(store, result)
            } else {
                result
            };
            active.remove(&ty);
            seen.insert(ty, result);
            result
        }
        visit(
            self,
            store,
            root,
            &mut HashMap::default(),
            &mut HashSet::default(),
        )
    }

    pub(super) fn compatible(
        &self,
        store: &TypeInterner,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
    ) -> bool {
        self.compatible_with_context(
            store,
            expected,
            actual,
            relation,
            &self.generic_declarations(),
        )
    }
    fn compatible_with_context(
        &self,
        store: &TypeInterner,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
        context: &hirsel_types::GenericDeclarations,
    ) -> bool {
        self.related(
            store,
            expected,
            actual,
            relation,
            Reception::Holder,
            context,
        )
    }
    /// One root policy. A readonly argument or a whole taken value is a
    /// non-writing view; a live copied holder keeps nested uniqueness; a `mut`
    /// hand-back keeps every nested contract. Nested callable promises are
    /// subsumed by `hirsel_types` wherever the value is not handed back.
    fn related(
        &self,
        store: &TypeInterner,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
        reception: Reception,
        context: &hirsel_types::GenericDeclarations,
    ) -> bool {
        // A relation is a pure proof over this body's declarations: asked
        // once per body, its tooling limit reported at every site that asks.
        let memoized =
            std::ptr::eq(store, self.t) && std::ptr::eq(context, self.generic_declarations());
        let key = (expected, actual, relation, reception);
        let relations = &self.scratch.relations[self.site.body.0 as usize];
        let known = memoized
            .then(|| relations.borrow().get(&key).cloned())
            .flatten();
        let answer = match known {
            Some(answer) => answer,
            None => {
                let answer = self.relate(store, expected, actual, relation, reception, context);
                if memoized {
                    relations.borrow_mut().insert(key, answer.clone());
                }
                answer
            }
        };
        answer.unwrap_or_else(|error| self.proof_failure(error, false))
    }
    fn relate(
        &self,
        store: &TypeInterner,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
        reception: Reception,
        context: &hirsel_types::GenericDeclarations,
    ) -> Result<bool, hirsel_types::QueryError> {
        let authorities = self.scratch.parameter_authorities[self.site.body.0 as usize]
            .get_or_init(|| {
                let mut authorities = BTreeMap::new();
                for body in &self.definition.bodies {
                    for (ordinal, local) in body.parameters.iter().enumerate() {
                        let Some(local) = body.locals.get(local.0 as usize) else {
                            return None;
                        };
                        let authority = self.authority_with_generics(
                            self.t,
                            local.ty,
                            &self.generic_declarations(),
                        );
                        let reference =
                            ParameterRef::body(body.parameter_scope, ParamId(ordinal as u32));
                        if let Some(previous) = authorities.insert(reference, authority.clone()) {
                            if previous != authority {
                                return None;
                            }
                        }
                    }
                }
                Some(authorities)
            });
        let Some(authorities) = authorities else {
            return Ok(false);
        };
        let Ok(expected) = instantiate_parameter_authorities(store, expected, authorities) else {
            return Ok(false);
        };
        let Ok(actual) = instantiate_parameter_authorities(store, actual, authorities) else {
            return Ok(false);
        };
        let mut lookup = |path: DefId| {
            Ok::<_, hirsel_types::QueryError>(
                self.signatures.get(path).cloned().map(std::sync::Arc::new),
            )
        };
        // A callback may retain a conditional cleanup row from its signature.
        // Resolve only bounds of the exact declaring binder before comparing
        // owned receipts; no receiving row supplies a cleanup proof (GEN-1/MEM-5).
        let expected = instantiate_cleanup_bounds(store, expected, context)?;
        let actual = instantiate_cleanup_bounds(store, actual, context)?;
        let Some(expected) = self.comparison_authorities(store, expected) else {
            return Ok(false);
        };
        let Some(actual) = self.comparison_authorities(store, actual) else {
            return Ok(false);
        };
        let expected = self.canonical_plain(store, expected);
        let actual = self.canonical_plain(store, actual);
        let mut memo = hirsel_types::ContextMemo::new(store, context, &mut lookup);
        match reception {
            Reception::View => memo.view(expected, actual, relation),
            Reception::Holder => memo.compatible_type(expected, actual, relation),
            Reception::HandBack => memo.handback(expected, actual),
        }
    }
    pub(super) fn comparison_evidence(
        &self,
        store: &TypeInterner,
        expected: TypeId,
        actual: TypeId,
    ) -> TypeId {
        conservative_result_evidence(
            store,
            expected,
            actual,
            &mut HashMap::default(),
            &mut |store, root| {
                self.proof(
                    store,
                    root,
                    super::proofs::Proof::AfterSharing,
                    &self.generic_declarations(),
                )
                .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable))
                    == Sendability::Sendable
            },
        )
    }
    pub(super) fn derive_sendability(&self, store: &TypeInterner, root: TypeId) -> TypeId {
        let mut row = store.get(root).unwrap().clone();
        row.modes.sendability =
            structural_sendability(store, root, &self.generic_declarations(), &mut |path| {
                Ok::<_, hirsel_types::QueryError>(
                    self.signatures.get(path).cloned().map(std::sync::Arc::new),
                )
            })
            .unwrap_or_else(|error| {
                self.proof_failure(error, hirsel_types::Sendability::NotSendable)
            });
        store.intern(row)
    }
    fn boolean(&mut self, o: &Operand, entity: &'static str) {
        if !matches!(
            self.t.get(self.place_type(o.place)).unwrap().kind,
            TypeKind::Primitive(Primitive::Bool)
        ) {
            self.error(ViolationKind::TypeMismatch { entity });
        }
    }
    fn authorities(&self, arguments: &[GenericArgument]) -> Vec<AuthoritySummary> {
        arguments
            .iter()
            .map(|a| match a {
                GenericArgument::Type(t) => self.authority(*t),
                _ => AuthoritySummary::Unknown,
            })
            .collect()
    }
    pub(super) fn authority(&self, t: TypeId) -> AuthoritySummary {
        self.authority_in(self.t, t)
    }
    fn authority_in(&self, store: &TypeInterner, t: TypeId) -> AuthoritySummary {
        self.authority_with_generics(store, t, &self.generic_declarations())
    }
    pub(super) fn authority_with_generics(
        &self,
        store: &TypeInterner,
        t: TypeId,
        generics: &hirsel_types::GenericDeclarations,
    ) -> AuthoritySummary {
        let mut roles = self
            .signatures
            .get(self.definition.path)
            .map(|s| s.metadata.roles.clone())
            .unwrap_or_default();
        // Canonical heap identity can also be supplied by a referenced snapshot.
        // This preserves the independent validator's typed-heap fixture boundary.
        if roles.heap.is_none() {
            roles.heap = self.signatures.first_heap();
        }
        structural_authority(store, t, &roles, generics, &mut |_, path| {
            Ok::<_, hirsel_types::QueryError>(
                path.and_then(|p| self.signatures.get(p))
                    .cloned()
                    .map(std::sync::Arc::new),
            )
        })
        .unwrap()
    }
    fn imported(&self, root: TypeId, arguments: &[GenericArgument]) -> Option<TypeId> {
        self.imported_in(root, arguments, &self.generic_declarations())
    }
    pub(super) fn imported_in(
        &self,
        root: TypeId,
        arguments: &[GenericArgument],
        generics: &GenericDeclarations,
    ) -> Option<TypeId> {
        if arguments.is_empty() {
            Some(root)
        } else {
            hirsel_types::substitute(
                self.t,
                self.memo,
                root,
                &Substitution {
                    binder: Some(hirsel_types::GenericBinder::Bound(0)),
                    roles: &self
                        .signatures
                        .get(self.definition.path)
                        .map(|s| s.metadata.roles.clone())
                        .unwrap_or_default(),
                    arguments,
                    authorities: &self.authorities(arguments),
                },
            )
            .ok()
            .map(|imported| {
                if self.t.semantic_eq(root, imported) {
                    imported
                } else {
                    hirsel_types::canonical_current_type(self.t, imported, generics, &mut |path| {
                        Ok::<_, hirsel_types::QueryError>(
                            self.signatures.get(path).cloned().map(std::sync::Arc::new),
                        )
                    })
                    .unwrap_or_else(|error| self.proof_failure(error, imported))
                }
            })
        }
    }
    fn imported_in_place(
        &self,
        target: &TypeInterner,
        root: TypeId,
        arguments: &[GenericArgument],
        generics: &GenericDeclarations,
    ) -> Option<TypeId> {
        let roles = self
            .signatures
            .get(self.definition.path)
            .map(|s| s.metadata.roles.clone())
            .unwrap_or_default();
        let imported = hirsel_types::substitute(
            self.t,
            self.memo,
            root,
            &Substitution {
                binder: (!arguments.is_empty()).then_some(GenericBinder::Bound(0)),
                roles: &roles,
                arguments,
                authorities: &self.authorities(arguments),
            },
        )
        .ok()?;
        if arguments.is_empty() || self.t.semantic_eq(root, imported) {
            Some(imported)
        } else {
            Some(
                hirsel_types::canonical_current_type(target, imported, generics, &mut |path| {
                    Ok::<_, hirsel_types::QueryError>(
                        self.signatures.get(path).cloned().map(std::sync::Arc::new),
                    )
                })
                .unwrap_or_else(|error| self.proof_failure(error, imported)),
            )
        }
    }

    fn cross_type(
        &mut self,
        _source: &TypeInterner,
        root: TypeId,
        actual: TypeId,
        args: &[GenericArgument],
        entity: &'static str,
    ) {
        let target = self.t;
        let matches = if let Some(expected) = self.imported(root, args) {
            let expected = if matches!(entity, "body parameter" | "body result") {
                match bind_parameter_scopes(target, expected, &[self.body.parameter_scope]) {
                    Ok(t) => t,
                    Err(_) => {
                        self.error(ViolationKind::TypeMismatch { entity });
                        return;
                    }
                }
            } else {
                expected
            };
            let actual = if matches!(entity, "body parameter" | "body result") {
                let mut row = target.get(actual).unwrap().clone();
                let declared = target.get(expected).unwrap();
                if !matches!(
                    row.kind,
                    TypeKind::Function(_)
                        | TypeKind::Builtin {
                            kind: BuiltinType::Iter,
                            ..
                        }
                ) && row.sendability_requirement == declared.sendability_requirement
                {
                    row.modes.sendability = declared.modes.sendability.clone();
                }
                target.intern(row)
            } else {
                actual
            };
            self.compatible(target, expected, actual, TypeRelation::Exact)
        } else {
            false
        };
        if !matches {
            self.error(ViolationKind::TypeMismatch { entity });
        }
    }

    /// A read-only capture of a whole replaced slot whose storage is
    /// declared Local: only the parent's flow can prove that its current
    /// content is global, which the environment specialisation needs.
    pub(super) fn current_content_source(&self, operand: &Operand) -> bool {
        let access = &self.body.places[operand.place.0 as usize];
        let types = self.t;
        operand.usage == UseKind::Borrow
            && access.projection.is_empty()
            && self.receiving_root(access.root)
            && types
                .get(access.ty)
                .unwrap()
                .modes
                .relation
                .has_environment()
            && self
                .lifetime(
                    &types
                        .get(self.body.locals[access.root.0 as usize].ty)
                        .unwrap()
                        .modes
                        .relation,
                )
                .has_environment()
    }
    /// CLO-1: a local body result is local to the environment, which creation
    /// binds as a call binds an `in x` result's arguments. The closure's
    /// type may promise a global result when the body's result can depend on
    /// the environment alone (no borrowed non-plain parameter), some capture
    /// is borrowed and every captured source is global; a source whose global
    /// content only flow can prove is checked there.
    pub(super) fn environment_specialised(
        &self,
        ty: TypeId,
        body: BodyId,
        captures: &[Operand],
    ) -> bool {
        let types = self.t;
        let TypeKind::Function(f) = &types.get(ty).unwrap().kind else {
            return false;
        };
        let nested = &self.definition.bodies[body.0 as usize];
        types
            .get(nested.result)
            .unwrap()
            .modes
            .relation
            .has_environment()
            && self
                .lifetime(&types.get(f.result).unwrap().modes.relation)
                .is_global()
            && captures
                .iter()
                .any(|c| matches!(c.usage, UseKind::Borrow | UseKind::Mut))
            && captures.iter().all(|c| {
                hirsel_types::structurally_global(types, self.place_type(c.place), &|o| {
                    self.arena_place(o)
                }) || self.current_content_source(c)
            })
            && f.parameters.iter().all(|p| {
                p.passing == PassingMode::Take
                    || self.bound(types, p.ty, Bound::Plain, self.generic_declarations())
            })
    }
    /// A whole take captures current content, whose root evidence can differ
    /// from the storage row after sharing or replacement. Static typing checks
    /// shape without these facts; flow derives them from the operand and
    /// checks the original capture claim. Nested and dependent rows stay exact.
    fn taken_capture_shape(
        &self,
        store: &TypeInterner,
        expected: TypeId,
        actual: TypeId,
        operand: &Operand,
    ) -> (TypeId, TypeId) {
        if operand.usage != UseKind::Take
            || !self.body.places[operand.place.0 as usize]
                .projection
                .is_empty()
        {
            return (expected, actual);
        }
        let mut expected = store.get(expected).unwrap().clone();
        let mut actual = store.get(actual).unwrap().clone();
        expected.modes.uniqueness = Uniqueness::Aliased;
        actual.modes.uniqueness = Uniqueness::Aliased;
        if !matches!(expected.modes.sendability, Sendability::DependsOn(_))
            && !matches!(
                actual.kind,
                TypeKind::Function(_)
                    | TypeKind::Builtin {
                        kind: BuiltinType::Iter,
                        ..
                    }
            )
        {
            expected.modes.sendability = Sendability::NotSendable;
            actual.modes.sendability = Sendability::NotSendable;
        }
        (store.intern(expected), store.intern(actual))
    }
    fn capture_type(&mut self, _source: &TypeInterner, root: TypeId, operand: &Operand) {
        let actual = self.place_type(operand.place);
        if matches!(operand.usage, UseKind::Borrow | UseKind::Mut) {
            let store = self.t;
            if let Some(expected) = self.imported(root, &[]) {
                self.storage_value_type_in(store, expected, actual, operand, "closure capture");
            } else {
                self.error(ViolationKind::TypeMismatch {
                    entity: "closure capture",
                });
            }
            return;
        }
        if operand.usage != UseKind::Copy {
            let target = self.t;
            let matches = self.imported(root, &[]).is_some_and(|expected| {
                let (expected, actual) =
                    self.taken_capture_shape(target, expected, actual, operand);
                self.compatible(target, expected, actual, TypeRelation::Exact)
            });
            if !matches {
                self.error(ViolationKind::TypeMismatch {
                    entity: "closure capture",
                });
            }
            return;
        }
        let target = self.t;
        let matches = self.imported(root, &[]).is_some_and(|expected| {
            // A flat copy owns independent bytes. Counted copies share
            // storage, so derive both uniqueness and sendability before
            // comparing the independently declared capture type.
            let actual = if self.copy_detaches(&target, actual) {
                let mut row = target.get(actual).unwrap().clone();
                row.modes.relation = Relation::global();
                row.modes.sendability = target.get(expected).unwrap().modes.sendability.clone();
                target.intern(row)
            } else {
                actual
            };
            let adjusted = if self.flat_value(operand.place) {
                let mut row = target.get(actual).unwrap().clone();
                row.modes.uniqueness = target.get(expected).unwrap().modes.uniqueness;
                target.intern(row)
            } else {
                self.weakened_type(target, actual, &self.generic_declarations())
            };
            let mut row = target.get(adjusted).unwrap().clone();
            row.sendability_requirement = hirsel_types::SendabilityRequirement::None;
            let adjusted = target.intern(row);
            let valid = self.compatible(target, expected, adjusted, TypeRelation::Exact);
            valid
        });
        if !matches {
            self.error(ViolationKind::TypeMismatch {
                entity: "closure capture",
            });
        }
    }

    fn parameter_type(
        &mut self,
        root: TypeId,
        actual: TypeId,
        args: &[GenericArgument],
        authorities: &BTreeMap<ParameterRef, AuthoritySummary>,
        captures: &BTreeMap<RelationPlace, CaptureProperties>,
        allocation_targets: &BTreeMap<ParameterRef, AllocationTarget>,
        origins: &super::flow_origins::CallOriginProof,
        actuals: &BTreeMap<ParameterRef, TypeId>,
        value: &Operand,
        preserves_state: bool,
        seal_payload: bool,
    ) {
        let place = value.place;
        let target = self.t;
        let expected = Some(root)
            .and_then(|t| instantiate_allocation_targets(target, t, allocation_targets).ok())
            .and_then(|t| super::flow_origins::sources(target, t, origins, actuals).ok())
            .and_then(|t| instantiate_parameter_authorities(target, t, authorities).ok())
            .and_then(|t| instantiate_parameter_captures(target, t, captures).ok())
            .and_then(|t| {
                super::flow_origins::canonical(target, t, &origins.names, &|o| self.arena_place(o))
                    .ok()
            });
        let expected = expected
            .and_then(|t| self.imported_in_place(target, t, args, &self.generic_declarations()));
        let actual = origins.proofs.get(&place.0).and_then(|proof| {
            super::flow_origins::canonical(target, actual, proof, &|o| self.arena_place(o)).ok()
        });
        let (Some(expected), Some(actual)) = (expected, actual) else {
            self.error(ViolationKind::InvalidOrigin(place));
            return;
        };
        let declarations = self.generic_declarations();
        let expected = instantiate_cleanup_bounds(target, expected, &declarations)
            .unwrap_or_else(|error| self.proof_failure(error, expected));
        let actual = instantiate_cleanup_bounds(target, actual, &declarations)
            .unwrap_or_else(|error| self.proof_failure(error, actual));
        // An unrelated parameter's Environment is access to the argument,
        // not placement of its owned payload. Match that access root before
        // hand-back comparison propagates structural placement into parts.
        // Every nested contract (including unique and brands) stays intact.
        let expected = if value.usage == UseKind::Mut
            && target
                .get(expected)
                .unwrap()
                .modes
                .relation
                .is_environment()
            && matches!(
                target.get(expected).unwrap().kind,
                TypeKind::Box(_) | TypeKind::Tuple(_) | TypeKind::Record(_) | TypeKind::Variant(_)
            ) {
            let mut row = target.get(expected).unwrap().clone();
            row.modes.relation = target.get(actual).unwrap().modes.relation.clone();
            target.intern(row)
        } else {
            expected
        };
        let mut row = target.get(actual).unwrap().clone();
        if !matches!(
            row.kind,
            TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                }
        ) {
            row.modes.sendability = if origins.current_sendable.contains(&place.0) {
                Sendability::Sendable
            } else {
                Sendability::NotSendable
            };
        }
        let actual = target.intern(row);
        let actual = self.copied_type(target, value, actual);
        let actual = if origins.current_unique.contains(&place.0)
            && target.get(actual).unwrap().modes.uniqueness != Uniqueness::Unique
        {
            self.unique_type(target, actual)
        } else {
            actual
        };
        let actual = if origins.weakened.contains(&place.0) {
            self.weakened_type(target, actual, &self.generic_declarations())
        } else {
            actual
        };
        let actual = if origins.currently_sendable(value, target.get(actual).unwrap()) {
            let mut row = target.get(actual).unwrap().clone();
            row.modes.sendability = Sendability::Sendable;
            target.intern(row)
        } else {
            actual
        };
        // Declaration-owned preservation quantifies over the actual incoming
        // environment; a narrowed formal never discards stronger actual facts.
        let incoming_valid = !preserves_state
            || self.related(
                target,
                expected,
                actual,
                TypeRelation::Parameter,
                Reception::View,
                &self.generic_declarations(),
            );
        if value.usage == UseKind::Mut
            && !preserves_state
            && let Some(bound) = self.carrier_bound(place)
            && !self.authority_in(&target, expected).is_subset_of(&bound)
        {
            self.error(ViolationKind::TypeMismatch {
                entity: "erased carrier hand-back",
            });
        }
        let expected = if preserves_state
            && value.usage == UseKind::Mut
            && matches!(
                target.get(actual).unwrap().kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                } | TypeKind::Named { .. }
            ) {
            // An erased carrier's cleanup is its declaration's; only its
            // evidence is per value.
            let incoming = target.get(actual).unwrap();
            let mut row = target.get(expected).unwrap().clone();
            row.erased_captures = incoming.erased_captures.clone();
            if !matches!(row.kind, TypeKind::Named { .. }) {
                row.drop_allocation = incoming.drop_allocation.clone();
            }
            target.intern(row)
        } else {
            expected
        };
        // PROV-4: a taken value is the callee's to keep, so an unplaced take
        // parameter cannot receive an arena value; a borrow cannot keep it.
        let expected = if seal_payload {
            // MEM-4 is a storage transfer. seal_flow independently proves the
            // actual grant, complete closure and absence of other live views.
            let mut row = target.row(expected).clone();
            row.modes.relation = row
                .modes
                .relation
                .union(&self.placement(&target.row(actual).modes.relation));
            target.intern(row)
        } else {
            expected
        };
        let laundered = value.usage == UseKind::Take
            && !self.placed(&target.get(expected).unwrap().modes.relation)
            && self.placed(&target.get(actual).unwrap().modes.relation);
        // A borrow or a take cannot write the caller's storage back, so the
        // callee may be ignorant of brands nested in a data value. A `mut`
        // argument is handed back and keeps the exact relation.
        let (reception, relation) = match value.usage {
            UseKind::Mut => (Reception::HandBack, TypeRelation::Parameter),
            UseKind::Take => (Reception::View, TypeRelation::Value),
            _ => (Reception::View, TypeRelation::Parameter),
        };
        // Generic actuals retain their physical nested contracts. A stronger
        // current payload is acquired only when this parameter needs it;
        // eagerly strengthening an already matching T changes its identity.
        let mut matches = incoming_valid
            && self.related(
                target,
                expected,
                actual,
                relation,
                reception,
                &self.generic_declarations(),
            );
        if !matches && incoming_valid && origins.acquired_global.contains(&place.0) {
            let acquired = self.acquired_global_type(target, actual);
            matches = self.related(
                target,
                expected,
                acquired,
                relation,
                reception,
                &self.generic_declarations(),
            );
            if !matches && value.usage == UseKind::Take {
                let receiving = self.acquired_global_type(target, expected);
                matches = self.related(
                    target,
                    receiving,
                    acquired,
                    relation,
                    reception,
                    &self.generic_declarations(),
                );
            }
        }
        if laundered || !matches {
            self.error(ViolationKind::TypeMismatch {
                entity: "call argument",
            });
        }
    }

    /// Expand only the required nominal root, importing all representation edges.
    /// The structural kind `ty` is laid out as, through nominal representations.
    /// A layout is found once per body: only `None` reports anything, so a
    /// found one is memoized and an absent one recomputed, reporting again.
    pub(super) fn layout(&mut self, store: &TypeInterner, ty: TypeId) -> Option<TypeKind> {
        if !std::ptr::eq(store, self.t) {
            return self.find_layout(store, ty);
        }
        let layouts = &self.scratch.layouts[self.site.body.0 as usize];
        if let Some(kind) = layouts.borrow().get(&ty) {
            return Some(kind.clone());
        }
        let kind = self.find_layout(store, ty)?;
        layouts.borrow_mut().insert(ty, kind.clone());
        Some(kind)
    }
    fn find_layout(&mut self, store: &TypeInterner, ty: TypeId) -> Option<TypeKind> {
        let mut ty = ty;
        let mut expansions = hirsel_types::NominalExpansions::default();
        let mut packed = false;
        loop {
            let kind = store.get(ty)?.kind.clone();
            if let TypeKind::Builtin {
                kind: BuiltinType::Packed,
                arguments,
            } = &kind
            {
                let [GenericArgument::Type(payload)] = arguments.as_slice() else {
                    self.error(ViolationKind::InvalidTypeGraph);
                    return None;
                };
                if packed {
                    self.error(ViolationKind::InvalidConstructor);
                    return None;
                }
                packed = true;
                ty = *payload;
                continue;
            }
            let TypeKind::Named {
                definition,
                arguments,
            } = kind
            else {
                if packed && !matches!(kind, TypeKind::Variant(_)) {
                    self.error(ViolationKind::InvalidConstructor);
                    return None;
                }
                return Some(kind);
            };
            if !expansions
                .enter(ty, 0, store)
                .unwrap_or_else(|error| self.proof_failure(error, false))
            {
                if self.proof_errors.borrow().is_empty() {
                    self.error(ViolationKind::InvalidTypeGraph);
                }
                return None;
            }
            let Some(s) = self.signatures.get(definition) else {
                self.error(ViolationKind::MissingSignature(definition.path().clone()));
                return None;
            };
            let SignatureKind::Type(t) = &s.kind else {
                self.error(ViolationKind::InvalidSignature(definition.path().clone()));
                return None;
            };
            if (t.capability.is_some()
                || s.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque)
                && s.definition.path().0.package != self.definition.path.path().0.package
            {
                // A forged input can retain an owner's private graph. Its mere
                // presence does not authorize a client to inspect it (PROG-2).
                return None;
            }
            let Some(rep) = t.representation else {
                self.error(ViolationKind::InvalidConstructor);
                return None;
            };
            ty = if arguments.is_empty() {
                rep
            } else {
                hirsel_types::substitute(
                    self.t,
                    self.memo,
                    rep,
                    &Substitution {
                        binder: Some(hirsel_types::GenericBinder::Bound(0)),
                        roles: &s.metadata.roles,
                        arguments: &arguments,
                        authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                    },
                )
                .ok()?
            };
            // Normalize the declaration-backed expectation after substitution,
            // including Plain proofs from the actual's lexical generic binder.
            // Keep current TIR rows intact so comparison still rejects stale
            // call-locality binders in aggregate fields.
            ty = canonical_plain_type_in(store, ty, self.generic_declarations(), &mut |path| {
                Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new))
            })
            .ok()?;
        }
    }

    pub(super) fn call_type(
        &mut self,
        callee: &Callee,
        arguments: &[CallArgument],
        destination: Option<PlaceId>,
        origins: Option<&super::flow_origins::CallOriginProof>,
    ) {
        if matches!(callee, Callee::Definition { path, .. } if path.is_core("cpu.map_scoped") || (path.0.package.package_is("std", "1") && path.0.modules == ["cpu"] && path.0.name == "map_scoped"))
        {
            self.cpu_call(arguments);
        }
        if matches!(callee, Callee::Definition { path, .. } if path.is_core("arena.seal")) {
            self.seal_type(arguments);
        }
        let (store, f, generics) = match callee {
            Callee::Definition { path, arguments } => {
                self.table_key_arguments(path, arguments);
                let Some(s) = self.signatures.get(path) else {
                    self.error(ViolationKind::MissingSignature(path.clone()));
                    return;
                };
                if hirsel_types::schema_failure(
                    self.t,
                    arguments,
                    &s.metadata.schema,
                    |ty, bound| self.bound(self.t, ty, bound, &self.generic_declarations()),
                )
                .is_some()
                {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "intrinsic bound",
                    });
                    return;
                }
                let SignatureKind::Function(f) = &s.kind else {
                    self.error(ViolationKind::InvalidSignature(path.clone()));
                    return;
                };
                (self.t, f, arguments.as_slice())
            }
            Callee::Indirect {
                forward_caller,
                value: o,
                arguments: generics,
            } => {
                // Only generated contract invocations are transparent function-value calls.
                // Reconstruct the target from all possible producers, including overwrites.
                let clause = !self.definition.contracts.is_empty()
                    && self
                        .local_callable_bodies(self.site.body, o.place)
                        .is_some_and(|bodies| {
                            bodies.iter().all(|body| {
                                self.definition
                                    .contracts
                                    .iter()
                                    .any(|(_, clause)| clause == body)
                            })
                        });
                if *forward_caller != clause {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "caller forwarding",
                    });
                }
                let row = self.t.get(self.place_type(o.place)).unwrap();
                if row.modes.multiplicity == Multiplicity::Once && o.usage != UseKind::Take {
                    self.error(ViolationKind::InvalidUse(o.place));
                }
                match &row.kind {
                    TypeKind::Function(f) => (self.t, f, generics.as_slice()),
                    _ => {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "indirect callee",
                        });
                        return;
                    }
                }
            }
        };
        let mut function = f.clone();
        function.generics = local_result_generics(store, &function).into();
        let f = &function;
        if f.generics.len() != generics.len() {
            self.error(ViolationKind::Arity {
                entity: "generic arguments",
                expected: f.generics.len(),
                actual: generics.len(),
            });
            return;
        }
        for (p, a) in f.generics.iter().zip(generics) {
            if let GenericArgument::Type(t) = a {
                if !generic_references_valid(self.t, *t, &self.generic_declarations()) {
                    self.error(ViolationKind::InvalidTypeGraph);
                }
            }
            if let GenericArgument::Size(Size::Parameter(parameter)) = a {
                self.size_parameter(*parameter);
            }
            if !matches!(
                (p, a),
                (GenericParameter::Type { .. }, GenericArgument::Type(_))
                    | (GenericParameter::Size { .. }, GenericArgument::Size(_))
            ) {
                self.error(ViolationKind::TypeMismatch {
                    entity: "generic argument kind",
                });
            }
        }
        if origins.is_none() {
            // A call diverges exactly when its instantiated callee result is
            // Never: no result slot and no normal successor (TIR v3).
            let target = self.t;
            let never = Some(f.result)
                .and_then(|t| {
                    self.imported_in_place(target, t, generics, &self.generic_declarations())
                })
                .is_some_and(|t| {
                    matches!(
                        target.get(t).unwrap().kind,
                        TypeKind::Primitive(Primitive::Never)
                    )
                });
            match (never, destination) {
                (true, Some(_)) => self.error(ViolationKind::TypeMismatch {
                    entity: "returning call to Never callee",
                }),
                (false, None) => self.error(ViolationKind::TypeMismatch {
                    entity: "diverging call to returning callee",
                }),
                _ => (),
            }
        }
        if let Some(origins) = origins {
            self.call_bounds(store, f, generics, arguments, Some(origins));
        }
        let actuals: BTreeMap<_, _> = arguments
            .iter()
            .map(|a| {
                (
                    ParameterRef::bound(a.parameter),
                    self.place_type(a.value.place),
                )
            })
            .collect();
        let authorities: BTreeMap<_, _> = arguments
            .iter()
            .map(|a| {
                (
                    ParameterRef::bound(a.parameter),
                    self.authority(self.place_type(a.value.place)),
                )
            })
            .collect();
        if self.contract {
            let parameters: Vec<_> = (0..f.parameters.len())
                .map(|i| {
                    authorities
                        .get(&ParameterRef::bound(ParamId(i as u32)))
                        .cloned()
                        .unwrap_or(AuthoritySummary::Unknown)
                })
                .collect();
            let generic_authorities: Vec<_> = generics
                .iter()
                .map(|g| match g {
                    GenericArgument::Type(t) => self.authority(*t),
                    _ => AuthoritySummary::inert(),
                })
                .collect();
            let required = match &f.authority {
                AuthoritySummary::Unknown => AuthoritySummary::Unknown,
                AuthoritySummary::Known(terms) => {
                    terms.iter().fold(AuthoritySummary::inert(), |a, term| {
                        let actual = match term {
                            AuthorityTerm::Parameter(ParameterRef::Bound {
                                depth: 0,
                                parameter,
                            }) => parameters
                                .get(parameter.0 as usize)
                                .cloned()
                                .unwrap_or(AuthoritySummary::Unknown),
                            AuthorityTerm::TypeParameter(TypeParameterRef::Bound {
                                depth: 0,
                                parameter,
                            }) => generic_authorities
                                .get(parameter.0 as usize)
                                .cloned()
                                .unwrap_or(AuthoritySummary::Unknown),
                            term => AuthoritySummary::Known(vec![term.clone()].into()),
                        };
                        a.join(actual)
                    })
                }
            };
            if required != AuthoritySummary::inert() {
                self.error(ViolationKind::TypeMismatch {
                    entity: "contract call authority",
                });
            }
        }
        self.invocation_effects(callee, f, generics, arguments);
        // A retained argument's crossing evidence is its flow-proven current
        // content, as for its parameter receipt, not only what its declared
        // parts prove (MODE-4: fresh aggregate evidence; receipt carries it).
        let mut captures: BTreeMap<_, _> = arguments
            .iter()
            .map(|a| {
                let t = self.place_type(a.value.place);
                let types = self.t;
                // Flow checks the occurrence independently. Retaining an
                // opaque carrier must not erase that checked crossing proof.
                let derived =
                    if self.bound(&types, t, Bound::Sendable, &self.generic_declarations()) {
                        t
                    } else {
                        self.derive_sendability(types, t)
                    };
                let row = types.get(derived).unwrap();
                let sendability = if origins.is_some_and(|o| o.currently_sendable(&a.value, row)) {
                    Sendability::Sendable
                } else {
                    row.modes.sendability.clone()
                };
                (
                    ParameterRef::bound(a.parameter).into(),
                    CaptureProperties::from_type(row, self.authority(t), sendability),
                )
            })
            .collect();
        for (state, callback, slot) in mutable_capture_states(store, f) {
            if let Some(argument) = arguments
                .iter()
                .find(|a| a.parameter.0 as usize == callback)
            {
                let proof = mutable_capture_state_sendability(
                    self.t,
                    self.place_type(argument.value.place),
                    slot,
                    &self.generic_declarations(),
                    &mut |path| Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new)),
                )
                .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
                if let Some(capture) = captures.get_mut(&hirsel_types::RelationPlace {
                    parameter: ParameterRef::bound(ParamId(state as u32)),
                    fields: vec![].into(),
                }) {
                    capture.sendability =
                        combine_crossing_evidence(capture.sendability.clone(), proof);
                }
            }
        }
        let allocation_targets: BTreeMap<_, _> = arguments
            .iter()
            .filter_map(|a| {
                self.t
                    .get(self.place_type(a.value.place))
                    .unwrap()
                    .allocation_target
                    .map(|t| (ParameterRef::bound(a.parameter), t))
            })
            .collect();
        let mut ordinals = BTreeSet::new();
        if arguments.len() != f.parameters.len() {
            self.error(ViolationKind::Arity {
                entity: "call arguments",
                expected: f.parameters.len(),
                actual: arguments.len(),
            });
        }
        for a in arguments {
            if !ordinals.insert(a.parameter.0) {
                self.error(ViolationKind::Duplicate {
                    entity: "call parameter",
                    index: a.parameter.0,
                });
            }
            let Some(p) = f.parameters.get(a.parameter.0 as usize) else {
                self.error(ViolationKind::InvalidId {
                    arena: "call parameter",
                    index: a.parameter.0,
                });
                continue;
            };
            if p.passing == PassingMode::Take
                && self.arena_type(p.ty)
                && self
                    .t
                    .row(self.place_type(a.value.place))
                    .allocation_target
                    .is_some()
            {
                self.error(ViolationKind::InvalidUse(a.value.place));
            }
            if let Some(origins) = origins {
                self.parameter_type(
                    p.ty,
                    self.place_type(a.value.place),
                    generics,
                    &authorities,
                    &captures,
                    &allocation_targets,
                    origins,
                    &actuals,
                    &a.value,
                    f.state_promises.iter().any(|promise| {
                        promise.selector.parameter == ParameterRef::bound(a.parameter)
                            && promise.selector.fields.is_empty()
                            && promise.preserve_captures
                    }),
                    matches!(callee, Callee::Definition { path, .. } if path.is_core("arena.seal"))
                        && a.parameter.0 == 1,
                );
            }
            let plain_take_borrow = p.passing == PassingMode::Borrow
                && a.value.usage == UseKind::Take
                && !super::flow::needs_release(self, self.place_type(a.value.place), self.t);
            if !plain_take_borrow
                && !matches!(
                    (p.passing, a.value.usage),
                    (PassingMode::Take, UseKind::Take)
                        | (PassingMode::Mut, UseKind::Mut)
                        | (PassingMode::Borrow, UseKind::Borrow | UseKind::Copy)
                )
            {
                self.error(ViolationKind::TypeMismatch {
                    entity: "argument use",
                });
            }
        }
        let (Some(origins), Some(destination)) = (origins, destination) else {
            return;
        };
        let target = self.t;
        // Substitute the value's type contract before its dependent source
        // evidence. A generic actual can carry Local access modes; those
        // cannot overwrite the lifetime supplied by local(x).
        let imported_result = Some(f.result).and_then(|t| {
            self.imported_in_place(target, t, generics, &self.generic_declarations())
        });
        let actual_result = self.place_type(destination);
        let inactive = imported_result.is_some_and(|expected| {
            target.get(expected).unwrap().brand.alternatives.is_empty()
                && target.get(actual_result).unwrap().brand.alternatives.is_empty()
                && matches!((&target.get(expected).unwrap().kind, &target.get(actual_result).unwrap().kind),
                    (TypeKind::Named { definition: a, .. }, TypeKind::Named { definition: b, .. }) if a == b)
                && self.brandless_variant_operand(target, destination, expected)
                && self.brandless_variant_operand(target, destination, actual_result)
        });
        // An absent payload needs no owner-name replay for its phantom brand.
        // Both declaration and receiving contracts must independently prove it;
        // active unbranded payloads cannot acquire a receiving brand here.
        let expected = imported_result
            .and_then(|t| {
                if inactive {
                    hirsel_types::erase_brands(target, t).ok()
                } else {
                    Some(t)
                }
            })
            .and_then(|t| instantiate_allocation_targets(target, t, &allocation_targets).ok())
            .map(|t| {
                hirsel_types::canonical_current_type(
                    target,
                    t,
                    &self.generic_declarations(),
                    &mut |path| Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new)),
                )
                .unwrap_or_else(|error| self.proof_failure(error, t))
            })
            .and_then(|t| super::flow_origins::sources(target, t, origins, &actuals).ok())
            .and_then(|t| instantiate_parameter_authorities(target, t, &authorities).ok())
            .and_then(|t| instantiate_parameter_captures(target, t, &captures).ok())
            .and_then(|t| {
                super::flow_origins::canonical(target, t, &origins.names, &|o| self.arena_place(o))
                    .ok()
            });
        let expected = expected.map(|root| {
            // Actual result sources can discharge a declaration's dependent
            // placement. Rebuild an underclaim after that substitution while
            // retaining stronger validated current-content promises.
            if target.get(root).unwrap().modes.sendability == Sendability::NotSendable {
                self.derive_sendability(target, root)
            } else {
                root
            }
        });
        let expected =
            expected.and_then(|root| hirsel_types::declared_receipt_evidence(target, root).ok());
        let sharing_result = match &store.get(f.result).unwrap().kind {
            TypeKind::Parameter(TypeParameterRef::Bound {
                depth: 0,
                parameter,
            }) => sharing_type_parameters(store, f).contains(parameter),
            TypeKind::Parameter(_) => self.bound(
                store,
                f.result,
                Bound::Copyable,
                &self.generic_declarations(),
            ),
            _ => false,
        };
        // MODE-2: a result that is a bare type parameter is a value occurrence,
        // so its root holds the declared modes: a generic body proves no `T`
        // unique, whatever contract argument instantiates it.
        let held_result = matches!(&store.get(f.result).unwrap().kind, TypeKind::Parameter(_))
            && store.get(f.result).unwrap().modes.uniqueness == Uniqueness::Aliased;
        let expected = expected.map(|root| {
            let mut row = target.get(root).unwrap().clone();
            if held_result {
                row.modes.uniqueness = Uniqueness::Aliased;
            }
            if sharing_result
                && row.sendability_requirement == SendabilityRequirement::None
                && row.modes.uniqueness == Uniqueness::Aliased
                && matches!(row.kind, TypeKind::Parameter(_))
            {
                row.modes.sendability = self
                    .proof(
                        &target,
                        root,
                        super::proofs::Proof::AfterSharing,
                        &self.generic_declarations(),
                    )
                    .unwrap_or(Sendability::NotSendable);
            }
            target.intern(row)
        });
        // PROV-4, caller side: content moved in through any relation-free
        // `mut` parameter may come back out as the result, so the arenas and
        // regions that content is placed in may place the result too. Where
        // that content lives is the source checker's flow evidence (a `mut`
        // call may have moved it there), so the result may carry the arena
        // and region placement its destination states.
        let holder = self.place_type(destination);
        let carries = arguments.iter().any(|a| {
            f.parameters.get(a.parameter.0 as usize).is_some_and(|p| {
                p.passing == PassingMode::Mut
                    && !store.get(p.ty).is_some_and(|t| {
                        t.modes
                            .relation
                            .reaches()
                            .iter()
                            .any(|r| matches!(r, Reach::Place(_)))
                    })
                    && hirsel_type_analysis::representation::may_hold_content(
                        target,
                        self.memo,
                        holder,
                        self.place_type(a.value.place),
                        &mut |path| self.signatures.get(path).cloned().map(std::sync::Arc::new),
                    )
            })
        });
        let placement = if carries {
            target
                .get(holder)
                .unwrap()
                .modes
                .relation
                .retain(|r| match r {
                    Reach::Place(o) => self.arena_place(o),
                    Reach::Region(_) => true,
                    Reach::Environment => false,
                })
        } else {
            Relation::global()
        };
        let expected = expected.map(|root| {
            if placement.is_global()
                || self.bound(&target, root, Bound::Plain, &self.generic_declarations())
            {
                return root;
            }
            let mut row = target.get(root).unwrap().clone();
            row.modes.relation = row.modes.relation.union(&placement);
            target.intern(row)
        });
        let actual_result = if inactive {
            hirsel_types::erase_brands(target, actual_result).unwrap_or(actual_result)
        } else {
            actual_result
        };
        let actual = super::flow_origins::canonical(target, actual_result, &origins.names, &|o| {
            self.arena_place(o)
        })
        .ok();
        // Both sides as this frame sees them: the source checker joins
        // distinct argument lifetimes into the frame, and a borrowed
        // parameter's relation adds to its own root (S2).
        let mut frame = |ty: TypeId| {
            let mut row = target.get(ty).unwrap().clone();
            row.modes.relation = self.frame_relation(&row.modes.relation);
            target.intern(row)
        };
        let (expected, actual) = (expected.map(&mut frame), actual.map(&mut frame));
        let expected = expected.map(|expected| {
            let mut row = target.get(expected).unwrap().clone();
            if actual.is_some_and(|actual| {
                target.get(actual).unwrap().sendability_requirement == SendabilityRequirement::None
            }) {
                // A caller may receive a qualified result into ordinary root
                // storage. Nested contracts still match exactly below.
                row.sendability_requirement = SendabilityRequirement::None;
            }
            target.intern(row)
        });
        let actual = actual.map(|actual| {
            expected.map_or(actual, |expected| {
                self.comparison_evidence(target, expected, actual)
            })
        });
        let valid = matches!((expected, actual), (Some(expected), Some(actual)) if self.compatible(target, expected, actual, TypeRelation::Exact));
        if !valid {
            self.error(ViolationKind::TypeMismatch {
                entity: "call result",
            });
        }
    }

    /// PROV-4: a global environment outlives every arena and region, and a
    /// captured value owns the blocks of its parts, so no non-Plain row of
    /// the captured value may be placed in one. Callable headers describe
    /// what a function receives, not what it holds, and are not entered.
    fn retains_scoped_placement(&self, root: TypeId) -> bool {
        let types = self.t;
        let mut pending = vec![root];
        let mut seen = HashSet::default();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let Some(row) = types.get(ty) else {
                continue;
            };
            if matches!(row.kind, TypeKind::Error | TypeKind::Function(_)) {
                continue;
            }
            if self.placed(&row.modes.relation)
                && !self.bound(types, ty, Bound::Plain, self.generic_declarations())
            {
                return true;
            }
            pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
        }
        false
    }
    /// MODE-6 is semantic even when an empty local seal has global-looking
    /// physical rows. Specialization includes generic and erased holders.
    fn retains_scoped_capability(&self, root: TypeId) -> bool {
        fn visit(
            checker: &Checker<'_>,
            store: &TypeInterner,
            root: TypeId,
            seen: &mut HashSet<TypeId>,
        ) -> bool {
            if !seen.insert(root) {
                return false;
            }
            if let AuthoritySummary::Known(terms) =
                checker.authority_with_generics(store, root, checker.generic_declarations())
            {
                return terms.iter().any(|term| match term {
                    AuthorityTerm::Capability(path) => matches!(checker.signatures.get(path).map(|s| &s.kind),
                        Some(SignatureKind::Type(d)) if d.capability == Some(CapabilityKind::Scoped)),
                    _ => false,
                });
            }
            // An erased callable's Unknown summary must not swallow a scoped
            // sibling in the same retained aggregate. Headers are not payload.
            let Some(row) = store.get(root) else {
                return false;
            };
            if row
                .kind
                .value_parts()
                .into_iter()
                .any(|part| visit(checker, store, part, seen))
            {
                return true;
            }
            if let TypeKind::Named { definition, .. } = &row.kind {
                if let Some(signature) = checker.signatures.get(*definition) {
                    if let SignatureKind::Type(d) = &signature.kind {
                        return d
                            .representation
                            .is_some_and(|r| visit(checker, store, r, seen));
                    }
                }
            }
            false
        }
        visit(self, self.t, root, &mut HashSet::default())
    }
    fn data(&self, root: TypeId) -> bool {
        self.bound(self.t, root, Bound::Data, &self.generic_declarations())
    }
    /// A row's own destination: the arena or region its relation names, else
    /// unknown. Sources shared with the caller place nothing for a row.
    pub(super) fn row_destination(&self, ty: TypeId) -> Destination {
        match destination(&self.t.get(ty).unwrap().modes.relation, |o| {
            self.arena_place(o)
        }) {
            Destination::Caller(_) => Destination::Inferred,
            d => d,
        }
    }
    fn construction_placement(
        &mut self,
        ty: TypeId,
        destination: &Destination,
        entity: &'static str,
    ) {
        let declared = &self.row_destination(ty);
        let scoped = matches!(destination, Destination::Arena(_) | Destination::Region(_));
        if (scoped && destination != declared)
            || (*destination != Destination::Inferred
                && *declared != Destination::Inferred
                && destination != declared)
        {
            self.error(ViolationKind::TypeMismatch { entity });
        }
    }

    fn constructor_placement(&mut self, ty: TypeId, destination: &Destination) {
        // PROV-4: a constructed value carries its parts' placement as well
        // as its fresh blocks' arena. The full relation is a set, so its
        // first arena is not an allocation decision. Fields and their held
        // bounds are independently checked below and by receipt flow.
        let relation = &self.t.get(ty).unwrap().modes.relation;
        let valid = match self.row_destination(ty) {
            Destination::Arena(_) => matches!(destination,
                Destination::Arena(arena) if self.arena_place(arena)
                    && relation.reaches().contains(&Reach::Place(arena.clone()))),
            Destination::Region(region) => *destination == Destination::Region(region),
            _ => {
                self.construction_placement(ty, destination, "constructor placement");
                return;
            }
        };
        if !valid {
            self.error(ViolationKind::TypeMismatch {
                entity: "constructor placement",
            });
        }
    }

    fn rvalue_type(&mut self, destination: PlaceId, r: &Rvalue) {
        self.construction_effects(destination, r);
        let ty = self.place_type(destination);
        let kind = &self.t.get(ty).unwrap().kind;
        let computed = matches!(
            r,
            Rvalue::Constant(_)
                | Rvalue::Size(_)
                | Rvalue::Shape(_)
                | Rvalue::Binary { .. }
                | Rvalue::Construct { .. }
                | Rvalue::Closure { .. }
        ) || matches!(r, Rvalue::Unary { op, .. } if *op != U::Unique);
        // SortedMap derives its brand from the dictionary; flow independently
        // replays that identity in constructor_origin_types.
        if computed
            && !matches!(
                r,
                Rvalue::Construct {
                    constructor: Constructor::SortedMap { .. },
                    ..
                }
            )
            && !self.t.get(ty).unwrap().brand.alternatives.is_empty()
        {
            self.error(ViolationKind::InvalidOrigin(destination));
        }
        match r {
            Rvalue::Constant(c) => {
                let valid = constant_valid(c, &kind);
                if !valid {
                    self.error(ViolationKind::TypeMismatch { entity: "constant" });
                }
            }
            Rvalue::FunctionReference {
                definition,
                arguments,
            } => {
                if definition.is_core("cpu.map_scoped") {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "indirect CPU operation",
                    });
                }
                self.table_key_arguments(definition, arguments);
                let Some(signature) = self.signatures.get(definition) else {
                    self.error(ViolationKind::MissingSignature(definition.clone()));
                    return;
                };
                let SignatureKind::Function(declared) = &signature.kind else {
                    self.error(ViolationKind::InvalidSignature(definition.clone()));
                    return;
                };
                let mut function = declared.clone();
                function.generics = local_result_generics(self.t, declared).into();
                if !arguments.is_empty() {
                    if arguments.len() != function.generics.len() {
                        self.error(ViolationKind::Arity {
                            entity: "function reference generics",
                            expected: function.generics.len(),
                            actual: arguments.len(),
                        });
                        return;
                    }
                    for (parameter, argument) in function.generics.iter().zip(arguments) {
                        match (parameter, argument) {
                            (GenericParameter::Type { bounds, .. }, GenericArgument::Type(t)) => {
                                for bound in bounds {
                                    if !self.bound(self.t, *t, *bound, &self.generic_declarations())
                                    {
                                        self.error(ViolationKind::TypeMismatch {
                                            entity: "function reference bound",
                                        });
                                    }
                                }
                            }
                            (
                                GenericParameter::Size { .. },
                                GenericArgument::Size(Size::Parameter(p)),
                            ) => self.size_parameter(*p),
                            (GenericParameter::Size { .. }, GenericArgument::Size(_)) => (),
                            _ => self.error(ViolationKind::TypeMismatch {
                                entity: "function reference generic kind",
                            }),
                        }
                    }
                }
                let store = self.t;
                for parameter in &mut function.parameters {
                    let Some(imported) = self.imported(parameter.ty, arguments) else {
                        self.error(ViolationKind::InvalidTypeGraph);
                        return;
                    };
                    parameter.ty = imported;
                }
                let Some(result) = self.imported(function.result, arguments) else {
                    self.error(ViolationKind::InvalidTypeGraph);
                    return;
                };
                function.result = result;
                function.error =
                    hirsel_types::result_error(&store, function.result, &signature.metadata.roles);
                if !arguments.is_empty() {
                    let type_authorities: Vec<_> = arguments
                        .iter()
                        .map(|argument| match argument {
                            GenericArgument::Type(ty) => self.authority_with_generics(
                                self.t,
                                *ty,
                                &self.generic_declarations(),
                            ),
                            GenericArgument::Size(_) => AuthoritySummary::inert(),
                        })
                        .collect();
                    let parameters: Vec<_> = function
                        .parameters
                        .iter()
                        .enumerate()
                        .map(|(ordinal, _)| {
                            AuthoritySummary::Known(
                                vec![hirsel_types::AuthorityTerm::Parameter(ParameterRef::bound(
                                    ParamId(ordinal as u32),
                                ))]
                                .into(),
                            )
                        })
                        .collect();
                    function.authority = Self::instantiated_authority(
                        &function.authority,
                        &parameters,
                        &type_authorities,
                    );
                    if let Uses::Only(allowed) = &mut function.uses {
                        *allowed =
                            Self::instantiated_authority(allowed, &parameters, &type_authorities);
                    }
                    function.generics = vec![].into();
                }
                match &function.uses {
                    Uses::Unrestricted => function.uses = Uses::Only(function.authority.clone()),
                    Uses::Only(allowed) => function.authority = allowed.clone(),
                }
                let derived = store.intern(Type {
                    modes: Modes {
                        relation: Relation::global(),
                        uniqueness: Uniqueness::Unique,
                        multiplicity: Multiplicity::Many,
                        sendability: Sendability::Sendable,
                    },
                    ..Type::with_kind(TypeKind::Function(function))
                });
                // PROG-4: a non-generic definition typed as a callback `FnPtr[F]`
                // has F's shape.
                let callback = match &store.get(ty).map(|t| &t.kind) {
                    Some(TypeKind::Builtin {
                        kind: BuiltinType::FnPtr,
                        arguments,
                    }) if declared.foreign.is_none() && declared.generics.is_empty() => {
                        match arguments[..] {
                            [GenericArgument::Type(signature)] => Some(signature),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                // RUN-3: an entry `Entry[F]` names a non-generic, non-extern, complete
                // definition of F's shape: parameters, passing modes, labels and result.
                let entry = match &store.get(ty).map(|t| &t.kind) {
                    Some(TypeKind::Named {
                        definition: named,
                        arguments,
                    }) if *named.path() == DefPath::core("Entry") => match arguments[..] {
                        [GenericArgument::Type(signature)] => Some(signature),
                        _ => None,
                    },
                    _ => None,
                };
                let matches = match (callback, entry) {
                    (Some(signature), _) => store.shape_eq(signature, derived).unwrap_or(false),
                    (None, Some(signature)) => {
                        declared.foreign.is_none()
                            && declared.generics.is_empty()
                            && !declared.incomplete
                            && store.shape_eq(signature, derived).unwrap_or(false)
                    }
                    (None, None) => self.compatible(store, ty, derived, TypeRelation::Value),
                };
                if !matches {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "function reference",
                    });
                }
            }
            Rvalue::ReadConstant {
                definition,
                arguments,
            } => {
                let Some(signature) = self.signatures.get(definition) else {
                    self.error(ViolationKind::InvalidOrigin(destination));
                    return;
                };
                if hirsel_types::constant_identity(definition.clone(), &mut |id| {
                    self.signatures
                        .get(id.path())
                        .cloned()
                        .map(std::sync::Arc::new)
                        .ok_or(hirsel_types::QueryError::NotImplemented)
                })
                .is_err()
                {
                    self.error(ViolationKind::InvalidOrigin(destination));
                    return;
                }
                let SignatureKind::Const { ty: root, generics } = &signature.kind else {
                    self.error(ViolationKind::InvalidOrigin(destination));
                    return;
                };
                if generics.len() != arguments.len() {
                    self.error(ViolationKind::Arity {
                        entity: "constant size arguments",
                        expected: generics.len(),
                        actual: arguments.len(),
                    });
                    return;
                }
                for argument in arguments {
                    if let Size::Parameter(parameter) = argument {
                        self.size_parameter(*parameter);
                    }
                }
                let actuals: Vec<_> = arguments
                    .iter()
                    .cloned()
                    .map(GenericArgument::Size)
                    .collect();
                let store = self.t;
                let Some(mut expected) = self.imported(*root, &actuals) else {
                    self.error(ViolationKind::InvalidTypeGraph);
                    return;
                };
                let generics = self.generic_declarations();
                if !self.bound(&store, expected, hirsel_types::Bound::Plain, &generics)
                    && !self.bound(&store, expected, hirsel_types::Bound::Flat, &generics)
                    && !matches!(
                        store.get(expected).unwrap().kind,
                        TypeKind::Builtin {
                            kind: BuiltinType::Shared,
                            ..
                        }
                    )
                {
                    expected = self.weakened_type(store, expected, &generics);
                }
                if !self.compatible(store, expected, ty, TypeRelation::Exact) {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "constant declaration",
                    });
                }
            }
            Rvalue::Size(parameter) => {
                self.size_parameter(*parameter);
                if *kind != TypeKind::Primitive(Primitive::U64) {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "size read",
                    });
                }
            }
            Rvalue::Use(o) => {
                let store = self.t;
                let actual = self.place_type(o.place);
                let mut expected = ty;
                if self.unplaced_holder(destination) {
                    expected = hirsel_types::adopt_local(store, ty, actual);
                }
                self.storage_value_type_in(store, expected, actual, o, "assignment");
            }
            Rvalue::Unary { op, value } => {
                if *op == U::Unique {
                    if value.usage != UseKind::Copy {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "unique extraction",
                        });
                        return;
                    }
                    // MODE-2: a checked deep copy owns an independent graph.
                    // Keep every nested type, origin, placement and other proof.
                    // Counted Rvalue::Use(Copy) cannot strengthen modes;
                    // flat copies independently derive owned bytes.
                    let store = self.t;
                    let mut result = store.get(self.place_type(value.place)).unwrap().clone();
                    result.modes.uniqueness = Uniqueness::Unique;
                    result.modes.relation = self.placement(&result.modes.relation);
                    let result = store.intern(result);
                    let result = self.derive_sendability(store, result);
                    self.value_type_in(store, ty, result, "unique result");
                    return;
                }
                let store = self.t;
                let mut result = store.get(self.place_type(value.place)).unwrap().clone();
                result.brand = Brand::default();
                let result = store.intern(result);
                if !self.t.semantic_eq(ty, result) {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "unary result",
                    });
                }
                let k = &self.t.get(self.place_type(value.place)).unwrap().kind;
                let valid = match (op, k) {
                    (U::Not, TypeKind::Primitive(Primitive::Bool)) => true,
                    (U::BitNot, TypeKind::Primitive(p)) => integer(*p),
                    (U::Neg, TypeKind::Primitive(p)) => numeric(*p),
                    _ => false,
                };
                if !valid {
                    self.error(ViolationKind::InvalidOperator);
                }
            }
            Rvalue::Binary { op, left, right } => {
                let lt = self.place_type(left.place);
                let rt = self.place_type(right.place);
                let comparison = matches!(op, B::Eq | B::Ne | B::Lt | B::Le | B::Gt | B::Ge);
                if comparison {
                    self.comparison_type(lt, rt);
                } else {
                    self.arithmetic_type(lt, rt, ty);
                }
                let k = &self.t.get(lt).unwrap().kind;
                let valid = match (op, k) {
                    (B::Add | B::Sub | B::Mul | B::Div, TypeKind::Primitive(p)) => numeric(*p),
                    (
                        B::Shl | B::Shr | B::BitAnd | B::BitOr | B::BitXor,
                        TypeKind::Primitive(p),
                    ) => integer(*p),
                    (B::Rem, TypeKind::Primitive(p)) => numeric(*p),
                    (B::Eq | B::Ne | B::Lt | B::Le | B::Gt | B::Ge, TypeKind::Primitive(p)) => {
                        *p != Primitive::Never
                    }
                    (B::Eq | B::Ne, _) => self.data(lt),
                    _ => false,
                };
                if !valid {
                    self.error(ViolationKind::InvalidOperator);
                }
                if comparison {
                    if !matches!(kind, TypeKind::Primitive(Primitive::Bool)) {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "comparison result",
                        });
                    }
                }
            }
            Rvalue::Construct {
                constructor,
                fields,
                destination: placement,
            } => {
                self.constructor_placement(ty, placement);
                let witness;
                let fields = if let Constructor::SortedMap { by } = constructor {
                    if !fields.is_empty() || by.usage != UseKind::Borrow {
                        self.error(ViolationKind::InvalidConstructor);
                    }
                    witness = vec![ConstructField {
                        field: 0,
                        value: by.clone(),
                    }];
                    &witness
                } else {
                    fields
                };
                if matches!(constructor, Constructor::Map | Constructor::Set) {
                    let values = self
                        .static_data
                        .get_or_insert_with(|| super::static_data::Values::new(self.body));
                    if !values.distinct_keys(self.body, constructor, fields) {
                        self.error(ViolationKind::InvalidConstructor);
                    }
                }
                let store = self.t;
                let expected = self.constructor_layout(store, ty, constructor, fields.len());
                let Some(expected) = expected else {
                    self.error(ViolationKind::InvalidConstructor);
                    return;
                };
                if fields.len() != expected.len() {
                    self.error(ViolationKind::Arity {
                        entity: "constructor fields",
                        expected: expected.len(),
                        actual: fields.len(),
                    });
                }
                let mut ordinals = BTreeSet::new();
                for field in fields {
                    if !ordinals.insert(field.field) {
                        self.error(ViolationKind::Duplicate {
                            entity: "constructor field",
                            index: field.field,
                        });
                    }
                    if let Some(t) = expected.get(field.field as usize) {
                        let mut expected_field = *t;
                        let actual = self.place_type(field.value.place);
                        if Self::has_siblings(&store, *t) {
                            // Operand identities are available only at this
                            // instruction's flow state. The complete field
                            // relationship is checked there, before extraction.
                            continue;
                        }
                        // An unplaced Local holder can receive a scoped part.
                        // Its erased placement is bounded independently in flow.
                        if self.scoped_into_unplaced(destination, &field.value)
                            && self
                                .lifetime(&store.get(*t).unwrap().modes.relation)
                                .has_environment()
                            && !self.placed(&store.get(*t).unwrap().modes.relation)
                        {
                            let mut field_row = store.get(*t).unwrap().clone();
                            field_row.modes.relation = field_row
                                .modes
                                .relation
                                .union(&self.placement(&store.get(actual).unwrap().modes.relation));
                            expected_field = store.intern(field_row);
                        }
                        let mut receiving = store.get(expected_field).unwrap().clone();
                        receiving.sendability_requirement =
                            hirsel_types::SendabilityRequirement::None;
                        // MODE-2: a `unique` part needs current uniqueness
                        // evidence, such as a fresh call receipt's, which only
                        // the flow's `constructor_origin_types` has.
                        receiving.modes.uniqueness = Uniqueness::Aliased;
                        expected_field = store.intern(receiving);
                        // Cleanup summaries remain exact. Placement uses the
                        // shared Value relation: heap values outlive arenas.
                        if self.resolved_cleanup(*t) != self.resolved_cleanup(actual) {
                            self.error(ViolationKind::TypeMismatch {
                                entity: "constructor field",
                            });
                        } else {
                            self.operand_value_type_in(
                                store,
                                expected_field,
                                actual,
                                &field.value,
                                "constructor field",
                            );
                        }
                    } else {
                        self.error(ViolationKind::InvalidId {
                            arena: "constructor field",
                            index: field.field,
                        });
                    }
                }
                // CAP-2: a constructed carrier's evidence covers every value
                // it is built from.
                let row = self.t.get(ty).unwrap();
                if let (TypeKind::Named { .. }, Some(evidence)) =
                    (&row.kind, row.erased_captures.clone())
                {
                    let held = fields
                        .iter()
                        .fold(AuthoritySummary::inert(), |held, field| {
                            held.join(self.authority(self.place_type(field.value.place)))
                        });
                    if !matches!(evidence, ErasedCaptures::Known(_))
                        || !held.is_subset_of(&evidence.authority())
                    {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "erased carrier evidence",
                        });
                    }
                }
            }
            Rvalue::Closure {
                context,
                body,
                captures,
                destination,
            } => {
                self.construction_placement(ty, destination, "closure placement");
                // Classification is a claim about the source envelope, never
                // inferred from Borrow capture operands alone.
                if *context == CaptureContext::Scoped
                    && captures.iter().any(|c| {
                        !matches!(c.usage, UseKind::Borrow | UseKind::Mut)
                            && !(c.usage == UseKind::Copy
                                && self.bound(
                                    self.t,
                                    self.place_type(c.place),
                                    Bound::Plain,
                                    &self.generic_declarations(),
                                ))
                    })
                {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "closure source classification",
                    });
                }
                let TypeKind::Function(f) = kind else {
                    self.error(ViolationKind::TypeMismatch { entity: "closure" });
                    return;
                };
                if self
                    .definition
                    .contracts
                    .iter()
                    .any(|(_, clause)| clause == body)
                {
                    if !captures.is_empty()
                        || self.definition.bodies[body.0 as usize]
                            .locals
                            .iter()
                            .any(|l| l.storage == Storage::Capture)
                    {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "contract captures",
                        });
                    }
                    if f.parameters
                        .iter()
                        .any(|p| p.passing != PassingMode::Borrow)
                    {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "contract parameters",
                        });
                    }
                    if !matches!(
                        self.t.get(f.result).unwrap().kind,
                        TypeKind::Primitive(Primitive::Bool)
                    ) {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "contract result",
                        });
                    }
                }
                if self.t.get(ty).unwrap().drop_allocation == DropAllocation::NoAlloc
                    && captures
                        .iter()
                        .filter(|c| matches!(c.usage, UseKind::Copy | UseKind::Take))
                        .any(|c| {
                            !self.bound(
                                self.t,
                                self.place_type(c.place),
                                Bound::NoallocDrop,
                                &self.generic_declarations(),
                            )
                        })
                {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "closure cleanup",
                    });
                }
                let mut authority = AuthoritySummary::inert();
                let mut once = false;
                for capture in captures {
                    let source = self.place_type(capture.place);
                    let mut row = self.t.get(source).unwrap().clone();
                    // MEM-11, CONC-2: a copied Shared root, Mutex or Atomic,
                    // or an aggregate of them and plain bits, is an
                    // independently counted global value, wherever it was read.
                    if capture.usage == UseKind::Copy && self.copy_detaches(self.t, source) {
                        row.modes.relation = Relation::global();
                    }
                    let row = &row;
                    let global_closure = self
                        .lifetime(&self.t.get(ty).unwrap().modes.relation)
                        .is_global();
                    if !self.lifetime(&row.modes.relation).is_global() && global_closure {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "closure locality",
                        });
                    }
                    if global_closure && self.retains_scoped_placement(source) {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "closure capture placement",
                        });
                    }
                    if global_closure && self.retains_scoped_capability(source) {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "scoped capability capture",
                        });
                    }
                    authority = authority.join(self.authority(source));
                    once |= capture.usage == UseKind::Take
                        || row.modes.multiplicity == Multiplicity::Once;
                    // Copy captures lose counted uniqueness. Prove crossing
                    // from the captured value, not the stronger source row.
                    let store = self.t;
                    let mut captured = row.clone();
                    if capture.usage == UseKind::Copy {
                        captured.modes.uniqueness = if self.flat_value(capture.place) {
                            Uniqueness::Unique
                        } else {
                            Uniqueness::Aliased
                        };
                    }
                    let captured = store.intern(captured);
                    let generics = self.generic_declarations();
                    let mut lookup = |path: DefId| {
                        Ok::<_, hirsel_types::QueryError>(
                            self.signatures.get(path).cloned().map(std::sync::Arc::new),
                        )
                    };
                    let copyable = structural_property(
                        &store,
                        captured,
                        StructuralProperty::Copyable,
                        &generics,
                        &mut lookup,
                    )
                    .unwrap_or_else(|error| self.proof_failure(error, false));
                    // Provisional spec #37 option B: a resource borrow alone
                    // permits Many; owned noncopyable captures require Once.
                    once |= !copyable && !matches!(capture.usage, UseKind::Borrow | UseKind::Mut);
                }
                let uses = authority.clone();
                let store = self.t;
                let mut derived = store.get(ty).unwrap().clone();
                if let TypeKind::Function(function) = &mut derived.kind {
                    function.uses = Uses::Only(uses);
                }
                let derived = store.intern(derived);
                if !self.compatible(store, ty, derived, TypeRelation::Value) {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "closure uses",
                    });
                }
                if !authority.is_subset_of(&f.authority) {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "closure authority",
                    });
                }
                if once && self.t.get(ty).unwrap().modes.multiplicity != Multiplicity::Once {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "closure multiplicity",
                    });
                }
                let claimed = &self.t.get(ty).unwrap().modes.sendability;
                if matches!(claimed, Sendability::DependsOn(_))
                    && *claimed != self.closure_sendability(None, captures)
                {
                    // A physical receiving row can underclaim current content.
                    // Live flow must discharge this independently; a construction
                    // on an unvisited edge or in an unvisited body stays invalid.
                    self.scratch.closure_receipts.borrow_mut().insert((
                        self.site.body.0,
                        self.site.block.map(|block| block.0),
                        self.site.instruction,
                    ));
                }
                let nested = &self.definition.bodies[body.0 as usize];
                let slots: Vec<_> = nested
                    .locals
                    .iter()
                    .filter(|l| l.storage == Storage::Capture)
                    .collect();
                if slots.len() != captures.len() {
                    self.error(ViolationKind::Arity {
                        entity: "closure captures",
                        expected: slots.len(),
                        actual: captures.len(),
                    });
                }
                for (l, o) in slots.iter().zip(captures) {
                    self.capture_type(self.t, l.ty, o);
                }
                for ((index, _), operand) in nested
                    .locals
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.storage == Storage::Capture)
                    .zip(captures)
                {
                    let source = &self.body.places[operand.place.0 as usize].binding;
                    if nested.places.iter().enumerate().any(|(place, p)| {
                        let introduced = nested.blocks.iter().flat_map(|b| &b.instructions).any(|i| matches!(i.kind, InstructionKind::BindOrigin { place: target, .. } if target.0 as usize == place));
                        !introduced && p.root.0 as usize == index
                            && p.projection.is_empty()
                            && p.binding != *source
                    }) {
                        self.error(ViolationKind::InvalidOrigin(operand.place));
                    }
                }
                let expected: Vec<_> = f
                    .parameters
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| {
                        p.passing != PassingMode::Take
                            && matches!(
                                self.t.get(p.ty).map(|t| &t.kind),
                                Some(TypeKind::Function(_))
                            )
                    })
                    .map(|(i, _)| ScopedParameter {
                        parameter: i as u32,
                        fields: Vec::new(),
                    })
                    .collect();
                if !scoped_parameters_valid(self.t, f, &nested.scoped_parameters, &expected) {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "nested scoped parameter contract",
                    });
                }
                if nested.parameters.len() != f.parameters.len() {
                    self.error(ViolationKind::Arity {
                        entity: "closure parameters",
                        expected: f.parameters.len(),
                        actual: nested.parameters.len(),
                    });
                }
                let specialised = self.environment_specialised(ty, *body, captures);
                let sendable_environment = captures.iter().all(|c| {
                    self.t
                        .get(self.place_type(c.place))
                        .unwrap()
                        .modes
                        .sendability
                        == Sendability::Sendable
                });
                // MODE-4: what the environment holds may cross even where the
                // environment itself may not, being local or aliased; a
                // callable shared from it carries that evidence.
                let sendable_content = captures.iter().all(|c| {
                    let store = self.t;
                    let ty = self.place_type(c.place);
                    let mut row = store.get(ty).unwrap().clone();
                    row.modes.relation = self.placement(&row.modes.relation);
                    row.modes.uniqueness = hirsel_types::Uniqueness::Unique;
                    let root = store.intern(row);
                    let derived = self.derive_sendability(store, root);
                    store.get(derived).unwrap().modes.sendability == Sendability::Sendable
                });
                let target = self.t;
                for (actual, expected, entity) in nested
                    .parameters
                    .iter()
                    .zip(&f.parameters)
                    .map(|(l, p)| (nested.locals[l.0 as usize].ty, p.ty, "closure parameter"))
                    .chain([(nested.result, f.result, "closure result")])
                {
                    let actual = if specialised && entity == "closure result" {
                        {
                            // A global result's sendability is structural again;
                            // a share of a sendable environment is sendable, as a
                            // function value's sendability is its environment's.
                            let mut row = target.get(actual).unwrap().clone();
                            row.modes.relation = self.placement(&row.modes.relation);
                            let root = target.intern(row.clone());
                            row.modes.sendability = if sendable_environment {
                                Sendability::Sendable
                            } else if matches!(row.kind, TypeKind::Function(_)) && !sendable_content
                            {
                                // A callable's crossing mode is evidence, not
                                // derived from its locality (MODE-4): a share
                                // of an environment holding what may not cross
                                // carries none.
                                Sendability::NotSendable
                            } else {
                                hirsel_types::structural_sendability(
                                    &target,
                                    root,
                                    &self.declarations[body.0 as usize],
                                    &mut |p| {
                                        Ok::<_, hirsel_types::QueryError>(
                                            self.signatures
                                                .get(p)
                                                .cloned()
                                                .map(std::sync::Arc::new),
                                        )
                                    },
                                )
                                .unwrap_or_else(|error| {
                                    self.proof_failure(
                                        error,
                                        hirsel_types::Sendability::NotSendable,
                                    )
                                })
                            };
                            target.intern(row)
                        }
                    } else {
                        actual
                    };
                    let expected =
                        bind_parameter_scopes(target, expected, &[nested.parameter_scope]);
                    let expected = expected.map(|expected| {
                        if entity != "closure parameter" {
                            return expected;
                        }
                        let mut row = target.get(expected).unwrap().clone();
                        if !matches!(
                            row.kind,
                            TypeKind::Function(_)
                                | TypeKind::Builtin {
                                    kind: BuiltinType::Iter,
                                    ..
                                }
                        ) {
                            row.modes.sendability = self
                                .proof(
                                    &target,
                                    expected,
                                    super::proofs::Proof::Content,
                                    &self.declarations[body.0 as usize],
                                )
                                .unwrap_or_else(|error| {
                                    self.proof_failure(error, Sendability::NotSendable)
                                });
                        }
                        target.intern(row)
                    });
                    let actual = if entity == "closure parameter" {
                        expected.as_ref().ok().map_or(actual, |expected| {
                            self.comparison_evidence(target, *expected, actual)
                        })
                    } else {
                        actual
                    };
                    // Parameters stay exact. A literal's executable result may
                    // provide stronger evidence than its chosen result promise.
                    let (relation, reception) = if entity == "closure result" {
                        (TypeRelation::Value, Reception::View)
                    } else {
                        (TypeRelation::Exact, Reception::Holder)
                    };
                    if !matches!(expected, Ok(expected) if self.related(target,expected,actual,relation,reception,&self.declarations[body.0 as usize]))
                    {
                        self.error(ViolationKind::TypeMismatch { entity });
                    }
                }
            }
            Rvalue::Shape(t) => {
                if !matches!(kind, TypeKind::Builtin { kind: BuiltinType::TypeShape, arguments } if arguments.len() == 1 && matches!(arguments[0], GenericArgument::Type(id) if self.t.semantic_eq(id, *t)))
                {
                    self.error(ViolationKind::TypeMismatch { entity: "shape" });
                }
            }
        }
    }
    pub(super) fn has_origins(store: &TypeInterner, ty: TypeId) -> bool {
        let mut found = false;
        let _ = visit_type_origins(store, ty, |_, _| {
            found = true;
            Ok(())
        });
        found
    }
    /// Every proper projected prefix of the place is declared unique. An
    /// unqualified intermediate part may share its declared unique parts with
    /// a counted copy made before an owned call receipt (MODE-2).
    fn receipt_path(&mut self, place: PlaceId) -> bool {
        let place = &self.body.places[place.0 as usize];
        let store = self.t;
        (1..place.projection.len()).all(|depth| {
            let mut prefix = place.clone();
            prefix.projection.truncate(depth);
            prefix.binding = None;
            self.projection_type(store, &prefix, |_, _| None)
                .is_some_and(|ty| store.get(ty).unwrap().modes.uniqueness == Uniqueness::Unique)
        })
    }
    /// CLO-4: the known authority of an erased carrier the place projects
    /// through; a write or `mut` hand-back there must stay within it.
    pub(super) fn carrier_bound(&mut self, place: PlaceId) -> Option<AuthoritySummary> {
        let place = self.body.places.get(place.0 as usize)?.clone();
        let store = self.t;
        let mut bound = None;
        for depth in 0..place.projection.len() {
            let mut prefix = place.clone();
            prefix.projection.truncate(depth);
            prefix.binding = None;
            let ty = self.projection_type(store, &prefix, |_, _| None)?;
            let row = store.get(ty).unwrap();
            if let (TypeKind::Named { definition, .. }, Some(ErasedCaptures::Known(_))) =
                (&row.kind, &row.erased_captures)
                && self.signatures.get(*definition).is_some_and(
                    |s| matches!(&s.kind, SignatureKind::Type(d) if d.captures.is_some()),
                )
            {
                bound = Some(self.authority_in(&store, ty));
            }
        }
        bound
    }
    pub(super) fn projection_type(
        &mut self,
        store: &TypeInterner,
        p: &Place,
        sibling: impl FnMut(usize, FieldId) -> Option<Origin>,
    ) -> Option<TypeId> {
        self.projection_type_at(store, p, self.body.locals[p.root.0 as usize].ty, sibling)
    }
    pub(super) fn projection_type_at(
        &mut self,
        store: &TypeInterner,
        p: &Place,
        mut ty: TypeId,
        mut sibling: impl FnMut(usize, FieldId) -> Option<Origin>,
    ) -> Option<TypeId> {
        let generics = self.generic_declarations();
        for (depth, projection) in p.projection.iter().enumerate() {
            let kind = self.layout(store, ty)?;
            let next = match (projection, kind) {
                (Projection::Field(0), TypeKind::Box(t)) => Some(t),
                (Projection::Field(n), TypeKind::Tuple(ts)) => ts.get(*n as usize).copied(),
                (Projection::Field(n), TypeKind::Record(fs)) => fs.get(*n as usize).map(|f| f.ty),
                (Projection::VariantField { variant, field }, TypeKind::Variant(vs)) => vs
                    .get(*variant as usize)
                    .and_then(|v| v.fields.get(*field as usize))
                    .map(|f| f.1),
                (Projection::Index(index), kind) if kind.map_entry_fields().is_some() => {
                    if !matches!(self.t.get(self.body.locals[index.0 as usize].ty).unwrap().kind, TypeKind::Primitive(p) if integer(p))
                    {
                        return None;
                    }
                    let fields = kind.map_entry_fields().unwrap();
                    Some(self.map_entry_type(store, fields, &generics))
                }
                (Projection::Index(index), TypeKind::Primitive(Primitive::Bytes)) => {
                    if !matches!(self.t.get(self.body.locals[index.0 as usize].ty).unwrap().kind, TypeKind::Primitive(p) if integer(p))
                    {
                        return None;
                    }
                    {
                        let mut byte = Type::boolean();
                        byte.kind = TypeKind::Primitive(Primitive::U8);
                        Some(store.intern(byte))
                    }
                }
                (Projection::Index(index), TypeKind::Builtin { kind, arguments })
                    if matches!(
                        kind,
                        BuiltinType::Array
                            | BuiltinType::List
                            | BuiltinType::Inline
                            | BuiltinType::Simd
                    ) =>
                {
                    if !matches!(self.t.get(self.body.locals[index.0 as usize].ty).unwrap().kind, TypeKind::Primitive(p) if integer(p))
                    {
                        return None;
                    }
                    arguments.first().and_then(|a| {
                        if let GenericArgument::Type(t) = a {
                            Some(*t)
                        } else {
                            None
                        }
                    })
                }
                _ => None,
            };
            if let Some(next) = next {
                let mut fields = BTreeMap::new();
                let _ = visit_type_origins(store, next, |origin, _| {
                    if let OriginRoot::Sibling(field) = origin.root {
                        if let Some(owner) = sibling(depth, field) {
                            fields.insert(field, owner);
                        }
                    }
                    Ok(())
                });
                let next = instantiate_sibling_origins(store, next, &fields).ok()?;
                let evidence = store.get(next).unwrap().modes.sendability.clone();
                let next = self.placed_part(store, ty, next, &generics);
                let mut projected = store.get(next).unwrap().clone();
                // The holder's access lifetime alone does not change content.
                // Real placement stays in its relation and is checked by flow.
                if self
                    .placement(&store.get(ty).unwrap().modes.relation)
                    .is_global()
                {
                    projected.modes.sendability = evidence;
                }
                let next = store.intern(projected);
                ty = self
                    .context(&generics, |memo| memo.projected(self.memo, ty, next))
                    .map_err(|error| self.proof_failure(error, ()))
                    .ok()?;
            } else {
                return None;
            }
        }
        Some(ty)
    }
    pub(super) fn readonly_map_entry(&mut self, place: PlaceId) -> bool {
        let access = self.body.places[place.0 as usize].clone();
        if !access
            .projection
            .iter()
            .any(|p| matches!(p, Projection::Index(_)))
        {
            return false;
        }
        let store = self.t;
        for (depth, projection) in access.projection.iter().enumerate() {
            if !matches!(projection, Projection::Index(_)) {
                continue;
            }
            let mut prefix = access.clone();
            prefix.projection.truncate(depth);
            let Some(owner) = self.projection_type(store, &prefix, |_, _| None) else {
                continue;
            };
            if self
                .layout(store, owner)
                .is_some_and(|kind| kind.map_entry_fields().is_some())
                && !matches!(access.projection.get(depth + 1), Some(Projection::Field(1)))
            {
                return true;
            }
        }
        false
    }
    pub(super) fn readonly_byte_index(&mut self, place: PlaceId) -> bool {
        let access = self.body.places[place.0 as usize].clone();
        let store = self.t;
        for (depth, projection) in access.projection.iter().enumerate() {
            if !matches!(projection, Projection::Index(_)) {
                continue;
            }
            let mut prefix = access.clone();
            prefix.projection.truncate(depth);
            if self
                .projection_type(store, &prefix, |_, _| None)
                .and_then(|owner| self.layout(store, owner))
                .is_some_and(|kind| matches!(kind, TypeKind::Primitive(Primitive::Bytes)))
            {
                return true;
            }
        }
        false
    }
    fn has_siblings(store: &TypeInterner, ty: TypeId) -> bool {
        let mut found = false;
        let _ = visit_type_origins(store, ty, |origin, _| {
            found |= matches!(origin.root, OriginRoot::Sibling(_));
            Ok(())
        });
        found
    }
    fn placed_part(
        &self,
        store: &TypeInterner,
        owner: TypeId,
        part: TypeId,
        generics: &hirsel_types::GenericDeclarations,
    ) -> TypeId {
        placed_part_type(store, owner, part, generics, &mut |path| {
            Ok::<_, hirsel_types::QueryError>(
                self.signatures.get(path).cloned().map(std::sync::Arc::new),
            )
        })
        .expect("validated field graph")
    }
    fn map_entry_type(
        &self,
        store: &TypeInterner,
        fields: [TypeId; 2],
        generics: &hirsel_types::GenericDeclarations,
    ) -> TypeId {
        let kind = TypeKind::Tuple(fields.to_vec().into());
        let drop_allocation = cleanup_summary(&kind, |leaf| match leaf {
            CleanupLeaf::Component(t) => store.get(t).unwrap().drop_allocation.clone(),
            _ => unreachable!("tuple cleanup contains only components"),
        });
        let tuple = store.intern(Type {
            drop_allocation,
            ..Type::with_kind(kind)
        });
        normalize_type_properties(store, tuple, generics, &mut |path| {
            Ok::<_, hirsel_types::QueryError>(
                self.signatures.get(path).cloned().map(std::sync::Arc::new),
            )
        })
        .unwrap_or_else(|error| self.proof_failure(error, tuple))
    }
    pub(super) fn constructor_layout(
        &mut self,
        store: &TypeInterner,
        ty: TypeId,
        constructor: &Constructor,
        field_count: usize,
    ) -> Option<Vec<TypeId>> {
        let layout = self.layout(store, ty);
        let fields = match (constructor, layout) {
            (
                Constructor::SortedMap { .. },
                Some(TypeKind::Builtin {
                    kind: BuiltinType::SortedMap,
                    arguments,
                }),
            ) => {
                let [GenericArgument::Type(key), GenericArgument::Type(_)] = arguments.as_slice()
                else {
                    return None;
                };
                let order_path = DefId::intern(&DefPath::core("Order"));
                let mut order = Type::with_kind(TypeKind::Named {
                    definition: order_path,
                    arguments: vec![GenericArgument::Type(*key)].into(),
                });
                order.modes.relation = Relation::environment();
                let declaration = self.signatures.get(order_path)?;
                let SignatureKind::Type(declared) = &declaration.kind else {
                    return None;
                };
                order.drop_allocation = hirsel_type_analysis::cleanup::declared_cleanup(
                    declared,
                    self.t,
                    &[GenericArgument::Type(*key)],
                    |ty| store.get(ty).unwrap().drop_allocation.clone(),
                );
                let order = store.intern(order);
                let order = normalize_type_properties(
                    store,
                    order,
                    &self.generic_declarations(),
                    &mut |path| {
                        Ok::<_, hirsel_types::QueryError>(
                            self.signatures.get(path).cloned().map(std::sync::Arc::new),
                        )
                    },
                )
                .ok()?;
                // The dictionary selects the brand; it is not an entry stored in the arena.
                return Some(vec![order]);
            }
            (Constructor::Tuple, Some(TypeKind::Tuple(ts))) => Some(ts.to_vec()),
            (Constructor::Box, Some(TypeKind::Box(t))) => Some(vec![t]),
            (Constructor::Record(path), Some(TypeKind::Record(fs))) => {
                if !self.constructor_owner(path, ty) {
                    self.error(ViolationKind::InvalidConstructor);
                }
                Some(fs.iter().map(|f| f.ty).collect())
            }
            (
                Constructor::Variant {
                    definition,
                    variant,
                },
                Some(TypeKind::Variant(vs)),
            ) => {
                if !self.constructor_owner(definition, ty) {
                    self.error(ViolationKind::InvalidConstructor);
                }
                vs.get(*variant as usize)
                    .map(|v| v.fields.iter().map(|f| f.1).collect())
            }
            (Constructor::Sequence, Some(TypeKind::Builtin { kind, arguments }))
                if matches!(
                    kind,
                    BuiltinType::Array
                        | BuiltinType::List
                        | BuiltinType::Inline
                        | BuiltinType::Simd
                ) =>
            {
                if let Some(GenericArgument::Type(t)) = arguments.first() {
                    let count = match arguments.get(1) {
                        None => field_count,
                        Some(GenericArgument::Size(Size::Constant(n))) => {
                            usize::try_from(*n).unwrap_or(usize::MAX)
                        }
                        // Sizes compare by equality only (LANG-7): an
                        // abstract size never equals a field count.
                        Some(_) => {
                            self.error(ViolationKind::TypeMismatch {
                                entity: "sequence size",
                            });
                            return None;
                        }
                    };
                    // Avoid allocating attacker-controlled size from a malformed type.
                    if count != field_count {
                        self.error(ViolationKind::Arity {
                            entity: "sequence fields",
                            expected: count,
                            actual: field_count,
                        });
                        None
                    } else {
                        Some(vec![*t; count])
                    }
                } else {
                    None
                }
            }
            (
                Constructor::Map,
                Some(
                    layout @ TypeKind::Builtin {
                        kind: BuiltinType::Map,
                        ..
                    },
                ),
            ) => {
                let [key, value] = layout.map_entry_fields()?;
                let declarations = self.generic_declarations();
                let key_ok = hirsel_types::structural_equality(
                    store,
                    key,
                    hirsel_types::Bound::Key,
                    &declarations,
                    &mut |path| {
                        Ok::<_, hirsel_types::QueryError>(
                            self.signatures.get(path).cloned().map(std::sync::Arc::new),
                        )
                    },
                )
                .unwrap_or_else(|error| self.proof_failure(error, false));
                let value_ok = hirsel_types::structural_equality(
                    store,
                    value,
                    hirsel_types::Bound::Data,
                    &declarations,
                    &mut |path| {
                        Ok::<_, hirsel_types::QueryError>(
                            self.signatures.get(path).cloned().map(std::sync::Arc::new),
                        )
                    },
                )
                .unwrap_or_else(|error| self.proof_failure(error, false));
                if !key_ok || !value_ok {
                    return None;
                }
                let entry = self.map_entry_type(store, [key, value], &declarations);
                Some(vec![entry; field_count])
            }
            (
                Constructor::Set,
                Some(TypeKind::Builtin {
                    kind: BuiltinType::Set,
                    arguments,
                }),
            ) => {
                let [GenericArgument::Type(element)] = arguments.as_slice() else {
                    return None;
                };
                let declarations = self.generic_declarations();
                if !hirsel_types::structural_equality(
                    store,
                    *element,
                    hirsel_types::Bound::Key,
                    &declarations,
                    &mut |path| {
                        Ok::<_, hirsel_types::QueryError>(
                            self.signatures.get(path).cloned().map(std::sync::Arc::new),
                        )
                    },
                )
                .unwrap_or_else(|error| self.proof_failure(error, false))
                {
                    return None;
                }
                Some(vec![*element; field_count])
            }
            _ => None,
        }?;
        // OWN-8: parts live in their owner's locality, so a field of a local
        // aggregate receives a value at most as local as the aggregate, and a
        // field of a global aggregate still requires a global one.
        let generics = self.generic_declarations();
        Some(
            fields
                .into_iter()
                .map(|part| {
                    let part = self
                        .context(&generics, |memo| memo.projected(self.memo, ty, part))
                        .expect("validated field graph");
                    self.placed_part(store, ty, part, &generics)
                })
                .collect(),
        )
    }
    pub(super) fn constructor_origin_types(
        &mut self,
        destination: PlaceId,
        constructor: &Constructor,
        fields: &[ConstructField],
        origins: &super::flow_origins::CallOriginProof,
    ) {
        let witness;
        let fields = if let Constructor::SortedMap { by } = constructor {
            witness = vec![ConstructField {
                field: 0,
                value: by.clone(),
            }];
            &witness
        } else {
            fields
        };
        let store = self.t;
        let Some(expected) = self.constructor_layout(
            store,
            self.place_type(destination),
            constructor,
            fields.len(),
        ) else {
            return;
        };
        if matches!(constructor, Constructor::SortedMap { .. }) {
            let result = super::flow_origins::canonical(
                store,
                self.place_type(destination),
                &origins.names,
                &|o| self.arena_place(o),
            )
            .ok();
            let brand = result.and_then(|ty| {
                store
                    .get(ty)?
                    .brand
                    .alternatives
                    .first()
                    .cloned()
                    .filter(|_| store.get(ty).unwrap().brand.alternatives.len() == 1)
            });
            let by = origins
                .arguments
                .get(&ParameterRef::bound(hirsel_types::ParamId(0)));
            if !brand.is_some_and(|brand| by.is_some_and(|by| by.identity == brand)) {
                self.error(ViolationKind::InvalidConstructor);
            }
        }
        let siblings: BTreeMap<_, _> = origins
            .arguments
            .iter()
            .filter_map(|(p, a)| match p {
                ParameterRef::Bound {
                    depth: 0,
                    parameter,
                } => Some((FieldId(parameter.0), a.identity.clone())),
                _ => None,
            })
            .collect();
        for field in fields {
            let Some(ty) = expected.get(field.field as usize).copied() else {
                continue;
            };
            let expected = instantiate_sibling_origins(store, ty, &siblings)
                .ok()
                .and_then(|ty| {
                    super::flow_origins::canonical(store, ty, &origins.names, &|o| {
                        self.arena_place(o)
                    })
                    .ok()
                });
            let actual = origins.proofs.get(&field.value.place.0).and_then(|proof| {
                super::flow_origins::canonical(
                    store,
                    self.place_type(field.value.place),
                    proof,
                    &|o| self.arena_place(o),
                )
                .ok()
            });
            let (Some(expected), Some(actual)) = (expected, actual) else {
                self.error(ViolationKind::InvalidOrigin(field.value.place));
                continue;
            };
            let mut row = store.get(actual).unwrap().clone();
            if !matches!(
                row.kind,
                TypeKind::Function(_)
                    | TypeKind::Builtin {
                        kind: BuiltinType::Iter,
                        ..
                    }
            ) {
                row.modes.sendability = if origins.current_sendable.contains(&field.value.place.0) {
                    Sendability::Sendable
                } else {
                    Sendability::NotSendable
                };
            }
            let actual = store.intern(row);
            let actual = if origins.current_unique.contains(&field.value.place.0) {
                self.unique_type(store, actual)
            } else if origins.weakened.contains(&field.value.place.0) {
                self.weakened_type(store, actual, &self.generic_declarations())
            } else {
                actual
            };
            if self.resolved_cleanup(expected) != self.resolved_cleanup(actual) {
                self.error(ViolationKind::TypeMismatch {
                    entity: "constructor field",
                });
            } else {
                self.operand_value_type_in(
                    store,
                    expected,
                    actual,
                    &field.value,
                    "constructor field",
                );
            }
        }
    }
    fn constructor_owner(&mut self, path: &DefPath, ty: TypeId) -> bool {
        let ty = match &self.t.get(ty).unwrap().kind {
            TypeKind::Builtin {
                kind: BuiltinType::Packed,
                arguments,
            } => match arguments.as_slice() {
                [GenericArgument::Type(payload)] => *payload,
                _ => return false,
            },
            _ => ty,
        };
        if let TypeKind::Named { definition, .. } = &self.t.get(ty).unwrap().kind {
            return definition.path() == path;
        }
        let Some(s) = self.signatures.get(path) else {
            self.error(ViolationKind::MissingSignature(path.clone()));
            return true;
        };
        if let SignatureKind::Type(t) = &s.kind {
            if let Some(rep) = t.representation {
                return self.t.semantic_eq(rep, ty);
            }
        }
        false
    }
}

// Compare current summaries independently of immutable receiving contracts.
// Producers may underclaim data evidence; stronger data evidence is accepted
// only when an independent, sharing-stable proof derives it. An identical
// qualified erased contract can underclaim its redundant summary. Requirements
// and callable promises stay intact, and no scratch row becomes flow evidence.
fn conservative_result_evidence(
    store: &TypeInterner,
    expected: TypeId,
    actual: TypeId,
    memo: &mut HashMap<(TypeId, TypeId), TypeId>,
    stable: &mut impl FnMut(&TypeInterner, TypeId) -> bool,
) -> TypeId {
    if let Some(known) = memo.get(&(expected, actual)) {
        return *known;
    }
    memo.insert((expected, actual), actual);
    let required = store.get(expected).unwrap().clone();
    let mut row = store.get(actual).unwrap().clone();
    if (!matches!(
        row.kind,
        TypeKind::Function(_)
            | TypeKind::Builtin {
                kind: BuiltinType::Iter,
                ..
            }
    ) || row.sendability_requirement == SendabilityRequirement::Sendable
        && required.sendability_requirement == SendabilityRequirement::Sendable)
        && row.modes.sendability == Sendability::NotSendable
    {
        row.modes.sendability = required.modes.sendability.clone();
    }
    if row.modes.sendability == Sendability::Sendable
        && required.modes.sendability == Sendability::NotSendable
        && !matches!(
            row.kind,
            TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                }
        )
        && (stable(store, actual) || required.modes.relation.has_environment())
    {
        row.modes.sendability = Sendability::NotSendable;
    }
    let mut child =
        |expected, actual| conservative_result_evidence(store, expected, actual, memo, stable);
    match (&required.kind, &mut row.kind) {
        (TypeKind::Tuple(a), TypeKind::Tuple(b)) if a.len() == b.len() => {
            for (a, b) in a.iter().zip(b.iter_mut()) {
                *b = child(*a, *b);
            }
        }
        (TypeKind::Box(a), TypeKind::Box(b)) => *b = child(*a, *b),
        (TypeKind::Record(a), TypeKind::Record(b)) if a.len() == b.len() => {
            for (a, b) in a.iter().zip(b.iter_mut()) {
                b.ty = child(a.ty, b.ty);
            }
        }
        (TypeKind::Variant(a), TypeKind::Variant(b)) if a.len() == b.len() => {
            for (a, b) in a.iter().zip(b.iter_mut()) {
                if a.fields.len() == b.fields.len() {
                    for (a, b) in a.fields.iter().zip(b.fields.iter_mut()) {
                        b.1 = child(a.1, b.1);
                    }
                }
            }
        }
        (TypeKind::Named { arguments: a, .. }, TypeKind::Named { arguments: b, .. })
        | (TypeKind::Builtin { arguments: a, .. }, TypeKind::Builtin { arguments: b, .. })
            if a.len() == b.len() =>
        {
            for (a, b) in a.iter().zip(b.iter_mut()) {
                if let (GenericArgument::Type(a), GenericArgument::Type(b)) = (a, b) {
                    *b = child(*a, *b);
                }
            }
        }
        (TypeKind::Function(a), TypeKind::Function(b))
            if a.parameters.len() == b.parameters.len() =>
        {
            for (a, b) in a.parameters.iter().zip(b.parameters.iter_mut()) {
                b.ty = child(a.ty, b.ty);
            }
            b.result = child(a.result, b.result);
            if let (Some(a), Some(b)) = (a.error, &mut b.error) {
                *b = child(a, *b);
            }
        }
        _ => (),
    }
    let result = store.intern(row);
    memo.insert((expected, actual), result);
    result
}

// Written requirements and current proof are separate. On a receiving-shape
// comparison, two identical qualified owned edges already agree as contracts.
// Remove their common requirement (including its duplicate erased promise)
// from scratch rows, without supplying either value any current proof.
pub(super) fn matching_crossing_contracts(
    store: &TypeInterner,
    expected: TypeId,
    actual: TypeId,
    memo: &mut HashMap<(TypeId, TypeId), (TypeId, TypeId)>,
) -> (TypeId, TypeId) {
    if let Some(pair) = memo.get(&(expected, actual)) {
        return *pair;
    }
    memo.insert((expected, actual), (expected, actual));
    let mut a = store.get(expected).unwrap().clone();
    let mut b = store.get(actual).unwrap().clone();
    if a.sendability_requirement == SendabilityRequirement::Sendable
        && b.sendability_requirement == SendabilityRequirement::Sendable
    {
        a.sendability_requirement = SendabilityRequirement::None;
        b.sendability_requirement = SendabilityRequirement::None;
        if matches!(
            a.kind,
            TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                }
        ) && matches!(
            b.kind,
            TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                }
        ) {
            a.modes.sendability = Sendability::NotSendable;
            b.modes.sendability = Sendability::NotSendable;
        }
    }
    let (left, right) = (a.kind.value_parts(), b.kind.value_parts());
    if left.len() == right.len() {
        let children: Vec<_> = left
            .iter()
            .zip(right.iter())
            .map(|(a, b)| matching_crossing_contracts(store, *a, *b, memo))
            .collect();
        fn install(kind: &mut TypeKind, children: impl Iterator<Item = TypeId>) {
            let mut children = children;
            match kind {
                TypeKind::Tuple(parts) => {
                    for part in parts {
                        *part = children.next().unwrap();
                    }
                }
                TypeKind::Box(part) => *part = children.next().unwrap(),
                TypeKind::Record(fields) => {
                    for field in fields {
                        field.ty = children.next().unwrap();
                    }
                }
                TypeKind::Variant(variants) => {
                    for field in variants.iter_mut().flat_map(|v| v.fields.iter_mut()) {
                        field.1 = children.next().unwrap();
                    }
                }
                TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                    for argument in arguments {
                        if let GenericArgument::Type(part) = argument {
                            *part = children.next().unwrap();
                        }
                    }
                }
                _ => (),
            }
        }
        install(&mut a.kind, children.iter().map(|p| p.0));
        install(&mut b.kind, children.iter().map(|p| p.1));
    }
    let result = (store.intern(a), store.intern(b));
    memo.insert((expected, actual), result);
    result
}

/// Assign/Return flow independently proves a received value's content
/// properties (uniqueness and crossing evidence), on every return path.
/// Storage and current value evidence have distinct lifetimes.
fn receiving_storage(store: &TypeInterner, expected: TypeId) -> TypeId {
    let mut receiving = store.get(expected).unwrap().clone();
    receiving.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
    receiving.modes.sendability = hirsel_types::Sendability::NotSendable;
    receiving.sendability_requirement = hirsel_types::SendabilityRequirement::None;
    store.intern(receiving)
}
