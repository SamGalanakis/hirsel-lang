use super::*;
use hirsel_syntax::{BinaryOp as B, PassingMode, UnaryOp as U};
use hirsel_types::*;

/// How a place receives a value, which decides what its nested rows may forget.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Reception {
    /// A readonly argument or a whole taken value: nothing writes it back.
    View,
    /// A copy whose source stays live beside the receiving place.
    Holder,
    /// A `mut` argument, written back through the parameter.
    HandBack,
}

pub(super) fn declaration_tables(
    definition: &Definition,
    signatures: &dyn SignatureLookup,
) -> Option<Vec<GenericDeclarations>> {
    let empty: std::sync::Arc<[GenericParameter]> = [].into();
    let mut scopes: Vec<_> = definition
        .bodies
        .iter()
        .map(|b| (b.parameter_scope, None, empty.clone()))
        .collect();
    if let Some(signature) = signatures.get(&definition.path) {
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
            if let TypeKind::Function(function) = &definition
                .types
                .get(parent.places[destination.0 as usize].ty)?
                .kind
            {
                scopes[child.0 as usize].1 = Some(parent_id);
                scopes[child.0 as usize].2 = function.generics.clone().into();
            }
        }
    }
    GenericDeclarations::lexical(scopes)
}

pub(super) fn operands(r: &Rvalue) -> Vec<&Operand> {
    match r {
        Rvalue::Use(o) | Rvalue::Unary { value: o, .. } => {
            vec![o]
        }
        Rvalue::Binary { left, right, .. } => vec![left, right],
        Rvalue::Construct { fields, .. } => fields.iter().map(|f| &f.value).collect(),
        Rvalue::Closure { captures, .. } => captures.iter().collect(),
        _ => vec![],
    }
}

pub(super) fn signature_valid(s: &Signature) -> bool {
    if !graph_valid(&s.types)
        || s.types
            .iter()
            .any(|(_, t)| t.allocation_target.is_some() && !t.is_heap_target(&s.metadata.roles))
    {
        return false;
    }
    if let SignatureKind::Function(f) = &s.kind {
        if !value_parameters_valid(&s.types, f) {
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
        SignatureKind::Const(t) => vec![*t],
    };
    roots
        .into_iter()
        .all(|t| owned_type(&s.types, t) && generic_references_valid(&s.types, t, &declarations))
}

fn value_parameters_valid(store: &TypeStore, f: &FunctionType) -> bool {
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
    let mut seen = BTreeSet::new();
    while let Some((t, mut scopes)) = pending.pop() {
        if !seen.insert((t, scopes.clone())) {
            continue;
        }
        if !owned_type(store, t) {
            return false;
        }
        let row = store.get(t).unwrap();
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
            | Primitive::String
            | Primitive::Bytes
            | Primitive::Never
    )
}
fn numeric(p: Primitive) -> bool {
    integer(p) || matches!(p, Primitive::F32 | Primitive::F64)
}

impl<'a> Checker<'a> {
    pub(super) fn types(&mut self) {
        self.match_tables();
        let declarations = self.generic_declarations();
        if self
            .body
            .locals
            .iter()
            .map(|l| l.ty)
            .chain(self.body.places.iter().map(|p| p.ty))
            .chain([self.body.result])
            .any(|t| !generic_references_valid(&self.definition.types, t, &declarations))
        {
            self.error(ViolationKind::InvalidTypeGraph);
            return;
        }
        self.reachability();
        self.owner_type();
        self.nominal_bounds();
        self.prepare_content_inputs();
        for (i, p) in self.body.places.iter().enumerate() {
            let mut store = self.working_store();
            let expected = self.projection_type(&mut store, p, |depth, field| {
                let binding = p.binding.as_ref()?;
                let base = p.projection.len().checked_sub(binding.fields.len())?;
                let relative = depth.checked_sub(base)?;
                let mut parent = binding.clone();
                parent.fields.truncate(relative);
                parent.fields.push(field);
                Some(parent)
            });
            let expected = expected.map(|root| {
                canonical_plain_type(&mut store, root, &mut |path| {
                    Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new))
                })
                .unwrap_or(root)
            });
            let current = self.global_root_projection(&mut store, p);
            let current = self.receiving_root(p.root).then_some(current).flatten();
            let actual = import_type(&self.definition.types, p.ty, &mut store).unwrap_or(p.ty);
            let physical = store.get(actual).unwrap().modes.clone();
            let actual = canonical_plain_type(&mut store, actual, &mut |path| {
                Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new))
            })
            .unwrap_or(actual);
            let actual = if !p.projection.is_empty() {
                let mut row = store.get(actual).unwrap().clone();
                row.modes.relation = physical.relation;
                row.modes.uniqueness = physical.uniqueness;
                store.push(row)
            } else {
                actual
            };
            let valid = expected.is_some_and(|expected| {
                let actual = self.comparison_evidence(&mut store, expected, actual);
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
                let actual = store.push(row);
                semantic_equal(&store, expected, &store, actual).unwrap_or(false)
                    || current.is_some_and(|current| {
                        semantic_equal(&store, current, &self.definition.types, p.ty)
                            .unwrap_or(false)
                    })
                    || (!p.projection.is_empty()
                        && (Self::has_origins(&store, expected)
                            || Self::has_origins(&self.definition.types, p.ty)))
            });
            if !valid {
                self.error(ViolationKind::InvalidProjection(PlaceId(i as u32)));
            }
        }
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
                        if super::flow::needs_release(
                            self,
                            self.place_type(*destination),
                            &self.definition.types,
                        ) {
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
                    let mut store = self.working_store();
                    let kind = self.layout(&mut store, self.place_type(value.place));
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
                    let mut store = self.working_store();
                    let (expected, unknown_join) = self.return_type(&mut store);
                    // A conservative source join accepts a more precise borrowed
                    // placement. CFG source inclusion independently proves that
                    // it cannot outlive any selected source; owned returns still
                    // use Value compatibility and cannot erase arena placement.
                    if unknown_join {
                        let expected = receiving_storage(&mut store, expected);
                        let actual =
                            self.copied_type(&mut store, value, self.place_type(value.place));
                        if !self.compatible_with_context(
                            &mut store,
                            expected,
                            actual,
                            TypeRelation::Parameter,
                            &self.generic_declarations(),
                        ) {
                            self.error(ViolationKind::TypeMismatch { entity: "return" });
                        }
                    } else {
                        self.storage_value_type_in(
                            &mut store,
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

    pub(super) fn return_type(&self, store: &mut TypeStore) -> (TypeId, bool) {
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
        let expected = store.push(expected);
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
        let Some(signature) = self.signatures.get(&self.definition.path) else {
            if !self.body.parameters.is_empty() {
                self.error(ViolationKind::MissingSignature(
                    self.definition.path.clone(),
                ));
            }
            return;
        };
        if let SignatureKind::Type(t) = &signature.kind {
            if !t.resource {
                self.error(ViolationKind::InvalidSignature(
                    signature.definition.clone(),
                ));
            } else if self.body.parameters.len() != 1 {
                self.error(ViolationKind::Arity {
                    entity: "finalizer parameters",
                    expected: 1,
                    actual: self.body.parameters.len(),
                });
            } else {
                let self_ty = self.body.locals[self.body.parameters[0].0 as usize].ty;
                let valid = match &self.definition.types.get(self_ty).unwrap().kind {
                    TypeKind::Named { definition, arguments } => definition == &signature.definition
                        && arguments.len() == t.generics.len()
                        && arguments.iter().zip(&t.generics).enumerate().all(|(i, (a, p))| match (a, p) {
                            (GenericArgument::Type(id), GenericParameter::Type { .. }) => matches!(
                                self.definition.types.get(*id).unwrap().kind, TypeKind::Parameter(p) if p.binder() == GenericBinder::Body(self.body.parameter_scope) && p.parameter().0 == i as u32),
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
            if !matches!(&self.definition.types.get(self.body.result).unwrap().kind, TypeKind::Tuple(ts) if ts.is_empty())
            {
                self.error(ViolationKind::TypeMismatch {
                    entity: "finalizer result",
                });
            }
            return;
        }
        if let SignatureKind::Const(result) = &signature.kind {
            if !self.body.parameters.is_empty() {
                self.error(ViolationKind::Arity {
                    entity: "constant parameters",
                    expected: 0,
                    actual: self.body.parameters.len(),
                });
            }
            self.cross_type(
                &signature.types,
                *result,
                self.body.result,
                &[],
                "constant result",
            );
            return;
        }
        let SignatureKind::Function(f) = &signature.kind else {
            self.error(ViolationKind::InvalidSignature(
                signature.definition.clone(),
            ));
            return;
        };
        if !f.generics.is_empty() {
            // Generic bodies retain declaration-scope parameters; import without
            // substitution here. Concrete calls substitute in the caller store.
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
                &signature.types,
                p.ty,
                self.body.locals[local.0 as usize].ty,
                &[],
                "body parameter",
            );
        }
        self.cross_type(
            &signature.types,
            f.result,
            self.body.result,
            &[],
            "body result",
        );
    }

    pub(super) fn place_type(&self, place: PlaceId) -> TypeId {
        self.body.places[place.0 as usize].ty
    }
    fn same(&mut self, a: TypeId, b: TypeId, entity: &'static str) {
        if !semantic_equal(&self.definition.types, a, &self.definition.types, b).unwrap_or(false) {
            self.error(ViolationKind::TypeMismatch { entity });
        }
    }
    /// Arithmetic reads scalar bits without requiring an operand's identity.
    /// Its result must independently have the same complete type with no brand.
    fn arithmetic_type(&mut self, left: TypeId, right: TypeId, result: TypeId) {
        let mut store = self.working_store();
        let mut left_row = store.get(left).unwrap().clone();
        let mut right_row = store.get(right).unwrap().clone();
        left_row.brand = Brand::default();
        right_row.brand = Brand::default();
        let left = store.push(left_row);
        let right = store.push(right_row);
        if !semantic_equal(&store, left, &store, right).unwrap_or(false) {
            self.error(ViolationKind::TypeMismatch {
                entity: "binary operands",
            });
        }
        if !semantic_equal(&store, result, &store, left).unwrap_or(false) {
            self.error(ViolationKind::TypeMismatch {
                entity: "binary result",
            });
        }
    }
    /// A comparison borrows both operands and reads their contents, so no
    /// root evidence about either holder takes part: uniqueness is set aside,
    /// differing relations, arena placement included, meet at a common
    /// borrowed root (a read is the same wherever the content lives), and
    /// brands, nested ones included, are identity evidence rather than operand
    /// type. Cleanup and every other nested qualifier remain exact. Sendability is derived again for a row whose
    /// holder evidence changed, never for one compared as published.
    fn comparison_type(&mut self, left: TypeId, right: TypeId) {
        let mut store = self.working_store();
        let rows = [left, right].map(|t| store.get(t).unwrap().clone());
        let relations = rows[0].modes.relation != rows[1].modes.relation;
        let [left, right] = rows.map(|mut row| {
            let derive = row.modes.uniqueness == Uniqueness::Unique
                || (relations && row.modes.relation != Relation::environment());
            row.modes.uniqueness = Uniqueness::Aliased;
            if relations {
                row.modes.relation = Relation::environment();
            }
            let root = store.push(row);
            // A graph that cannot be rewritten is compared as published.
            let root = hirsel_types::erase_brands(&mut store, root).unwrap_or(root);
            let root = if derive {
                self.derive_sendability(&mut store, root)
            } else {
                root
            };
            hirsel_types::erase_comparison_crossing(&mut store, root).unwrap_or(root)
        });
        if !semantic_equal(&store, left, &store, right).unwrap_or(false) {
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
        source: &mut TypeStore,
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
        let inputs = self.scratch.definition_inputs(self.definition);
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
        store: &mut TypeStore,
        place: PlaceId,
        expected: TypeId,
    ) -> bool {
        self.brandless_variant_from(store, self.site.body, place, expected)
    }

    pub(super) fn brandless_variant_from(
        &mut self,
        store: &mut TypeStore,
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
                        if !matches!(&store.get(expected).unwrap().kind, TypeKind::Named { definition: owner, .. } if *owner == definition)
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
                        let mut types = BTreeSet::new();
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
        store: &mut TypeStore,
        expected: TypeId,
        actual: TypeId,
        value: &Operand,
        entity: &'static str,
    ) {
        // Matching written contracts need no second proof in this shape
        // comparison. Executable transfers check current owned-part evidence.
        let (expected, actual) =
            matching_crossing_contracts(store, expected, actual, &mut BTreeMap::new());
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
        let expected = store.push(receiving);
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
            let acquired = store.push(acquired);
            if !semantic_equal(store, acquired, store, actual).unwrap_or(true)
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
    pub(super) fn received_type(
        &self,
        store: &mut TypeStore,
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
        let types = &self.definition.types;
        matches!((types.get(holder), types.get(value)), (Some(h), Some(v))
            if !self.placed(&h.modes.relation)
                && self.lifetime(&h.modes.relation).has_environment()
                && self.placed(&v.modes.relation))
    }
    fn unplaced_parts(&self, holder: TypeId, value: TypeId) -> bool {
        if self.unplaced_row(holder, value) {
            return true;
        }
        let types = &self.definition.types;
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
        store: &mut TypeStore,
        expected: TypeId,
        actual: TypeId,
        value: &Operand,
        entity: &'static str,
    ) {
        let (expected, actual) =
            matching_crossing_contracts(store, expected, actual, &mut BTreeMap::new());
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
        let expected = store.push(receiving);
        self.operand_value_type_in(store, expected, actual, value, entity);
    }
    /// A deep content proof permits acquisition through Global receiving storage.
    /// Traverse data payloads, never callable contracts or Shared contents.
    fn acquired_global_type(&self, store: &mut TypeStore, root: TypeId) -> TypeId {
        fn visit(
            checker: &Checker<'_>,
            store: &mut TypeStore,
            ty: TypeId,
            seen: &mut BTreeMap<TypeId, TypeId>,
            active: &mut BTreeSet<TypeId>,
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
                store.push(row)
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
            &mut BTreeMap::new(),
            &mut BTreeSet::new(),
        )
    }

    pub(super) fn compatible(
        &self,
        store: &mut TypeStore,
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
        store: &mut TypeStore,
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
        store: &mut TypeStore,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
        reception: Reception,
        context: &hirsel_types::GenericDeclarations,
    ) -> bool {
        let authorities = self.scratch.parameter_authorities[self.site.body.0 as usize]
            .get_or_init(|| {
                let mut authorities = BTreeMap::new();
                for body in &self.definition.bodies {
                    for (ordinal, local) in body.parameters.iter().enumerate() {
                        let Some(local) = body.locals.get(local.0 as usize) else {
                            return None;
                        };
                        let authority = self.authority_with_generics(
                            &self.definition.types,
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
            return false;
        };
        let Ok(expected) = instantiate_parameter_authorities(store, expected, authorities) else {
            return false;
        };
        let Ok(actual) = instantiate_parameter_authorities(store, actual, authorities) else {
            return false;
        };
        let mut lookup = |path: &DefPath| {
            Ok::<_, hirsel_types::QueryError>(
                self.signatures.get(path).cloned().map(std::sync::Arc::new),
            )
        };
        let expected = canonical_plain_type(store, expected, &mut lookup).unwrap_or(expected);
        let actual = canonical_plain_type(store, actual, &mut lookup).unwrap_or(actual);
        match reception {
            Reception::View => {
                compatible_view_type_in(store, expected, actual, relation, context, &mut lookup)
            }
            Reception::Holder => {
                compatible_type(store, expected, actual, relation, context, &mut lookup)
            }
            Reception::HandBack => {
                compatible_handback_type(store, expected, actual, context, &mut lookup)
            }
        }
        .unwrap_or_else(|error| self.proof_failure(error, false))
    }
    pub(super) fn comparison_evidence(
        &self,
        store: &mut TypeStore,
        expected: TypeId,
        actual: TypeId,
    ) -> TypeId {
        conservative_result_evidence(
            store,
            expected,
            actual,
            &mut BTreeMap::new(),
            &mut |store, root| {
                hirsel_types::sendability_after_sharing(
                    store,
                    root,
                    &self.generic_declarations(),
                    &mut |path| Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new)),
                )
                .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable))
                    == Sendability::Sendable
            },
        )
    }
    pub(super) fn derive_sendability(&self, store: &mut TypeStore, root: TypeId) -> TypeId {
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
        store.push(row)
    }
    fn boolean(&mut self, o: &Operand, entity: &'static str) {
        if !matches!(
            self.definition
                .types
                .get(self.place_type(o.place))
                .unwrap()
                .kind,
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
    fn authority(&self, t: TypeId) -> AuthoritySummary {
        self.authority_in(&self.definition.types, t)
    }
    fn authority_in(&self, store: &TypeStore, t: TypeId) -> AuthoritySummary {
        self.authority_with_generics(store, t, &self.generic_declarations())
    }
    pub(super) fn authority_with_generics(
        &self,
        store: &TypeStore,
        t: TypeId,
        generics: &hirsel_types::GenericDeclarations,
    ) -> AuthoritySummary {
        let mut roles = self
            .signatures
            .get(&self.definition.path)
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
    fn imported(
        &self,
        source: &TypeStore,
        root: TypeId,
        target: &mut TypeStore,
        arguments: &[GenericArgument],
    ) -> Option<TypeId> {
        self.imported_in(
            source,
            root,
            target,
            arguments,
            &self.generic_declarations(),
        )
    }
    pub(super) fn imported_in(
        &self,
        source: &TypeStore,
        root: TypeId,
        target: &mut TypeStore,
        arguments: &[GenericArgument],
        generics: &GenericDeclarations,
    ) -> Option<TypeId> {
        if arguments.is_empty() {
            import_type(source, root, target).ok()
        } else {
            substitute_type(
                source,
                root,
                target,
                Substitution {
                    binder: Some(hirsel_types::GenericBinder::Bound(0)),
                    roles: &self
                        .signatures
                        .get(&self.definition.path)
                        .map(|s| s.metadata.roles.clone())
                        .unwrap_or_default(),
                    arguments,
                    authorities: &self.authorities(arguments),
                },
            )
            .ok()
            .map(|imported| {
                if semantic_equal(source, root, target, imported).unwrap_or(false) {
                    imported
                } else {
                    hirsel_types::canonical_current_type(target, imported, generics, &mut |path| {
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
        target: &mut TypeStore,
        root: TypeId,
        arguments: &[GenericArgument],
        generics: &GenericDeclarations,
    ) -> Option<TypeId> {
        let roles = self
            .signatures
            .get(&self.definition.path)
            .map(|s| s.metadata.roles.clone())
            .unwrap_or_default();
        let imported = substitute_type_in_place(
            target,
            root,
            Substitution {
                binder: (!arguments.is_empty()).then_some(GenericBinder::Bound(0)),
                roles: &roles,
                arguments,
                authorities: &self.authorities(arguments),
            },
        )
        .ok()?;
        if arguments.is_empty() || semantic_equal(target, root, target, imported).unwrap_or(false) {
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
        source: &TypeStore,
        root: TypeId,
        actual: TypeId,
        args: &[GenericArgument],
        entity: &'static str,
    ) {
        let mut target = self.working_store();
        let matches = if let Some(expected) = self.imported(source, root, &mut target, args) {
            let expected = if matches!(entity, "body parameter" | "body result") {
                match bind_parameter_scopes(&mut target, expected, &[self.body.parameter_scope]) {
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
                target.push(row)
            } else {
                actual
            };
            self.compatible(&mut target, expected, actual, TypeRelation::Exact)
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
        let types = &self.definition.types;
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
        let types = &self.definition.types;
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
    fn capture_type(&mut self, source: &TypeStore, root: TypeId, operand: &Operand) {
        let actual = self.place_type(operand.place);
        if matches!(operand.usage, UseKind::Borrow | UseKind::Mut) {
            let mut store = self.definition.types.clone();
            if let Some(expected) = self.imported(source, root, &mut store, &[]) {
                self.storage_value_type_in(
                    &mut store,
                    expected,
                    actual,
                    operand,
                    "closure capture",
                );
            } else {
                self.error(ViolationKind::TypeMismatch {
                    entity: "closure capture",
                });
            }
            return;
        }
        if operand.usage != UseKind::Copy {
            self.cross_type(source, root, actual, &[], "closure capture");
            return;
        }
        let mut target = self.working_store();
        let matches = self
            .imported(source, root, &mut target, &[])
            .is_some_and(|expected| {
                // A flat copy owns independent bytes. Counted copies share
                // storage, so derive both uniqueness and sendability before
                // comparing the independently declared capture type.
                let actual = if handle_root(&target, actual) {
                    let mut row = target.get(actual).unwrap().clone();
                    row.modes.relation = Relation::global();
                    row.modes.sendability = target.get(expected).unwrap().modes.sendability.clone();
                    target.push(row)
                } else {
                    actual
                };
                let adjusted = if self.flat_value(operand.place) {
                    let mut row = target.get(actual).unwrap().clone();
                    row.modes.uniqueness = target.get(expected).unwrap().modes.uniqueness;
                    target.push(row)
                } else {
                    self.weakened_type(&mut target, actual, &self.generic_declarations())
                };
                let mut row = target.get(adjusted).unwrap().clone();
                row.sendability_requirement = hirsel_types::SendabilityRequirement::None;
                let adjusted = target.push(row);
                let valid = self.compatible(&mut target, expected, adjusted, TypeRelation::Exact);
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
        source: &TypeStore,
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
    ) {
        let place = value.place;
        let mut target = self.working_store();
        let expected = import_type(source, root, &mut target)
            .ok()
            .and_then(|t| instantiate_allocation_targets(&mut target, t, allocation_targets).ok())
            .and_then(|t| super::flow_origins::sources(&mut target, t, origins, actuals).ok())
            .and_then(|t| instantiate_parameter_authorities(&mut target, t, authorities).ok())
            .and_then(|t| instantiate_parameter_captures(&mut target, t, captures).ok())
            .and_then(|t| {
                super::flow_origins::canonical(&mut target, t, &origins.names, &|o| {
                    self.arena_place(o)
                })
                .ok()
            });
        let expected = expected.and_then(|t| {
            self.imported_in_place(&mut target, t, args, &self.generic_declarations())
        });
        let actual = origins.proofs.get(&place.0).and_then(|proof| {
            super::flow_origins::canonical(&mut target, actual, proof, &|o| self.arena_place(o))
                .ok()
        });
        let (Some(expected), Some(actual)) = (expected, actual) else {
            self.error(ViolationKind::InvalidOrigin(place));
            return;
        };
        let declarations = self.generic_declarations();
        let expected = instantiate_cleanup_bounds(&mut target, expected, &declarations)
            .unwrap_or_else(|error| self.proof_failure(error, expected));
        let actual = instantiate_cleanup_bounds(&mut target, actual, &declarations)
            .unwrap_or_else(|error| self.proof_failure(error, actual));
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
        let actual = target.push(row);
        let actual = self.copied_type(&mut target, value, actual);
        let actual = if origins.current_unique.contains(&place.0)
            && target.get(actual).unwrap().modes.uniqueness != Uniqueness::Unique
        {
            self.unique_type(&mut target, actual)
        } else {
            actual
        };
        let actual = if origins.weakened.contains(&place.0) {
            self.weakened_type(&mut target, actual, &self.generic_declarations())
        } else {
            actual
        };
        let actual = if origins.currently_sendable(value, target.get(actual).unwrap()) {
            let mut row = target.get(actual).unwrap().clone();
            row.modes.sendability = Sendability::Sendable;
            target.push(row)
        } else {
            actual
        };
        // Declaration-owned preservation quantifies over the actual incoming
        // environment; a narrowed formal never discards stronger actual facts.
        let incoming_valid = !preserves_state
            || self.related(
                &mut target,
                expected,
                actual,
                TypeRelation::Parameter,
                Reception::View,
                &self.generic_declarations(),
            );
        let expected = if preserves_state
            && value.usage == UseKind::Mut
            && matches!(
                target.get(actual).unwrap().kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                }
            ) {
            let incoming = target.get(actual).unwrap();
            let mut row = target.get(expected).unwrap().clone();
            row.erased_captures = incoming.erased_captures.clone();
            row.drop_allocation = incoming.drop_allocation.clone();
            target.push(row)
        } else {
            expected
        };
        // PROV-4: a taken value is the callee's to keep, so an unplaced take
        // parameter cannot receive an arena value; a borrow cannot keep it.
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
                &mut target,
                expected,
                actual,
                relation,
                reception,
                &self.generic_declarations(),
            );
        if !matches && incoming_valid && origins.acquired_global.contains(&place.0) {
            let acquired = self.acquired_global_type(&mut target, actual);
            matches = self.related(
                &mut target,
                expected,
                acquired,
                relation,
                reception,
                &self.generic_declarations(),
            );
            if !matches && value.usage == UseKind::Take {
                let receiving = self.acquired_global_type(&mut target, expected);
                matches = self.related(
                    &mut target,
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
    pub(super) fn layout(&mut self, store: &mut TypeStore, ty: TypeId) -> Option<TypeKind> {
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
                .enter(
                    ty,
                    0,
                    |a, b| semantic_equal(&store, a, &store, b).unwrap_or(false),
                    |a, b| hirsel_types::nominal_arguments_grow(&store, a, &store, b),
                )
                .unwrap_or_else(|error| self.proof_failure(error, false))
            {
                if self.proof_errors.borrow().is_empty() {
                    self.error(ViolationKind::InvalidTypeGraph);
                }
                return None;
            }
            let Some(s) = self.signatures.get(&definition) else {
                self.error(ViolationKind::MissingSignature(definition));
                return None;
            };
            let SignatureKind::Type(t) = &s.kind else {
                self.error(ViolationKind::InvalidSignature(definition));
                return None;
            };
            if (t.capability.is_some()
                || s.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque)
                && s.definition.0.package != self.definition.path.0.package
            {
                // A forged input can retain an owner's private graph. Its mere
                // presence does not authorize a client to inspect it (PROG-2).
                return None;
            }
            let Some(rep) = t.representation else {
                self.error(ViolationKind::InvalidConstructor);
                return None;
            };
            // Arguments here already belong to `store`, not the signature store.
            ty = if arguments.is_empty() {
                import_type(&s.types, rep, store).ok()?
            } else {
                substitute_type(
                    &s.types,
                    rep,
                    store,
                    Substitution {
                        binder: Some(hirsel_types::GenericBinder::Bound(0)),
                        roles: &s.metadata.roles,
                        arguments: &arguments,
                        authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                    },
                )
                .ok()?
            };
        }
    }

    pub(super) fn call_type(
        &mut self,
        callee: &Callee,
        arguments: &[CallArgument],
        destination: Option<PlaceId>,
        origins: Option<&super::flow_origins::CallOriginProof>,
    ) {
        let (store, f, generics) = match callee {
            Callee::Definition { path, arguments } => {
                let Some(s) = self.signatures.get(path) else {
                    self.error(ViolationKind::MissingSignature(path.clone()));
                    return;
                };
                if hirsel_types::schema_failure(
                    &self.definition.types,
                    arguments,
                    &s.metadata.schema,
                    |ty, bound| {
                        self.bound(
                            &self.definition.types,
                            ty,
                            bound,
                            &self.generic_declarations(),
                        )
                    },
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
                (&s.types, f, arguments.as_slice())
            }
            Callee::Indirect {
                value: o,
                arguments: generics,
            } => {
                let row = self.definition.types.get(self.place_type(o.place)).unwrap();
                if row.modes.multiplicity == Multiplicity::Once && o.usage != UseKind::Take {
                    self.error(ViolationKind::InvalidUse(o.place));
                }
                match &row.kind {
                    TypeKind::Function(f) => (&self.definition.types, f, generics.as_slice()),
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
                if !generic_references_valid(
                    &self.definition.types,
                    *t,
                    &self.generic_declarations(),
                ) {
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
            let mut target = self.working_store();
            let never = import_type(store, f.result, &mut target)
                .ok()
                .and_then(|t| {
                    self.imported_in_place(&mut target, t, generics, &self.generic_declarations())
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
                let mut types = self.working_store();
                let derived = self.derive_sendability(&mut types, t);
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
                    &self.definition.types,
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
                self.definition
                    .types
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
            if let Some(origins) = origins {
                self.parameter_type(
                    store,
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
                );
            }
            let plain_take_borrow = p.passing == PassingMode::Borrow
                && a.value.usage == UseKind::Take
                && !super::flow::needs_release(
                    self,
                    self.place_type(a.value.place),
                    &self.definition.types,
                );
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
        let mut target = self.working_store();
        // Substitute the value's type contract before its dependent source
        // evidence. A generic actual can carry Local access modes; those
        // cannot overwrite the lifetime supplied by local(x).
        let imported_result = import_type(store, f.result, &mut target)
            .ok()
            .and_then(|t| {
                self.imported_in_place(&mut target, t, generics, &self.generic_declarations())
            });
        let actual_result = self.place_type(destination);
        let inactive = imported_result.is_some_and(|expected| {
            target.get(expected).unwrap().brand.alternatives.is_empty()
                && target.get(actual_result).unwrap().brand.alternatives.is_empty()
                && matches!((&target.get(expected).unwrap().kind, &target.get(actual_result).unwrap().kind),
                    (TypeKind::Named { definition: a, .. }, TypeKind::Named { definition: b, .. }) if a == b)
                && self.brandless_variant_operand(&mut target, destination, expected)
                && self.brandless_variant_operand(&mut target, destination, actual_result)
        });
        // An absent payload needs no owner-name replay for its phantom brand.
        // Both declaration and receiving contracts must independently prove it;
        // active unbranded payloads cannot acquire a receiving brand here.
        let expected = imported_result
            .and_then(|t| {
                if inactive {
                    hirsel_types::erase_brands(&mut target, t).ok()
                } else {
                    Some(t)
                }
            })
            .and_then(|t| instantiate_allocation_targets(&mut target, t, &allocation_targets).ok())
            .map(|t| {
                hirsel_types::canonical_current_type(
                    &mut target,
                    t,
                    &self.generic_declarations(),
                    &mut |path| Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new)),
                )
                .unwrap_or_else(|error| self.proof_failure(error, t))
            })
            .and_then(|t| super::flow_origins::sources(&mut target, t, origins, &actuals).ok())
            .and_then(|t| instantiate_parameter_authorities(&mut target, t, &authorities).ok())
            .and_then(|t| instantiate_parameter_captures(&mut target, t, &captures).ok())
            .and_then(|t| {
                super::flow_origins::canonical(&mut target, t, &origins.names, &|o| {
                    self.arena_place(o)
                })
                .ok()
            });
        let expected = expected.map(|root| {
            // Actual result sources can discharge a declaration's dependent
            // placement. Rebuild an underclaim after that substitution while
            // retaining stronger validated current-content promises.
            if target.get(root).unwrap().modes.sendability == Sendability::NotSendable {
                self.derive_sendability(&mut target, root)
            } else {
                root
            }
        });
        let expected = expected
            .and_then(|root| hirsel_types::declared_receipt_evidence(&mut target, root).ok());
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
        let expected = expected.map(|root| {
            let mut row = target.get(root).unwrap().clone();
            if sharing_result
                && row.sendability_requirement == SendabilityRequirement::None
                && row.modes.uniqueness == Uniqueness::Aliased
                && matches!(row.kind, TypeKind::Parameter(_))
            {
                row.modes.sendability = hirsel_types::sendability_after_sharing(
                    &target,
                    root,
                    &self.generic_declarations(),
                    &mut |path| Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new)),
                )
                .unwrap_or(Sendability::NotSendable);
            }
            target.push(row)
        });
        let actual_result = if inactive {
            hirsel_types::erase_brands(&mut target, actual_result).unwrap_or(actual_result)
        } else {
            actual_result
        };
        let actual =
            super::flow_origins::canonical(&mut target, actual_result, &origins.names, &|o| {
                self.arena_place(o)
            })
            .ok();
        // Both sides as this frame sees them: the source checker joins
        // distinct argument lifetimes into the frame, and a borrowed
        // parameter's relation adds to its own root (S2).
        let mut frame = |ty: TypeId| {
            let mut row = target.get(ty).unwrap().clone();
            row.modes.relation = self.frame_relation(&row.modes.relation);
            target.push(row)
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
            target.push(row)
        });
        let actual = actual.map(|actual| {
            expected.map_or(actual, |expected| {
                self.comparison_evidence(&mut target, expected, actual)
            })
        });
        let valid = matches!((expected, actual), (Some(expected), Some(actual)) if self.compatible(&mut target, expected, actual, TypeRelation::Exact));
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
        let types = &self.definition.types;
        let mut pending = vec![root];
        let mut seen = BTreeSet::new();
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
            store: &TypeStore,
            root: TypeId,
            seen: &mut BTreeSet<TypeId>,
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
                if let Some(signature) = checker.signatures.get(definition) {
                    if let SignatureKind::Type(d) = &signature.kind {
                        return d
                            .representation
                            .is_some_and(|r| visit(checker, &signature.types, r, seen));
                    }
                }
            }
            false
        }
        visit(self, &self.definition.types, root, &mut BTreeSet::new())
    }
    fn data(&self, root: TypeId) -> bool {
        self.bound(
            &self.definition.types,
            root,
            Bound::Data,
            &self.generic_declarations(),
        )
    }
    /// A row's own destination: the arena or region its relation names, else
    /// unknown. Sources shared with the caller place nothing for a row.
    pub(super) fn row_destination(&self, ty: TypeId) -> Destination {
        match destination(
            &self.definition.types.get(ty).unwrap().modes.relation,
            |o| self.arena_place(o),
        ) {
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

    fn rvalue_type(&mut self, destination: PlaceId, r: &Rvalue) {
        self.construction_effects(destination, r);
        let ty = self.place_type(destination);
        let kind = &self.definition.types.get(ty).unwrap().kind;
        let computed = matches!(
            r,
            Rvalue::Constant(_)
                | Rvalue::Size(_)
                | Rvalue::Shape(_)
                | Rvalue::Binary { .. }
                | Rvalue::Construct { .. }
                | Rvalue::Closure { .. }
        ) || matches!(r, Rvalue::Unary { op, .. } if *op != U::Unique);
        if computed
            && !self
                .definition
                .types
                .get(ty)
                .unwrap()
                .brand
                .alternatives
                .is_empty()
        {
            self.error(ViolationKind::InvalidOrigin(destination));
        }
        match r {
            Rvalue::Constant(c) => {
                let valid = match (c, kind) {
                    (Constant::Unit, TypeKind::Tuple(ts)) => ts.is_empty(),
                    (Constant::Bool(_), TypeKind::Primitive(Primitive::Bool))
                    | (Constant::F32(_), TypeKind::Primitive(Primitive::F32))
                    | (Constant::F64(_), TypeKind::Primitive(Primitive::F64))
                    | (Constant::String(_), TypeKind::Primitive(Primitive::String))
                    | (Constant::Bytes(_), TypeKind::Primitive(Primitive::Bytes)) => true,
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
                };
                if !valid {
                    self.error(ViolationKind::TypeMismatch { entity: "constant" });
                }
            }
            Rvalue::FunctionReference {
                definition,
                arguments,
            } => {
                let Some(signature) = self.signatures.get(definition) else {
                    self.error(ViolationKind::MissingSignature(definition.clone()));
                    return;
                };
                let SignatureKind::Function(declared) = &signature.kind else {
                    self.error(ViolationKind::InvalidSignature(definition.clone()));
                    return;
                };
                let mut function = declared.clone();
                function.generics = local_result_generics(&signature.types, declared).into();
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
                                    if !self.bound(
                                        &self.definition.types,
                                        *t,
                                        *bound,
                                        &self.generic_declarations(),
                                    ) {
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
                let mut store = self.working_store();
                for parameter in &mut function.parameters {
                    let Some(imported) =
                        self.imported(&signature.types, parameter.ty, &mut store, arguments)
                    else {
                        self.error(ViolationKind::InvalidTypeGraph);
                        return;
                    };
                    parameter.ty = imported;
                }
                let Some(result) =
                    self.imported(&signature.types, function.result, &mut store, arguments)
                else {
                    self.error(ViolationKind::InvalidTypeGraph);
                    return;
                };
                function.result = result;
                if let Some(error) = function.error {
                    function.error = self.imported(&signature.types, error, &mut store, arguments);
                }
                if !arguments.is_empty() {
                    let type_authorities: Vec<_> = arguments
                        .iter()
                        .map(|argument| match argument {
                            GenericArgument::Type(ty) => self.authority_with_generics(
                                &self.definition.types,
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
                let derived = store.push(Type {
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
                let matches = match callback {
                    Some(signature) => {
                        hirsel_types::shape_equal(&store, signature, &store, derived)
                            .unwrap_or(false)
                    }
                    None => self.compatible(&mut store, ty, derived, TypeRelation::Value),
                };
                if !matches {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "function reference",
                    });
                }
            }
            Rvalue::ReadConstant { definition } => {
                let Some(signature) = self.signatures.get(definition) else {
                    self.error(ViolationKind::InvalidOrigin(destination));
                    return;
                };
                let SignatureKind::Const(root) = signature.kind else {
                    self.error(ViolationKind::InvalidOrigin(destination));
                    return;
                };
                let mut store = self.working_store();
                let Some(mut expected) = self.imported(&signature.types, root, &mut store, &[])
                else {
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
                    expected = self.weakened_type(&mut store, expected, &generics);
                }
                if !self.compatible(&mut store, expected, ty, TypeRelation::Exact) {
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
                let mut store = self.working_store();
                let actual = self.place_type(o.place);
                let mut expected = ty;
                if self.unplaced_holder(destination) {
                    expected = hirsel_types::adopt_local(&mut store, ty, actual);
                }
                self.storage_value_type_in(&mut store, expected, actual, o, "assignment");
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
                    let mut store = self.working_store();
                    let mut result = store.get(self.place_type(value.place)).unwrap().clone();
                    result.modes.uniqueness = Uniqueness::Unique;
                    result.modes.relation = self.placement(&result.modes.relation);
                    let result = store.push(result);
                    let result = self.derive_sendability(&mut store, result);
                    self.value_type_in(&mut store, ty, result, "unique result");
                    return;
                }
                let mut store = self.working_store();
                let mut result = store.get(self.place_type(value.place)).unwrap().clone();
                result.brand = Brand::default();
                let result = store.push(result);
                if !semantic_equal(&store, ty, &store, result).unwrap_or(false) {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "unary result",
                    });
                }
                let k = &self
                    .definition
                    .types
                    .get(self.place_type(value.place))
                    .unwrap()
                    .kind;
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
                let k = &self.definition.types.get(lt).unwrap().kind;
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
                self.construction_placement(ty, placement, "constructor placement");
                if matches!(constructor, Constructor::Map | Constructor::Set) {
                    let values = self
                        .static_data
                        .get_or_insert_with(|| super::static_data::Values::new(self.body));
                    if !values.distinct_keys(self.body, constructor, fields) {
                        self.error(ViolationKind::InvalidConstructor);
                    }
                }
                let mut store = self.working_store();
                let expected = self.constructor_layout(&mut store, ty, constructor, fields.len());
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
                            expected_field = store.push(field_row);
                        }
                        let mut receiving = store.get(expected_field).unwrap().clone();
                        receiving.sendability_requirement =
                            hirsel_types::SendabilityRequirement::None;
                        expected_field = store.push(receiving);
                        // Cleanup summaries remain exact. Placement uses the
                        // shared Value relation: heap values outlive arenas.
                        if store.get(*t).unwrap().drop_allocation
                            != store.get(actual).unwrap().drop_allocation
                        {
                            self.error(ViolationKind::TypeMismatch {
                                entity: "constructor field",
                            });
                        } else {
                            self.operand_value_type_in(
                                &mut store,
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
            }
            Rvalue::Closure {
                body,
                captures,
                destination,
            } => {
                self.construction_placement(ty, destination, "closure placement");
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
                        self.definition.types.get(f.result).unwrap().kind,
                        TypeKind::Primitive(Primitive::Bool)
                    ) {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "contract result",
                        });
                    }
                }
                if self.definition.types.get(ty).unwrap().drop_allocation == DropAllocation::NoAlloc
                    && captures
                        .iter()
                        .filter(|c| matches!(c.usage, UseKind::Copy | UseKind::Take))
                        .any(|c| {
                            !self.bound(
                                &self.definition.types,
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
                let mut sendability = Sendability::Sendable;
                let mut once = false;
                for capture in captures {
                    let source = self.place_type(capture.place);
                    let mut row = self.definition.types.get(source).unwrap().clone();
                    // MEM-11: a copied Shared root is an independently
                    // counted global handle, wherever the root was read.
                    if capture.usage == UseKind::Copy && handle_root(&self.definition.types, source)
                    {
                        row.modes.relation = Relation::global();
                    }
                    let row = &row;
                    let global_closure = self
                        .lifetime(&self.definition.types.get(ty).unwrap().modes.relation)
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
                    let mut store = self.working_store();
                    let mut captured = row.clone();
                    if capture.usage == UseKind::Copy {
                        captured.modes.uniqueness = if self.flat_value(capture.place) {
                            Uniqueness::Unique
                        } else {
                            Uniqueness::Aliased
                        };
                    }
                    let captured = store.push(captured);
                    let generics = self.generic_declarations();
                    let mut lookup = |path: &DefPath| {
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
                    let derived = if capture.usage == UseKind::Copy {
                        hirsel_types::sendability_after_sharing(
                            &store,
                            captured,
                            &generics,
                            &mut lookup,
                        )
                    } else {
                        hirsel_types::content_sendability(&store, captured, &generics, &mut lookup)
                    }
                    .unwrap_or_else(|error| {
                        self.proof_failure(error, hirsel_types::Sendability::NotSendable)
                    });
                    sendability = match (sendability, derived) {
                        (Sendability::NotSendable, _) | (_, Sendability::NotSendable) => {
                            Sendability::NotSendable
                        }
                        (Sendability::Sendable, b) | (b, Sendability::Sendable) => b,
                        (Sendability::DependsOn(mut a), Sendability::DependsOn(b)) => {
                            a.extend(b);
                            a.sort();
                            a.dedup();
                            Sendability::DependsOn(a)
                        }
                    };
                }
                let uses = authority.clone();
                let mut store = self.working_store();
                let mut derived = store.get(ty).unwrap().clone();
                if let TypeKind::Function(function) = &mut derived.kind {
                    function.uses = Uses::Only(uses);
                }
                let derived = store.push(derived);
                if !self.compatible(&mut store, ty, derived, TypeRelation::Value) {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "closure uses",
                    });
                }
                if !authority.is_subset_of(&f.authority) {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "closure authority",
                    });
                }
                if once
                    && self.definition.types.get(ty).unwrap().modes.multiplicity
                        != Multiplicity::Once
                {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "closure multiplicity",
                    });
                }
                let claimed = &self.definition.types.get(ty).unwrap().modes.sendability;
                // Current unconditional evidence is checked against live
                // capture operands during flow replay. A physical storage row
                // can underclaim a freshly replaced referent's proof.
                if matches!(claimed, Sendability::DependsOn(_)) && *claimed != sendability {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "closure sendability",
                    });
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
                    self.capture_type(&self.definition.types, l.ty, o);
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
                if nested.parameters.len() != f.parameters.len() {
                    self.error(ViolationKind::Arity {
                        entity: "closure parameters",
                        expected: f.parameters.len(),
                        actual: nested.parameters.len(),
                    });
                }
                let specialised = self.environment_specialised(ty, *body, captures);
                let sendable_environment = captures.iter().all(|c| {
                    self.definition
                        .types
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
                    let mut store = self.working_store();
                    let Ok(ty) =
                        import_type(&self.definition.types, self.place_type(c.place), &mut store)
                    else {
                        return false;
                    };
                    let mut row = store.get(ty).unwrap().clone();
                    row.modes.relation = self.placement(&row.modes.relation);
                    row.modes.uniqueness = hirsel_types::Uniqueness::Unique;
                    let root = store.push(row);
                    let derived = self.derive_sendability(&mut store, root);
                    store.get(derived).unwrap().modes.sendability == Sendability::Sendable
                });
                let mut target = self.working_store();
                for (actual, expected, entity) in nested
                    .parameters
                    .iter()
                    .zip(&f.parameters)
                    .map(|(l, p)| (nested.locals[l.0 as usize].ty, p.ty, "closure parameter"))
                    .chain([(nested.result, f.result, "closure result")])
                {
                    let actual = import_type(&self.definition.types, actual, &mut target);
                    let actual = match actual {
                        Ok(actual) if specialised && entity == "closure result" => {
                            // A global result's sendability is structural again;
                            // a share of a sendable environment is sendable, as a
                            // function value's sendability is its environment's.
                            let mut row = target.get(actual).unwrap().clone();
                            row.modes.relation = self.placement(&row.modes.relation);
                            let root = target.push(row.clone());
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
                            Ok(target.push(row))
                        }
                        actual => actual,
                    };
                    let expected =
                        bind_parameter_scopes(&mut target, expected, &[nested.parameter_scope]);
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
                            row.modes.sendability = hirsel_types::content_sendability(
                                &target,
                                expected,
                                &self.declarations[body.0 as usize],
                                &mut |path| {
                                    Ok(self.signatures.get(path).cloned().map(std::sync::Arc::new))
                                },
                            )
                            .unwrap_or_else(|error| {
                                self.proof_failure(error, Sendability::NotSendable)
                            });
                        }
                        target.push(row)
                    });
                    let actual = actual.map(|actual| {
                        if entity == "closure parameter" {
                            expected.as_ref().ok().map_or(actual, |expected| {
                                self.comparison_evidence(&mut target, *expected, actual)
                            })
                        } else {
                            actual
                        }
                    });
                    // Parameters stay exact. A literal's executable result may
                    // provide stronger evidence than its chosen result promise.
                    let (relation, reception) = if entity == "closure result" {
                        (TypeRelation::Value, Reception::View)
                    } else {
                        (TypeRelation::Exact, Reception::Holder)
                    };
                    if !matches!((expected,actual),(Ok(expected),Ok(actual)) if self.related(&mut target,expected,actual,relation,reception,&self.declarations[body.0 as usize]))
                    {
                        self.error(ViolationKind::TypeMismatch { entity });
                    }
                }
            }
            Rvalue::Shape(t) => {
                if !matches!(kind, TypeKind::Builtin { kind: BuiltinType::Shape, arguments } if arguments.len() == 1 && matches!(arguments[0], GenericArgument::Type(id) if semantic_equal(&self.definition.types, id, &self.definition.types, *t).unwrap_or(false)))
                {
                    self.error(ViolationKind::TypeMismatch { entity: "shape" });
                }
            }
        }
    }
    pub(super) fn has_origins(store: &TypeStore, ty: TypeId) -> bool {
        let mut found = false;
        let _ = visit_type_origins(store, ty, |_, _| {
            found = true;
            Ok(())
        });
        found
    }
    pub(super) fn projection_type(
        &mut self,
        store: &mut TypeStore,
        p: &Place,
        sibling: impl FnMut(usize, FieldId) -> Option<Origin>,
    ) -> Option<TypeId> {
        self.projection_type_at(store, p, self.body.locals[p.root.0 as usize].ty, sibling)
    }
    pub(super) fn projection_type_at(
        &mut self,
        store: &mut TypeStore,
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
                    if !matches!(self.definition.types.get(self.body.locals[index.0 as usize].ty).unwrap().kind, TypeKind::Primitive(p) if integer(p))
                    {
                        return None;
                    }
                    let fields = kind.map_entry_fields().unwrap();
                    Some(self.map_entry_type(store, fields, &generics))
                }
                (Projection::Index(index), TypeKind::Primitive(Primitive::Bytes)) => {
                    if !matches!(self.definition.types.get(self.body.locals[index.0 as usize].ty).unwrap().kind, TypeKind::Primitive(p) if integer(p))
                    {
                        return None;
                    }
                    {
                        let mut byte = Type::boolean();
                        byte.kind = TypeKind::Primitive(Primitive::U8);
                        Some(store.push(byte))
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
                    if !matches!(self.definition.types.get(self.body.locals[index.0 as usize].ty).unwrap().kind, TypeKind::Primitive(p) if integer(p))
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
                let next = store.push(projected);
                ty = projected_type(store, ty, next, &generics, &mut |path| {
                    Ok::<_, hirsel_types::QueryError>(
                        self.signatures.get(path).cloned().map(std::sync::Arc::new),
                    )
                })
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
        let mut store = self.working_store();
        for (depth, projection) in access.projection.iter().enumerate() {
            if !matches!(projection, Projection::Index(_)) {
                continue;
            }
            let mut prefix = access.clone();
            prefix.projection.truncate(depth);
            let Some(owner) = self.projection_type(&mut store, &prefix, |_, _| None) else {
                continue;
            };
            if self
                .layout(&mut store, owner)
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
        let mut store = self.working_store();
        for (depth, projection) in access.projection.iter().enumerate() {
            if !matches!(projection, Projection::Index(_)) {
                continue;
            }
            let mut prefix = access.clone();
            prefix.projection.truncate(depth);
            if self
                .projection_type(&mut store, &prefix, |_, _| None)
                .and_then(|owner| self.layout(&mut store, owner))
                .is_some_and(|kind| matches!(kind, TypeKind::Primitive(Primitive::Bytes)))
            {
                return true;
            }
        }
        false
    }
    fn has_siblings(store: &TypeStore, ty: TypeId) -> bool {
        let mut found = false;
        let _ = visit_type_origins(store, ty, |origin, _| {
            found |= matches!(origin.root, OriginRoot::Sibling(_));
            Ok(())
        });
        found
    }
    fn placed_part(
        &self,
        store: &mut TypeStore,
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
        store: &mut TypeStore,
        fields: [TypeId; 2],
        generics: &hirsel_types::GenericDeclarations,
    ) -> TypeId {
        let kind = TypeKind::Tuple(fields.to_vec().into());
        let drop_allocation = cleanup_summary(&kind, |leaf| match leaf {
            CleanupLeaf::Component(t) => store.get(t).unwrap().drop_allocation.clone(),
            _ => unreachable!("tuple cleanup contains only components"),
        });
        let tuple = store.push(Type {
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
        store: &mut TypeStore,
        ty: TypeId,
        constructor: &Constructor,
        field_count: usize,
    ) -> Option<Vec<TypeId>> {
        let layout = self.layout(store, ty);
        let fields = match (constructor, layout) {
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
                    let part = projected_type(store, ty, part, &generics, &mut |path| {
                        Ok::<_, hirsel_types::QueryError>(
                            self.signatures.get(path).cloned().map(std::sync::Arc::new),
                        )
                    })
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
        let mut store = self.working_store();
        let Some(expected) = self.constructor_layout(
            &mut store,
            self.place_type(destination),
            constructor,
            fields.len(),
        ) else {
            return;
        };
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
            let expected = instantiate_sibling_origins(&mut store, ty, &siblings)
                .ok()
                .and_then(|ty| {
                    super::flow_origins::canonical(&mut store, ty, &origins.names, &|o| {
                        self.arena_place(o)
                    })
                    .ok()
                });
            let actual = origins.proofs.get(&field.value.place.0).and_then(|proof| {
                super::flow_origins::canonical(
                    &mut store,
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
            let actual = store.push(row);
            let actual = if origins.current_unique.contains(&field.value.place.0) {
                self.unique_type(&mut store, actual)
            } else if origins.weakened.contains(&field.value.place.0) {
                self.weakened_type(&mut store, actual, &self.generic_declarations())
            } else {
                actual
            };
            let declared = store.get(expected).unwrap();
            let supplied = store.get(actual).unwrap();
            if declared.drop_allocation != supplied.drop_allocation {
                self.error(ViolationKind::TypeMismatch {
                    entity: "constructor field",
                });
            } else {
                self.operand_value_type_in(
                    &mut store,
                    expected,
                    actual,
                    &field.value,
                    "constructor field",
                );
            }
        }
    }
    fn constructor_owner(&mut self, path: &DefPath, ty: TypeId) -> bool {
        let ty = match &self.definition.types.get(ty).unwrap().kind {
            TypeKind::Builtin {
                kind: BuiltinType::Packed,
                arguments,
            } => match arguments.as_slice() {
                [GenericArgument::Type(payload)] => *payload,
                _ => return false,
            },
            _ => ty,
        };
        if let TypeKind::Named { definition, .. } = &self.definition.types.get(ty).unwrap().kind {
            return definition == path;
        }
        let Some(s) = self.signatures.get(path) else {
            self.error(ViolationKind::MissingSignature(path.clone()));
            return true;
        };
        if let SignatureKind::Type(t) = &s.kind {
            if let Some(rep) = t.representation {
                return semantic_equal(&s.types, rep, &self.definition.types, ty).unwrap_or(false);
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
    store: &mut TypeStore,
    expected: TypeId,
    actual: TypeId,
    memo: &mut BTreeMap<(TypeId, TypeId), TypeId>,
    stable: &mut impl FnMut(&TypeStore, TypeId) -> bool,
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
    let result = store.push(row);
    memo.insert((expected, actual), result);
    result
}

// Written requirements and current proof are separate. On a receiving-shape
// comparison, two identical qualified owned edges already agree as contracts.
// Remove their common requirement (including its duplicate erased promise)
// from scratch rows, without supplying either value any current proof.
pub(super) fn matching_crossing_contracts(
    store: &mut TypeStore,
    expected: TypeId,
    actual: TypeId,
    memo: &mut BTreeMap<(TypeId, TypeId), (TypeId, TypeId)>,
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
    let result = (store.push(a), store.push(b));
    memo.insert((expected, actual), result);
    result
}

/// Assign/Return flow independently proves a received value's content
/// properties (uniqueness and crossing evidence), on every return path.
/// Storage and current value evidence have distinct lifetimes.
fn receiving_storage(store: &mut TypeStore, expected: TypeId) -> TypeId {
    let mut receiving = store.get(expected).unwrap().clone();
    receiving.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
    receiving.modes.sendability = hirsel_types::Sendability::NotSendable;
    receiving.sendability_requirement = hirsel_types::SendabilityRequirement::None;
    store.push(receiving)
}
