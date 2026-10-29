//! Invocation promises are checked against executable calls, independently of K.
use super::*;
use hirsel_types::{
    Allocation, AuthoritySummary, AuthorityTerm, BuiltinType, Destination, FunctionType,
    GenericArgument, ParameterRef, SignatureKind, TypeParameterRef, Uses,
    destination as destination_of,
};

impl Checker<'_, '_> {
    pub(super) fn executable_header(&self) -> Option<&FunctionType> {
        if self.site.body == self.definition.entry {
            return None;
        }
        let inputs = self.scratch.definition_inputs(self.t, self.definition);
        let TypeKind::Function(function) = &self
            .t
            .get(inputs.producers[self.site.body.0 as usize].last_header?)
            .unwrap()
            .kind
        else {
            return None;
        };
        Some(function)
    }

    pub(super) fn invocation_header(&self) -> Option<&FunctionType> {
        if self.site.body == self.definition.entry {
            return self
                .signatures
                .get(self.definition.path)
                .and_then(|signature| {
                    if let SignatureKind::Function(function) = &signature.kind {
                        Some(function)
                    } else {
                        None
                    }
                });
        }
        self.executable_header()
    }

    /// Resolve comparison selectors from V's signature map, independently of
    /// elaboration. Only exact capability identities and empty summaries may
    /// replace a selector; callback environments retain their own receipt.
    pub(super) fn comparison_authorities(
        &self,
        store: &TypeInterner,
        root: TypeId,
    ) -> Option<TypeId> {
        hirsel_types::normalize_callable_authorities(store, root, |ty| {
            if let Some(TypeKind::Named { definition, .. }) = store.get(ty).map(|row| &row.kind)
                && self.signatures.get(*definition).is_some_and(|signature|
                    matches!(&signature.kind, SignatureKind::Type(declaration) if declaration.capability.is_some()))
            {
                return Some(AuthoritySummary::Known(vec![AuthorityTerm::Capability(definition.path().clone())].into()));
            }
            (self.authority_with_generics(store, ty, &self.generic_declarations()) == AuthoritySummary::inert())
                .then(AuthoritySummary::inert)
        }).ok()
    }

    pub(super) fn noalloc_body(&self) -> bool {
        self.executable_header()
            .is_some_and(|header| header.allocation == Allocation::NoAlloc)
            || self
                .seal_finalizer()
                .is_some_and(|d| d.destructor_allocation == Allocation::NoAlloc)
    }

    fn seal_finalizer(&self) -> Option<&hirsel_types::TypeDefinition> {
        if self.site.body != self.definition.entry {
            return None;
        }
        match &self.signatures.get(self.definition.path)?.kind {
            SignatureKind::Type(d) if d.resource && d.capability.is_some() => Some(d),
            _ => None,
        }
    }

    /// A replacement only detaches when its address traverses counted storage.
    /// Reconstruct each owner from declarations, not the producer's leaf row.
    pub(super) fn replacement_allocates(&mut self, place: PlaceId, unique: bool) -> bool {
        let mut access = self.body.places[place.0 as usize].clone();
        let path = access.projection.clone();
        let root = self
            .t
            .get(self.body.locals[access.root.0 as usize].ty)
            .unwrap();
        if !path.is_empty() && root.modes.relation.scoped(|o| self.arena_place(o)) {
            return true;
        }
        if unique {
            return false;
        }
        let store = self.t;
        for depth in 0..path.len() {
            access.projection.truncate(depth);
            let Some(owner) = self.projection_type(store, &access, |_, _| None) else {
                return true;
            };
            let Some(kind) = self.layout(store, owner) else {
                return true;
            };
            if matches!(
                kind,
                TypeKind::Box(_)
                    | TypeKind::Builtin {
                        kind: BuiltinType::Array
                            | BuiltinType::List
                            | BuiltinType::Map
                            | BuiltinType::SortedMap,
                        ..
                    }
            ) {
                return true;
            }
            access.projection = path.clone();
        }
        false
    }

    /// The roots whose content may leave the frame. A Plain value owns no
    /// blocks (MEM-5), so moving or copying one, such as a `u64` read
    /// out of a local list, makes nothing of its root escape.
    fn escaping_roots(&self) -> &BTreeSet<u32> {
        self.scratch.escaping_roots[self.site.body.0 as usize].get_or_init(|| {
            let mut edges = vec![Vec::new(); self.body.locals.len()];
            let mut seeds = vec![];
            let plain = |place: PlaceId| {
                self.bound(
                    self.t,
                    self.place_type(place),
                    hirsel_types::Bound::Plain,
                    self.generic_declarations(),
                )
            };
            let root = |place: PlaceId| self.body.places[place.0 as usize].root.0;
            for block in &self.body.blocks {
                for instruction in &block.instructions {
                    match &instruction.kind {
                        InstructionKind::Assign { destination, value } => {
                            let operands: Vec<&Operand> = match &self.body.rvalues[value.0 as usize]
                            {
                                Rvalue::Use(value) | Rvalue::Unary { value, .. } => vec![value],
                                Rvalue::Construct { fields, .. } => {
                                    fields.iter().map(|field| &field.value).collect()
                                }
                                Rvalue::Closure { captures, .. } => captures.iter().collect(),
                                // A named constant is a checked readonly snapshot,
                                // rather than a caller construction of its value.
                                _ => vec![],
                            };
                            edges[root(*destination) as usize].extend(
                                operands
                                    .into_iter()
                                    .filter(|value| !plain(value.place))
                                    .map(|value| root(value.place)),
                            );
                        }
                        InstructionKind::Replace { value, .. } if !plain(value.place) => {
                            seeds.push(root(value.place))
                        }
                        _ => (),
                    }
                }
                match &block.terminator {
                    Terminator::Return { value, .. } if !plain(value.place) => {
                        seeds.push(root(value.place))
                    }
                    Terminator::Call {
                        callee,
                        arguments,
                        returns,
                        ..
                    } => {
                        let callable = match callee {
                            Callee::Definition { path, .. } => {
                                self.signatures.get(path).and_then(|signature| {
                                    match &signature.kind {
                                        SignatureKind::Function(function) => {
                                            Some((self.t, function))
                                        }
                                        _ => None,
                                    }
                                })
                            }
                            Callee::Indirect { value, .. } => {
                                match &self.t.get(self.place_type(value.place)).unwrap().kind {
                                    TypeKind::Function(function) => Some((self.t, function)),
                                    _ => None,
                                }
                            }
                        };
                        if let Some((types, function)) = callable {
                            let locality = &types.get(function.result).unwrap().modes.relation;
                            for argument in arguments.iter().filter(|a| !plain(a.value.place)) {
                                let parameter =
                                    function.parameters.get(argument.parameter.0 as usize);
                                if parameter.is_some_and(|parameter| {
                                    parameter.passing == hirsel_syntax::PassingMode::Take
                                        && self
                                            .signature_lifetime(
                                                types,
                                                function,
                                                &types.get(parameter.ty).unwrap().modes.relation,
                                            )
                                            .is_global()
                                }) {
                                    seeds.push(root(argument.value.place));
                                }
                                let retained = locality.has_environment()
                                    || locality
                                        .parameter_roots()
                                        .contains(&ParameterRef::bound(argument.parameter));
                                if let (true, Some(r)) = (retained, returns) {
                                    edges[root(r.destination) as usize]
                                        .push(root(argument.value.place));
                                }
                            }
                        }
                    }
                    _ => (),
                }
            }
            let mut escaping = BTreeSet::new();
            while let Some(root) = seeds.pop() {
                if escaping.insert(root) {
                    seeds.extend(&edges[root as usize]);
                }
            }
            escaping
        })
    }

    fn fixed_construction_type(&mut self, root: hirsel_types::TypeId) -> bool {
        if let Some(fixed) = self.scratch.fixed_constructions[self.site.body.0 as usize]
            .borrow()
            .get(&root)
        {
            return *fixed;
        }
        // Explicit worklist, including nominal representations. A recursive
        // owned shape cannot have a fixed-size construction destination.
        let store = self.t;
        let mut todo = vec![(root, None)];
        let mut active = BTreeSet::new();
        let mut fixed = true;
        while let Some((id, leaving)) = todo.pop() {
            if let Some(path) = leaving {
                active.remove(&path);
                continue;
            }
            let kind = store.get(id).unwrap().kind.clone();
            if self.bound(
                &store,
                id,
                hirsel_types::Bound::Plain,
                &self.generic_declarations(),
            ) {
                continue;
            }
            let children = match kind {
                TypeKind::Builtin {
                    kind: hirsel_types::BuiltinType::Shared,
                    ..
                } => vec![],
                TypeKind::Primitive(primitive)
                    if !matches!(
                        primitive,
                        hirsel_types::Primitive::String | hirsel_types::Primitive::Bytes
                    ) =>
                {
                    vec![]
                }
                TypeKind::Tuple(fields) => fields.to_vec(),
                TypeKind::Record(fields) => fields.iter().map(|field| field.ty).collect(),
                TypeKind::Variant(variants) => variants
                    .iter()
                    .flat_map(|variant| variant.fields.iter().map(|(_, ty)| *ty))
                    .collect(),
                TypeKind::Builtin {
                    kind: hirsel_types::BuiltinType::Inline | hirsel_types::BuiltinType::Simd,
                    arguments,
                } => arguments
                    .iter()
                    .filter_map(|argument| match argument {
                        GenericArgument::Type(ty) => Some(*ty),
                        _ => None,
                    })
                    .collect(),
                TypeKind::Named {
                    definition,
                    arguments,
                } => {
                    if !active.insert(definition.clone()) {
                        fixed = false;
                        break;
                    }
                    let Some(signature) = self.signatures.get(definition) else {
                        fixed = false;
                        break;
                    };
                    let SignatureKind::Type(declaration) = &signature.kind else {
                        fixed = false;
                        break;
                    };
                    let Some(representation) = declaration.representation else {
                        fixed = false;
                        break;
                    };
                    let representation = hirsel_types::substitute(
                        self.t,
                        self.memo,
                        representation,
                        &hirsel_types::Substitution {
                            binder: Some(hirsel_types::GenericBinder::Bound(0)),
                            roles: &signature.metadata.roles,
                            arguments: &arguments,
                            authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                        },
                    )
                    .ok();
                    let Some(representation) = representation else {
                        fixed = false;
                        break;
                    };
                    todo.push((id, Some(definition)));
                    vec![representation]
                }
                _ => {
                    fixed = false;
                    break;
                }
            };
            todo.extend(children.into_iter().map(|child| (child, None)));
        }
        self.scratch.fixed_constructions[self.site.body.0 as usize]
            .borrow_mut()
            .insert(root, fixed);
        fixed
    }

    pub(super) fn construction_effects(&mut self, destination: PlaceId, value: &Rvalue) {
        if !self.noalloc_body() {
            return;
        }
        let row = self.t.get(self.place_type(destination)).unwrap().clone();
        if let Rvalue::Unary {
            op: hirsel_syntax::UnaryOp::Unique,
            value,
        } = value
        {
            // Unique copies the owned graph even when the source row claims
            // exclusivity. Plain shapes and Shared boundaries own no blocks;
            // explicit arena/region destinations do not use the counted heap.
            if !row.modes.relation.scoped(|o| self.arena_place(o))
                && !self.fixed_construction_type(self.place_type(value.place))
            {
                self.error(ViolationKind::TypeMismatch {
                    entity: "body unique extraction allocation",
                });
            }
            return;
        }
        let construction = matches!(
            value,
            Rvalue::Constant(Constant::String(_) | Constant::Bytes(_))
                | Rvalue::Unary {
                    op: hirsel_syntax::UnaryOp::Box,
                    ..
                }
                | Rvalue::Construct {
                    constructor: Constructor::Sequence
                        | Constructor::Map
                        | Constructor::Set
                        | Constructor::SortedMap { .. }
                        | Constructor::Box,
                    ..
                }
        ) || matches!(value, Rvalue::Closure { .. })
            && self.lifetime(&row.modes.relation).is_global();
        if !construction {
            return;
        }
        let placement = match value {
            Rvalue::Construct { destination, .. } | Rvalue::Closure { destination, .. }
                if *destination != Destination::Inferred =>
            {
                destination.clone()
            }
            // A row's relation places its blocks only through an arena or
            // region it names; sources it shares with the caller do not.
            _ => match destination_of(&row.modes.relation, |o| self.arena_place(o)) {
                Destination::Caller(_) => Destination::Inferred,
                d => d,
            },
        };
        let counted = match placement {
            Destination::Heap | Destination::Caller(_) => true,
            Destination::Arena(_) | Destination::Region(_) => false,
            Destination::Inferred => {
                self.escaping_roots()
                    .contains(&self.body.places[destination.0 as usize].root.0)
                    && !self.fixed_construction_type(self.place_type(destination))
            }
        };
        if counted {
            self.error(ViolationKind::TypeMismatch {
                entity: "body construction allocation",
            });
        }
    }

    pub(super) fn instantiated_authority(
        summary: &AuthoritySummary,
        parameters: &[AuthoritySummary],
        generics: &[AuthoritySummary],
    ) -> AuthoritySummary {
        let AuthoritySummary::Known(terms) = summary else {
            return AuthoritySummary::Unknown;
        };
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
                }) => generics
                    .get(parameter.0 as usize)
                    .cloned()
                    .unwrap_or(AuthoritySummary::Unknown),
                term => AuthoritySummary::Known(vec![term.clone()].into()),
            };
            a.join(actual)
        })
    }

    pub(super) fn frame_authority(&self) -> Option<AuthoritySummary> {
        self.scratch.frame_authorities[self.site.body.0 as usize]
            .get_or_init(|| self.derive_frame_authority())
            .clone()
    }

    fn derive_frame_authority(&self) -> Option<AuthoritySummary> {
        if let Some(d) = self.seal_finalizer() {
            let generics: Vec<_> = d
                .generics
                .iter()
                .enumerate()
                .map(|(ordinal, _)| {
                    let reference = TypeParameterRef::body(
                        self.body.parameter_scope,
                        hirsel_types::TypeParamId(ordinal as u32),
                    );
                    if hirsel_types::bounds_imply_inert(
                        self.generic_declarations().bounds(reference),
                    ) {
                        AuthoritySummary::inert()
                    } else {
                        AuthoritySummary::Known(
                            vec![AuthorityTerm::TypeParameter(reference)].into(),
                        )
                    }
                })
                .collect();
            // Destruction may inspect the private representation, but its
            // authority must stay within the owner's checked ceiling. The
            // public nominal identity permits whole-self staging as well.
            return Some(
                Self::instantiated_authority(d.implementation_authority.as_ref()?, &[], &generics)
                    .join(d.authority.clone()),
            );
        }
        let header = self.invocation_header()?;
        let Uses::Only(allowed) = &header.uses else {
            return None;
        };
        let parameters: Vec<_> = self
            .body
            .parameters
            .iter()
            .map(|parameter| {
                self.authority_with_generics(
                    self.t,
                    self.body.locals[parameter.0 as usize].ty,
                    &self.generic_declarations(),
                )
            })
            .collect();
        let declarations = self.generic_declarations();
        let generics: Vec<_> = header
            .generics
            .iter()
            .enumerate()
            .map(|(ordinal, _)| {
                let reference = hirsel_types::TypeParameterRef::body(
                    self.body.parameter_scope,
                    hirsel_types::TypeParamId(ordinal as u32),
                );
                if hirsel_types::bounds_imply_inert(declarations.bounds(reference)) {
                    AuthoritySummary::inert()
                } else {
                    AuthoritySummary::Known(vec![AuthorityTerm::TypeParameter(reference)].into())
                }
            })
            .collect();
        // Unknown parameter evidence cannot widen a known uses restriction.
        // Invocation of an explicitly selected callback is proved separately.
        let parameters: Vec<_> = parameters
            .into_iter()
            .map(|authority| {
                if authority == AuthoritySummary::Unknown {
                    AuthoritySummary::inert()
                } else {
                    authority
                }
            })
            .collect();
        let allowed = Self::instantiated_authority(allowed, &parameters, &generics);
        // Named definitions promise the written ceiling. Their inputs are
        // provision, not permission to widen it. Nested callable inputs enter
        // independently of the capture ceiling (CLO-2).
        Some(if self.site.body == self.definition.entry {
            allowed
        } else {
            parameters.into_iter().fold(allowed, AuthoritySummary::join)
        })
    }

    /// CAP-2/CLO-4: aggregate parts carry no authority beyond their holder.
    /// Reconstruct the path from declarations; a capability seal is a nominal
    /// authority boundary, so its representation cannot inherit this receipt.
    fn callback_projection(&self, place: &Place) -> bool {
        use hirsel_type_analysis::representation::{
            RepresentationError, RepresentationFailure, expand_representation,
        };
        let mut ty = self.body.locals[place.root.0 as usize].ty;
        for projection in &place.projection {
            let layout = expand_representation(
                self.t,
                self.memo,
                ty,
                &mut |_, definition, arguments| {
                    Ok(self.signatures.get(definition).and_then(|signature| {
                        let SignatureKind::Type(declaration) = &signature.kind else {
                            return None;
                        };
                        declaration.capability.is_none().then(|| {
                            (
                                std::sync::Arc::new(signature.clone()),
                                vec![AuthoritySummary::Unknown; arguments.len()],
                            )
                        })
                    }))
                },
                None,
            );
            let layout = match layout {
                Ok(layout) => layout,
                Err(RepresentationError {
                    failure: RepresentationFailure::Query(error),
                    ..
                }) => {
                    return self.proof_failure(error, false);
                }
                Err(_) => return false,
            };
            ty = match (projection, &self.t.get(layout).unwrap().kind) {
                (Projection::Field(0), TypeKind::Box(part)) => *part,
                (Projection::Field(i), TypeKind::Tuple(parts)) => {
                    let Some(part) = parts.get(*i as usize) else {
                        return false;
                    };
                    *part
                }
                (Projection::Field(i), TypeKind::Record(fields)) => {
                    let Some(field) = fields.get(*i as usize) else {
                        return false;
                    };
                    field.ty
                }
                (Projection::VariantField { variant, field }, TypeKind::Variant(variants)) => {
                    let Some((_, part)) = variants
                        .get(*variant as usize)
                        .and_then(|v| v.fields.get(*field as usize))
                    else {
                        return false;
                    };
                    *part
                }
                _ => return false,
            };
        }
        true
    }

    /// Trace staging and authority-preserving field paths to this callable's
    /// own selected parameters. Flow independently proves availability and
    /// authority after writes and at the invocation.
    pub(super) fn parameter_callback(
        &self,
        place: PlaceId,
        selected: Option<&AuthoritySummary>,
    ) -> bool {
        let mut pending = vec![place];
        let mut seen = BTreeSet::new();
        let mut parameter = false;
        while let Some(place) = pending.pop() {
            let place = &self.body.places[place.0 as usize];
            if !self.callback_projection(place) {
                return false;
            }
            if !seen.insert(place.root.0) {
                continue;
            }
            if self.authority_with_generics(self.t, place.ty, &self.generic_declarations())
                == AuthoritySummary::inert()
            {
                continue;
            }
            if let Some(ordinal) = self
                .body
                .parameters
                .iter()
                .position(|root| *root == place.root)
            {
                if selected.is_some_and(|selected| !matches!(selected,
                    AuthoritySummary::Known(terms) if terms.contains(&AuthorityTerm::Parameter(ParameterRef::bound(hirsel_types::ParamId(ordinal as u32)))))) {
                    return false;
                }
                parameter = true;
                continue;
            }
            let producers = &self.scratch.variant_inputs[self.site.body.0 as usize]
                .get_or_init(|| super::variant_brands::Inputs::new(self.body))
                .roots[place.root.0 as usize];
            if producers.is_empty() {
                return false;
            }
            for producer in producers {
                match producer {
                    super::variant_brands::Producer::Transfer(source) => pending.push(*source),
                    super::variant_brands::Producer::Closure(callback)
                        if !self
                            .lifetime(&self.t.get(place.ty).unwrap().modes.relation)
                            .is_global() =>
                    {
                        let mut found = false;
                        for instruction in self
                            .body
                            .blocks
                            .iter()
                            .flat_map(|block| &block.instructions)
                        {
                            if let InstructionKind::Assign { destination, value } =
                                &instruction.kind
                            {
                                if self.body.places[destination.0 as usize].root == place.root {
                                    if let Rvalue::Closure { body, captures, .. } =
                                        &self.body.rvalues[value.0 as usize]
                                    {
                                        if body == callback {
                                            found = true;
                                            pending.extend(
                                                captures.iter().map(|capture| capture.place),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        if !found {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }
        }
        parameter
    }

    pub(super) fn invocation_effects(
        &mut self,
        callee: &Callee,
        function: &FunctionType,
        generics: &[GenericArgument],
        arguments: &[CallArgument],
    ) {
        let seal_finalizer = self.seal_finalizer().is_some();
        if self.invocation_header().is_none() && !seal_finalizer {
            return;
        }
        // Scoped forwarding is also an entry-body obligation: a scope
        // callback may capture only the enclosing declaration's selected
        // unknown parameters, independently of source-created capture rows.
        let scoped_unknown_callback = arguments.iter().any(|argument| {
            let ty = self.place_type(argument.value.place);
            function
                .parameters
                .get(argument.parameter.0 as usize)
                .is_some_and(|parameter| parameter.passing == hirsel_syntax::PassingMode::Borrow)
                && matches!(self.t.get(ty).unwrap().kind, TypeKind::Function(_))
                && !self
                    .lifetime(&self.t.get(ty).unwrap().modes.relation)
                    .is_global()
                && self.authority_with_generics(self.t, ty, &self.generic_declarations())
                    == AuthoritySummary::Unknown
        });
        // Named-body invocation checks elsewhere retain their existing policy.
        // This additional proof is the exact unrestricted parameter invocation
        // receipt; nested executable callables keep the complete effect audit.
        if self.site.body == self.definition.entry && !seal_finalizer {
            let parameter_invocation = if let Callee::Indirect { value, .. } = callee {
                let place = &self.body.places[value.place.0 as usize];
                self.body.locals[place.root.0 as usize].storage == Storage::Parameter
                    && !self
                        .lifetime(&self.t.get(place.ty).unwrap().modes.relation)
                        .is_global()
                    && matches!(function.uses, Uses::Unrestricted)
            } else {
                false
            };
            if !parameter_invocation && !scoped_unknown_callback {
                return;
            }
        }
        // CAP-2/CLO-2, spec #58: authority supplied through an operand is
        // proved by the flow walk, including capabilities derived inside the
        // frame. Only authority required beyond those inputs is ambient.
        let authorities = vec![AuthoritySummary::inert(); function.parameters.len()];
        let type_authorities: Vec<_> = generics
            .iter()
            .map(|argument| match argument {
                GenericArgument::Type(ty) => {
                    self.authority_with_generics(self.t, *ty, &self.generic_declarations())
                }
                _ => AuthoritySummary::inert(),
            })
            .collect();
        let mut required =
            Self::instantiated_authority(&function.authority, &authorities, &type_authorities);
        let ambient_required = required.clone();
        let selected = self
            .invocation_header()
            .and_then(|header| match &header.uses {
                Uses::Only(AuthoritySummary::Known(terms))
                    if terms
                        .iter()
                        .any(|term| matches!(term, AuthorityTerm::Parameter(_))) =>
                {
                    if let Uses::Only(selected) = &header.uses {
                        Some(selected.clone())
                    } else {
                        unreachable!()
                    }
                }
                _ => None,
            });
        let mut allocation = function.allocation;
        let mut provided_unknown = false;
        for argument in arguments {
            if function
                .parameters
                .get(argument.parameter.0 as usize)
                .is_some_and(|parameter| parameter.passing != hirsel_syntax::PassingMode::Take)
            {
                if let TypeKind::Function(callback) = &self
                    .t
                    .get(self.body.places[argument.value.place.0 as usize].ty)
                    .unwrap()
                    .kind
                {
                    if callback.allocation == Allocation::MayAllocate {
                        allocation = Allocation::MayAllocate;
                    }
                    if function.parameters[argument.parameter.0 as usize].passing
                        == hirsel_syntax::PassingMode::Borrow
                        && self.authority_with_generics(
                            self.t,
                            self.place_type(argument.value.place),
                            &self.generic_declarations(),
                        ) == AuthoritySummary::Unknown
                    {
                        provided_unknown |= required != AuthoritySummary::Unknown;
                        required = AuthoritySummary::Unknown;
                    }
                }
            }
        }
        if let Callee::Indirect { value, .. } = callee {
            let place = &self.body.places[value.place.0 as usize];
            let row = self.t.get(place.ty).unwrap();
            let parameter = self.body.locals[place.root.0 as usize].storage == Storage::Parameter;
            if !self.lifetime(&row.modes.relation).is_global() {
                // Known capture authority is checked at its source in flow.
                // An unrestricted scoped parameter still has unknown effects;
                // a capture slot's or computed callee's row records the
                // authority its creation fixed (CLO-1), which flow checks.
                // A nested callable's capture limit is separate from authority
                // supplied through its own parameters. Flow independently proves
                // the callee operand and its transports at invocation.
                required = match &function.uses {
                    Uses::Unrestricted if parameter => AuthoritySummary::Unknown,
                    Uses::Unrestricted => required,
                    Uses::Only(_) => AuthoritySummary::inert(),
                };
            }
            // A taken once callback is staged before entry, but still obtains
            // its authority from the caller rather than the capture limit.
            provided_unknown |= required == AuthoritySummary::Unknown
                && matches!(function.uses, Uses::Unrestricted)
                && self.parameter_callback(value.place, selected.as_ref());
        }
        if self.noalloc_body() && allocation == Allocation::MayAllocate {
            self.error(ViolationKind::TypeMismatch {
                entity: "body allocation",
            });
        }
        if self.contract {
            return;
        } // Clause purity has the stricter inert check.
        let Some(allowed) = self.frame_authority() else {
            return;
        };
        let selected_parameter = if let Callee::Indirect { value, .. } = callee {
            selected
                .as_ref()
                .is_some_and(|selected| self.parameter_callback(value.place, Some(selected)))
        } else {
            false
        };
        let additional_callbacks = arguments.iter().any(|argument| {
            let ty = self.place_type(argument.value.place);
            matches!(self.t.get(ty).unwrap().kind, TypeKind::Function(_))
                && !self
                    .authority_with_generics(self.t, ty, &self.generic_declarations())
                    .is_subset_of(&allowed)
        });
        let provided_unknown = (self.site.body != self.definition.entry
            || scoped_unknown_callback && selected.is_some())
            && provided_unknown
            && (ambient_required == AuthoritySummary::Unknown
                || ambient_required.is_subset_of(&allowed))
            && selected.as_ref().is_none_or(|selected| {
                arguments.iter().all(|argument| {
                    let ty = self.place_type(argument.value.place);
                    !matches!(self.t.get(ty).unwrap().kind, TypeKind::Function(_))
                        || self.authority_with_generics(self.t, ty, &self.generic_declarations())
                            != AuthoritySummary::Unknown
                        || self.parameter_callback(argument.value.place, Some(selected))
                })
            });
        if !(required == AuthoritySummary::Unknown
            && (selected_parameter && !additional_callbacks || provided_unknown))
            && !required.is_subset_of(&allowed)
        {
            self.error(ViolationKind::TypeMismatch {
                entity: "body uses",
            });
        }
    }
}
