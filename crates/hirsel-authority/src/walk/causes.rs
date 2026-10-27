//! Causes of authority-pass judgments: brands, placements, scoped values and
//! headers. Both sides come from the types and syntax in hand when a judgment
//! fails; nothing is recorded for an accepted body.
use super::*;
use hirsel_modes::explain::{
    Cause, Explanation, Fact, Qualifier, Repair, Requirement, Sites, placement_repairs,
};

impl Checker<'_> {
    /// Explain a reached nested List contract failure from the rows already
    /// compared by the authority pass. Other projections and invariant
    /// weakening keep their existing diagnostic. No inference is repeated.
    pub(super) fn nested_element_diagnostic(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        span: Span,
    ) -> bool {
        let mut pair = (actual, expected);
        let mut contract = None;
        for _ in 0..16 {
            let (Some(a), Some(b)) = (self.t.get(pair.0), self.t.get(pair.1)) else {
                return false;
            };
            let (
                TypeKind::Builtin {
                    kind: BuiltinType::List,
                    arguments: aa,
                },
                TypeKind::Builtin {
                    kind: BuiltinType::List,
                    arguments: ba,
                },
            ) = (&a.kind, &b.kind)
            else {
                return false;
            };
            let (Some(GenericArgument::Type(a)), Some(GenericArgument::Type(b))) =
                (aa.first(), ba.first())
            else {
                return false;
            };
            pair = (*a, *b);
            if self
                .t
                .get(pair.0)
                .is_some_and(|row| row.modes.uniqueness != Uniqueness::Unique)
                && self
                    .t
                    .get(pair.1)
                    .is_some_and(|row| row.modes.uniqueness == Uniqueness::Unique)
            {
                contract = Some(pair.1);
                break;
            }
        }
        let Some(contract) = contract else {
            return false;
        };
        let required = self.diagnostic_type(expected);
        let element = self.diagnostic_type(contract);
        let value = self.received_span(span);
        let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
        let copied = self.nested_contract_copy();
        // The argument block preserves evaluation order. Restrict edits to
        // fresh literal trees; a named or copied supplier may actually share.
        let repair = self
            .received_value()
            .filter(|e| self.span(*e) == span)
            .and_then(|e| self.fresh_string_sequence(e, expected, &mut 16))
            .filter(|literal| literal.len() <= 256)
            .and_then(|literal| {
                self.element_binding_name().map(|name| {
                    // Only builtin List/String trees reach this branch.
                    // A user declaration named List must not capture the edit.
                    let annotation = required.replace("List[", "core@1/List[");
                    format!("{{ let {name}: {annotation} = {literal}; take {name} }}")
                })
            });
        let before = self.out.diagnostics.len();
        let rule = const { RuleId::of("MODE-2") };
        let message = if copied.is_some() {
            format!(
                "`unique` copies the value but cannot add the nested `unique` element contract `{element}` required by `{required}`"
            )
        } else {
            format!(
                "the supplier does not prove the nested element contract `{element}` required by `{required}`"
            )
        };
        self.error(rule.clone(), span, &message);
        if self.out.diagnostics.len() > before {
            let d = self.out.diagnostics.last_mut().unwrap();
            if d.rule != rule {
                return true;
            }
            d.secondary.push(Label {
                span: value,
                message: "supplier: this input lacks proof of the required nested uniqueness; the receiving type cannot create that proof".into(),
            });
            d.secondary.push(Label {
                span: receiver,
                message: format!("required: nested element contract `{element}` in `{required}`"),
            });
            if let Some((copy, annotation)) = copied {
                d.secondary.push(Label {
                    span: copy,
                    message: "this unique copy preserves the source's fixed nested element type"
                        .into(),
                });
                if let Some(annotation) = annotation {
                    d.secondary.push(Label {
                        span: annotation,
                        message:
                            "binding annotation: this does not change the copy's fixed nested element type"
                                .into(),
                    });
                }
                d.notes.push(format!("help: Construct the list element-by-element under the complete type, for example `let mut owned: {required} = []; for element in source {{ list.push(mut owned, unique element) }}`, or use a fresh literal under that type. A whole-value `unique source` copy changes ownership, not the fixed element contract (GEN-1, LANG-2, MODE-2)."));
            } else if let Some(replacement) = repair {
                d.notes.push(format!("help: Give this fresh literal a complete type before passing it to the consumer: `{replacement}`. The element annotation is necessary; an annotation on the outer List alone is insufficient."));
                d.fix = Some(hirsel_diag::Fix {
                    span,
                    replacement,
                    message: "bind the fresh literal with its complete nested element contract"
                        .into(),
                });
            } else {
                d.notes.push("help: Construct fresh elements under the complete receiving type, or use a permitted deep unique copy before storing an element. Annotating an already shared value cannot establish uniqueness.".into());
            }
        }
        true
    }

    /// Explain only an actual unique-copy producer, directly or through an
    /// immutable binding's still-retained receipt. A write or nonpreserving
    /// hand-back must not turn stale declaration syntax into a current cause.
    fn nested_contract_copy(&self) -> Option<(Span, Option<Span>)> {
        let value = self.transparent_value(self.received_value()?);
        if matches!(
            self.tree.expressions[value.0 as usize].kind,
            ExprKind::Unary {
                op: UnaryOp::Unique,
                ..
            }
        ) {
            return Some((self.span(value), None));
        }
        let place = self.source_place(value);
        if !place.projections.is_empty() || place.access == SourceAccess::Copy {
            return None;
        }
        let SourceRoot::Binding { owner, binding } = &place.root else {
            return None;
        };
        let binding = self.root_binding(*owner, binding)?;
        let NameSite::Pattern(pattern) = binding.site else {
            return None;
        };
        let (_, _, initializer) = self.derivations.get(&binding_key(binding))?;
        let initializer = self.transparent_value(*initializer);
        if !matches!(
            self.tree.expressions[initializer.0 as usize].kind,
            ExprKind::Unary {
                op: UnaryOp::Unique,
                ..
            }
        ) {
            return None;
        }
        let annotation = self
            .tree
            .statements
            .iter()
            .find_map(|stmt| match stmt.kind {
                StmtKind::Let {
                    pattern: p,
                    ty: Some(ty),
                    ..
                } if p == pattern => Some(self.tree.types[ty.0 as usize].origin.span),
                _ => None,
            });
        Some((self.span(initializer), annotation))
    }

    /// Spell only small, fresh, heap String/List trees. Their explicit
    /// receiving contract can check without strengthening existing storage.
    fn fresh_string_sequence(
        &self,
        expression: ExprId,
        ty: TypeId,
        budget: &mut usize,
    ) -> Option<String> {
        *budget = budget.checked_sub(1)?;
        let row = self.t.get(ty)?;
        if !row.modes.relation.is_global() {
            return None;
        }
        match (
            &self.tree.expressions[expression.0 as usize].kind,
            &row.kind,
        ) {
            (
                ExprKind::Literal(hirsel_syntax::Literal::String(text)),
                TypeKind::Primitive(Primitive::String),
            ) if text.len() <= 256 => Some(text.clone()),
            (
                ExprKind::Sequence(items),
                TypeKind::Builtin {
                    kind: BuiltinType::List,
                    arguments,
                },
            ) if !items.is_empty() => {
                let GenericArgument::Type(element) = arguments.first()? else {
                    return None;
                };
                let parts = items
                    .iter()
                    .map(|item| self.fresh_string_sequence(*item, *element, budget))
                    .collect::<Option<Vec<_>>>()?;
                Some(format!("[{}]", parts.join(", ")))
            }
            _ => None,
        }
    }

    /// Avoid written identifiers and module declarations/imports. Retain
    /// lookup misses too, so adding a declaration invalidates a cached edit.
    fn element_binding_name(&mut self) -> Option<String> {
        let used: BTreeSet<_> = self
            .tree
            .tokens
            .iter()
            .filter_map(|token| match token.kind {
                hirsel_syntax::TokenKind::Ident(name) => Some(name.as_str().to_owned()),
                _ => None,
            })
            .collect();
        for suffix in 1..=64 {
            let name = if suffix == 1 {
                "saved".to_owned()
            } else {
                format!("saved_{suffix}")
            };
            if used.contains(&name) {
                continue;
            }
            let owner = self.signature.definition.path();
            let context = hirsel_syntax::FileContext {
                package: owner.0.package.clone(),
                modules: owner.0.modules.clone(),
            };
            let path = hirsel_syntax::Path {
                package: Some(owner.0.package.clone()),
                segments: owner
                    .0
                    .modules
                    .iter()
                    .chain(std::iter::once(&name))
                    .map(|s| Symbol::intern(s))
                    .collect(),
            };
            let output = self.reader.borrow_mut().provider().lookup(&context, &path);
            let available = matches!(output.result, Err(QueryError::NotImplemented));
            self.out.reads.push(
                DependencyRead::ProviderLookup {
                    context: (context).into(),
                    package: path.package,
                    segments: (path
                        .segments
                        .iter()
                        .map(|s| s.as_str().to_owned())
                        .collect::<Vec<_>>())
                    .into(),
                    output: (output).into(),
                }
                .shared(),
            );
            if available {
                return Some(name);
            }
        }
        None
    }

    /// Follow only parts that the existing MODE-4 proof rejects. Like rustc's
    /// BuiltinDerived causes (a7c37f9, traits/suggestions.rs:4441-4499), expose
    /// the containing field/variant chain without changing the judgment.
    fn sendability_cause(&mut self, root: TypeId) -> Option<String> {
        let mut pending = vec![(root, String::new(), false)];
        let mut seen = HashSet::default();
        while let Some((id, mut path, inherited_unique)) = pending.pop() {
            if !seen.insert((id, inherited_unique)) || seen.len() > 256 {
                continue; // A diagnostic may elide a huge or expanding type.
            }
            let mut row = self.t.get(id)?.clone();
            let unique = inherited_unique || row.modes.uniqueness == Uniqueness::Unique;
            if unique {
                row.modes.uniqueness = Uniqueness::Unique;
            }
            let qualified = self.t.intern(row.clone());
            if self.sendable(qualified) {
                continue;
            }
            // Handles fail on the carrier itself, even with scalar parts;
            // counted roots fail before their parts unless unique.
            if !row.modes.relation.is_global()
                || matches!(
                    row.kind,
                    TypeKind::Builtin {
                        kind: BuiltinType::Receiver | BuiltinType::Task,
                        ..
                    }
                )
                || (!unique && matches!(row.kind, TypeKind::Box(_) | TypeKind::Builtin { .. }))
            {
                let ty = self.diagnostic_type(id);
                return Some(if path.is_empty() {
                    ty
                } else {
                    format!("{path}({ty})")
                });
            }
            if let TypeKind::Named { definition, .. } = &row.kind {
                let signature = self.read(*definition)?;
                if signature.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque {
                    return Some(format!("{path}{}", definition.path().0.name));
                }
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(&definition.path().0.name);
            }
            let kind = if matches!(row.kind, TypeKind::Named { .. }) {
                self.layout(id)?
            } else {
                row.kind.clone()
            };
            let mut children = vec![];
            match kind {
                TypeKind::Record(fields) => {
                    for field in fields.iter() {
                        children.push((
                            field.ty,
                            format!("{path}.{}", field.name.as_str()),
                            unique,
                        ));
                    }
                }
                TypeKind::Variant(variants) => {
                    for variant in variants.iter() {
                        for (index, (name, ty)) in variant.fields.iter().enumerate() {
                            let suffix = name.map_or_else(
                                || format!("[{index}]"),
                                |n| format!(".{}", n.as_str()),
                            );
                            let leaf = self.t.get(*ty)?;
                            let next = if matches!(leaf.kind, TypeKind::Primitive(_)) {
                                format!(
                                    "{path}.{}({})",
                                    variant.name.as_str(),
                                    self.diagnostic_type(*ty)
                                )
                            } else {
                                format!("{path}.{}{suffix}", variant.name.as_str())
                            };
                            children.push((*ty, next, unique));
                        }
                    }
                }
                TypeKind::Tuple(parts) => {
                    for (index, ty) in parts.iter().enumerate() {
                        children.push((*ty, format!("{path}[{index}]"), unique));
                    }
                }
                TypeKind::Builtin { arguments, .. } => {
                    for (index, argument) in arguments.iter().enumerate() {
                        if let GenericArgument::Type(ty) = argument {
                            children.push((*ty, format!("{path}[{index}]"), unique));
                        }
                    }
                }
                TypeKind::Box(ty) => children.push((ty, format!("{path}.box"), unique)),
                _ => (),
            }
            if children.is_empty() {
                return Some(if path.is_empty() {
                    self.diagnostic_type(id)
                } else {
                    path
                });
            }
            pending.extend(children.into_iter().rev());
        }
        None
    }
    fn diagnostic_type(&self, root: TypeId) -> String {
        hirsel_type_analysis::spelling::type_spelling(
            self.t,
            root,
            &self.out.body.resolution.generic_declarations,
            &self.signature.definition.path().0.package,
            true,
        )
    }
    /// A source result annotation, only for the callback literal actually
    /// passed here. Never turn a declaration elsewhere into a local edit.
    pub(super) fn callback_sendability_explanation(
        &mut self,
        call: ExprId,
        result: TypeId,
        provided: Span,
        required: Span,
    ) -> Explanation {
        let cause = self.sendability_cause(result);
        // Both producers inspect the complete result of this actual callback.
        // Use the call's declaration-ordinal mapping, including labelled calls,
        // and a definition-local source identity rather than a type spelling.
        let callback = self.call_row(call).and_then(|row| {
            row.arguments
                .iter()
                .filter_map(|a| a.parameter.as_ref().map(|(i, _)| (*i, a.expression)))
                .max_by_key(|(i, _)| *i)
                .map(|(_, expression)| expression)
        });
        let site = self.span(call);
        let mut explanation = Explanation {
            root: callback.map(|_| {
                hirsel_modes::explain::root_key(
                    self.signature.definition.path(),
                    ("callback-result", site.start, site.end),
                )
            }),
            ..Explanation::default()
        }
        .provided(Fact::new(
            Cause::NotSendable,
            callback.map_or(provided, |e| self.span(e)),
            &cause.as_deref().into_iter().collect::<Vec<_>>(),
        ))
        .required(Fact::new(Requirement::Sendable, required, &[]));
        let Some(callback) = callback else {
            return explanation;
        };
        let ExprKind::Closure { signature, body } =
            &self.tree.expressions[callback.0 as usize].kind
        else {
            return explanation;
        };
        let body = *body;
        let contracts = !signature.contracts.is_empty();
        let Some(annotation) = signature.result else {
            return explanation;
        };
        let site = self.tree.types[annotation.0 as usize].origin.span;
        let Some(function) = self.ty(callback).and_then(|t| match &self.t.get(t)?.kind {
            TypeKind::Function(f) => Some(f.clone()),
            _ => None,
        }) else {
            return explanation;
        };
        let declared = function.result;
        let spelling = self.diagnostic_type(declared);
        explanation = explanation.required(Fact::new(
            Requirement::SendableType,
            site,
            &[&hirsel_modes::explain::prose(&format!(
                "the declared callback result `{spelling}`"
            ))],
        ));
        // An annotation alone cannot repair an ordinary-returning call.
        // A single simple tail has no other return paths to repair. Prove the
        // copy and crossing obligations, and respect the allocation promise,
        // before offering a deep unique result expression.
        let ExprKind::Block {
            label: None,
            statements,
            tail: Some(tail),
        } = &self.tree.expressions[body.0 as usize].kind
        else {
            return explanation;
        };
        let tail = *tail;
        if !statements.is_empty()
            || contracts
            || function.allocation == Allocation::NoAlloc
            || !matches!(
                self.tree.expressions[tail.0 as usize].kind,
                ExprKind::Call { .. } | ExprKind::Path(_)
            )
        {
            return explanation;
        }
        if let Some(expression) = self.spelled(tail)
            && self.property(declared, Bound::Copyable)
            && self.sendable_when_unique(declared)
        {
            explanation.repairs.push(Repair::UniqueResult {
                ty: spelling,
                expression,
            });
            let span = self.span(tail);
            explanation.fix = Some(hirsel_diag::Fix {
                span: Span {
                    end: span.start,
                    ..span
                },
                replacement: "unique ".into(),
                message: "return a deep unique copy of the callback result".into(),
            });
        }
        explanation
    }
    pub(super) fn sites(&self) -> Sites<'_> {
        Sites {
            syntax: self.syntax,
        }
    }
    pub(super) fn spelled(&self, e: ExprId) -> Option<String> {
        hirsel_modes::explain::spell(self.tree, e)
    }
    /// A function type's source spelling for a quoted type or a suggested
    /// annotation: its header with scalar and tuple parts. None where a part
    /// has no short, certain spelling (a nominal type, a relation, generics).
    pub(super) fn function_spelling(&self, ty: TypeId, qualified: bool) -> Option<String> {
        fn part(types: &TypeInterner, ty: TypeId, depth: usize) -> Option<String> {
            let row = types.get(ty)?;
            Some(match &row.kind {
                TypeKind::Primitive(p) => match p {
                    Primitive::Bool => "bool",
                    Primitive::U8 => "u8",
                    Primitive::U16 => "u16",
                    Primitive::U32 => "u32",
                    Primitive::U64 => "u64",
                    Primitive::U128 => "u128",
                    Primitive::I8 => "i8",
                    Primitive::I16 => "i16",
                    Primitive::I32 => "i32",
                    Primitive::I64 => "i64",
                    Primitive::I128 => "i128",
                    Primitive::F32 => "f32",
                    Primitive::F64 => "f64",
                    Primitive::Char => "char",
                    _ => return None,
                }
                .into(),
                TypeKind::Tuple(parts) if depth < 3 => format!(
                    "({})",
                    parts
                        .iter()
                        .map(|p| part(types, *p, depth + 1))
                        .collect::<Option<Vec<_>>>()?
                        .join(", ")
                ),
                _ => return None,
            })
        }
        let types = self.t;
        let row = types.get(ty)?;
        let TypeKind::Function(f) = &row.kind else {
            return None;
        };
        if !f.generics.is_empty() || f.error.is_some() || !f.contracts.is_empty() {
            return None;
        }
        let uses = match &f.uses {
            Uses::Unrestricted => "",
            Uses::Only(AuthoritySummary::Known(terms)) if terms.is_empty() => " uses {}",
            Uses::Only(_) => return None,
        };
        let parameters = f
            .parameters
            .iter()
            .map(|p| {
                let mode = match p.passing {
                    PassingMode::Take => "take ",
                    PassingMode::Mut => "mut ",
                    _ => "",
                };
                let name = p.name.as_ref().map_or(String::new(), |n| format!("{n}: "));
                Some(format!("{mode}{name}{}", part(types, p.ty, 0)?))
            })
            .collect::<Option<Vec<_>>>()?
            .join(", ");
        Some(format!(
            "{}{}{}fn({parameters}) -> {}{uses}",
            if qualified && row.modes.uniqueness == Uniqueness::Unique {
                "unique "
            } else {
                ""
            },
            if qualified && row.modes.multiplicity == Multiplicity::Once {
                "once "
            } else {
                ""
            },
            if f.allocation == Allocation::NoAlloc {
                "@noalloc "
            } else {
                ""
            },
            part(types, f.result, 0)?
        ))
    }
    /// The value's spelling, or a description when it has none.
    pub(super) fn value_name(&self, e: Option<ExprId>) -> String {
        e.and_then(|e| self.spelled(e))
            .map(|n| {
                n.trim_start_matches("take ")
                    .trim_start_matches("copy ")
                    .to_owned()
            })
            .unwrap_or_else(|| hirsel_modes::explain::prose("the value"))
    }
    /// Where a value's type was declared: its binding's site, else the value.
    /// An element is declared by its sequence's binding.
    pub(super) fn value_site(&self, e: ExprId) -> Span {
        let mut root = e;
        while let ExprKind::Unary {
            op: UnaryOp::Take | UnaryOp::Copy | UnaryOp::Mut,
            value: base,
        }
        | ExprKind::Index { base, .. } = self.tree.expressions[root.0 as usize].kind
        {
            root = base;
        }
        match self.binding(root) {
            Some(binding)
                if matches!(
                    self.tree.expressions[root.0 as usize].kind,
                    ExprKind::Path(_)
                ) =>
            {
                self.sites().binding(&binding).1
            }
            _ => self.span(e),
        }
    }
    /// An origin's name and declaring site.
    pub(super) fn origin_site(&self, origin: &Origin) -> (String, Option<Span>) {
        let sites = self.sites();
        let parameter = |r: &ParameterRef| match r {
            ParameterRef::Bound {
                depth: 0,
                parameter,
            } => sites.parameter(NameSite::Item, *parameter),
            ParameterRef::Body { scope, parameter } if scope.owner == self.lexical => {
                sites.parameter(scope.site, *parameter)
            }
            _ => None,
        };
        let (name, span) = match &origin.root {
            OriginRoot::Parameter(r) | OriginRoot::ArgumentBrand(r) => match parameter(r) {
                Some((name, span)) => (name, Some(span)),
                None => (hirsel_modes::explain::prose("a parameter"), None),
            },
            OriginRoot::Local(_, local) => match self
                .out
                .body
                .bindings
                .iter()
                .find(|b| b.binding == ResolvedName::Local(*local))
            {
                Some(binding) => {
                    let (name, span) = sites.binding(binding);
                    (name, Some(span))
                }
                None => (hirsel_modes::explain::prose("a local"), None),
            },
            OriginRoot::Expression(_, e) => {
                // Fresh value identities are rooted at their initializer, even
                // after a let names the value. Recover that declared owner for
                // diagnostics instead of calling its literal/call a temporary.
                let binding = self.tree.statements.iter().find_map(|s| match s.kind {
                    StmtKind::Let { pattern, value, .. } if value == *e => self
                        .out
                        .body
                        .bindings
                        .iter()
                        .find(|b| b.site == NameSite::Pattern(pattern)),
                    _ => None,
                });
                match binding {
                    Some(binding) => {
                        let (name, site) = sites.binding(binding);
                        (name, Some(site))
                    }
                    None => (
                        self.spelled(*e)
                            .unwrap_or_else(|| hirsel_modes::explain::prose("a temporary")),
                        Some(self.span(*e)),
                    ),
                }
            }
            OriginRoot::Definition(path) => (path.0.name.clone(), None),
            OriginRoot::Sibling(_) => (hirsel_modes::explain::prose("a sibling field"), None),
        };
        let mut name = name;
        for field in origin.fields.iter() {
            name.push_str(&format!(".{}", field.0));
        }
        (name, span)
    }
    /// Report through `error` (which may restate a clause failure as FACT-1)
    /// and attach the explanation to the diagnostic it emitted.
    pub(super) fn explained(
        &mut self,
        rule: RuleId,
        span: Span,
        message: &str,
        mut explanation: Explanation,
    ) {
        explanation.name_values(self.sites());
        let before = self.out.diagnostics.len();
        self.error(rule.clone(), span, message);
        if self.out.diagnostics.len() > before {
            let diagnostic = self.out.diagnostics.last_mut().unwrap();
            if diagnostic.rule == rule {
                explanation.render(diagnostic);
            }
        }
    }
    /// The source of a boundary payload, using H's declaration-ordinal mapping
    /// rather than the source argument position (labelled calls may reorder it).
    pub(super) fn payload_explanation(
        &self,
        call: ExprId,
        ordinal: u32,
        cause: Cause,
        required: Requirement,
    ) -> Explanation {
        let value = self
            .out
            .body
            .calls
            .iter()
            .find(|c| c.expression == call)
            .and_then(|c| {
                c.arguments
                    .iter()
                    .find(|a| a.parameter.as_ref().is_some_and(|(i, _)| *i == ordinal))
            })
            .map(|a| a.expression);
        self.boundary_explanation(value, self.span(call), cause, required)
    }
    pub(super) fn boundary_explanation(
        &self,
        value: Option<ExprId>,
        required_site: Span,
        cause: Cause,
        required: Requirement,
    ) -> Explanation {
        let provided_site = value.map_or(required_site, |e| self.value_site(e));
        let mut explanation = Explanation::default();
        if let Some(value) = value {
            let mut root = value;
            while let ExprKind::Unary { value, .. } = self.tree.expressions[root.0 as usize].kind {
                root = value;
            }
            if let Some(binding) = self.binding(root) {
                let (name, site) = self.sites().binding(&binding);
                let kind = match binding.usage {
                    UseKind::Borrow => Some(Cause::BorrowedParameter),
                    UseKind::Mut => Some(Cause::MutParameter),
                    UseKind::Take => Some(Cause::TakenParameter),
                    _ => None,
                };
                if let Some(kind) =
                    kind.filter(|_| matches!(binding.binding, ResolvedName::Parameter(_)))
                {
                    explanation = explanation.provided(Fact::new(kind, site, &[&name]));
                }
            }
        }
        explanation
            .provided(Fact::new(cause, provided_site, &[]))
            .required(Fact::new(required, required_site, &[]))
    }
    fn receiving_fact(
        &self,
        kind: Requirement,
        names: [&str; 2],
        fallback: Span,
    ) -> Fact<Requirement> {
        let (description, span) = match &self.receiving {
            Some(r) => (r.description.clone(), r.span),
            None => ("this use".into(), fallback),
        };
        Fact::new(kind, span, &[names[0], names[1], &description])
    }
    /// The judged value's site, or `fallback`.
    pub(super) fn received_span(&self, fallback: Span) -> Span {
        self.received_value().map_or(fallback, |e| self.span(e))
    }
    fn received_value(&self) -> Option<ExprId> {
        self.receiving.as_ref().and_then(|r| r.value)
    }
    /// The expression whose placement a value carries: through block tails
    /// and into the constructor part placed in `arena` (#79), else the value.
    fn placement_source(&self, mut e: ExprId, arena: &Origin) -> ExprId {
        loop {
            if let ExprKind::Block {
                label: None,
                tail: Some(tail),
                ..
            } = self.tree.expressions[e.0 as usize].kind
            {
                e = tail;
                continue;
            }
            let part = self
                .call_row(e)
                .filter(|call| matches!(call.target, CallTarget::Constructor(_)))
                .and_then(|call| {
                    call.arguments.iter().map(|a| a.expression).find(|part| {
                        self.ty(*part).is_some_and(|t| {
                            let relation = &self.t.get(t).unwrap().modes.relation;
                            self.named_arena(relation).as_ref() == Some(arena)
                        })
                    })
                });
            match part {
                Some(part) => e = part,
                None => return e,
            }
        }
    }
    /// PROV-1/5: the value is branded by another owner.
    pub(super) fn brand_explanation(
        &self,
        actual: &Origin,
        expected: &Origin,
        span: Span,
    ) -> Explanation {
        let value = self.received_value();
        let x = self.value_name(value);
        let (owner, _) = self.origin_site(actual);
        let (wanted, _) = self.origin_site(expected);
        let at = value.map_or(span, |e| self.value_site(e));
        let mut explanation = Explanation::default()
            .provided(Fact::new(
                Cause::Branded,
                at,
                &[
                    &x,
                    &owner,
                    &hirsel_modes::explain::qualifier(Qualifier::Brand(&owner)),
                ],
            ))
            .required(self.receiving_fact(
                Requirement::Brand,
                [
                    &wanted,
                    &hirsel_modes::explain::qualifier(Qualifier::Brand(&wanted)),
                ],
                span,
            ));
        if self.receiving.as_ref().is_some_and(|r| r.argument)
            && matches!(actual.root, OriginRoot::Parameter(_))
            && actual.fields.is_empty()
        {
            // Probe 4a: pass the scope that owns the value.
            explanation.repairs.push(Repair::UseOwner { owner });
        }
        explanation
    }
    /// PROV-2: a union or an erased brand.
    pub(super) fn unbranded_explanation(&self, expected: &Brand, span: Span) -> Explanation {
        let value = self.received_value();
        let x = self.value_name(value);
        let wanted = expected
            .alternatives
            .first()
            .map(|o| self.origin_site(o).0)
            .unwrap_or_default();
        let mut explanation = Explanation::default()
            .provided(Fact::new(
                Cause::Unbranded,
                value.map_or(span, |e| self.span(e)),
                &[&x],
            ))
            .required(self.receiving_fact(
                Requirement::Brand,
                [
                    &wanted,
                    &hirsel_modes::explain::qualifier(Qualifier::Brand(&wanted)),
                ],
                span,
            ));
        if let Some(name) = self.receiving.as_ref().and_then(|r| r.annotated.clone()) {
            // Probe 5a: the receiver need not be branded.
            explanation.repairs.push(Repair::DropBrand(name));
        }
        explanation
    }
    /// The first arena a relation names (PROV-4).
    pub(super) fn named_arena(&self, relation: &Relation) -> Option<Origin> {
        relation.reaches().iter().find_map(|r| match r {
            Reach::Place(o) if self.arena_place(o) => Some(o.clone()),
            _ => None,
        })
    }
    /// A relation place spelled as written: an arena, a binding or a field
    /// path of one.
    fn relation_place_name(&mut self, origin: &Origin) -> String {
        let root = Origin {
            root: origin.root.clone(),
            fields: vec![].into(),
        };
        let (mut name, _) = self.origin_site(&root);
        let mut ty = self.origin_binding(&root).map(|b| b.ty);
        for field in origin.fields.iter() {
            let declared = ty.and_then(|ty| self.fields(ty));
            match declared.as_ref().and_then(|f| f.get(field.0 as usize)) {
                Some(f) => {
                    name.push('.');
                    name.push_str(f.name.as_str());
                    ty = Some(f.ty);
                }
                None => {
                    name.push_str(&format!(".{}", field.0));
                    ty = None;
                }
            }
        }
        name
    }
    /// PROV-4 for a let relation naming bindings (v22 R3): the value holds an
    /// arena outside what the relation's places hold. Returns the headline
    /// and the two-sided explanation.
    pub(super) fn within_explanation(
        &mut self,
        held: &Relation,
        allowed: &Relation,
        relation: &Relation,
        span: Span,
    ) -> (String, Explanation) {
        let escaping = held.retain(|r| !allowed.reaches().contains(r));
        let mut explanation = self.placement_explanation(&escaping, relation, span);
        let places: Vec<String> = relation
            .reaches()
            .iter()
            .filter_map(|r| match r {
                Reach::Place(o) => Some(o.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .iter()
            .map(|o| self.relation_place_name(o))
            .collect();
        let names: Vec<&str> = places.iter().map(String::as_str).collect();
        let spelled = match names.as_slice() {
            [one] => (*one).to_owned(),
            _ => format!("{{{}}}", names.join(", ")),
        };
        explanation.required = Some(self.receiving_fact(
            Requirement::Within,
            [
                &spelled,
                &hirsel_modes::explain::qualifier(Qualifier::In(&names)),
            ],
            span,
        ));
        (format!("the value is not within `{spelled}`"), explanation)
    }
    /// PROV-4: the value lives in another arena, or in none.
    pub(super) fn placement_explanation(
        &self,
        actual: &Relation,
        expected: &Relation,
        span: Span,
    ) -> Explanation {
        let (actual, expected_arena) = (self.named_arena(actual), self.named_arena(expected));
        let value = self.received_value().map(|e| {
            actual
                .as_ref()
                .map_or(e, |arena| self.placement_source(e, arena))
        });
        let x = self.value_name(value);
        let at = value.map_or(span, |e| self.value_site(e));
        let provided = match &actual {
            Some(origin) => {
                let (arena, _) = self.origin_site(origin);
                Fact::new(
                    Cause::Placed,
                    at,
                    &[
                        &x,
                        &arena,
                        &hirsel_modes::explain::qualifier(Qualifier::In(&[&arena])),
                    ],
                )
            }
            None => Fact::new(Cause::NoPlacement, at, &[&x]),
        };
        let required = match &expected_arena {
            Some(origin) => {
                let (arena, _) = self.origin_site(origin);
                self.receiving_fact(
                    Requirement::Placement,
                    [
                        &arena,
                        &hirsel_modes::explain::qualifier(Qualifier::In(&[&arena])),
                    ],
                    span,
                )
            }
            // `in heap` is the empty relation.
            None if expected.is_global() => self.receiving_fact(
                Requirement::Heap,
                ["", &hirsel_modes::explain::qualifier(Qualifier::Heap)],
                span,
            ),
            None => self.receiving_fact(Requirement::NoPlacement, ["", ""], span),
        };
        let mut explanation = Explanation::default().provided(provided).required(required);
        // Probes 6a, 6b, 6g: a parameter placed by its declared type moves to
        // the required arena or the heap, or the result follows it.
        let parameter = value.and_then(|e| {
            let mut root = e;
            while let ExprKind::Unary { value, .. } = self.tree.expressions[root.0 as usize].kind {
                root = value;
            }
            self.binding(root)
                .filter(|b| matches!(b.binding, ResolvedName::Parameter(_)))
        });
        if let (Some(_), Some(actual), Some(expected)) = (parameter, &actual, &expected_arena) {
            let (from, _) = self.origin_site(actual);
            let (to, _) = self.origin_site(expected);
            let result_arena = self
                .frames
                .last()
                .and_then(|f| f.result)
                .and_then(|r| self.t.get(r))
                .is_some_and(|r| self.named_arena(&r.modes.relation).as_ref() == Some(expected));
            let result = self.receiving.as_ref().is_some_and(|r| r.result) || result_arena;
            let (repairs, cautions) = placement_repairs(&x, &from, &to, result);
            explanation.repairs = repairs;
            explanation.cautions = cautions;
        }
        explanation
    }
}
