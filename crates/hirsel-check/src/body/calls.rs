use super::*;
pub(super) struct PendingCall {
    id: ast::ExprId,
    expected: Option<TypeId>,
    frame: usize,
    target: CallTarget,
    function: FunctionType,
    arguments: Vec<ast::Argument>,
    ordinals: Vec<Option<usize>>,
    inferred: Vec<Option<GenericArgument>>,
    type_contracts: BTreeSet<usize>,
    bad: bool,
    capability_parameters: BTreeSet<usize>,
    missing_capabilities: bool,
    /// Leading written arguments the checker has already checked.
    checked: usize,
}
impl Checker<'_, '_> {
    /// A function value has no inference variables of its own. Its complete
    /// receiving callable supplies every actual, using only the declaration.
    pub(super) fn instantiate_function_value(
        &mut self,
        id: ast::ExprId,
        source: TypeId,
        mut function: FunctionType,
        expected: Option<TypeId>,
    ) -> Result<TypeId, QueryError> {
        // RUN-3: a generic definition forms no entry.
        if expected.and_then(|ty| self.entry_signature(ty)).is_some() {
            self.report(
                const { RuleId::of("RUN-3") },
                self.span(id),
                Self::ENTRY_FORMATION,
            );
            return Ok(self.error());
        }
        let mut inferred = vec![None; function.generics.len()];
        if let Some(TypeKind::Function(wanted)) = expected.map(|ty| self.kind(ty))
            && wanted.generics.is_empty()
            && !wanted.incomplete
            && wanted.parameters.len() == function.parameters.len()
        {
            // Modes and effects remain the declaration's promises. Ordinary
            // callable receipt checks them after substitution.
            for (declared, wanted) in function.parameters.iter().zip(&wanted.parameters) {
                self.infer(declared.ty, wanted.ty, &mut inferred)?;
            }
            self.infer(function.result, wanted.result, &mut inferred)?;
        }
        for (parameter, actual) in function.generics.iter().zip(&inferred) {
            if actual.is_none()
                || matches!(actual, Some(GenericArgument::Type(ty)) if self.partial_result(*ty).is_some())
            {
                let name = match parameter {
                    GenericParameter::Type { name, .. } | GenericParameter::Size { name } => name,
                };
                self.report(
                    const { RuleId::of("LANG-2") },
                    self.span(id),
                    &format!("generic function value cannot determine parameter `{name}` from its immediate complete expected callable type"),
                );
                return Ok(self.error());
            }
        }
        let arguments: Vec<_> = inferred.into_iter().flatten().collect();
        // As for direct calls, a borrowed local result may require Copyable
        // even when the declaration did not spell that bound explicitly.
        for (parameter, actual) in local_result_generics(self.types, &function)
            .iter()
            .zip(&arguments)
        {
            if let (GenericParameter::Type { bounds, .. }, GenericArgument::Type(actual)) =
                (parameter, actual)
            {
                for bound in bounds {
                    if !matches!(bound, ast::Bound::Sendable | ast::Bound::Inert)
                        && !self.bound(*actual, *bound)?
                    {
                        self.report(
                            if *bound == ast::Bound::Serializable {
                                const { RuleId::of("SER-1") }
                            } else {
                                const { RuleId::of("GEN-1") }
                            },
                            self.span(id),
                            "function value actual lacks its required structural bound",
                        );
                        return Ok(self.error());
                    }
                }
            }
        }
        for parameter in &mut function.parameters {
            parameter.ty = self.substitute(parameter.ty, &arguments)?;
        }
        function.result = self.substitute(function.result, &arguments)?;
        function.error = result_error(
            self.types,
            function.result,
            &self.input.signature.metadata.roles,
        );
        function.generics = vec![].into();
        if let Some((_, ResolvedName::Definition(callee))) = self.input_name(id) {
            self.body.instantiations.push(Instantiation {
                expression: id,
                callee: callee.clone(),
                arguments: arguments.clone(),
            });
            Ok(self.named_function(function))
        } else {
            self.function_value_arguments.insert(id, arguments);
            let mut row = self.types.get(source).unwrap().clone();
            row.kind = TypeKind::Function(function);
            Ok(self.types.intern(row))
        }
    }
    /// A borrowed callable parameter's Environment is its access view,
    /// not part of the concrete type argument shared with another occurrence.
    /// Keep the occurrence's receiving refinements and explicit placements.
    fn contract_type_argument(&mut self, actual: TypeId) -> TypeId {
        let mut row = self.types.get(actual).unwrap().clone();
        if !matches!(row.kind, TypeKind::Parameter(_)) {
            row.modes.relation = row
                .modes
                .relation
                .retain(|r| !matches!(r, Reach::Environment));
        }
        self.types.intern(row)
    }
    pub(super) fn labels(
        &mut self,
        actual: &[Option<String>],
        declared: &[Option<String>],
        rest: bool,
        span: Span,
    ) -> Vec<Option<usize>> {
        let mut used = BTreeSet::new();
        let mut result = vec![];
        let mut bad = !rest && actual.len() != declared.len();
        for (i, label) in actual.iter().enumerate() {
            let slot = match label {
                Some(label) => declared.iter().position(|l| l.as_ref() == Some(label)),
                None => declared.get(i).filter(|l| l.is_none()).map(|_| i),
            };
            let slot = slot.filter(|slot| used.insert(*slot));
            bad |= slot.is_none();
            result.push(slot);
        }
        if bad {
            let slots = |labels: &[Option<String>]| {
                labels
                    .iter()
                    .enumerate()
                    .map(|(i, label)| match label {
                        Some(label) => format!("`{label}: ...`"),
                        None => format!("positional argument {}", i + 1),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            self.report(
                const { RuleId::of("LANG-12") },
                span,
                &format!("arguments must provide each required label and positional slot exactly once; declared: ({}); supplied: ({})", slots(declared), slots(actual)),
            );
        }
        result
    }
    pub(super) fn start_call(
        &mut self,
        id: ast::ExprId,
        expected: Option<TypeId>,
        frame: usize,
        checked: usize,
    ) -> Result<(), QueryError> {
        let ast::ExprKind::Call { callee, arguments } = self.input.syntax.tree.expressions
            [id.0 as usize]
            .kind
            .clone()
        else {
            unreachable!()
        };
        let callee_ty = self.value(callee);
        let resolved = self.input_name(callee).map(|(_, r)| r.clone());
        let formatter = self.formatters.get(&callee).cloned();
        let mut target = match (resolved, &formatter) {
            (_, Some((p, _))) => CallTarget::Definition(p.clone()),
            (Some(ResolvedName::Definition(p)), _) => CallTarget::Definition(p),
            _ => CallTarget::Value(callee),
        };
        let mut bad = self.invalid(callee_ty);
        // A transparent alias constructs the nominal it represents (LANG-7).
        let callee_ty = self.unalias(callee_ty)?;
        let f = match self.kind(callee_ty) {
            TypeKind::Function(f) => f,
            TypeKind::Named {
                definition,
                arguments: mut args,
            } => {
                // A contextual Result with a fresh error still needs constructor
                // inference: its payload may determine that error type.
                if self.partial_result(callee_ty).is_some() {
                    args.clear();
                }
                let s = self.snapshot(&definition)?;
                let SignatureKind::Type(td) = &s.kind else {
                    self.report(
                        const { RuleId::of("LANG-7") },
                        self.span(id),
                        "callee is not callable",
                    );
                    return self.recover_call(id, expected, frame, arguments);
                };
                let repr = if let Some(r) = td.representation {
                    let r = self.import(r)?;
                    if args.is_empty() {
                        r
                    } else {
                        self.substitute(r, &args)?
                    }
                } else {
                    self.error()
                };
                let ctx = self
                    .body
                    .resolution
                    .contextual
                    .iter()
                    .find(|(site, name)| {
                        *site == NameSite::Expression(callee)
                            && matches!(
                                name,
                                ContextualName::Constructor(_) | ContextualName::Variant { .. }
                            )
                    })
                    .map(|(_, c)| c.clone());
                let name = match &self.input.syntax.tree.expressions[callee.0 as usize].kind {
                    ast::ExprKind::Path(p) => {
                        p.segments.last().map(|s| self.name(*s)).unwrap_or_default()
                    }
                    _ => String::new(),
                };
                let (ctx, fields) = match self.kind(repr) {
                    TypeKind::Record(fs) => (
                        ContextualName::Constructor(definition.path().clone()),
                        fs.into_iter()
                            .map(|f| (if td.distinct { None } else { Some(f.name) }, f.ty))
                            .collect(),
                    ),
                    TypeKind::Variant(vs) => {
                        let selected = match ctx {
                            Some(ContextualName::Variant { variant, .. }) => {
                                Some(variant.0 as usize)
                            }
                            _ => vs.iter().position(|v| v.name.as_str() == name),
                        };
                        if let Some(i) = selected.filter(|i| *i < vs.len()) {
                            (
                                ContextualName::Variant {
                                    owner: definition.path().clone(),
                                    variant: VariantId(i as u32),
                                },
                                vs[i].fields.clone(),
                            )
                        } else {
                            self.report(
                                const { RuleId::of("LANG-7") },
                                self.span(id),
                                "constructor is not a variant of this type",
                            );
                            return self.recover_call(id, expected, frame, arguments);
                        }
                    }
                    _ => {
                        self.report(
                            const { RuleId::of("LANG-7") },
                            self.span(id),
                            "opaque type cannot be constructed here",
                        );
                        return self.recover_call(id, expected, frame, arguments);
                    }
                };
                target = CallTarget::Constructor(ctx);
                let generics = if args.is_empty() {
                    td.generics.clone()
                } else {
                    vec![]
                };
                let nominal_args = if args.is_empty() {
                    td.generics
                        .iter()
                        .enumerate()
                        .map(|(i, g)| match g {
                            GenericParameter::Type { .. } => {
                                let t = self.ty(TypeKind::Parameter(TypeParameterRef::bound(
                                    TypeParamId(i as u32),
                                )));
                                GenericArgument::Type(t)
                            }
                            GenericParameter::Size { .. } => GenericArgument::Size(
                                Size::Parameter(SizeParameterRef::bound(SizeParamId(i as u32))),
                            ),
                        })
                        .collect()
                } else {
                    args
                };
                let result = self.ty(TypeKind::Named {
                    definition,
                    arguments: nominal_args,
                });
                let result = match expected {
                    Some(expected) if self.compatible(result, expected) => {
                        self.construction_type(result, expected)
                    }
                    Some(expected)
                        if self
                            .packed_payload(expected)
                            .is_some_and(|payload| self.compatible(result, payload)) =>
                    {
                        expected
                    }
                    _ => result,
                };
                FunctionType {
                    state_promises: Default::default(),
                    generics: generics.into(),
                    parameters: fields
                        .into_iter()
                        .map(|(label, ty)| Parameter {
                            name: None,
                            label: label.map(|l: hirsel_intern::Symbol| l.as_str().to_owned()),
                            passing: ast::PassingMode::Take,
                            ty: self.placed_part_type(result, ty),
                        })
                        .collect(),
                    result,
                    error: None,
                    allocation: Allocation::MayAllocate,
                    authority: AuthoritySummary::Unknown,
                    uses: Uses::Unrestricted,
                    contracts: vec![].into(),
                    incomplete: false,
                    foreign: None,
                }
            }
            TypeKind::Error => return self.recover_call(id, expected, frame, arguments),
            _ => {
                self.report(
                    const { RuleId::of("LANG-7") },
                    self.span(id),
                    "callee is not a function",
                );
                return self.recover_call(id, expected, frame, arguments);
            }
        };
        if f.incomplete && !self.frames[frame].incomplete {
            self.report(
                const { RuleId::of("LANG-6") },
                self.span(id),
                "complete definition cannot call an incomplete signature",
            );
            bad = true;
        }
        if (f.foreign.is_some()) && !f.generics.is_empty() {
            self.report(
                const { RuleId::of("GEN-4") },
                self.span(id),
                "foreign calls must be monomorphic",
            );
            bad = true;
        }
        // Section 23 assigns missing capability arguments to the authority
        // checker. Retain actual types and a typed result for that phase rather
        // than emitting a derivative arity/type mismatch here.
        let mut capability_parameters = BTreeSet::new();
        for (i, parameter) in f.parameters.iter().enumerate() {
            if !matches!(target, CallTarget::Constructor(_))
                && let TypeKind::Named { definition, .. } = self.kind(parameter.ty)
            {
                let signature = self.snapshot(&definition)?;
                if matches!(&signature.kind, SignatureKind::Type(td) if td.capability.is_some()) {
                    capability_parameters.insert(i);
                }
            }
        }
        let missing_capabilities = arguments.len() < f.parameters.len()
            && arguments.len() >= f.parameters.len() - capability_parameters.len();
        let mut omitted = if missing_capabilities {
            f.parameters.len() - arguments.len()
        } else {
            0
        };
        let slots: Vec<_> = (0..f.parameters.len())
            .filter(|i| {
                let supplied_label = f.parameters[*i].label.as_ref().is_some_and(|label| {
                    arguments
                        .iter()
                        .any(|a| a.label.is_some_and(|symbol| self.name(symbol) == *label))
                });
                if omitted > 0 && capability_parameters.contains(i) && !supplied_label {
                    omitted -= 1;
                    false
                } else {
                    true
                }
            })
            .collect();
        let mut ordinals = self
            .labels(
                &arguments
                    .iter()
                    .map(|a| a.label.map(|s| self.name(s)))
                    .collect::<Vec<_>>(),
                &slots
                    .iter()
                    .map(|i| f.parameters[*i].label.clone())
                    .collect::<Vec<_>>(),
                missing_capabilities,
                self.span(id),
            )
            .into_iter()
            .map(|i| i.map(|i| slots[i]))
            .collect::<Vec<_>>();
        if let Some((_, declared)) = formatter {
            ordinals = declared.into_iter().map(Some).collect();
        }
        bad |= ordinals.iter().any(Option::is_none)
            || (!missing_capabilities && arguments.len() != f.parameters.len());
        let mut inferred = vec![None; f.generics.len()];
        if let Some(e) = expected {
            let constraint = self.crossing_inference_constraint(&f, f.result, e, false)?;
            self.infer(f.result, constraint, &mut inferred)?;
        }
        // An expected type fixes a parameter's modes only where it occurs as a
        // type contract; the modes of the expected value itself are its
        // receiver's requirement, which the result meets by weakening.
        let type_contracts = expected
            .map(|e| self.occurrences(f.result, e))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(p, _, contract)| contract.then_some(p.0 as usize))
            .collect();
        let call = self.calls.len();
        self.calls.push(PendingCall {
            id,
            expected,
            frame,
            target,
            function: f,
            arguments,
            ordinals,
            inferred,
            type_contracts,
            capability_parameters,
            missing_capabilities,
            bad,
            checked,
        });
        self.next_argument(call, 0)
    }
    fn recover_call(
        &mut self,
        id: ast::ExprId,
        expected: Option<TypeId>,
        frame: usize,
        arguments: Vec<ast::Argument>,
    ) -> Result<(), QueryError> {
        let e = self.error();
        let ast::ExprKind::Call { callee, .. } =
            self.input.syntax.tree.expressions[id.0 as usize].kind
        else {
            unreachable!()
        };
        self.body.calls.push(CheckedCall {
            expression: id,
            target: CallTarget::Value(callee),
            arguments: arguments
                .iter()
                .map(|a| CheckedArgument {
                    expression: a.value,
                    parameter: None,
                })
                .collect(),
            generic_arguments: vec![],
            result: e,
            state: CallState::Recovery,
        });
        self.put(id, e, expected, UseKind::Borrow)?;
        for arg in arguments.into_iter().rev() {
            self.work.push(Work::Enter(arg.value, Some(e), frame));
        }
        Ok(())
    }
    fn next_argument(&mut self, call: usize, index: usize) -> Result<(), QueryError> {
        if index == self.calls[call].arguments.len() {
            return self.finish_call(call);
        }
        if index < self.calls[call].checked {
            return self.call_argument(call, index);
        }
        let c = &self.calls[call];
        let arg = c.arguments[index].value;
        let frame = c.frame;
        let unmapped = c.ordinals[index].is_none();
        let template = c.ordinals[index]
            .filter(|i| !c.capability_parameters.contains(i))
            .and_then(|i| c.function.parameters.get(i))
            .map(|p| p.ty);
        let mut inferred = c.inferred.clone();
        let type_contracts = c.type_contracts.clone();
        let generics = c.function.generics.clone();
        // The argument is expected at the type argument the call will be
        // instantiated with: a parameter known so far only from a value keeps
        // none of that value's access modes.
        for (i, argument) in inferred.iter_mut().enumerate() {
            if let Some(GenericArgument::Type(actual)) = argument {
                if !type_contracts.contains(&i) {
                    let arenas = self.arena_places(*actual);
                    *actual = generic_type_argument(self.types, *actual, |o| arenas.contains(o))
                        .map_err(|e| {
                            QueryError::Operational(OperationalFailure {
                                operation: "generic type argument".into(),
                                message: format!("{e:?}"),
                            })
                        })?;
                } else {
                    *actual = self.contract_type_argument(*actual);
                }
            }
        }
        let mut expected = if unmapped {
            // Invalid labels cannot supply a contextual literal judgment.
            // Still walk the operand to check its independent prefix facts.
            Some(self.error())
        } else {
            template
                .map(|t| self.partial_substitute(t, &inferred, &generics))
                .transpose()?
                .flatten()
        };
        if let Some(slot) = self.creation_callback_slot(call, index) {
            expected = expected.map(|t| self.preserving_callback(t, slot));
        }
        // RUN-3: an entry parameter is expected as an entry even before its
        // type arguments are known, so a named definition forms an entry
        // whose callable type then supplies them. Unknown ones are holes.
        if expected.is_none()
            && let Some(template) = template
            && self.entry_signature(template).is_some()
        {
            let holes: Vec<_> = inferred
                .iter()
                .enumerate()
                .map(|(i, a)| match (a, generics.get(i)) {
                    (Some(a), _) => a.clone(),
                    (None, Some(GenericParameter::Size { .. })) => GenericArgument::Size(
                        Size::Parameter(SizeParameterRef::bound(SizeParamId(i as u32))),
                    ),
                    (None, _) => {
                        let variable = TypeParameterRef::body(
                            self.site(NameSite::Expression(arg)),
                            TypeParamId(i as u32),
                        );
                        self.inference_variables.insert(variable);
                        GenericArgument::Type(self.ty(TypeKind::Parameter(variable)))
                    }
                })
                .collect();
            expected = Some(self.substitute(template, &holes)?);
        }
        self.work
            .push(Work::AfterChild(After::CallArgument(call, index)));
        self.expected_sites.insert(
            arg,
            (
                self.span(self.calls[call].id),
                const { RuleId::of("LANG-7") },
            ),
        );
        self.work.push(Work::Enter(arg, expected, frame));
        Ok(())
    }
    /// Inside a creation owner, a closure literal written at the mutable
    /// callback slot of a call retaining that callback and its state promises
    /// to preserve the state it advances. Its body proves the promise;
    /// nothing else changes meaning.
    fn creation_callback_slot(&self, call: usize, index: usize) -> Option<usize> {
        let c = &self.calls[call];
        let creation = self.frames[c.frame]
            .result
            .and_then(|t| self.types.get(t))
            .is_some_and(|row| {
                matches!(row.erased_captures, Some(ErasedCaptures::FromArguments(_)))
            });
        if !creation
            || !matches!(
                self.input.syntax.tree.expressions[c.arguments[index].value.0 as usize].kind,
                ast::ExprKind::Closure { .. }
            )
        {
            return None;
        }
        let ordinal = c.ordinals[index]?;
        hirsel_types::mutable_capture_states(self.types, &c.function)
            .into_iter()
            .find(|(_, callback, _)| *callback == ordinal)
            .map(|(_, _, slot)| slot)
    }
    fn preserving_callback(&mut self, ty: TypeId, slot: usize) -> TypeId {
        let mut row = self.types.get(ty).unwrap().clone();
        let TypeKind::Function(f) = &mut row.kind else {
            return ty;
        };
        let selector = RelationPlace::from(ParameterRef::bound(ParamId(slot as u32)));
        if f.state_promises.iter().any(|p| p.selector == selector) {
            return ty;
        }
        let mut promises = f.state_promises.to_vec();
        promises.push(StatePromise {
            selector,
            advance_only: false,
            preserve_captures: true,
            preserve_unique: false,
            unretained: Default::default(),
        });
        promises.sort_by(|a, b| a.selector.cmp(&b.selector));
        f.state_promises = promises.into();
        self.types.intern(row)
    }
    pub(super) fn call_argument(&mut self, call: usize, index: usize) -> Result<(), QueryError> {
        let c = &self.calls[call];
        let expression = c.arguments[index].value;
        let actual = self.value(expression);
        let template = c.ordinals[index]
            .filter(|i| !c.capability_parameters.contains(i))
            .and_then(|i| c.function.parameters.get(i))
            .map(|p| p.ty);
        let id = c.id;
        // Spec #43: a borrowed local result shares its generic parts.
        let bounds = local_result_generics(self.types, &c.function);
        let mut inferred = c.inferred.clone();
        let mut bad = self.invalid(actual);
        if !bad {
            if let Some(template) = template {
                let readonly = c.ordinals[index]
                    .and_then(|i| c.function.parameters.get(i))
                    .is_some_and(|p| p.passing != ast::PassingMode::Mut);
                let function = c.function.clone();
                let constraint =
                    self.crossing_inference_constraint(&function, template, actual, readonly)?;
                // A value occurrence supplied shape, not a fixed holder-mode
                // contract. The first actual header/part contract can refine it.
                for (parameter, actual, contract) in self.occurrences(template, constraint) {
                    let ordinal = parameter.0 as usize;
                    if contract && !self.calls[call].type_contracts.contains(&ordinal)
                        && inferred.get(ordinal).is_some_and(|previous| matches!(previous, Some(GenericArgument::Type(previous)) if self.compatible(*previous, actual))) {
                        inferred[ordinal] = Some(GenericArgument::Type(actual));
                    }
                }
                if !self.infer(template, constraint, &mut inferred)? {
                    let expected = self
                        .partial_substitute(template, &inferred, &function.generics)?
                        .unwrap_or(template);
                    self.type_mismatch(
                        const { RuleId::of("LANG-7") },
                        self.span(id),
                        "argument does not match the declared parameter type",
                        actual,
                        expected,
                    );
                    bad = true;
                }
                for (parameter, actual, type_contract) in self.occurrences(template, actual) {
                    if type_contract {
                        self.calls[call].type_contracts.insert(parameter.0 as usize);
                    }
                    if let Some(GenericParameter::Type { name, bounds }) =
                        bounds.get(parameter.0 as usize)
                    {
                        for b in bounds {
                            if matches!(
                                b,
                                ast::Bound::Sendable | ast::Bound::Inert | ast::Bound::Serializable
                            ) {
                                continue;
                            }
                            if !self.bound(actual, *b)? {
                                let bound = match b {
                                    ast::Bound::Plain => "plain",
                                    ast::Bound::Flat => "flat",
                                    ast::Bound::Data => "data",
                                    ast::Bound::Key => "key",
                                    ast::Bound::Sendable => "sendable",
                                    ast::Bound::Inert => "inert",
                                    ast::Bound::Copyable => "copyable",
                                    ast::Bound::NoallocDrop => "noalloc_drop",
                                    ast::Bound::Serializable => "serializable",
                                };
                                let provided = self.type_spelling(actual);
                                self.report(
                                    if *b == ast::Bound::Serializable {
                                        const { RuleId::of("SER-1") }
                                    } else {
                                        const { RuleId::of("GEN-1") }
                                    },
                                    self.span(id),
                                    &format!("type `{provided}` for generic parameter `{name}` lacks the required `{bound}` bound"),
                                );
                                let operand = self.span(expression);
                                let diagnostic = self.diagnostics.last_mut().unwrap();
                                diagnostic.secondary.push(hirsel_diag::Label {
                                    span: operand,
                                    message: format!(
                                        "this argument supplies `{provided}` for `{name}: {bound}`"
                                    ),
                                });
                                diagnostic.notes.push(format!("help: use an argument satisfying `{bound}`; if `{provided}` is a type parameter, add `{bound}` to its declared bounds"));
                                bad = true;
                            }
                        }
                    }
                }
            }
        }
        self.calls[call].inferred = inferred;
        self.calls[call].bad |= bad;
        self.next_argument(call, index + 1)
    }
    /// Each occurrence of one of the callee's own type parameters in
    /// `template`, with the row `actual` supplies for it. An occurrence inside
    /// a type argument or a function parameter is a type contract and keeps
    /// its written modes; one at the root, in a tuple or box part or in a
    /// function result is a value, whose access modes describe its holder.
    fn occurrences(&self, template: TypeId, actual: TypeId) -> Vec<(TypeParamId, TypeId, bool)> {
        let mut found = vec![];
        let mut pending = vec![(template, actual, false, 0)];
        while let Some((template, actual, type_contract, depth)) = pending.pop() {
            match (self.kind(template), self.kind(actual)) {
                (TypeKind::Parameter(p), _) if p.binder() == GenericBinder::Bound(depth) => {
                    found.push((p.parameter(), actual, type_contract))
                }
                (TypeKind::Function(a), TypeKind::Function(b)) if a.generics == b.generics => {
                    pending.push((a.result, b.result, false, depth + 1));
                    pending.extend(
                        a.parameters
                            .into_iter()
                            .zip(b.parameters)
                            .map(|(a, b)| (a.ty, b.ty, true, depth + 1)),
                    );
                }
                (TypeKind::Tuple(a), TypeKind::Tuple(b)) => pending.extend(
                    a.into_iter()
                        .zip(b)
                        .map(|(a, b)| (a, b, type_contract, depth)),
                ),
                (TypeKind::Box(a), TypeKind::Box(b)) => pending.push((a, b, type_contract, depth)),
                (
                    TypeKind::Builtin { arguments: a, .. },
                    TypeKind::Builtin { arguments: b, .. },
                )
                | (TypeKind::Named { arguments: a, .. }, TypeKind::Named { arguments: b, .. }) => {
                    pending.extend(a.into_iter().zip(b).filter_map(|(a, b)| match (a, b) {
                        (GenericArgument::Type(a), GenericArgument::Type(b)) => {
                            Some((a, b, true, depth))
                        }
                        _ => None,
                    }))
                }
                _ => (),
            }
        }
        found
    }
    fn crossing_inference_constraint(
        &mut self,
        function: &FunctionType,
        template: TypeId,
        actual: TypeId,
        readonly: bool,
    ) -> Result<TypeId, QueryError> {
        let sharing = sharing_type_parameters(self.types, function);
        let declarations = self.body.resolution.generic_declarations.clone();
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Contract {
            Value,
            Fixed,
            BorrowedParameter,
        }
        fn walk(
            builder: &mut Checker<'_, '_>,
            formal: TypeId,
            actual: TypeId,
            depth: u32,
            variance: Variance,
            contract: Contract,
            sharing: &BTreeSet<TypeParamId>,
            declarations: &GenericDeclarations,
        ) -> Result<TypeId, QueryError> {
            let expected = builder.types.row(formal).clone();
            let mut row = builder.types.row(actual).clone();
            if let TypeKind::Parameter(parameter) = expected.kind {
                if parameter.binder() == GenericBinder::Bound(depth) {
                    if contract == Contract::Value {
                        let arenas = builder.arena_places(actual);
                        return generic_type_argument(builder.types, actual, |o| {
                            arenas.contains(o)
                        })
                        .map_err(|error| {
                            QueryError::Operational(OperationalFailure {
                                operation: "generic type argument".into(),
                                message: format!("{error:?}"),
                            })
                        });
                    }
                    // A callback's borrow of an abstract T is an access view,
                    // unlike T's declared relation in a nominal type argument.
                    let actual = if contract == Contract::BorrowedParameter {
                        row.modes.relation = row
                            .modes
                            .relation
                            .retain(|r| !matches!(r, Reach::Environment));
                        builder.types.intern(row.clone())
                    } else {
                        actual
                    };
                    if expected.sendability_requirement == SendabilityRequirement::Sendable {
                        row.sendability_requirement = SendabilityRequirement::None;
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
                        let actual = builder.types.intern(row);
                        return Ok(actual);
                    }
                    if sharing.contains(&parameter.parameter()) && variance == Variance::Covariant {
                        let store = builder.types;
                        return forget_read_crossing_requirements(
                            store,
                            actual,
                            declarations,
                            &mut |path| builder.snapshot(path).map(Some),
                        );
                    }
                    return Ok(actual);
                }
                return Ok(actual);
            }
            let edges = expected.kind.graph_edges();
            let actual_edges = row.kind.graph_edges();
            let mut replacement = Vec::new();
            for (index, ((a, b), child)) in edges
                .into_iter()
                .zip(actual_edges)
                .zip(variance.edges(&expected.kind))
                .enumerate()
            {
                let contract = match a.role {
                    TypeEdgeRole::FunctionParameter
                        if matches!(&expected.kind, TypeKind::Function(f)
                            if f.parameters[index].passing != ast::PassingMode::Take) =>
                    {
                        Contract::BorrowedParameter
                    }
                    TypeEdgeRole::TypeArgument | TypeEdgeRole::FunctionParameter => Contract::Fixed,
                    TypeEdgeRole::FunctionResult | TypeEdgeRole::FunctionError => Contract::Value,
                    TypeEdgeRole::Component => contract,
                };
                replacement.push(walk(
                    builder,
                    a.ty,
                    b.ty,
                    depth + a.role.parameter_binders(),
                    child,
                    contract,
                    sharing,
                    declarations,
                )?);
            }
            let mut replacements = replacement.into_iter();
            match &mut row.kind {
                TypeKind::Tuple(parts) => {
                    for part in parts.iter_mut() {
                        if let Some(new) = replacements.next() {
                            *part = new;
                        }
                    }
                }
                TypeKind::Box(part) => {
                    if let Some(new) = replacements.next() {
                        *part = new;
                    }
                }
                TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                    for argument in arguments.iter_mut() {
                        if let GenericArgument::Type(part) = argument {
                            if let Some(new) = replacements.next() {
                                *part = new;
                            }
                        }
                    }
                }
                TypeKind::Function(f) => {
                    for parameter in f.parameters.iter_mut() {
                        if let Some(new) = replacements.next() {
                            parameter.ty = new;
                        }
                    }
                    if let Some(new) = replacements.next() {
                        f.result = new;
                    }
                    if let Some(error) = &mut f.error {
                        if let Some(new) = replacements.next() {
                            *error = new;
                        }
                    }
                }
                _ => (),
            }
            Ok(builder.types.intern(row))
        }
        walk(
            self,
            template,
            actual,
            0,
            if readonly {
                Variance::Covariant
            } else {
                Variance::Exact
            },
            Contract::Value,
            &sharing,
            &declarations,
        )
    }
    fn finish_call(&mut self, call: usize) -> Result<(), QueryError> {
        let c = &self.calls[call];
        let id = c.id;
        let expected = c.expected;
        let target = c.target.clone();
        let result = c.function.result;
        let inferred = c.inferred.clone();
        let type_contracts = c.type_contracts.clone();
        let mut bad = c.bad;
        let source_arguments = c.arguments.clone();
        let ordinals = c.ordinals.clone();
        let parameters = c.function.parameters.clone();
        let missing_capabilities = c.missing_capabilities;
        let generics = c.function.generics.clone();
        if !bad
            && inferred.iter().any(|argument| match argument {
                None => true,
                Some(GenericArgument::Type(ty)) => self.partial_result(*ty).is_some(),
                Some(GenericArgument::Size(_)) => false,
            })
        {
            self.report(
                const { RuleId::of("LANG-2") },
                self.span(id),
                "generic call arguments cannot be inferred from its prefix and expected type",
            );
            bad = true;
        }
        // Result-directed inference can bind a parameter absent from every
        // operand (codec.decode). Prove SER-1 on the complete actuals before
        // generic value normalization can erase a source brand.
        if !bad {
            for (parameter, actual) in generics.iter().zip(&inferred) {
                if let (GenericParameter::Type { bounds, .. }, Some(GenericArgument::Type(actual))) =
                    (parameter, actual)
                    && bounds.contains(&ast::Bound::Serializable)
                    && !self.bound(*actual, ast::Bound::Serializable)?
                {
                    self.report(const { RuleId::of("SER-1") }, self.span(id),
                        "actual generic argument needs a transparent serializable schema without brands or authority");
                    bad = true;
                }
            }
        }
        let args: Vec<_> = inferred
            .into_iter()
            .enumerate()
            .filter_map(|(i, a)| a.map(|a| (i, a)))
            .map(|(i, argument)| match argument {
                GenericArgument::Type(actual) if !type_contracts.contains(&i) => {
                    let arenas = self.arena_places(actual);
                    generic_type_argument(self.types, actual, |o| arenas.contains(o))
                        .map(GenericArgument::Type)
                        .map_err(|e| {
                            QueryError::Operational(OperationalFailure {
                                operation: "generic type argument".into(),
                                message: format!("{e:?}"),
                            })
                        })
                }
                GenericArgument::Type(actual) => {
                    Ok(GenericArgument::Type(self.contract_type_argument(actual)))
                }
                argument => Ok(argument),
            })
            .collect::<Result<_, _>>()?;
        if !bad {
            bad = !self.intrinsic_call(id, &target, &args)?;
            bad |= !self.forget_call(id, &target, &args)?;
        }
        let ty = if bad {
            self.error()
        } else if args.is_empty() {
            result
        } else {
            self.substitute(result, &args)?
        };
        if !bad {
            if let CallTarget::Definition(callee) = &target {
                if !args.is_empty() {
                    self.body.instantiations.push(Instantiation {
                        expression: id,
                        callee: callee.clone(),
                        arguments: args.clone(),
                    });
                }
            }
        }
        if !bad {
            if let CallTarget::Constructor(ContextualName::Constructor(owner)) = &target {
                let declaration = self.snapshot(owner)?;
                if matches!(&declaration.kind, SignatureKind::Type(td) if td.fact.is_some()) {
                    let helper =
                        hirsel_check_protocol::fact::fact_signature(self.types, &declaration)?;
                    let SignatureKind::Function(f) = &helper.kind else {
                        unreachable!()
                    };
                    let result = self.import(f.result)?;
                    self.plans.facts.push(FactPlan {
                        expression: id,
                        predicate: helper.definition.path().clone(),
                        arguments: args.clone(),
                        result,
                        usage: UseKind::Borrow,
                    });
                    self.body.instantiations.push(Instantiation {
                        expression: id,
                        callee: helper.definition.path().clone(),
                        arguments: args.clone(),
                    });
                }
            }
        }
        // `map.empty()`, `set.empty()` and `sorted_map.empty(by)` are the keyed constructors:
        // their result is placed by its expected relation (PROV-4).
        let placed = !bad
            && matches!(&target, CallTarget::Definition(path)
                if hirsel_types::keyed_constructor(path).is_some());
        let mut arguments = vec![];
        for (argument, ordinal) in source_arguments.iter().zip(ordinals) {
            let parameter = if let Some(ordinal) = ordinal {
                let mut parameter = parameters[ordinal].clone();
                if !bad && !args.is_empty() {
                    parameter.ty = self.substitute(parameter.ty, &args)?;
                }
                Some((ordinal as u32, parameter))
            } else {
                None
            };
            arguments.push(CheckedArgument {
                expression: argument.value,
                parameter,
            });
        }
        self.body.calls.push(CheckedCall {
            expression: id,
            target,
            arguments,
            generic_arguments: args.clone(),
            result,
            state: if bad {
                CallState::Recovery
            } else if missing_capabilities {
                CallState::MissingCapabilities
            } else {
                CallState::Complete
            },
        });
        let ty = match expected {
            Some(expected) if placed => self.construction_type(ty, expected),
            _ => ty,
        };
        self.put(id, ty, expected, UseKind::Borrow)?;
        self.body
            .expressions
            .last_mut()
            .unwrap()
            .instantiated_arguments = args;
        Ok(())
    }
    fn partial_substitute(
        &mut self,
        root: TypeId,
        args: &[Option<GenericArgument>],
        generics: &[GenericParameter],
    ) -> Result<Option<TypeId>, QueryError> {
        if args.is_empty() {
            return Ok(Some(root));
        }
        let complete: Vec<_> = args
            .iter()
            .enumerate()
            .map(|(i, a)| {
                a.clone().unwrap_or_else(|| {
                    if matches!(generics.get(i), Some(GenericParameter::Size { .. })) {
                        GenericArgument::Size(Size::Parameter(SizeParameterRef::bound(
                            SizeParamId(i as u32),
                        )))
                    } else {
                        let t = self.ty(TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(
                            i as u32,
                        ))));
                        GenericArgument::Type(t)
                    }
                })
            })
            .collect();
        let t = self.substitute(root, &complete)?;
        let mut todo = vec![(t, 0)];
        while let Some((t, depth)) = todo.pop() {
            match self.kind(t) {
                TypeKind::Parameter(p)
                    if p.binder() == GenericBinder::Bound(depth)
                        && args
                            .get(p.parameter().0 as usize)
                            .is_some_and(Option::is_none) =>
                {
                    return Ok(None);
                }
                TypeKind::Tuple(ts) => todo.extend(ts.into_iter().map(|t| (t, depth))),
                TypeKind::Box(t) => todo.push((t, depth)),
                TypeKind::Function(f) => {
                    todo.extend(f.parameters.into_iter().map(|p| (p.ty, depth + 1)));
                    todo.push((f.result, depth + 1));
                    todo.extend(f.error.map(|t| (t, depth + 1)));
                }
                TypeKind::Builtin { arguments, .. } | TypeKind::Named { arguments, .. } => {
                    for argument in arguments {
                        match argument {
                            GenericArgument::Type(t) => todo.push((t, depth)),
                            GenericArgument::Size(Size::Parameter(p))
                                if p.binder() == GenericBinder::Bound(depth)
                                    && args
                                        .get(p.parameter().0 as usize)
                                        .is_some_and(Option::is_none) =>
                            {
                                return Ok(None);
                            }
                            _ => (),
                        }
                    }
                }
                _ => (),
            }
        }
        Ok(Some(t))
    }
    /// Inference changes only the call's fresh variables, never a declaration.
    pub(super) fn infer(
        &mut self,
        template: TypeId,
        actual: TypeId,
        args: &mut [Option<GenericArgument>],
    ) -> Result<bool, QueryError> {
        let mut todo = vec![(template, actual, 0)];
        let mut seen = std::collections::HashSet::new();
        while let Some((a, b, depth)) = todo.pop() {
            if !seen.insert((a, b, depth))
                || self.invalid(a)
                || self.invalid(b)
                || self.bottom(b)
                || self.inference_variable(a)
                || self.inference_variable(b)
            {
                continue;
            }
            match (self.kind(a), self.kind(b)) {
                (TypeKind::Parameter(p), _)
                    if p.binder() == GenericBinder::Bound(depth)
                        && (p.parameter().0 as usize) < args.len() =>
                {
                    let slot = &mut args[p.parameter().0 as usize];
                    if let Some(GenericArgument::Type(t)) = slot {
                        if !self.compatible(*t, b) {
                            return Ok(false);
                        }
                        *t = self.resolve_result_context(*t, b)?;
                    } else if slot.is_some() {
                        return Ok(false);
                    } else {
                        *slot = Some(GenericArgument::Type(b));
                    }
                }
                (TypeKind::Tuple(a), TypeKind::Tuple(b)) if a.len() == b.len() => {
                    todo.extend(a.into_iter().zip(b).map(|(a, b)| (a, b, depth)))
                }
                (TypeKind::Box(a), TypeKind::Box(b)) => todo.push((a, b, depth)),
                (
                    TypeKind::Named {
                        definition: a,
                        arguments: aa,
                    },
                    TypeKind::Named {
                        definition: b,
                        arguments: bb,
                    },
                ) if a == b => {
                    if !infer_arguments(aa.to_vec(), bb.to_vec(), args, depth, &mut todo) {
                        return Ok(false);
                    }
                }
                (
                    TypeKind::Builtin {
                        kind: a,
                        arguments: aa,
                    },
                    TypeKind::Builtin {
                        kind: b,
                        arguments: bb,
                    },
                ) if a == b => {
                    if !infer_arguments(aa.to_vec(), bb.to_vec(), args, depth, &mut todo) {
                        return Ok(false);
                    }
                }
                (TypeKind::Function(a), TypeKind::Function(b))
                    if a.parameters.len() == b.parameters.len() =>
                {
                    if a.generics != b.generics {
                        return Ok(false);
                    } else {
                        // Passing modes are part of a function type's shape.
                        if a.parameters
                            .iter()
                            .zip(&b.parameters)
                            .any(|(a, b)| a.passing != b.passing)
                        {
                            return Ok(false);
                        }
                        todo.push((a.result, b.result, depth + 1));
                        todo.extend(
                            a.parameters
                                .into_iter()
                                .zip(b.parameters)
                                .map(|(a, b)| (a.ty, b.ty, depth + 1)),
                        );
                    }
                }
                _ => {
                    if !self.compatible(a, b) {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }
    pub(super) fn result_parts(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<(ast::DefPath, TypeId, TypeId)>, QueryError> {
        let TypeKind::Named {
            definition,
            arguments,
        } = self.kind(ty)
        else {
            return Ok(None);
        };
        let s = self.snapshot(&definition)?;
        if s.metadata.roles.result_type.as_ref() != Some(definition.path()) {
            return Ok(None);
        }
        if let [GenericArgument::Type(ok), GenericArgument::Type(error)] = arguments.as_slice() {
            Ok(Some((definition.path().clone(), *ok, *error)))
        } else {
            Ok(None)
        }
    }
    fn result_tags(&mut self, ty: TypeId) -> Result<(VariantId, VariantId), QueryError> {
        let (_, repr) = self.representation(ty)?;
        if let TypeKind::Variant(vs) = self.kind(repr) {
            let success = vs
                .iter()
                .position(|v| v.name.as_str() == "Ok" && v.fields.len() == 1);
            let error = vs
                .iter()
                .position(|v| v.name.as_str() == "Error" && v.fields.len() == 1);
            if let (Some(ok), Some(error)) = (success, error) {
                return Ok((VariantId(ok as u32), VariantId(error as u32)));
            }
        }
        Err(QueryError::Operational(OperationalFailure {
            operation: "Result propagation plan".into(),
            message: "registered Result lacks its declared Ok/Error unary variants".into(),
        }))
    }
    pub(super) fn try_type(
        &mut self,
        id: ast::ExprId,
        value: ast::ExprId,
        map: Option<&ast::Path>,
        frame: usize,
    ) -> Result<TypeId, QueryError> {
        let ty = self.value(value);
        if self.invalid(ty) {
            return Ok(ty);
        }
        let Some((result, ok, error)) = self.result_parts(ty)? else {
            self.report(
                const { RuleId::of("LANG-8") },
                self.span(id),
                "? requires the registered Result type",
            );
            return Ok(self.error());
        };
        let Some(expected) = self.frames[frame].error else {
            self.report(
                if self.frames[frame].finalizer {
                    const { RuleId::of("DROP-3") }
                } else {
                    const { RuleId::of("LANG-8") }
                },
                self.span(id),
                "? requires an enclosing Result error channel",
            );
            return Ok(self.error());
        };
        let mut mapper_passing = None;
        let mapper = if let Some(path) = map {
            let r = self
                .input
                .resolution
                .paths
                .iter()
                .find(|(s, _)| {
                    work::count(Counter::BodySourceRows);
                    *s == NameSite::Expression(id)
                })
                .map(|(_, r)| r.clone());
            let candidate = match r {
                Some(ResolvedName::Definition(p)) => Some((self.snapshot(&p)?, p)),
                _ => None,
            };
            // A type is never a mapper: a name that resolved to one, such as a
            // core type a user variant shadows, is the expected error's variant
            // (LANG-4, LANG-8).
            let candidate = candidate.filter(|(s, _)| !matches!(s.kind, SignatureKind::Type(_)));
            if let Some((s, p)) = candidate {
                let SignatureKind::Function(f) = &s.kind else {
                    self.report(
                        const { RuleId::of("LANG-8") },
                        self.span(id),
                        "error mapper must be a unary function or variant",
                    );
                    return Ok(self.error());
                };
                let f = self.import_function(f)?;
                let mut args = vec![None; f.generics.len()];
                // The propagated error is a temporary, never a `mut` place (OWN-3).
                let matches = f.parameters.len() == 1
                    && f.parameters[0].passing != ast::PassingMode::Mut
                    && self.infer(f.parameters[0].ty, error, &mut args)?
                    && self.infer(f.result, expected, &mut args)?
                    && args.iter().all(Option::is_some);
                if !matches {
                    self.report(
                        const { RuleId::of("LANG-8") },
                        self.span(id),
                        "error mapper must map the source error to the enclosing error",
                    );
                    return Ok(self.error());
                }
                mapper_passing = Some(f.parameters[0].passing);
                for (parameter, argument) in f.generics.iter().zip(&args) {
                    if let (
                        GenericParameter::Type { bounds, .. },
                        Some(GenericArgument::Type(actual)),
                    ) = (parameter, argument)
                    {
                        for bound in bounds {
                            if !matches!(bound, ast::Bound::Sendable | ast::Bound::Inert)
                                && !self.bound(*actual, *bound)?
                            {
                                self.report(
                                    if *bound == ast::Bound::Serializable {
                                        const { RuleId::of("SER-1") }
                                    } else {
                                        const { RuleId::of("GEN-1") }
                                    },
                                    self.span(id),
                                    "error mapper actual lacks its required structural bound",
                                );
                                return Ok(self.error());
                            }
                        }
                    }
                }
                if !args.is_empty() {
                    self.body.instantiations.push(Instantiation {
                        expression: id,
                        callee: p.clone(),
                        arguments: args.into_iter().flatten().collect(),
                    });
                }
                Some(CallTarget::Definition(p))
            } else {
                let name = path
                    .segments
                    .last()
                    .map(|s| self.name(*s))
                    .unwrap_or_default();
                let (owner, repr) = self.representation(expected)?;
                let variant = if let TypeKind::Variant(vs) = self.kind(repr) {
                    vs.iter()
                        .enumerate()
                        .find(|(_, v)| {
                            v.name.as_str() == name
                                && v.fields.len() == 1
                                && self.compatible(v.fields[0].1, error)
                        })
                        .map(|(i, _)| i)
                } else {
                    None
                };
                if let (Some(owner), Some(i)) = (owner, variant) {
                    Some(CallTarget::Constructor(ContextualName::Variant {
                        owner,
                        variant: VariantId(i as u32),
                    }))
                } else {
                    self.report(
                        const { RuleId::of("LANG-8") },
                        self.span(id),
                        "variant mapper needs exactly one matching error payload",
                    );
                    return Ok(self.error());
                }
            }
        } else if self.compatible(error, expected) {
            // Identity comes first, so `type E = Wrap(E) | Leaf` propagates `E`.
            None
        } else {
            // The selected arm is exactly the constructor `? Arm` names.
            match self.inject(id, value, error, expected)? {
                Some(target) => Some(target),
                None => return Ok(self.error()),
            }
        };
        let (success_variant, error_variant) = self.result_tags(ty)?;
        let (_, propagated_error_variant) = self.result_tags(self.frames[frame].result.unwrap())?;
        self.plans.tries.push(TryPlan {
            expression: id,
            result,
            mapper,
            mapper_passing,
            success_variant,
            error_variant,
            propagated_error_variant,
            error_type: expected,
            propagated_result: self.frames[frame].result.unwrap(),
        });
        Ok(ok)
    }
}
fn infer_arguments(
    a: Vec<GenericArgument>,
    b: Vec<GenericArgument>,
    args: &mut [Option<GenericArgument>],
    depth: u32,
    todo: &mut Vec<(TypeId, TypeId, u32)>,
) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (a, b) in a.into_iter().zip(b) {
        match (a, b) {
            (GenericArgument::Type(a), GenericArgument::Type(b)) => todo.push((a, b, depth)),
            (GenericArgument::Size(Size::Parameter(p)), b @ GenericArgument::Size(_))
                if p.binder() == GenericBinder::Bound(depth)
                    && (p.parameter().0 as usize) < args.len() =>
            {
                let slot = &mut args[p.parameter().0 as usize];
                if slot.as_ref().is_some_and(|a| *a != b) {
                    return false;
                }
                *slot = Some(b);
            }
            (a, b) if a == b => (),
            _ => return false,
        }
    }
    true
}
