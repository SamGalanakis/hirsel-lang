use super::*;
impl Checker<'_, '_> {
    pub(super) fn name(&self, symbol: hirsel_intern::Symbol) -> String {
        symbol.as_str().to_owned()
    }
    pub(super) fn snapshot(&mut self, path: impl AsDef) -> Result<SignatureRef, QueryError> {
        let definition = path.def();
        if let Some(s) = self.snapshots.get(&definition) {
            return Ok(s.clone());
        }
        let result = self.provider.signature(definition);
        std::sync::Arc::make_mut(&mut self.body.resolution.reads).push(
            DependencyRead::Signature {
                definition,
                fingerprint: result.as_ref().ok().map(|s| s.fingerprint),
            }
            .shared(),
        );
        match result {
            Ok(s) => {
                self.dependencies.insert(definition, s.fingerprint);
                self.snapshots.insert(definition, s.clone());
                Ok(s)
            }
            Err(QueryError::Diagnostics(ds)) => {
                self.diagnostics.extend(ds);
                Err(QueryError::Diagnostics(vec![]))
            }
            Err(e) => Err(e),
        }
    }
    /// A declared row read from a signature. Rows are session rows, so this
    /// only records the signature reads its nominals need.
    pub(super) fn import(&mut self, root: TypeId) -> Result<TypeId, QueryError> {
        self.imported(root)
    }
    fn imported(&mut self, ty: TypeId) -> Result<TypeId, QueryError> {
        if !self.types.valid(ty)
            && self
                .types
                .validate(ty)
                .is_err_and(|e| !matches!(e, GraphError::RecoveryType(_)))
        {
            return Err(QueryError::Operational(OperationalFailure {
                operation: "body type import".into(),
                message: "malformed declared type graph".into(),
            }));
        }
        let own = self.input.signature.definition;
        let paths: BTreeSet<_> = closure(self.types, [ty])
            .iter()
            .filter_map(|t| match &self.types.row(*t).kind {
                TypeKind::Named { definition, .. } if *definition != own => {
                    Some(definition.path().clone())
                }
                _ => None,
            })
            .collect();
        for p in paths {
            match self.snapshot(&p) {
                Ok(_) | Err(QueryError::Diagnostics(_)) => (),
                Err(e) => return Err(e),
            }
        }
        Ok(ty)
    }
    pub(super) fn import_function(&mut self, f: &FunctionType) -> Result<FunctionType, QueryError> {
        let root = self.types.intern(Type {
            drop_allocation: DropAllocation::MayAllocate,
            ..Type::with_kind(TypeKind::Function(f.clone()))
        });
        let root = self.imported(root)?;
        let TypeKind::Function(f) = self.kind(root) else {
            unreachable!()
        };
        Ok(f)
    }
    pub(super) fn substitute(
        &mut self,
        root: TypeId,
        arguments: &[GenericArgument],
    ) -> Result<TypeId, QueryError> {
        self.substitute_at(root, arguments, GenericBinder::Bound(0))
    }
    pub(super) fn substitute_at(
        &mut self,
        root: TypeId,
        arguments: &[GenericArgument],
        binder: GenericBinder,
    ) -> Result<TypeId, QueryError> {
        // Authority checking belongs to J; preserve the declared substitution channel.
        let authorities: Vec<_> = arguments
            .iter()
            .map(|_| AuthoritySummary::Unknown)
            .collect();
        match substitute(
            self.types,
            self.memo,
            root,
            &Substitution {
                binder: Some(binder),
                roles: &self.input.signature.metadata.roles,
                arguments,
                authorities: &authorities,
            },
        ) {
            Ok(t) => Ok(self.normalize_properties(t)),
            Err(GraphError::RecoveryType(_)) => Ok(self.error()),
            Err(_) => Err(QueryError::Operational(OperationalFailure {
                operation: "body generic substitution".into(),
                message: "invalid declared generic graph".into(),
            })),
        }
    }
    pub(super) fn representation(
        &mut self,
        ty: TypeId,
    ) -> Result<(Option<ast::DefPath>, TypeId), QueryError> {
        if let TypeKind::Named {
            definition,
            arguments,
        } = self.kind(ty)
        {
            let definition = definition.path().clone();
            let s = match self.snapshot(&definition) {
                Ok(s) => s,
                Err(QueryError::Diagnostics(_)) => return Ok((Some(definition), self.error())),
                Err(e) => return Err(e),
            };
            if let SignatureKind::Type(t) = &s.kind {
                if let Some(r) = t.representation {
                    if let Some((_, known)) = self
                        .representations
                        .get(&definition)
                        .and_then(|known| known.iter().find(|(a, _)| a[..] == arguments[..]))
                    {
                        return Ok((Some(definition), *known));
                    }
                    let r = self.import(r)?;
                    let r = if arguments.is_empty() {
                        r
                    } else {
                        self.substitute(r, &arguments)?
                    };
                    self.representations
                        .entry(definition.clone())
                        .or_default()
                        .push((arguments.to_vec(), r));
                    return Ok((Some(definition), r));
                }
            }
            Ok((Some(definition), ty))
        } else {
            Ok((None, ty))
        }
    }
    /// The nominal type a chain of transparent aliases represents, with the
    /// aliases' arguments substituted; any other type is returned unchanged.
    pub(super) fn unalias(&mut self, ty: TypeId) -> Result<TypeId, QueryError> {
        let mut current = ty;
        let mut seen = BTreeSet::new();
        while let TypeKind::Named { definition, .. } = self.kind(current) {
            if !seen.insert(definition.clone()) {
                break;
            }
            let alias = match self.snapshot(&definition) {
                Ok(s) => s.metadata.alias,
                Err(QueryError::Diagnostics(_)) => false,
                Err(e) => return Err(e),
            };
            let representation = self.representation(current)?.1;
            if !alias || !matches!(self.kind(representation), TypeKind::Named { .. }) {
                break;
            }
            current = representation;
        }
        Ok(current)
    }
    pub(super) fn inference_variable(&self, ty: TypeId) -> bool {
        matches!(self.kind(ty), TypeKind::Parameter(p) if self.inference_variables.contains(&p))
    }
    /// A Result context from `?` fixes only its success type. Its fresh error
    /// is resolved by the operand, independently of the enclosing error channel.
    pub(super) fn partial_result(&self, ty: TypeId) -> Option<(DefId, TypeId)> {
        let TypeKind::Named {
            definition,
            arguments,
        } = self.kind(ty)
        else {
            return None;
        };
        match arguments.as_slice() {
            [GenericArgument::Type(ok), GenericArgument::Type(error)]
                if self.inference_variable(*error) =>
            {
                Some((definition, *ok))
            }
            _ => None,
        }
    }
    /// Bind a partial Result's error from actual evidence, including its
    /// conditional cleanup and sendability summaries.
    pub(super) fn resolve_result_context(
        &mut self,
        expected: TypeId,
        actual: TypeId,
    ) -> Result<TypeId, QueryError> {
        let Some((definition, _)) = self.partial_result(expected) else {
            return Ok(expected);
        };
        let TypeKind::Named {
            definition: actual_definition,
            arguments,
        } = self.kind(actual)
        else {
            return Ok(expected);
        };
        let [GenericArgument::Type(_), GenericArgument::Type(error)] = arguments.as_slice() else {
            return Ok(expected);
        };
        if definition != actual_definition || self.inference_variable(*error) {
            return Ok(expected);
        }
        let TypeKind::Named { arguments, .. } = self.kind(expected) else {
            unreachable!()
        };
        let GenericArgument::Type(variable) = arguments[1] else {
            unreachable!()
        };
        let TypeKind::Parameter(variable) = self.kind(variable) else {
            unreachable!()
        };
        self.substitute_at(
            expected,
            &[GenericArgument::Type(*error)],
            variable.binder(),
        )
    }
    /// A deliberately small, source-spellable adapter domain. These values
    /// can be borrowed without transfer, and the scalar result retains no loan.
    /// No callable variance is introduced: the emitted named function is checked
    /// as an ordinary definition and receipt still requires identical modes.
    pub(super) fn borrow_callback_adapter_fits(&self, actual: TypeId, expected: TypeId) -> bool {
        let (TypeKind::Function(have), TypeKind::Function(want)) =
            (self.kind(actual), self.kind(expected))
        else {
            return false;
        };
        let scalar = |ty| {
            matches!(self.kind(ty), TypeKind::Primitive(p)
            if !matches!(p, Primitive::String | Primitive::Bytes | Primitive::Never))
                || matches!(self.kind(ty), TypeKind::Tuple(items) if items.is_empty())
        };
        let simple = |ty| {
            scalar(ty)
                || matches!(self.kind(ty), TypeKind::Builtin {
            kind: BuiltinType::List, arguments,
        } if matches!(arguments.as_slice(), [GenericArgument::Type(t)] if scalar(*t)))
        };
        let ordinary = |ty| {
            let row = self.types.get(ty).unwrap();
            row.modes.uniqueness == Uniqueness::Aliased
                && row.modes.multiplicity == Multiplicity::Many
                && (row.modes.relation.is_global() || row.modes.relation == Relation::environment())
                && row.sendability_requirement == SendabilityRequirement::None
                && row.brand == Brand::default()
        };
        have.parameters.len() == 1
            && want.parameters.len() == 1
            && have.parameters[0].passing == ast::PassingMode::Borrow
            && want.parameters[0].passing == ast::PassingMode::Mut
            && have.parameters[0].label.is_none()
            && want.parameters[0].label.is_none()
            && have.parameters[0].ty == want.parameters[0].ty
            && have.result == want.result
            && scalar(want.result)
            && simple(want.parameters[0].ty)
            && ordinary(want.parameters[0].ty)
            && self.element(want.parameters[0].ty).is_none_or(ordinary)
            && self.types.row(want.result).modes.relation.is_global()
            && ordinary(want.result)
            && self.types.row(actual).modes.multiplicity == Multiplicity::Many
            && self.types.row(expected).modes.multiplicity == Multiplicity::Many
            && self.types.row(expected).modes.uniqueness == Uniqueness::Aliased
            && self.types.row(expected).sendability_requirement == SendabilityRequirement::None
            && self.types.row(expected).brand == Brand::default()
            && self.types.row(expected).erased_captures.is_none()
            && have.generics.is_empty()
            && want.generics.is_empty()
            && have.authority == AuthoritySummary::inert()
            && matches!(&have.uses, Uses::Only(a) if *a == AuthoritySummary::inert())
            && want.allocation == Allocation::MayAllocate
            && have.state_promises.is_empty()
            && want.state_promises.is_empty()
            && have.contracts.is_empty()
            && want.contracts.is_empty()
            && !have.incomplete
            && !want.incomplete
            && have.foreign.is_none()
            && want.foreign.is_none()
    }
    /// The core stdin reader borrows its capability token but hands back its
    /// mutable buffer. Only a mode-only receipt with unrestricted effects gets
    /// this concrete recipe; the source check pins the complete declaration.
    pub(super) fn stdin_callback_adapter_fits(&self, actual: TypeId, expected: TypeId) -> bool {
        let (TypeKind::Function(have), TypeKind::Function(want)) =
            (self.kind(actual), self.kind(expected))
        else {
            return false;
        };
        have.parameters.len() == 3
            && want.parameters.len() == 3
            && want.parameters.iter().map(|p| p.passing).eq([
                ast::PassingMode::Mut,
                ast::PassingMode::Mut,
                ast::PassingMode::Borrow,
            ])
            && have
                .parameters
                .iter()
                .zip(&want.parameters)
                .all(|(a, b)| a.ty == b.ty && a.label == b.label)
            && have.result == want.result
            && want.generics.is_empty()
            && !want.incomplete
            && want.allocation == Allocation::MayAllocate
            && want.uses == Uses::Unrestricted
            && want.state_promises.is_empty()
            && want.contracts.is_empty()
            && want.foreign.is_none()
            && self.types.row(expected).modes.multiplicity == Multiplicity::Many
            && self.types.row(expected).modes.uniqueness == Uniqueness::Aliased
            && self.types.row(expected).sendability_requirement == SendabilityRequirement::None
            && self.types.row(expected).brand == Brand::default()
            && self.types.row(expected).erased_captures.is_none()
    }
    pub(super) fn compatible(&self, a: TypeId, b: TypeId) -> bool {
        if self.invalid(a)
            || self.invalid(b)
            || self.bottom(a)
            || self.inference_variable(a)
            || self.inference_variable(b)
        {
            return true;
        }
        // H compares the type shape. I/J separately judge modes, origin and effects.
        shape_equal_with_holes(
            self.types,
            a,
            b,
            |kind| matches!(kind, TypeKind::Parameter(p) if self.inference_variables.contains(p)),
        )
        .unwrap_or(false)
    }
    /// A pattern's provisional local that its checked pattern replaced by a
    /// nullary variant: resolution introduced it, and no binding holds it.
    pub(super) fn superseded_pattern_local(&self, local: LocalId) -> bool {
        self.input
            .resolution
            .bindings
            .iter()
            .any(|(_, _, l)| *l == local)
            && !self
                .body
                .bindings
                .iter()
                .any(|b| b.binding == ResolvedName::Local(local))
    }
    pub(super) fn lookup_binding(
        &self,
        id: ast::ExprId,
        name: &ResolvedName,
        frame: usize,
    ) -> Option<TypeId> {
        self.lookup_binding_at(NameSite::Expression(id), name, frame)
    }
    fn lookup_binding_at(
        &self,
        site: NameSite,
        name: &ResolvedName,
        frame: usize,
    ) -> Option<TypeId> {
        let scope = self.input_scope(site).map(|(_, o)| o);
        let mut current = Some(frame);
        while let Some(f) = current {
            let fr = &self.frames[f];
            match name {
                ResolvedName::Local(l) => {
                    if let Some(t) = fr.locals.get(l) {
                        return Some(*t);
                    }
                }
                ResolvedName::Parameter(p) if scope.is_none_or(|s| *s == fr.owner) => {
                    if let Some(t) = fr.parameters.get(p.0 as usize) {
                        return Some(*t);
                    }
                }
                _ => (),
            }
            current = fr.parent;
        }
        None
    }
    pub(super) fn packed_payload(&self, ty: TypeId) -> Option<TypeId> {
        match self.kind(ty) {
            TypeKind::Builtin {
                kind: BuiltinType::Packed,
                arguments,
            } => match arguments.as_slice() {
                [GenericArgument::Type(payload)] => Some(*payload),
                _ => None,
            },
            _ => None,
        }
    }
    fn expected_variant(
        &mut self,
        id: ast::ExprId,
        path: &ast::Path,
        t: TypeId,
        callee: bool,
    ) -> Result<Option<TypeId>, QueryError> {
        let nominal = self.packed_payload(t).unwrap_or(t);
        let (owner, r) = self.representation(nominal)?;
        if let TypeKind::Variant(vs) = self.kind(r) {
            let name = path
                .segments
                .last()
                .map(|s| self.name(*s))
                .unwrap_or_default();
            if let Some(index) = vs
                .iter()
                .position(|v| v.name.as_str() == name && (callee || v.fields.is_empty()))
            {
                let variant = VariantId(index as u32);
                let site = NameSite::Expression(id);
                // H owns a deferred choice once the actual expected
                // type selects its declaration. Retain it for J/K.
                match self.source_rows.names.get(&id) {
                    Some(&row) => {
                        self.body.resolution.expressions[row].1 = ResolvedName::Variant(variant)
                    }
                    None => self
                        .body
                        .resolution
                        .expressions
                        .push((id, ResolvedName::Variant(variant))),
                }
                self.body.resolution.contextual.retain(|(at, name)| {
                    *at != site
                        || !matches!(
                            name,
                            ContextualName::Constructor(_) | ContextualName::Variant { .. }
                        )
                });
                if let Some(owner) = owner {
                    self.body
                        .resolution
                        .contextual
                        .push((site.clone(), ContextualName::Variant { owner, variant }));
                }
                self.body.resolution.deferred.retain(|d| d.site != site);
                return Ok(Some(if callee {
                    if let Some((definition, _)) = self.partial_result(nominal) {
                        self.ty(TypeKind::Named {
                            definition,
                            arguments: vec![].into(),
                        })
                    } else {
                        nominal
                    }
                } else {
                    t
                }));
            }
        }
        Ok(None)
    }
    /// The value type of a named function definition.
    /// `F` when `ty` is the core entry type `Entry[F]` (RUN-3).
    pub(super) fn entry_signature(&self, ty: TypeId) -> Option<TypeId> {
        match self.kind(ty) {
            TypeKind::Named {
                definition,
                arguments,
            } if *definition.path() == ast::DefPath::core("Entry") => match arguments[..] {
                [GenericArgument::Type(signature)] => Some(signature),
                _ => None,
            },
            _ => None,
        }
    }
    /// RUN-3: a definition fits the entry type `Entry[F]` when it has F's shape (its
    /// parameters, their passing modes and relations, and its result) and keeps within F's
    /// allocation contract and `uses`. The entry's type is F with the definition's
    /// parameter and result types, so a generic call infers them, and F's own effect
    /// contract: a written F spells the default effects, which a definition's derived
    /// evidence never equals.
    fn entry_fits(&mut self, f: &FunctionType, named: TypeId, signature: TypeId) -> Option<TypeId> {
        let (TypeKind::Function(mut expected), TypeKind::Function(actual)) =
            (self.kind(signature), self.kind(named))
        else {
            return None;
        };
        let held = match &f.uses {
            Uses::Only(allowed) => allowed,
            Uses::Unrestricted => &f.authority,
        };
        let fits = self.compatible(named, signature)
            && expected.parameters.len() == actual.parameters.len()
            && (expected.allocation != Allocation::NoAlloc || f.allocation == Allocation::NoAlloc)
            && match &expected.uses {
                Uses::Unrestricted => true,
                Uses::Only(allowed) => held.is_subset_of(allowed),
            };
        if !fits {
            return None;
        }
        expected.parameters = expected
            .parameters
            .iter()
            .zip(actual.parameters.iter())
            .map(|(want, have)| Parameter {
                ty: have.ty,
                ..want.clone()
            })
            .collect();
        expected.result = actual.result;
        expected.error = actual.error;
        let mut row = self.types.get(signature).unwrap().clone();
        row.kind = TypeKind::Function(expected);
        Some(self.types.intern(row))
    }
    pub(super) fn named_function(&mut self, mut f: FunctionType) -> TypeId {
        // Spec #58: a known definition carries signature-derived
        // value evidence; written function types keep defaults.
        match &f.uses {
            Uses::Unrestricted => f.uses = Uses::Only(f.authority.clone()),
            Uses::Only(allowed) => f.authority = allowed.clone(),
        }
        let ty = self.ty(TypeKind::Function(f));
        // Named references have no environment; parameter types
        // are invocation inputs rather than captured values.
        let mut row = self.types.get(ty).unwrap().clone();
        row.modes.sendability = Sendability::Sendable;
        self.types.intern(row)
    }
    pub(super) fn path_type(
        &mut self,
        id: ast::ExprId,
        path: &ast::Path,
        expected: Option<TypeId>,
        frame: usize,
        callee: bool,
        record_place: bool,
    ) -> Result<TypeId, QueryError> {
        let mut resolved = self.input_name(id).map(|(_, r)| r.clone());
        // Resolution may have bound a pattern name to a provisional local
        // that the checked scrutinee type then selected as a nullary variant.
        // A bare use of that name has no binding: it is the contextual
        // variant, chosen by its own expected type.
        if let Some(ResolvedName::Local(local)) = resolved {
            if path.segments.len() == 1 && self.superseded_pattern_local(local) {
                resolved = None;
            }
        }
        if path.package.is_none()
            && path.segments.len() == 1
            && matches!(resolved, Some(ResolvedName::Definition(_)))
        {
            if let Some(expected) = expected {
                if self.expected_variant(id, path, expected, callee)?.is_some() {
                    // Select the contextual declaration, then use the ordinary
                    // fresh construction row below. Receiving locality is not
                    // evidence about a newly constructed value.
                    resolved = None;
                }
            }
        }
        match resolved {
            Some(r @ (ResolvedName::Local(_) | ResolvedName::Parameter(_))) => {
                let mut current = Some(frame);
                while let Some(f) = current {
                    if let Some((_, span)) = self.frames[f].restricted.iter().find(|(n, _)| *n == r)
                    {
                        let span = *span;
                        self.report(
                            const { RuleId::of("SEM-4") },
                            span,
                            "mutable iteration source may not be used in the body",
                        );
                        return Ok(self.error());
                    }
                    current = self.frames[f].parent;
                }
                let mut t = self
                    .lookup_binding(id, &r, frame)
                    .unwrap_or_else(|| self.error());
                let mut segments = path.segments.iter().skip(1).peekable();
                let mut projections = Vec::new();
                let mut payload_path = false;
                while let Some(field) = segments.next() {
                    if self.invalid(t) {
                        break;
                    }
                    while let TypeKind::Box(part) = self.kind(t) {
                        projections.push(SourceProjection {
                            expression: id,
                            kind: SourceProjectionKind::Field(FieldId(0)),
                        });
                        t = self.projected_type(t, part);
                    }
                    let payload = self.packed_payload(t).unwrap_or(t);
                    let (owner, repr) = self.representation(payload)?;
                    let name = self.name(*field);
                    let found = match self.kind(repr) {
                        TypeKind::Record(fs) => fs
                            .into_iter()
                            .enumerate()
                            .find(|(_, f)| f.name.as_str() == name)
                            .map(|(i, f)| (SourceProjectionKind::Field(FieldId(i as u32)), f.ty)),
                        TypeKind::Variant(vs) => {
                            let selected =
                                vs.iter().enumerate().find(|(_, v)| v.name.as_str() == name);
                            selected.and_then(|(variant, v)| {
                                let field_name = self.name(*segments.next()?);
                                let (field, (_, ty)) =
                                    v.fields.iter().enumerate().find(|(_, (label, _))| {
                                        label.map(|l| l.as_str()) == Some(field_name.as_str())
                                    })?;
                                payload_path = true;
                                if let Some(owner) = owner {
                                    self.body.resolution.contextual.push((
                                        NameSite::Expression(id),
                                        ContextualName::Field {
                                            owner,
                                            variant: Some(VariantId(variant as u32)),
                                            field: FieldId(field as u32),
                                        },
                                    ));
                                }
                                Some((
                                    SourceProjectionKind::VariantField {
                                        variant: VariantId(variant as u32),
                                        field: FieldId(field as u32),
                                    },
                                    *ty,
                                ))
                            })
                        }
                        _ => None,
                    };
                    if let Some((projection, ty)) = found {
                        projections.push(SourceProjection {
                            expression: id,
                            kind: projection,
                        });
                        t = self.projected_type(t, ty);
                    } else {
                        let shared = if let Some(part) = self.shared_payload(t) {
                            let (_, repr) = self.representation(part)?;
                            matches!(self.kind(repr), TypeKind::Record(fields)
                                if fields.iter().any(|f| f.name.as_str() == name))
                            .then_some(part)
                        } else {
                            None
                        };
                        self.shared_access_error(
                            const { RuleId::of("LANG-7") },
                            self.span(id),
                            "field is unavailable on this type",
                            shared,
                        );
                        t = self.error();
                    }
                }
                if record_place && !self.invalid(t) {
                    self.record_place(SourcePlace {
                        expression: id,
                        root: self.place_root(id),
                        projections: projections.into(),
                        access: SourceAccess::Read,
                    });
                }
                if payload_path && !self.invalid(t) {
                    self.body
                        .resolution
                        .deferred
                        .retain(|d| d.site != NameSite::Expression(id));
                }
                Ok(t)
            }
            Some(ResolvedName::Size(_)) => Ok(self.primitive(Primitive::U64)),
            Some(ResolvedName::Definition(p)) => {
                let s = match self.snapshot(&p) {
                    Ok(s) => s,
                    Err(QueryError::Diagnostics(_)) => return Ok(self.error()),
                    Err(e) => return Err(e),
                };
                match &s.kind {
                    SignatureKind::Family(_) | SignatureKind::Implementation(_) => {
                        self.report(
                            const { RuleId::of("GEN-6") },
                            self.span(id),
                            "a signature or implementation is not a value; name an implementation only in a `with` clause",
                        );
                        Ok(self.error())
                    }
                    SignatureKind::Function(_)
                        if !s.metadata.implementations.is_empty() && !self.static_callee(id) =>
                    {
                        let binders = s
                            .metadata
                            .implementations
                            .iter()
                            .map(|b| format!("{} = ..", b.name))
                            .collect::<Vec<_>>()
                            .join(", ");
                        self.report(
                            const { RuleId::of("GEN-6") },
                            self.span(id),
                            &format!("`{p}` declares implementation binders; call it as `{} with ({binders})(..)`", p.0.name),
                        );
                        Ok(self.error())
                    }
                    SignatureKind::Function(f) => {
                        let f = self.import_function(f)?;
                        let callable = f.generics.is_empty() && f.foreign.is_none();
                        let complete = !f.incomplete;
                        let named = self.named_function(f.clone());
                        // RUN-3: a non-generic, non-extern, complete definition named
                        // where an entry `Entry[F]` is expected forms that entry when it
                        // fits F. Anything else there is RUN-3 (`put`).
                        if let Some(entry) = expected.filter(|_| callable && complete)
                            && let Some(signature) = self.entry_signature(entry)
                            && let Some(fitted) = self.entry_fits(&f, named, signature)
                        {
                            return Ok(self.ty(TypeKind::Named {
                                definition: DefId::intern(&ast::DefPath::core("Entry")),
                                arguments: vec![GenericArgument::Type(fitted)].into(),
                            }));
                        }
                        // PROG-4: a non-generic definition named where a callback
                        // `FnPtr[F]` of its shape is expected is that callback.
                        if let Some(pointer) = expected.filter(|_| callable)
                            && let TypeKind::Builtin {
                                kind: BuiltinType::FnPtr,
                                arguments,
                            } = self.kind(pointer)
                            && let [GenericArgument::Type(signature)] = arguments[..]
                            && self.compatible(named, signature)
                        {
                            return Ok(pointer);
                        }
                        Ok(named)
                    }
                    SignatureKind::Const { ty: t, .. } => {
                        if let Err(mut error) =
                            constant_identity(p.clone(), &mut |id| self.snapshot(id))
                        {
                            if let QueryError::Diagnostics(ds) = &mut error {
                                for d in ds {
                                    d.primary = self.span(id);
                                }
                            }
                            return Err(error);
                        }
                        let root = self.import(*t)?;
                        // A counted named constant read shares program-lifetime
                        // storage, just as Copy does; it cannot acquire exclusivity.
                        if self.bound(root, ast::Bound::Plain)?
                            || self.bound(root, ast::Bound::Flat)?
                            || matches!(
                                self.kind(root),
                                TypeKind::Builtin {
                                    kind: BuiltinType::Shared,
                                    ..
                                }
                            )
                        {
                            Ok(root)
                        } else {
                            let mut row = self.types.get(root).unwrap().clone();
                            row.modes.uniqueness = Uniqueness::Aliased;
                            let root = self.types.intern(row);
                            let generics = self.body.resolution.generic_declarations.clone();
                            let store = self.types;
                            let sendability =
                                sendability_after_sharing(&store, root, &generics, &mut |path| {
                                    self.snapshot(path).map(Some)
                                })?;
                            let mut row = self.types.get(root).unwrap().clone();
                            row.modes.sendability = sendability;
                            Ok(self.types.intern(row))
                        }
                    }
                    SignatureKind::Type(t) => {
                        let args = expected
                            .filter(|t| !callee || self.partial_result(*t).is_none())
                            .and_then(|t| match self.kind(self.packed_payload(t).unwrap_or(t)) {
                                TypeKind::Named {
                                    definition,
                                    arguments,
                                } if *definition.path() == p => Some(arguments),
                                _ => None,
                            })
                            .unwrap_or_default();
                        let missing_arguments = args.is_empty();
                        let nominal = self.ty(TypeKind::Named {
                            definition: DefId::intern(&p),
                            arguments: args,
                        });
                        if !callee && record_place {
                            let mut representation = nominal;
                            let mut seen = BTreeSet::new();
                            while let TypeKind::Named { definition, .. } = self.kind(representation)
                            {
                                if !seen.insert(definition) {
                                    break;
                                }
                                representation = self.representation(representation)?.1;
                            }
                            // Spec #64: type names are not ordinary values;
                            // only record names introduce bare constructors.
                            if !matches!(self.kind(representation), TypeKind::Record(_)) {
                                self.report(
                                    const { RuleId::of("LANG-7") },
                                    self.span(id),
                                    "a type name cannot be used as a value",
                                );
                                return Ok(self.error());
                            }
                            // Direct calls infer from their arguments; a bare
                            // record must have a complete contextual type.
                            if !t.generics.is_empty() && missing_arguments {
                                self.report(
                                    const { RuleId::of("LANG-2") },
                                    self.span(id),
                                    "a bare generic constructor requires an expected type",
                                );
                                return Ok(self.error());
                            }
                            if matches!(self.kind(representation), TypeKind::Record(fields) if !fields.is_empty())
                            {
                                self.report(
                                    const { RuleId::of("LANG-12") },
                                    self.span(id),
                                    "record fields must be supplied by a labelled constructor call",
                                );
                                return Ok(self.error());
                            }
                        }
                        Ok(nominal)
                    }
                }
            }
            _ => {
                // H can receive a branch context that P did not yet know.
                // It overrides P's provisional constructor owner.
                if let Some(expected) = expected
                    && let Some(variant) = self.expected_variant(id, path, expected, callee)?
                {
                    return Ok(variant);
                }
                let ctx = self
                    .body
                    .resolution
                    .contextual
                    .iter()
                    .find(|(site, name)| {
                        *site == NameSite::Expression(id)
                            && matches!(
                                name,
                                ContextualName::Constructor(_) | ContextualName::Variant { .. }
                            )
                    })
                    .map(|(_, c)| c.clone());
                if let Some(
                    ContextualName::Constructor(p) | ContextualName::Variant { owner: p, .. },
                ) = ctx
                {
                    let args = expected
                        .filter(|t| !callee || self.partial_result(*t).is_none())
                        .and_then(|t| match self.kind(self.packed_payload(t).unwrap_or(t)) {
                            TypeKind::Named {
                                definition,
                                arguments,
                            } if *definition.path() == p => Some(arguments),
                            _ => None,
                        })
                        .unwrap_or_default();
                    let t = self.ty(TypeKind::Named {
                        definition: DefId::intern(&p),
                        arguments: args,
                    });
                    if !callee {
                        let (_, r) = self.representation(t)?;
                        if let TypeKind::Variant(vs) = self.kind(r) {
                            let name = path
                                .segments
                                .last()
                                .map(|s| self.name(*s))
                                .unwrap_or_default();
                            if !vs
                                .iter()
                                .any(|v| v.name.as_str() == name && v.fields.is_empty())
                            {
                                self.report(
                                    const { RuleId::of("LANG-7") },
                                    self.span(id),
                                    "variant payload must be supplied by a constructor call",
                                );
                                return Ok(self.error());
                            }
                        }
                    }
                    return Ok(if !callee {
                        expected
                            .filter(|expected| {
                                self.packed_payload(*expected)
                                    .is_some_and(|payload| self.compatible(t, payload))
                            })
                            .unwrap_or(t)
                    } else {
                        t
                    });
                }
                if let Some(expected) = expected {
                    if let Some(variant) = self.expected_variant(id, path, expected, callee)? {
                        return Ok(variant);
                    }
                    let nominal = self.packed_payload(expected).unwrap_or(expected);
                    let (owner, representation) = self.representation(nominal)?;
                    if let TypeKind::Variant(variants) = self.kind(representation) {
                        let name = path
                            .segments
                            .last()
                            .map(|s| self.name(*s))
                            .unwrap_or_default();
                        if !variants.iter().any(|variant| variant.name.as_str() == name) {
                            let type_name = owner
                                .as_ref()
                                .map_or("expected type", |p| p.0.name.as_str());
                            let names = variants
                                .iter()
                                .map(|variant| variant.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            self.report(
                                const { RuleId::of("LANG-2") },
                                self.span(id),
                                &format!(
                                    "{name} is not a variant of {type_name}; variants: {names}"
                                ),
                            );
                            if path.package.is_none() && path.segments.len() == 1 {
                                let span = self.span(id);
                                let name_span =
                                    (span.end - span.start == name.len() as u32).then_some(span);
                                hirsel_diag::suggest_names(
                                    self.diagnostics.last_mut().unwrap(),
                                    &name,
                                    variants.into_iter().map(|v| v.name.to_string()),
                                    "",
                                    name_span,
                                );
                            }
                            return Ok(self.error());
                        }
                    }
                }
                self.report(
                    const { RuleId::of("LANG-2") },
                    self.span(id),
                    "bare variant needs an unambiguous expected type",
                );
                Ok(self.error())
            }
        }
    }
    pub(super) fn hint(
        &mut self,
        id: ast::ExprId,
        frame: usize,
    ) -> Result<Option<TypeId>, QueryError> {
        // Probe declared result/projection types without checking arguments or
        // reading any implementation. Keep deep callable chains off the host stack.
        enum Projection {
            Call,
            Field(String),
            Index,
        }
        let mut id = id;
        let mut projections = vec![];
        let mut ty = loop {
            match self.input.syntax.tree.expressions[id.0 as usize]
                .kind
                .clone()
            {
                ast::ExprKind::Literal(l) => {
                    break literals::fixed_primitive(&l).map(|p| self.primitive(p));
                }
                ast::ExprKind::Path(path) => {
                    let resolved = self.input_name(id).map(|(_, r)| r.clone());
                    break match resolved {
                        Some(
                            ResolvedName::Local(_)
                            | ResolvedName::Parameter(_)
                            | ResolvedName::Definition(_),
                        ) => {
                            // An operand hint probes its type; only the actual
                            // expression check records executable field steps.
                            let hinted = self.path_type(id, &path, None, frame, false, false);
                            Some(hinted?)
                        }
                        Some(ResolvedName::Size(_)) => Some(self.primitive(Primitive::U64)),
                        _ => None,
                    };
                }
                ast::ExprKind::Call { callee, .. } => {
                    projections.push(Projection::Call);
                    id = callee;
                }
                ast::ExprKind::Field { base, field } => {
                    projections.push(Projection::Field(self.name(field)));
                    id = base;
                }
                ast::ExprKind::Index { base, .. } => {
                    projections.push(Projection::Index);
                    id = base;
                }
                ast::ExprKind::Unary { value, .. } | ast::ExprKind::Comptime(value) => id = value,
                _ => break None,
            }
        };
        while let (Some(t), Some(projection)) = (ty, projections.pop()) {
            ty = match projection {
                Projection::Call => match self.kind(t) {
                    TypeKind::Function(f) if f.generics.is_empty() => Some(f.result),
                    _ => None,
                },
                Projection::Field(name) => {
                    let (_, repr) = self.representation(t)?;
                    match self.kind(repr) {
                        TypeKind::Record(fields) => fields
                            .into_iter()
                            .find(|f| f.name.as_str() == name)
                            .map(|f| f.ty),
                        _ => None,
                    }
                }
                Projection::Index => self.element(t),
            };
        }
        Ok(ty)
    }
    pub(super) fn numeric(&self, t: TypeId) -> bool {
        matches!(
            self.kind(t),
            TypeKind::Primitive(
                Primitive::U8
                    | Primitive::U16
                    | Primitive::U32
                    | Primitive::U64
                    | Primitive::U128
                    | Primitive::I8
                    | Primitive::I16
                    | Primitive::I32
                    | Primitive::I64
                    | Primitive::I128
                    | Primitive::F32
                    | Primitive::F64
            )
        )
    }
    pub(super) fn integer(&self, t: TypeId) -> bool {
        self.numeric(t)
            && !matches!(
                self.kind(t),
                TypeKind::Primitive(Primitive::F32 | Primitive::F64)
            )
    }
    pub(super) const SEQUENCE_ANNOTATION_HELP: &str = "help: annotate the binding with the intended complete sequence type: `List[T]`, `Array[T]`, or `Inline[T, N]` (replace T with the element type and N with the literal length); type arguments are written in annotations, not expressions";
    pub(super) const REPEAT_SEQUENCE_HELP: &str = "help: annotate the binding, for example `let values: Array[u64] = [1; 2]` or `let values: Inline[u64, 2] = [1; 2]`; a numeric suffix alone cannot choose the repeat sequence kind, and List does not accept repeat literals";

    /// Enrich the existing first-element failure, never manufacture a second
    /// error from recovery or use later elements to determine the type.
    pub(super) fn sequence_prefix_guidance(&mut self, children: &[ast::ExprId], repeat: bool) {
        let Some(first) = children.first().copied() else {
            return;
        };
        if !self.invalid(self.value(first)) {
            return;
        }
        // Repeat syntax wraps its value in a synthetic copy expression.
        let first = if repeat {
            match self.input.syntax.tree.expressions[first.0 as usize].kind {
                ast::ExprKind::Unary {
                    op: ast::UnaryOp::Copy,
                    value,
                } => value,
                _ => first,
            }
        } else {
            first
        };
        let span = self.span(first);
        let kind = &self.input.syntax.tree.expressions[first.0 as usize].kind;
        let numeric = matches!(
            kind,
            ast::ExprKind::Literal(ast::Literal::Integer(_) | ast::Literal::Float(_))
        ) || matches!(kind, ast::ExprKind::Unary { op: ast::UnaryOp::Neg, value }
                if matches!(self.input.syntax.tree.expressions[value.0 as usize].kind,
                    ast::ExprKind::Literal(ast::Literal::Integer(_) | ast::Literal::Float(_))));
        let constructor = matches!(kind, ast::ExprKind::Path(_))
            || self.body.calls.iter().any(|call| {
                call.expression == first && matches!(call.target, CallTarget::Constructor(_))
            });
        if !numeric && !constructor {
            return;
        }
        if let Some(diagnostic) = self
            .diagnostics
            .iter_mut()
            .find(|d| d.rule.as_str() == "LANG-2" && d.primary == span)
        {
            if repeat {
                diagnostic
                    .notes
                    .push("a repeat sequence still needs a written kind and element type".into());
                diagnostic.notes.push(Self::REPEAT_SEQUENCE_HELP.into());
            } else {
                diagnostic.notes.push("the first sequence element does not determine a complete element type; later elements cannot supply its missing context".into());
                diagnostic.notes.push(Self::SEQUENCE_ANNOTATION_HELP.into());
                if numeric {
                    diagnostic.notes.push("help: a numeric suffix on the first element can establish List's element type, for example `[1u64, 2]`, `[-1i64, 2]`, or `[1.0f64, 2.0]`; all later elements must match. An annotation also works, for example `let values: List[u64] = [1, 2u64]`, `let values: Array[u64] = [1, 2u64]`, or `let values: Inline[u64, 2] = [1, 2u64]`".into());
                } else {
                    diagnostic.notes.push("help: for `[None(), Some(\"x\")]`, use `let values: List[Option[String]] = [None(), Some(\"x\")]`, `let values: Array[Option[String]] = [None(), Some(\"x\")]`, or `let values: Inline[Option[String], 2] = [None(), Some(\"x\")]`".into());
                }
            }
        }
    }

    pub(super) fn element(&self, t: TypeId) -> Option<TypeId> {
        match self.kind(t) {
            TypeKind::Builtin {
                kind:
                    BuiltinType::Array | BuiltinType::List | BuiltinType::Inline | BuiltinType::Iter,
                arguments,
            } => arguments.first().and_then(|a| match a {
                GenericArgument::Type(t) => Some(*t),
                _ => None,
            }),
            _ => None,
        }
    }
    /// Written callable headers have their own parameter names, even though
    /// those names have no runtime binding row. Preserve their selectors in
    /// the receiving contract instead of the body's Environment placeholder.
    fn callable_header_relation(
        &mut self,
        mut syntax: ast::TypeExprId,
        ty: TypeId,
        owner: NameSite,
        parameters: &[Parameter],
        current: Option<usize>,
    ) -> Result<TypeId, QueryError> {
        while let ast::TypeExprKind::Qualified {
            base,
            relation,
            brand,
            ..
        } = self.input.syntax.tree.types[syntax.0 as usize].kind.clone()
        {
            if let Some(relation) = relation {
                let site = NameSite::Type(syntax);
                let references: Vec<_> = self
                    .input
                    .resolution
                    .paths
                    .iter()
                    .filter(|(at, _)| *at == site)
                    .map(|(_, name)| name.clone())
                    .collect();
                let scopes: Vec<_> = self
                    .input
                    .resolution
                    .reference_scopes
                    .iter()
                    .filter(|(at, _)| *at == site)
                    .map(|(_, scope)| *scope)
                    .collect();
                let mut index = usize::from(brand.as_ref().is_some_and(|brand| !brand.sibling));
                let mut declared = Relation::global();
                let mut own = false;
                for place in relation.places {
                    match place {
                        ast::RelationPlace::Heap => (),
                        ast::RelationPlace::Environment => {
                            declared = declared.union(&Relation::environment());
                        }
                        ast::RelationPlace::Path(path) => {
                            let reference = references.get(index);
                            let scope = scopes.get(index);
                            index += 1;
                            let Some(ResolvedName::Parameter(parameter)) =
                                reference.filter(|_| scope == Some(&owner))
                            else {
                                declared = declared.union(&Relation::environment());
                                continue;
                            };
                            let ordinal = parameter.0 as usize;
                            let Some(source) = parameters.get(ordinal) else {
                                continue;
                            };
                            own = true;
                            if current == Some(ordinal) && path.segments.len() == 1 {
                                if source.passing == ast::PassingMode::Take {
                                    declared = declared.union(&Relation::environment());
                                }
                                continue;
                            }
                            let mut part = source.ty;
                            let mut fields = vec![];
                            for name in path.segments.iter().skip(1) {
                                let (_, representation) = self.representation(part)?;
                                let TypeKind::Record(parts) = self.kind(representation) else {
                                    return Ok(ty);
                                };
                                let Some((ordinal, field)) = parts
                                    .iter()
                                    .enumerate()
                                    .find(|(_, field)| field.name == *name)
                                else {
                                    return Ok(ty);
                                };
                                fields.push(FieldId(ordinal as u32));
                                part = field.ty;
                            }
                            declared = declared.union(&Relation::place(Origin {
                                root: OriginRoot::Parameter(ParameterRef::bound(*parameter)),
                                fields: fields.into(),
                            }));
                        }
                    }
                }
                if own {
                    if current.is_some_and(|ordinal| {
                        parameters[ordinal].passing != ast::PassingMode::Take
                    }) {
                        declared = declared.union(&Relation::environment());
                    }
                    let mut row = self.types.get(ty).unwrap().clone();
                    row.modes.relation = declared;
                    let ty = self.types.intern(row);
                    return Ok(self.normalize_properties(ty));
                }
            }
            syntax = base;
        }
        Ok(ty)
    }
    pub(super) fn function_syntax(
        &mut self,
        s: &ast::FunctionSyntax,
        frame: usize,
        owner: NameSite,
    ) -> Result<FunctionType, QueryError> {
        if let Some(attribute) = s
            .attributes
            .iter()
            .find(|a| self.name(a.name) == "track_caller")
        {
            return Err(QueryError::Diagnostics(vec![
                hirsel_diag::Diagnostic::error(
                    const { RuleId::of("LANG-6") },
                    attribute.origin.span,
                    "@track_caller applies only to ordinary named function items",
                ),
            ]));
        }
        let mut parameters = vec![];
        for p in &s.parameters {
            self.environment_relation(p.ty, "a parameter", &[])?;
            let mut ty = self.syntax_type(p.ty, frame)?;
            // Nested headers have the same declared defaults as top-level
            // signatures (MODE-7): a borrowed or mut parameter is a
            // call-bounded view, and a written relation adds to that root
            // (S2). Plain values remain global (MODE-1).
            if p.passing != ast::PassingMode::Take && !self.bound(ty, ast::Bound::Plain)? {
                let mut row = self.types.get(ty).unwrap().clone();
                row.modes.relation = Relation::environment().union(&row.modes.relation);
                ty = self.types.intern(row);
            }
            parameters.push(Parameter {
                name: p.name.map(|s| self.name(s)),
                label: p.label.map(|s| self.name(s)),
                passing: p.passing,
                ty,
            });
        }
        let result = s
            .result
            .map(|t| self.syntax_type(t, frame))
            .transpose()?
            .unwrap_or_else(|| self.ty(TypeKind::Tuple(vec![].into())));
        if matches!(
            owner,
            NameSite::Type(_) | NameSite::Statement(_) | NameSite::Expression(_)
        ) {
            for (ordinal, syntax) in s.parameters.iter().enumerate() {
                parameters[ordinal].ty = self.callable_header_relation(
                    syntax.ty,
                    parameters[ordinal].ty,
                    owner,
                    &parameters,
                    Some(ordinal),
                )?;
            }
        }
        let result = if let Some(syntax) = s.result.filter(|_| {
            matches!(
                owner,
                NameSite::Type(_) | NameSite::Statement(_) | NameSite::Expression(_)
            )
        }) {
            self.callable_header_relation(syntax, result, owner, &parameters, None)?
        } else {
            result
        };
        // MODE-1: a local function is named and its local result names its
        // sources (spec #70). Body rows do
        // not record named sources, so a written relation decides.
        if let (NameSite::Statement(_), Some(t)) = (owner, s.result)
            && hirsel_check_protocol::declaration::written_local(&self.input.syntax.tree, t)
                .unwrap_or(
                    self.types
                        .get(result)
                        .unwrap()
                        .modes
                        .relation
                        .has_environment(),
                )
        {
            self.report(
                const { RuleId::of("MODE-1") },
                self.input.syntax.tree.types[t.0 as usize].origin.span,
                "a named function has no environment: its local result names its sources, `in x`",
            );
        }
        // MODE-1 (#72): a closure or function type's `in env` alone may not
        // narrow a non-Plain parameter out of its result.
        if !matches!(owner, NameSite::Statement(_))
            && let Some(span) = s.result.and_then(|t| {
                hirsel_check_protocol::declaration::environment_alone(&self.input.syntax.tree, t)
            })
        {
            let mut candidates = vec![];
            for (i, p) in parameters.iter().enumerate() {
                if !self.bound(p.ty, ast::Bound::Plain)? {
                    candidates.push(
                        p.name
                            .as_ref()
                            .map_or_else(|| format!("parameter {}", i + 1), |n| n.to_string()),
                    );
                }
            }
            if !candidates.is_empty() {
                self.report(
                    const { RuleId::of("MODE-1") },
                    span,
                    &hirsel_check_protocol::declaration::environment_candidates(&candidates),
                );
            }
        }
        let error = self.result_parts(result)?.map(|(_, _, e)| e);
        let scope = self.site(owner);
        let mut abstract_type = |root| {
            map_generic_binders(self.types, root, |b, nested| {
                Ok(if b == GenericBinder::Body(scope) {
                    GenericBinder::Bound(nested)
                } else {
                    b
                })
            })
            .expect("checked generic binder")
        };
        let lexical: Vec<TypeId> = parameters.iter().map(|p| p.ty).collect();
        for p in &mut parameters {
            p.ty = abstract_type(p.ty);
        }
        let result = abstract_type(result);
        let error = error.map(abstract_type);
        let arenas: BTreeSet<ParamId> = match &self.input.signature.metadata.roles.heap {
            Some(heap) => parameters
                .iter()
                .enumerate()
                .filter(|(_, p)| hirsel_types::is_arena_type(self.types, p.ty, &heap.arena))
                .map(|(i, _)| ParamId(i as u32))
                .collect(),
            None => BTreeSet::new(),
        };
        let result = local_result_destination(self.types, result, &parameters, |o| {
            matches!(o.root, OriginRoot::Parameter(ParameterRef::Bound {
                depth: 0,
                parameter,
            }) if o.fields.is_empty() && arenas.contains(&parameter))
        });
        let (state_promises, result) =
            self.callable_state_promises(s, &mut parameters, &lexical, result)?;
        let (authority, uses) = match &s.uses {
            None => (AuthoritySummary::Unknown, Uses::Unrestricted),
            Some(paths) => {
                let allowed = self.written_uses(owner, paths, frame)?;
                (allowed.clone(), Uses::Only(allowed))
            }
        };
        Ok(FunctionType {
            state_promises,
            generics: s
                .generics
                .iter()
                .map(|g| match &g.kind {
                    ast::GenericKind::Type(bounds) => GenericParameter::Type {
                        name: self.name(g.name),
                        bounds: bounds.clone().into(),
                    },
                    // The resolver reports a local implementation binder (GEN-6).
                    ast::GenericKind::Size | ast::GenericKind::Implementation { .. } => {
                        GenericParameter::Size {
                            name: self.name(g.name),
                        }
                    }
                })
                .collect(),
            parameters: parameters.into(),
            result,
            error,
            allocation: if s.attributes.iter().any(|a| self.name(a.name) == "noalloc") {
                Allocation::NoAlloc
            } else {
                Allocation::MayAllocate
            },
            authority,
            uses,
            contracts: vec![].into(),
            incomplete: s
                .attributes
                .iter()
                .any(|a| self.name(a.name) == "incomplete"),
            foreign: None,
        })
    }
    /// A nested callable's state selectors name its own parameter binder and
    /// declaration-order fields, exactly as a header's do (contract 6).
    fn callable_state_promises(
        &mut self,
        s: &ast::FunctionSyntax,
        parameters: &mut [Parameter],
        lexical: &[TypeId],
        mut result: TypeId,
    ) -> Result<(SharedSlice<StatePromise>, TypeId), QueryError> {
        let fail = |rule: RuleId, span: Span, message: &str| {
            QueryError::Diagnostics(vec![hirsel_diag::Diagnostic::error(rule, span, message)])
        };
        let mut promises = BTreeMap::<RelationPlace, StatePromise>::new();
        let mut captures = Vec::new();
        let mut captures_span = None;
        for attribute in &s.attributes {
            let [ast::AttributeArgument::StatePromise(promise)] = attribute.arguments.as_slice()
            else {
                continue;
            };
            let span = promise.origin.span;
            for path in &promise.selectors {
                let unknown = || {
                    fail(
                        const { RuleId::of("LANG-6") },
                        span,
                        "a callable selector names its own parameter and explicit fields",
                    )
                };
                let (&first, rest) = path.segments.split_first().ok_or_else(unknown)?;
                let name = self.name(first);
                let ordinal = s
                    .parameters
                    .iter()
                    .position(|p| p.name.is_some_and(|n| self.name(n) == name))
                    .ok_or_else(unknown)?;
                let mut ty = lexical[ordinal];
                let mut fields = vec![];
                for segment in rest {
                    while let TypeKind::Box(part) = self.kind(ty) {
                        fields.push(FieldId(0));
                        ty = part;
                    }
                    let (_, repr) = self.representation(ty)?;
                    let name = self.name(*segment);
                    let TypeKind::Record(parts) = self.kind(repr) else {
                        return Err(unknown());
                    };
                    let (index, part) = parts
                        .iter()
                        .enumerate()
                        .find(|(_, f)| f.name.as_str() == name)
                        .ok_or_else(unknown)?;
                    fields.push(FieldId(index as u32));
                    ty = part.ty;
                }
                let selector = RelationPlace {
                    parameter: ParameterRef::bound(ParamId(ordinal as u32)),
                    fields: fields.into(),
                };
                if promise.kind == ast::StatePromiseKind::Captures {
                    captures.push(selector);
                    captures_span = Some(span);
                    continue;
                }
                let unique = promise.kind == ast::StatePromiseKind::PreserveUnique;
                let passing = parameters[ordinal].passing;
                if passing != ast::PassingMode::Mut
                    && !(unique && passing == ast::PassingMode::Take)
                {
                    return Err(fail(
                        const { RuleId::of("MEM-14") },
                        span,
                        "a state preservation selector requires a mut parameter",
                    ));
                }
                if unique && !selector.fields.is_empty() {
                    return Err(fail(
                        const { RuleId::of("MODE-2") },
                        span,
                        "@preserves_unique selects a whole mut parameter",
                    ));
                }
                let row = promises.entry(selector.clone()).or_insert(StatePromise {
                    selector,
                    advance_only: false,
                    preserve_captures: false,
                    preserve_unique: false,
                    unretained: Default::default(),
                });
                row.advance_only |= promise.kind == ast::StatePromiseKind::AdvanceOnly;
                row.preserve_captures |= !unique;
                row.preserve_unique |= unique;
            }
        }
        if !captures.is_empty() {
            captures.sort();
            captures.dedup();
            let mut row = self.types.get(result).unwrap().clone();
            let carrier = match &row.kind {
                TypeKind::Named { definition, .. } => matches!(
                    self.snapshot(&definition.clone()).map(|s| s.kind.clone()),
                    Ok(SignatureKind::Type(d)) if d.captures.is_some()
                ),
                _ => false,
            };
            if !carrier
                && !matches!(
                    row.kind,
                    TypeKind::Function(_)
                        | TypeKind::Builtin {
                            kind: BuiltinType::Iter,
                            ..
                        }
                )
            {
                return Err(fail(
                    const { RuleId::of("CLO-4") },
                    captures_span.unwrap(),
                    "@captures requires a callable, erased iterator or erased carrier result",
                ));
            }
            row.erased_captures = Some(ErasedCaptures::FromArguments(captures.into()));
            result = self.types.intern(row);
        }
        let promises: SharedSlice<_> = promises.into_values().collect();
        apply_state_promises(self.types, parameters, &promises);
        Ok((promises, result))
    }
    /// Whether a written relation bounds a lifetime: it names `env` or a
    /// place that is not an Arena binding (MODE-1). An arena place only says
    /// where blocks live (PROV-4).
    /// `env` names a closure or function type's environment, so only that
    /// type's result relation may name it (MODE-1).
    /// Potential cursor covers are diagnosed by ownership after borrowed-only
    /// classification; ordinary `env` relations retain MODE-1.
    pub(super) fn environment_relation(
        &mut self,
        t: ast::TypeExprId,
        what: &str,
        mutable: &[TypeId],
    ) -> Result<(), QueryError> {
        if let ast::TypeExprKind::Qualified {
            relation: Some(relation),
            ..
        } = &self.input.syntax.tree.types[t.0 as usize].kind
            && relation.places.contains(&ast::RelationPlace::Environment)
        {
            let span = relation.origin.span;
            for &ty in mutable {
                if !self.bound(ty, ast::Bound::Plain)?
                    && self.bound(ty, ast::Bound::Data)?
                    && self.bound(ty, ast::Bound::Copyable)?
                {
                    return Ok(());
                }
            }
            self.report(
                const { RuleId::of("MODE-1") },
                span,
                &format!(
                    "`env` is a closure or function type's environment: {what} is not related to it"
                ),
            );
        }
        Ok(())
    }
    fn lending_relation(&self, site: ast::TypeExprId, relation: &ast::RelationSyntax) -> bool {
        let heap = self.input.signature.metadata.roles.heap.as_ref();
        let mut resolved = self
            .input
            .resolution
            .paths
            .iter()
            .filter(|(s, _)| *s == NameSite::Type(site))
            .map(|(_, name)| name);
        relation.places.iter().any(|place| match place {
            ast::RelationPlace::Heap => false,
            ast::RelationPlace::Environment => true,
            ast::RelationPlace::Path(path) => {
                let name = resolved.next();
                path.segments.len() > 1
                    || !name.is_some_and(|name| {
                        self.body.bindings.iter().rev().any(|b| {
                            &b.binding == name
                                && heap.is_some_and(|h| {
                                    hirsel_types::is_arena_type(self.types, b.ty, &h.arena)
                                })
                        })
                    })
            }
        })
    }
    /// The mode axes written on an annotation's root, through its qualifiers.
    pub(super) fn written_modes(&self, mut syntax: ast::TypeExprId) -> WrittenModes {
        let mut written = WrittenModes::default();
        loop {
            match &self.input.syntax.tree.types[syntax.0 as usize].kind {
                ast::TypeExprKind::Qualified {
                    base,
                    modifiers,
                    relation,
                    ..
                } => {
                    // A written relation fixes a binding's receiving contract (LANG-15).
                    written.relation |= relation.is_some();
                    for modifier in modifiers {
                        match modifier {
                            ast::TypeModifier::Unique => written.unique = true,
                            ast::TypeModifier::Once => written.once = true,
                            ast::TypeModifier::Sendable => written.sendable = true,
                        }
                    }
                    syntax = *base;
                }
                ast::TypeExprKind::Function(_) => {
                    written.function = true;
                    return written;
                }
                _ => return written,
            }
        }
    }
    pub(super) fn syntax_type(
        &mut self,
        root: ast::TypeExprId,
        frame: usize,
    ) -> Result<TypeId, QueryError> {
        let mut stack = vec![(root, false)];
        let mut done = BTreeMap::new();
        while let Some((id, exit)) = stack.pop() {
            let node = &self.input.syntax.tree.types[id.0 as usize];
            if !exit {
                stack.push((id, true));
                let children = match &node.kind {
                    ast::TypeExprKind::Tuple(ts) => ts.clone(),
                    ast::TypeExprKind::Box(t) | ast::TypeExprKind::Qualified { base: t, .. } => {
                        vec![*t]
                    }
                    ast::TypeExprKind::Named { arguments, .. } => arguments
                        .iter()
                        .filter_map(|a| match a {
                            ast::TypeArgument::Type(t)
                                if !self.input.resolution.types.iter().any(|(id, name)| {
                                    id == t && matches!(name, ResolvedName::Size(_))
                                }) =>
                            {
                                Some(*t)
                            }
                            _ => None,
                        })
                        .collect(),
                    _ => vec![],
                };
                for t in children.into_iter().rev() {
                    stack.push((t, false));
                }
                continue;
            }
            // A part is as local as its owner (OWN-8; LANG-7 as in signatures).
            let parts: Vec<_> = match &node.kind {
                ast::TypeExprKind::Tuple(ts) => ts.clone(),
                ast::TypeExprKind::Box(t) => vec![*t],
                ast::TypeExprKind::Named { arguments, .. } => arguments
                    .iter()
                    .filter_map(|a| match a {
                        ast::TypeArgument::Type(t) if done.contains_key(t) => Some(*t),
                        _ => None,
                    })
                    .collect(),
                _ => vec![],
            };
            for part in parts {
                if !self
                    .types
                    .get(done[&part])
                    .unwrap()
                    .modes
                    .relation
                    .is_global()
                {
                    self.report(
                        const { RuleId::of("LANG-7") },
                        self.input.syntax.tree.types[part.0 as usize].origin.span,
                        hirsel_check_protocol::declaration::PART_LOCALITY,
                    );
                }
            }
            let ty = match &node.kind {
                ast::TypeExprKind::Tuple(ts) => {
                    self.ty(TypeKind::Tuple(ts.iter().map(|t| done[t]).collect()))
                }
                ast::TypeExprKind::Box(t) => self.ty(TypeKind::Box(done[t])),
                ast::TypeExprKind::Function(f) => {
                    let f = self.function_syntax(f, frame, NameSite::Type(id))?;
                    self.ty(TypeKind::Function(f))
                }
                ast::TypeExprKind::Qualified {
                    base,
                    modifiers,
                    relation,
                    ..
                } => {
                    let mut row = self.types.get(done[base]).unwrap().clone();
                    if let Some(relation) = relation {
                        if self.lending_relation(id, relation) {
                            row.modes.relation = Relation::environment();
                        } else if relation.places == [ast::RelationPlace::Heap] {
                            // `in heap` is the empty relation (PROV-4).
                            row.modes.relation =
                                row.modes.relation.retain(|r| *r == Reach::Environment)
                        }
                        // An arena alone places the value; the authority
                        // pass applies placement (PROV-4).
                    }
                    for m in modifiers {
                        match m {
                            ast::TypeModifier::Unique => row.modes.uniqueness = Uniqueness::Unique,
                            ast::TypeModifier::Once => {
                                // See spec #40: once is only established for a
                                // known function, including in nested headers.
                                if !matches!(row.kind, TypeKind::Function(_)) {
                                    self.report(
                                        const { RuleId::of("MODE-3") },
                                        node.origin.span,
                                        "once requires a function type",
                                    );
                                }
                                row.modes.multiplicity = Multiplicity::Once;
                            }
                            ast::TypeModifier::Sendable => {
                                row.sendability_requirement = SendabilityRequirement::Sendable;
                            }
                        }
                    }
                    let id = self.types.intern(row.clone());
                    // Mode and placement qualifiers say nothing about plain
                    // bits, as in signatures (MODE-1, MODE-2, PROV-4).
                    let written = row.clone();
                    row.canonicalize_plain();
                    if row != written && self.bound(id, ast::Bound::Plain)? {
                        self.types.intern(row)
                    } else {
                        id
                    }
                }
                ast::TypeExprKind::Named { path, arguments } => {
                    let r = self
                        .input
                        .resolution
                        .types
                        .iter()
                        .find(|(t, _)| *t == id)
                        .map(|(_, r)| r.clone());
                    let args: Vec<_> = arguments
                        .iter()
                        .map(|a| match a {
                            ast::TypeArgument::Type(t) => match self
                                .input
                                .resolution
                                .types
                                .iter()
                                .find(|(i, _)| i == t)
                                .map(|(_, r)| r)
                            {
                                Some(ResolvedName::Size(s)) => {
                                    GenericArgument::Size(Size::Parameter(*s))
                                }
                                _ => GenericArgument::Type(done[t]),
                            },
                            ast::TypeArgument::SizeLiteral(n) => {
                                GenericArgument::Size(Size::Constant(*n))
                            }
                        })
                        .collect();
                    match r {
                        Some(ResolvedName::Primitive(p)) => self.primitive(p),
                        Some(ResolvedName::TypeParameter(p)) => self.ty(TypeKind::Parameter(p)),
                        Some(ResolvedName::Size(_)) => self.error(),
                        Some(ResolvedName::Definition(p)) => {
                            let s = match self.snapshot(&p) {
                                Ok(s) => Some(s),
                                Err(QueryError::Diagnostics(_)) => None,
                                Err(e) => return Err(e),
                            };
                            if let Some(s) = s {
                                let mut serializable = true;
                                for (generic, actual) in s.generics().iter().zip(&args) {
                                    if let (
                                        GenericParameter::Type { bounds, .. },
                                        GenericArgument::Type(actual),
                                    ) = (generic, actual)
                                    {
                                        if bounds.contains(&ast::Bound::Serializable)
                                            && !self.bound(*actual, ast::Bound::Serializable)?
                                        {
                                            self.report(const { RuleId::of("SER-1") }, self.input.syntax.tree.types[id.0 as usize].origin.span,
                                                "type argument needs a transparent serializable schema");
                                            serializable = false;
                                        }
                                    }
                                }
                                if !serializable {
                                    self.error()
                                } else if !self.schema_arguments(
                                    self.input.syntax.tree.types[id.0 as usize].origin.span,
                                    &s.metadata.schema,
                                    &args,
                                )? {
                                    self.error()
                                } else if s.metadata.alias {
                                    if let SignatureKind::Type(td) = &s.kind {
                                        if let Some(t) = td.representation {
                                            let t = self.import(t)?;
                                            if args.is_empty() {
                                                t
                                            } else {
                                                self.substitute(t, &args)?
                                            }
                                        } else {
                                            self.error()
                                        }
                                    } else {
                                        self.error()
                                    }
                                } else if let Some(b) = builtin(&p.0.name)
                                    .filter(|_| p.package_is("core", "1") && p.0.modules.is_empty())
                                {
                                    // Builtin identity is the core@1 declaration's, never
                                    // the spelling's: a user `Map` stays nominal (LANG-4).
                                    // A builtin's bound holds wherever its type is
                                    // written, as in signatures (GEN-1).
                                    let unmet = match (
                                        hirsel_check_protocol::declaration::builtin_bound(b),
                                        args.first(),
                                    ) {
                                        (Some(bound), Some(GenericArgument::Type(t)))
                                            if !matches!(
                                                self.types.get(*t).unwrap().kind,
                                                TypeKind::Error
                                            ) =>
                                        {
                                            !self.bound(*t, bound)?
                                        }
                                        _ => false,
                                    };
                                    if unmet {
                                        self.report(
                                            const { RuleId::of("GEN-1") },
                                            self.input.syntax.tree.types[id.0 as usize].origin.span,
                                            "builtin argument does not satisfy its bound",
                                        );
                                        self.error()
                                    } else {
                                        self.ty(TypeKind::Builtin {
                                            kind: b,
                                            arguments: args.into(),
                                        })
                                    }
                                } else {
                                    self.ty(TypeKind::Named {
                                        definition: DefId::intern(&p),
                                        arguments: args.into(),
                                    })
                                }
                            } else {
                                self.error()
                            }
                        }
                        None if path.package.is_none()
                            && path.segments.len() == 1
                            && self.name(path.segments[0]) == "FnPtr" =>
                        {
                            // FnPtr is compiler-native and has no registry declaration.
                            // Use the same type-argument rule as header normalization.
                            if args.is_empty()
                                || !args.iter().all(|a| matches!(a, GenericArgument::Type(_)))
                            {
                                self.report(
                                    const { RuleId::of("GEN-1") },
                                    node.origin.span,
                                    "FnPtr requires type arguments",
                                );
                                self.error()
                            } else {
                                self.ty(TypeKind::Builtin {
                                    kind: BuiltinType::FnPtr,
                                    arguments: args.into(),
                                })
                            }
                        }
                        _ => self.error(),
                    }
                }
                ast::TypeExprKind::Error => self.error(),
            };
            done.insert(id, ty);
        }
        Ok(done[&root])
    }
    fn type_authority(&mut self, root: TypeId) -> Result<AuthoritySummary, QueryError> {
        let declarations = self.body.resolution.generic_declarations.clone();
        let store = self.types;
        let roles = self.input.signature.metadata.roles.clone();
        structural_authority(
            &store,
            root,
            &roles,
            &declarations,
            &mut |_, path| match path {
                None => Ok(None),
                Some(path) => match self.snapshot(path) {
                    Ok(s) => Ok(Some(s)),
                    Err(QueryError::Diagnostics(_)) => Ok(None),
                    Err(e) => Err(e),
                },
            },
        )
    }
    /// A written `uses` clause, kept exactly as written (#58), as signatures
    /// keep theirs: a definition is a capability, and a binding contributes
    /// its type's authority, as the authority pass reads it. A name that did
    /// not resolve leaves the authority unknown.
    fn written_uses(
        &mut self,
        site: NameSite,
        paths: &[ast::Path],
        frame: usize,
    ) -> Result<AuthoritySummary, QueryError> {
        let resolved: Vec<ResolvedName> = self
            .input
            .resolution
            .paths
            .iter()
            .filter(|(s, _)| *s == site)
            .map(|(_, name)| name.clone())
            .collect();
        let scopes: Vec<_> = self
            .input
            .resolution
            .reference_scopes
            .iter()
            .filter(|(at, _)| *at == site)
            .map(|(_, owner)| *owner)
            .collect();
        let mut scopes = scopes.into_iter();
        let mut authority = AuthoritySummary::inert();
        for index in 0..paths.len() {
            let term = match resolved.get(index) {
                Some(ResolvedName::Definition(d)) => {
                    AuthoritySummary::Known(vec![AuthorityTerm::Capability(d.clone())].into())
                }
                Some(name) => {
                    let owner = scopes.next();
                    match name {
                        ResolvedName::Parameter(parameter) if owner == Some(site) => {
                            AuthoritySummary::Known(
                                vec![AuthorityTerm::Parameter(ParameterRef::bound(*parameter))]
                                    .into(),
                            )
                        }
                        _ => match self.lookup_binding_at(site, name, frame) {
                            Some(ty) => self.type_authority(ty)?,
                            None => AuthoritySummary::Unknown,
                        },
                    }
                }
                None => AuthoritySummary::Unknown,
            };
            authority = authority.join(term);
        }
        Ok(authority)
    }
    pub(super) fn bound(&mut self, root: TypeId, bound: ast::Bound) -> Result<bool, QueryError> {
        let declarations = self.body.resolution.generic_declarations.clone();
        if bound == ast::Bound::Serializable {
            return structural_serializable(
                self.types,
                root,
                &declarations,
                &mut |path| match self.snapshot(path) {
                    Ok(s) => Ok(Some(s)),
                    Err(QueryError::Diagnostics(_)) => Ok(None),
                    Err(e) => Err(e),
                },
            );
        }
        if bound == ast::Bound::Inert {
            return self
                .type_authority(root)
                .map(|a| a == AuthoritySummary::inert());
        }
        if bound == ast::Bound::Flat {
            let generics = declarations.clone();
            return structural_flat(self.types, root, &generics, &mut |path| match self
                .snapshot(path)
            {
                Ok(s) => Ok(Some(s)),
                Err(QueryError::Diagnostics(_)) => Ok(None),
                Err(e) => Err(e),
            });
        }
        let property = match bound {
            ast::Bound::Plain => Some(StructuralProperty::Plain),
            ast::Bound::Copyable => Some(StructuralProperty::Copyable),
            ast::Bound::Sendable => Some(StructuralProperty::Sendable),
            _ => None,
        };
        if let Some(property) = property {
            let generics = declarations.clone();
            return structural_property(
                self.types,
                root,
                property,
                &generics,
                &mut |path| match self.snapshot(path) {
                    Ok(s) => Ok(Some(s)),
                    Err(QueryError::Diagnostics(_)) => Ok(None),
                    Err(e) => Err(e),
                },
            );
        }
        if bound == ast::Bound::NoallocDrop {
            match &self.types.get(root).unwrap().drop_allocation {
                DropAllocation::NoAlloc => return Ok(true),
                DropAllocation::DependsOn(parameters) => {
                    // Dependencies refer to lexical generic bounds. Rebuilding a
                    // parameter type would reproduce this same dependency forever.
                    return Ok(parameters.iter().all(|parameter| {
                        let bounds = declarations.bounds(*parameter);
                        bounds.contains(&ast::Bound::NoallocDrop)
                            || bounds.contains(&ast::Bound::Plain)
                    }));
                }
                // The summary includes hidden cleanup, such as a SortedMap's
                // comparator; walking only visible type arguments cannot prove it.
                DropAllocation::MayAllocate => return Ok(false),
            }
        }
        structural_equality(
            self.types,
            root,
            bound,
            &declarations,
            &mut |path| match self.snapshot(path) {
                Ok(s) => Ok(Some(s)),
                Err(QueryError::Diagnostics(_)) => Ok(None),
                Err(e) => Err(e),
            },
        )
    }
    /// Report only from resolved operand rows, before recovery erases either.
    pub(super) fn string_addition_error(
        &mut self,
        rule: RuleId,
        span: Span,
        a: TypeId,
        b: TypeId,
    ) -> bool {
        if self.invalid(a) || self.invalid(b) || self.bottom(a) || self.bottom(b) {
            return false;
        }
        let strings =
            [a, b].map(|ty| matches!(self.kind(ty), TypeKind::Primitive(Primitive::String)));
        if strings.contains(&true) {
            let mut diagnostic = Diagnostic::error(
                rule,
                span,
                format!(
                    "operator `+` is not defined for operands `{}` and `{}`",
                    self.type_spelling(a),
                    self.type_spelling(b),
                ),
            );
            // A qualified lookup respects the selected package graph, including
            // negative reads. Never infer String or std availability from spelling.
            let path = ast::Path {
                package: Some(ast::PackageId {
                    name: "std".into(),
                    version: "1".into(),
                }),
                segments: vec![Symbol::intern("string"), Symbol::intern("concat")],
            };
            let lookup = self.provider.lookup(self.input.context, &path);
            let concat_available = lookup.result.is_ok();
            std::sync::Arc::make_mut(&mut self.body.resolution.reads).push(
                DependencyRead::ProviderLookup {
                    context: (self.input.context.clone()).into(),
                    package: path.package,
                    segments: (vec!["string".into(), "concat".into()]).into(),
                    output: (lookup).into(),
                }
                .shared(),
            );
            if strings != [true, true] {
                diagnostic.notes.push("convert the non-String operand explicitly to String first; f-string holes do not implicitly format arbitrary types".into());
            }
            diagnostic.notes.push(if concat_available {
                "help: for String bindings `left` and `right`, use `f\"{left}{right}\"` or `std@1/string.concat(left, right)`; to append a newline to String `text`, use `f\"{text}\\n\"`".into()
            } else {
                "help: select std@1 in the manifest's dependencies to use f-strings; then, for String bindings `left` and `right`, use `f\"{left}{right}\"`; to append a newline to String `text`, use `f\"{text}\\n\"`".into()
            });
            self.diagnostics.push(diagnostic);
            return true;
        }
        false
    }
    pub(super) fn binary(
        &mut self,
        id: ast::ExprId,
        op: ast::BinaryOp,
        left: ast::ExprId,
        right: ast::ExprId,
    ) -> Result<TypeId, QueryError> {
        use ast::BinaryOp::*;
        let a = self.value(left);
        let b = self.value(right);
        if self.invalid(a) || self.invalid(b) {
            return Ok(self.error());
        }
        if op == Add
            && self.string_addition_error(const { RuleId::of("LANG-7") }, self.span(id), a, b)
        {
            return Ok(self.error());
        }
        if !self.compatible(a, b) {
            self.type_mismatch(
                const { RuleId::of("LANG-7") },
                self.span(id),
                "operator operands have different types",
                b,
                a,
            );
            return Ok(self.error());
        }
        match op {
            Eq | Ne => {
                if !self.bound(a, ast::Bound::Data)? {
                    self.report(
                        const { RuleId::of("GEN-1") },
                        self.span(id),
                        "equality requires data operands",
                    );
                    return Ok(self.error());
                }
                Ok(self.primitive(Primitive::Bool))
            }
            Lt | Le | Gt | Ge => {
                if !self.numeric(a)
                    && !matches!(
                        self.kind(a),
                        TypeKind::Primitive(Primitive::Char | Primitive::String | Primitive::Bytes)
                    )
                {
                    self.report(
                        const { RuleId::of("LANG-7") },
                        self.span(id),
                        "ordering requires scalars or byte strings",
                    );
                    return Ok(self.error());
                }
                Ok(self.primitive(Primitive::Bool))
            }
            And | Or => Ok(self.primitive(Primitive::Bool)),
            Range | RangeInclusive => {
                if !self.integer(a) {
                    self.report(
                        const { RuleId::of("SEM-4") },
                        self.span(id),
                        "range bounds require one integer type",
                    );
                    return Ok(self.error());
                }
                self.ranges.insert(id, a);
                Ok(self.ty(TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    arguments: vec![GenericArgument::Type(a)].into(),
                }))
            }
            _ => {
                if !self.numeric(a)
                    || (matches!(op, Shl | Shr | BitAnd | BitOr | BitXor) && !self.integer(a))
                {
                    self.report(
                        const { RuleId::of("LANG-7") },
                        self.span(id),
                        "operator is not defined for this type",
                    );
                    Ok(self.error())
                } else {
                    // PROV-2: computation produces an unmarked value, not an
                    // alias of its operand, even under a branded expectation.
                    let mut result = self.types.get(a).unwrap().clone();
                    if result.brand.alternatives.is_empty() {
                        Ok(a)
                    } else {
                        result.brand = Brand::default();
                        Ok(self.types.intern(result))
                    }
                }
            }
        }
    }
    pub(super) fn iteration_type(
        &mut self,
        source: ast::ExprId,
        ty: TypeId,
    ) -> Result<TypeId, QueryError> {
        if self.invalid(ty) {
            return Ok(ty);
        }
        if let Some(t) = self.ranges.get(&source) {
            return Ok(*t);
        }
        if let TypeKind::Builtin { kind, arguments } = self.kind(ty) {
            if kind == BuiltinType::Iter
                && !matches!(
                    self.input.syntax.tree.expressions[source.0 as usize].kind,
                    ast::ExprKind::Unary {
                        op: ast::UnaryOp::Mut | ast::UnaryOp::Take,
                        ..
                    }
                )
            {
                self.report(
                    const { RuleId::of("SEM-4") },
                    self.span(source),
                    "iterator source needs mut or take",
                );
                return Ok(self.error());
            }
            if kind == BuiltinType::Set {
                if let [GenericArgument::Type(element)] = arguments.as_slice() {
                    return Ok(*element);
                }
            }
            if let Some(fields) = (TypeKind::Builtin { kind, arguments }).map_entry_fields() {
                let tuple = self.ty(TypeKind::Tuple(fields.to_vec().into()));
                return Ok(self.projected_type(ty, tuple));
            }
        }
        if let Some(item) = self.declared_loop_item(ty)? {
            if matches!(
                self.input.syntax.tree.expressions[source.0 as usize].kind,
                ast::ExprKind::Unary {
                    op: ast::UnaryOp::Mut | ast::UnaryOp::Take,
                    ..
                }
            ) {
                self.report(
                    const { RuleId::of("SEM-4") },
                    self.span(source),
                    "a borrowed loop association admits neither mut nor take",
                );
                return Ok(self.error());
            }
            return Ok(item);
        }
        if matches!(self.kind(ty), TypeKind::Primitive(Primitive::Bytes)) {
            return Ok(self.primitive(Primitive::U8));
        }
        if let Some(e) = self.element(ty) {
            return Ok(e);
        }
        self.report(
            const { RuleId::of("SEM-4") },
            self.span(source),
            "expression is not a loop source; use a range, Array, List, Inline, Bytes, Map, SortedMap, Set, Iter, string view or declared loop source",
        );
        Ok(self.error())
    }
}
fn builtin(name: &str) -> Option<BuiltinType> {
    use BuiltinType::*;
    Some(match name {
        "Array" => Array,
        "List" => List,
        "Map" => Map,
        "Set" => Set,
        "SortedMap" => SortedMap,
        "Inline" => Inline,
        "Simd" => Simd,
        "Packed" => Packed,
        "Shared" => Shared,
        "Slots" => Slots,
        "SlotId" => SlotId,
        "Region" => Region,
        "Iter" => Iter,
        "Task" => Task,
        "Sender" => Sender,
        "Receiver" => Receiver,
        "Mutex" => Mutex,
        "Atomic" => Atomic,
        "Ptr" => Ptr,
        "FnPtr" => FnPtr,
        "TypeShape" => TypeShape,
        _ => return None,
    })
}
