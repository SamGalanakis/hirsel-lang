use super::*;
impl Checker<'_, '_> {
    pub(super) fn mutable_map_pattern(&mut self, root: ast::PatternId) {
        let mut pending = vec![(root, false, true)];
        while let Some((pattern, key, whole)) = pending.pop() {
            let Some(row) = self.body.patterns.iter().find(|p| p.pattern == pattern) else {
                continue;
            };
            let fields = row.fields.clone();
            // An alternative stands where its or-pattern does.
            if let PatternDecisionKind::Or(alternatives) = &row.kind {
                pending.extend(alternatives.iter().map(|a| (*a, key, whole)));
            }
            let usage = if key || whole {
                if row.usage == UseKind::Copy {
                    UseKind::Copy
                } else {
                    UseKind::Borrow
                }
            } else {
                UseKind::Mut
            };
            let mutable = !key && !whole;
            if let Some(row) = self.body.patterns.iter_mut().find(|p| p.pattern == pattern) {
                row.usage = usage;
            }
            if let Some(binding) = self
                .body
                .bindings
                .iter_mut()
                .find(|b| b.site == NameSite::Pattern(pattern))
            {
                binding.usage = usage;
                binding.mutable = mutable;
            }
            pending.extend(
                fields
                    .into_iter()
                    .map(|(field, child)| (child, key || whole && field.0 == 0, false)),
            );
        }
    }
    pub(super) fn pattern(
        &mut self,
        root: ast::PatternId,
        ty: TypeId,
        usage: UseKind,
        mutable: bool,
        irrefutable: bool,
        annotation: Option<WrittenModes>,
        frame: usize,
    ) -> Result<(), QueryError> {
        let root_span = self.input.syntax.tree.patterns[root.0 as usize].origin.span;
        let mut todo = vec![(root, ty)];
        let mut refutable = false;
        let mut alternations = vec![];
        while let Some((id, ty)) = todo.pop() {
            let p = self.input.syntax.tree.patterns[id.0 as usize].clone();
            let inspected = self.box_payload(ty);
            let usage = if usage == UseKind::Borrow && self.bound(ty, ast::Bound::Plain)? {
                UseKind::Copy
            } else {
                usage
            };
            let mut fields = vec![];
            let mut children = vec![];
            let kind = if self.invalid(ty) {
                match &p.kind {
                    ast::PatternKind::Tuple(ps) | ast::PatternKind::Or(ps) => {
                        children.extend(ps.iter().map(|p| (*p, ty)))
                    }
                    ast::PatternKind::Constructor { fields: fs, .. } => {
                        children.extend(fs.iter().map(|f| (f.pattern, ty)))
                    }
                    _ => (),
                }
                PatternDecisionKind::Error
            } else {
                match &p.kind {
                    ast::PatternKind::Wildcard => PatternDecisionKind::Wildcard,
                    ast::PatternKind::Binding(name) => {
                        // Resolution may have lacked the instantiated payload
                        // type. Its provisional local must not hide a nullary
                        // variant selected by this checked expected type.
                        let (owner, repr) = self.representation(inspected)?;
                        let variant = if let TypeKind::Variant(vs) = self.kind(repr) {
                            vs.iter()
                                .enumerate()
                                .find(|(_, v)| v.name == *name && v.fields.is_empty())
                                .map(|(i, _)| (i, vs.len()))
                        } else {
                            None
                        };
                        if let (Some(owner), Some((i, n))) = (owner, variant) {
                            refutable |= n > 1;
                            PatternDecisionKind::Constructor(ContextualName::Variant {
                                owner,
                                variant: VariantId(i as u32),
                            })
                        } else if let Some((_, _, local)) = self
                            .input
                            .resolution
                            .bindings
                            .iter()
                            .find(|(p, s, _)| *p == id && s == name)
                        {
                            let local = *local;
                            self.frames[frame].locals.insert(local, ty);
                            self.body.bindings.push(BindingDecision {
                                owner: self.frames[frame].owner.clone(),
                                binding: ResolvedName::Local(local),
                                site: NameSite::Pattern(id),
                                ty,
                                usage,
                                mutable,
                            });
                            PatternDecisionKind::Binding(local)
                        } else {
                            self.report(
                                const { RuleId::of("LANG-13") },
                                p.origin.span,
                                "pattern name is not a binding or variant of this type",
                            );
                            PatternDecisionKind::Error
                        }
                    }
                    ast::PatternKind::Literal(l) => {
                        refutable = true;
                        let t = self.literal(l, Some(inspected), p.origin.span, false);
                        if !self.compatible(t, inspected) {
                            self.type_mismatch(
                                const { RuleId::of("LANG-7") },
                                p.origin.span,
                                "literal pattern has the wrong type",
                                t,
                                inspected,
                            );
                        }
                        self.plans.pattern_literals.push((id, l.clone()));
                        PatternDecisionKind::Literal
                    }
                    ast::PatternKind::Range { start, end } => {
                        refutable = true;
                        self.range_pattern(start, end, inspected, p.origin.span)
                    }
                    ast::PatternKind::Tuple(ps) => {
                        if let TypeKind::Tuple(ts) = self.kind(inspected) {
                            if ts.len() != ps.len() {
                                self.report(
                                    const { RuleId::of("LANG-13") },
                                    p.origin.span,
                                    "tuple pattern has the wrong arity",
                                );
                            }
                            for (i, child) in ps.iter().enumerate() {
                                let t = ts.get(i).copied().unwrap_or_else(|| self.error());
                                fields.push((FieldId(i as u32), *child));
                                children.push((*child, t));
                            }
                            PatternDecisionKind::Tuple
                        } else {
                            self.report(
                                const { RuleId::of("LANG-13") },
                                p.origin.span,
                                "tuple pattern requires a tuple",
                            );
                            for child in ps {
                                let e = self.error();
                                children.push((*child, e));
                            }
                            PatternDecisionKind::Error
                        }
                    }
                    ast::PatternKind::Constructor {
                        path,
                        fields: fs,
                        rest,
                    } => {
                        let name = path
                            .segments
                            .last()
                            .map(|s| self.name(*s))
                            .unwrap_or_default();
                        let (owner, repr) = self.representation(inspected)?;
                        if let Some(owner) = owner {
                            let s = self.snapshot(&owner)?;
                            let distinct = matches!(&s.kind,SignatureKind::Type(td) if td.distinct);
                            if let SignatureKind::Type(td) = &s.kind {
                                if td.drop.is_some()
                                    && (owner.0.package != self.input.context.package
                                        || owner.0.modules != self.input.context.modules)
                                {
                                    self.report(const { RuleId::of("LANG-13") },root_span,"a type with drop may be destructured only in its defining module");
                                }
                            }
                            let shape = match self.kind(repr) {
                                TypeKind::Record(rs) => Some((
                                    ContextualName::Constructor(owner.clone()),
                                    rs.into_iter()
                                        .map(|f| (if distinct { None } else { Some(f.name) }, f.ty))
                                        .collect::<Vec<_>>(),
                                    false,
                                )),
                                TypeKind::Variant(vs) => vs
                                    .iter()
                                    .enumerate()
                                    .find(|(_, v)| v.name.as_str() == name)
                                    .map(|(i, v)| {
                                        (
                                            ContextualName::Variant {
                                                owner: owner.clone(),
                                                variant: VariantId(i as u32),
                                            },
                                            v.fields.to_vec(),
                                            vs.len() > 1,
                                        )
                                    }),
                                _ => None,
                            };
                            if let Some((ctx, slots, refut)) = shape {
                                if matches!(&s.kind, SignatureKind::Type(td) if td.resource && td.drop.is_some())
                                {
                                    self.plans
                                        .resource_patterns
                                        .push((id, slots.iter().map(|(_, ty)| *ty).collect()));
                                }
                                refutable |= refut;
                                let labels: Vec<_> =
                                    fs.iter().map(|f| f.label.map(|s| self.name(s))).collect();
                                let ordinals = self.labels(
                                    &labels,
                                    &slots
                                        .iter()
                                        .map(|(l, _)| l.map(|l| l.as_str().to_owned()))
                                        .collect::<Vec<_>>(),
                                    *rest,
                                    p.origin.span,
                                );
                                for (i, f) in fs.iter().enumerate() {
                                    if let Some(ordinal) = ordinals.get(i).copied().flatten() {
                                        fields.push((FieldId(ordinal as u32), f.pattern));
                                        children.push((f.pattern, slots[ordinal].1));
                                    } else {
                                        let e = self.error();
                                        children.push((f.pattern, e));
                                    }
                                }
                                PatternDecisionKind::Constructor(ctx)
                            } else {
                                self.report(
                                    const { RuleId::of("LANG-13") },
                                    p.origin.span,
                                    "constructor pattern does not belong to the expected type",
                                );
                                for f in fs {
                                    let e = self.error();
                                    children.push((f.pattern, e));
                                }
                                PatternDecisionKind::Error
                            }
                        } else {
                            let shared = if let Some(part) = self.shared_payload(inspected) {
                                let (owner, repr) = self.representation(part)?;
                                let belongs = match self.kind(repr) {
                                    TypeKind::Record(_) => owner.is_some_and(|owner| {
                                        owner.0.name == name
                                            && self.input.resolution.contextual.iter().any(|(site, resolved)| {
                                                *site == NameSite::Pattern(id)
                                                    && matches!(resolved, ContextualName::Constructor(definition) if *definition == owner)
                                            })
                                    }),
                                    TypeKind::Variant(variants) => owner.is_some_and(|owner| {
                                        variants.iter().enumerate().any(|(index, variant)| {
                                            variant.name.as_str() == name
                                                && self.input.resolution.contextual.iter().any(|(site, resolved)| {
                                                    *site == NameSite::Pattern(id)
                                                        && matches!(resolved, ContextualName::Variant { owner: definition, variant }
                                                            if *definition == owner && variant.0 as usize == index)
                                                })
                                        })
                                    }),
                                    _ => false,
                                };
                                belongs.then_some(part)
                            } else {
                                None
                            };
                            self.shared_access_error(
                                const { RuleId::of("LANG-13") },
                                p.origin.span,
                                "constructor pattern needs a nominal type",
                                shared,
                            );
                            for f in fs {
                                let e = self.error();
                                children.push((f.pattern, e));
                            }
                            PatternDecisionKind::Error
                        }
                    }
                    ast::PatternKind::Or(alternatives) => {
                        // Every alternative inspects the same place as the whole.
                        children.extend(alternatives.iter().map(|a| (*a, ty)));
                        alternations.push(id);
                        PatternDecisionKind::Or(alternatives.clone())
                    }
                    ast::PatternKind::Error => PatternDecisionKind::Error,
                }
            };
            // Recovery binders still have a row, even when the RHS is invalid.
            if matches!(kind, PatternDecisionKind::Error) {
                if let ast::PatternKind::Binding(name) = p.kind {
                    if let Some((_, _, local)) = self
                        .input
                        .resolution
                        .bindings
                        .iter()
                        .find(|(p, s, _)| *p == id && *s == name)
                    {
                        let local = *local;
                        self.frames[frame].locals.insert(local, ty);
                        self.body.bindings.push(BindingDecision {
                            owner: self.frames[frame].owner.clone(),
                            binding: ResolvedName::Local(local),
                            site: NameSite::Pattern(id),
                            ty,
                            usage,
                            mutable,
                        });
                    }
                }
            }
            self.body.patterns.push(PatternDecision {
                pattern: id,
                ty,
                usage,
                kind,
                fields,
                annotation: annotation.filter(|_| id == root),
            });
            todo.extend(children.into_iter().rev());
        }
        for alternation in &alternations {
            self.alternatives_agree(*alternation);
        }
        if !alternations.is_empty() && self.input.syntax.tree.pattern_choices(root).is_none() {
            self.report(
                const { RuleId::of("LANG-13") },
                root_span,
                &format!(
                    "pattern matches in more than {} ways through its alternatives",
                    hirsel_syntax::MAX_PATTERN_CHOICES
                ),
            );
        } else if irrefutable && !self.invalid(ty) {
            // Alternatives may cover together what each leaves uncovered.
            let refutable = if alternations.is_empty() {
                refutable
            } else {
                self.useful(ty, vec![vec![Some(root)]], vec![None])?
            };
            if refutable {
                self.report(
                    const { RuleId::of("LANG-13") },
                    root_span,
                    "binding pattern must be irrefutable",
                );
            }
        }
        Ok(())
    }
    /// LANG-13: a let-else pattern can fail to match, and its else block
    /// diverges (return, break, continue or a call that never returns).
    pub(super) fn let_else(
        &mut self,
        pattern: ast::PatternId,
        ty: TypeId,
        otherwise: ast::ExprId,
    ) -> Result<(), QueryError> {
        if !self.invalid(ty) && !self.useful(ty, vec![vec![Some(pattern)]], vec![None])? {
            self.report(
                const { RuleId::of("LANG-13") },
                self.input.syntax.tree.patterns[pattern.0 as usize]
                    .origin
                    .span,
                "a let-else pattern matches every value; remove the else",
            );
        }
        let block = self.value(otherwise);
        if !self.invalid(block) && !self.bottom(block) {
            self.report(
                const { RuleId::of("LANG-13") },
                self.span(otherwise),
                "a let-else block must diverge: end it with return, break, continue \
                 or a call that never returns",
            );
        }
        Ok(())
    }
    /// LANG-13: an if-let or while-let pattern can fail to match.
    pub(super) fn let_condition(
        &mut self,
        condition: ast::LetCondition,
        ty: TypeId,
        pattern: ast::PatternId,
    ) -> Result<(), QueryError> {
        if !self.invalid(ty) && !self.useful(ty, vec![vec![Some(pattern)]], vec![None])? {
            self.report(
                const { RuleId::of("LANG-13") },
                self.input.syntax.tree.patterns[pattern.0 as usize]
                    .origin
                    .span,
                match condition {
                    ast::LetCondition::If { .. } => {
                        "an if-let pattern matches every value; bind it with let"
                    }
                    ast::LetCondition::While { .. } => {
                        "a while-let pattern matches every value; bind it with let in the loop"
                    }
                },
            );
        }
        Ok(())
    }
    /// LANG-13: in let and for, every way a pattern's alternatives match moves
    /// or copies its bindings into one slot per name, so no binding is a view:
    /// a view names one place, and the alternatives' places differ.
    pub(super) fn joined_bindings_own(&mut self, root: ast::PatternId) {
        if self
            .input
            .syntax
            .tree
            .pattern_choices(root)
            .is_none_or(|choices| choices.len() < 2)
        {
            return;
        }
        let mut todo = vec![root];
        while let Some(id) = todo.pop() {
            let Some(row) = self.body.patterns.iter().find(|p| p.pattern == id).cloned() else {
                continue;
            };
            if matches!(row.kind, PatternDecisionKind::Binding(_))
                && matches!(row.usage, UseKind::Borrow | UseKind::Mut)
                && !self.invalid(row.ty)
            {
                self.report(
                    const { RuleId::of("LANG-13") },
                    self.input.syntax.tree.patterns[id.0 as usize].origin.span,
                    "an or-pattern in let or for binds plain values or takes its payloads, \
                     but this binding borrows one of several places; take the value, or \
                     bind it with if let or match",
                );
            }
            // Alternatives agree, so the first one's bindings stand for all.
            match &row.kind {
                PatternDecisionKind::Or(alternatives) => todo.extend(alternatives.first()),
                _ => todo.extend(row.subpatterns()),
            }
        }
    }
    /// The bindings a pattern introduces through its first alternatives, with
    /// their decisions' types and mutability.
    fn pattern_bindings(&self, root: ast::PatternId) -> BTreeMap<LocalId, (TypeId, bool)> {
        let mut out = BTreeMap::new();
        let mut todo = vec![root];
        while let Some(id) = todo.pop() {
            let Some(row) = self.body.patterns.iter().find(|p| p.pattern == id) else {
                continue;
            };
            match &row.kind {
                PatternDecisionKind::Binding(local) => {
                    if let Some(b) = self
                        .body
                        .bindings
                        .iter()
                        .find(|b| b.site == NameSite::Pattern(id))
                    {
                        out.insert(*local, (b.ty, b.mutable));
                    }
                }
                PatternDecisionKind::Or(alternatives) => todo.extend(alternatives.first()),
                _ => todo.extend(row.fields.iter().map(|(_, child)| *child)),
            }
        }
        out
    }
    /// LANG-13: alternatives bind the same names with the same types and modes.
    fn alternatives_agree(&mut self, alternation: ast::PatternId) {
        let ast::PatternKind::Or(alternatives) = self.input.syntax.tree.patterns
            [alternation.0 as usize]
            .kind
            .clone()
        else {
            return;
        };
        let first = self.pattern_bindings(alternatives[0]);
        for alternative in &alternatives[1..] {
            let bound = self.pattern_bindings(*alternative);
            for (local, (ty, mutable)) in &bound {
                let Some((expected, expected_mutable)) = first.get(local) else {
                    continue;
                };
                let same = mutable == expected_mutable
                    && (self.invalid(*ty)
                        || self.invalid(*expected)
                        || self.types.semantic_eq(*ty, *expected));
                if !same {
                    let span = self.input.syntax.tree.patterns[alternative.0 as usize]
                        .origin
                        .span;
                    self.type_mismatch(
                        const { RuleId::of("LANG-13") },
                        span,
                        "every alternative binds each name with the same type",
                        *ty,
                        *expected,
                    );
                }
            }
        }
    }
    /// LANG-13: a closed range of integer or char literals, whose start does
    /// not exceed its end.
    fn range_pattern(
        &mut self,
        start: &ast::Literal,
        end: &ast::Literal,
        inspected: TypeId,
        span: Span,
    ) -> PatternDecisionKind {
        let primitive = match self.kind(inspected) {
            TypeKind::Primitive(p) if p.pattern_domain().is_some() => p,
            _ => {
                self.report(
                    const { RuleId::of("LANG-13") },
                    span,
                    "a range pattern needs an integer or char type",
                );
                return PatternDecisionKind::Error;
            }
        };
        let mut valid = true;
        for bound in [start, end] {
            let t = self.literal(bound, Some(inspected), span, false);
            if self.invalid(t) {
                valid = false;
            } else if !self.compatible(t, inspected) {
                self.type_mismatch(
                    const { RuleId::of("LANG-7") },
                    span,
                    "range pattern bound has the wrong type",
                    t,
                    inspected,
                );
                valid = false;
            }
        }
        if valid
            && let (Some(low), Some(high)) = (
                self.scalar_key(start, primitive),
                self.scalar_key(end, primitive),
            )
            && low > high
        {
            self.report(
                const { RuleId::of("LANG-13") },
                span,
                "range pattern is empty: its start exceeds its end",
            );
        }
        PatternDecisionKind::Range
    }
    fn scalar_key(&self, literal: &ast::Literal, primitive: Primitive) -> Option<u128> {
        let width = primitive.integer_width().map_or(32, |(width, _)| width);
        primitive.pattern_key(hirsel_syntax::scalar_bits(literal, width)?)
    }
    /// The key interval a checked literal or range pattern covers in a scalar column.
    fn scalar_interval(&self, cell: Cell, primitive: Primitive) -> Option<(u128, u128)> {
        let id = cell?;
        match &self.input.syntax.tree.patterns[id.0 as usize].kind {
            ast::PatternKind::Literal(l) => {
                let key = self.scalar_key(l, primitive)?;
                Some((key, key))
            }
            ast::PatternKind::Range { start, end } => Some((
                self.scalar_key(start, primitive)?,
                self.scalar_key(end, primitive)?,
            )),
            _ => None,
        }
    }
    /// Whether every value of `root` matches some unguarded arm (LANG-7).
    pub(super) fn exhaustive(
        &mut self,
        root: TypeId,
        arms: &[ast::MatchArm],
    ) -> Result<bool, QueryError> {
        let rows = arms
            .iter()
            .filter(|a| a.guard.is_none())
            .map(|a| vec![Some(a.pattern)])
            .collect();
        Ok(!self.useful(root, rows, vec![None])?)
    }
    /// Usefulness where every row and the query take every alternative.
    fn useful(
        &mut self,
        root: TypeId,
        rows: Vec<Vec<Cell>>,
        query: Vec<Cell>,
    ) -> Result<bool, QueryError> {
        let free = std::rc::Rc::new(ast::PatternChoice::new());
        let rows = rows.into_iter().map(|r| (r, free.clone())).collect();
        self.useful_choosing(root, rows, (query, free))
    }
    /// LANG-13: a range pattern or an alternative no value reaches, because
    /// earlier arms or earlier alternatives of its or-pattern match every value
    /// it matches, is rejected. Other arms remain accepted; covered catch-all
    /// arms have a producer plan to omit their bodies during lowering.
    pub(super) fn unreachable_patterns(
        &mut self,
        root: TypeId,
        arms: &[ast::MatchArm],
    ) -> Result<(), QueryError> {
        let free = std::rc::Rc::new(ast::PatternChoice::new());
        let mut rows: Vec<Row> = vec![];
        for arm in arms {
            let whole = vec![Some(arm.pattern)];
            // Only a catch-all needs the new reachability query. Keep the
            // existing range/or diagnostics and recursive-pattern work budget.
            let (alternations, range) = self.refining_patterns(arm.pattern);
            let catch_all = self.wild(Some(arm.pattern));
            let reachable = if catch_all || range.is_some() && alternations.is_empty() {
                self.useful_choosing(root, rows.clone(), (whole.clone(), free.clone()))?
            } else {
                true
            };
            if catch_all && !reachable {
                self.plans.unreachable_arms.push(arm.pattern);
            }
            if alternations.is_empty() {
                if let Some(range) = range
                    && !reachable
                {
                    self.report(
                        const { RuleId::of("LANG-13") },
                        self.input.syntax.tree.patterns[range.0 as usize]
                            .origin
                            .span,
                        "range pattern can never match: earlier arms match every value it matches",
                    );
                }
            }
            for (alternation, alternatives, parents, preceding) in alternations {
                let mut earlier = rows.clone();
                earlier.extend(preceding.into_iter().map(|choice| (whole.clone(), choice)));
                for alternative in alternatives {
                    let mut choice = parents.clone();
                    choice.insert(alternation, alternative);
                    let choice = std::rc::Rc::new(choice);
                    if !self.useful_choosing(
                        root,
                        earlier.clone(),
                        (whole.clone(), choice.clone()),
                    )? {
                        self.report(
                            const { RuleId::of("LANG-13") },
                            self.input.syntax.tree.patterns[alternative.0 as usize].origin.span,
                            "pattern alternative can never match: earlier patterns match every value it matches",
                        );
                    }
                    earlier.push((whole.clone(), choice));
                }
            }
            if arm.guard.is_none() {
                rows.push((whole, free.clone()));
            }
        }
        Ok(())
    }
    /// A pattern's or-patterns with their alternatives, and its first range.
    #[allow(clippy::type_complexity)]
    fn refining_patterns(
        &self,
        root: ast::PatternId,
    ) -> (Vec<RefiningAlternation>, Option<ast::PatternId>) {
        let mut alternations = vec![];
        let mut range = None;
        let mut todo = vec![(root, ast::PatternChoice::new(), vec![])];
        while let Some((id, parents, preceding)) = todo.pop() {
            let Some(row) = self.body.patterns.iter().find(|p| p.pattern == id) else {
                continue;
            };
            match &row.kind {
                PatternDecisionKind::Range => {
                    range.get_or_insert(id);
                }
                PatternDecisionKind::Or(alternatives) => {
                    alternations.push((
                        id,
                        alternatives.clone(),
                        parents.clone(),
                        preceding.clone(),
                    ));
                    let mut earlier = preceding;
                    let mut children = vec![];
                    for alternative in alternatives {
                        // Query this occurrence through its enclosing alternatives.
                        // Other branches cannot witness a nested alternative's
                        // usefulness, and preceding branches already claimed values.
                        let mut choice = parents.clone();
                        choice.insert(id, *alternative);
                        children.push((*alternative, choice.clone(), earlier.clone()));
                        earlier.push(std::rc::Rc::new(choice));
                    }
                    todo.extend(children.into_iter().rev());
                }
                _ => todo.extend(
                    row.fields
                        .iter()
                        .rev()
                        .map(|(_, child)| (*child, parents.clone(), preceding.clone())),
                ),
            }
        }
        (alternations, range)
    }
    /// The alternatives a row takes at an or-pattern cell: its choice there, or all.
    fn alternatives_of(
        &self,
        cell: Cell,
        choice: &ast::PatternChoice,
    ) -> Option<Vec<ast::PatternId>> {
        let id = cell?;
        let ast::PatternKind::Or(alternatives) =
            &self.input.syntax.tree.patterns[id.0 as usize].kind
        else {
            return None;
        };
        Some(match choice.get(&id) {
            Some(chosen) => vec![*chosen],
            None => alternatives.clone(),
        })
    }
    /// Usefulness by matrix specialization: whether some value matches `query`
    /// and no row. Nested products and variants split by constructor, rather
    /// than treating any occurrence of a variant tag as complete payload
    /// coverage; integer and char columns split their domain at the bounds
    /// their literal and range patterns name. As in Maranget's algorithm (and
    /// rustc's pattern analysis), a wildcard query splits a column only when
    /// its rows name every constructor; otherwise only the default matrix is
    /// searched, so the work is bounded by the patterns, never by the type.
    /// Each row, and the query, takes the alternatives its choice names and
    /// every alternative elsewhere.
    fn useful_choosing(
        &mut self,
        root: TypeId,
        rows: Vec<Row>,
        query: Row,
    ) -> Result<bool, QueryError> {
        let mut todo = vec![(vec![root], rows, query)];
        while let Some((columns, rows, (query, asked_choice))) = todo.pop() {
            work::count(Counter::BodyUsefulnessMatrices);
            let rows: Vec<Row> = if columns.is_empty() {
                rows
            } else {
                rows.into_iter()
                    .flat_map(
                        |(cells, choice)| match self.alternatives_of(cells[0], &choice) {
                            Some(alternatives) => alternatives
                                .into_iter()
                                .map(|a| {
                                    let mut c = cells.clone();
                                    c[0] = Some(a);
                                    (c, choice.clone())
                                })
                                .collect(),
                            None => vec![(cells, choice)],
                        },
                    )
                    .collect()
            };
            if !columns.is_empty()
                && let Some(alternatives) = self.alternatives_of(query[0], &asked_choice)
            {
                for alternative in alternatives {
                    let mut q = query.clone();
                    q[0] = Some(alternative);
                    todo.push((columns.clone(), rows.clone(), (q, asked_choice.clone())));
                }
                continue;
            }
            if rows.iter().any(|(r, _)| r.iter().all(|p| self.wild(*p))) {
                continue;
            }
            if rows.is_empty() {
                return Ok(true);
            }
            if columns.is_empty() {
                continue;
            }
            let first = columns[0];
            let rest = &columns[1..];
            let inspected = self.box_payload(first);
            let (_, repr) = self.representation(inspected)?;
            let mut choices: Option<Vec<(Tag, Vec<TypeId>)>> = match self.kind(repr) {
                TypeKind::Primitive(Primitive::Bool) => {
                    Some(vec![(Tag::Bool(false), vec![]), (Tag::Bool(true), vec![])])
                }
                TypeKind::Tuple(ts) => Some(vec![(Tag::Tuple, ts.to_vec())]),
                TypeKind::Record(fs) => {
                    Some(vec![(Tag::Record, fs.into_iter().map(|f| f.ty).collect())])
                }
                TypeKind::Variant(vs) => Some(
                    vs.into_iter()
                        .enumerate()
                        .map(|(i, v)| {
                            (
                                Tag::Variant(i as u32),
                                v.fields.into_iter().map(|(_, t)| t).collect(),
                            )
                        })
                        .collect(),
                ),
                _ => None,
            };
            let scalar = match self.kind(repr) {
                TypeKind::Primitive(p) if p.pattern_domain().is_some() => Some(p),
                _ => None,
            };
            if let Some(primitive) = scalar {
                let intervals: Vec<_> = rows
                    .iter()
                    .map(|(r, _)| self.scalar_interval(r[0], primitive))
                    .collect();
                let asked = self.scalar_interval(query[0], primitive);
                if intervals.iter().any(Option::is_some) || asked.is_some() {
                    choices = Some(
                        scalar_segments(primitive, &intervals, asked)
                            .into_iter()
                            .map(|(low, high)| (Tag::Scalar(low, high), vec![]))
                            .collect(),
                    );
                }
            }
            let missing = |choices: &[(Tag, Vec<TypeId>)]| {
                choices.iter().any(|(tag, fields)| {
                    !rows.iter().any(|(r, _)| {
                        !self.wild(r[0]) && self.specialize(r, tag, fields.len(), scalar).is_some()
                    })
                })
            };
            // A wildcard query over a column that names only some constructors
            // is useful exactly where it is useful among the wildcard rows: a
            // missing constructor keeps those rows alone. Constructors no row
            // names are never unfolded, so a recursive type is explored only
            // as deep as its patterns go.
            let choices = choices.filter(|c| !(self.wild(query[0]) && missing(c)));
            let Some(choices) = choices else {
                // An open domain always has a value no literal names.
                if !self.wild(query[0]) {
                    let spelling = self.input.syntax.tree.patterns[query[0].unwrap().0 as usize]
                        .kind
                        .clone();
                    let kept = rows
                        .into_iter()
                        .filter(|(r, _)| {
                            self.wild(r[0])
                                || self.input.syntax.tree.patterns[r[0].unwrap().0 as usize].kind
                                    == spelling
                        })
                        .map(|(r, c)| (r[1..].to_vec(), c))
                        .collect();
                    todo.push((rest.to_vec(), kept, (query[1..].to_vec(), asked_choice)));
                    continue;
                }
                let defaults = rows
                    .into_iter()
                    .filter(|(r, _)| self.wild(r[0]))
                    .map(|(r, c)| (r[1..].to_vec(), c))
                    .collect();
                todo.push((rest.to_vec(), defaults, (query[1..].to_vec(), asked_choice)));
                continue;
            };
            for (tag, fields) in choices {
                let Some(asked) = self.specialize(&query, &tag, fields.len(), scalar) else {
                    continue;
                };
                let specialized = rows
                    .iter()
                    .filter_map(|(row, c)| {
                        Some((self.specialize(row, &tag, fields.len(), scalar)?, c.clone()))
                    })
                    .collect();
                let mut next = fields;
                next.extend_from_slice(rest);
                todo.push((next, specialized, (asked, asked_choice.clone())));
            }
        }
        Ok(false)
    }
    /// A row specialized by a constructor: its head's fields then its tail, or
    /// None when the head names another constructor.
    fn specialize(
        &self,
        row: &[Cell],
        tag: &Tag,
        arity: usize,
        scalar: Option<Primitive>,
    ) -> Option<Vec<Cell>> {
        let head = row[0];
        let mut cells = vec![None; arity];
        if !self.wild(head) {
            if let Tag::Scalar(low, high) = tag {
                let (from, to) = self.scalar_interval(head, scalar?)?;
                if from > *low || to < *high {
                    return None;
                }
            } else if self.tag(head).as_ref() != Some(tag) {
                return None;
            } else {
                let p = self
                    .body
                    .patterns
                    .iter()
                    .find(|p| Some(p.pattern) == head)?;
                for (f, child) in &p.fields {
                    if let Some(c) = cells.get_mut(f.0 as usize) {
                        *c = Some(*child);
                    }
                }
            }
        }
        cells.extend_from_slice(&row[1..]);
        Some(cells)
    }
    fn wild(&self, p: Cell) -> bool {
        p.is_none_or(|id| {
            self.body
                .patterns
                .iter()
                .find(|p| p.pattern == id)
                .is_some_and(|p| {
                    matches!(
                        p.kind,
                        PatternDecisionKind::Binding(_)
                            | PatternDecisionKind::Wildcard
                            | PatternDecisionKind::Error
                    )
                })
        })
    }
    fn tag(&self, p: Cell) -> Option<Tag> {
        let id = p?;
        let d = self.body.patterns.iter().find(|p| p.pattern == id)?;
        match &d.kind {
            PatternDecisionKind::Tuple => Some(Tag::Tuple),
            PatternDecisionKind::Constructor(ContextualName::Constructor(_)) => Some(Tag::Record),
            PatternDecisionKind::Constructor(ContextualName::Variant { variant, .. }) => {
                Some(Tag::Variant(variant.0))
            }
            PatternDecisionKind::Literal => {
                match self.input.syntax.tree.patterns[id.0 as usize].kind {
                    ast::PatternKind::Literal(ast::Literal::Bool(v)) => Some(Tag::Bool(v)),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}
type Cell = Option<ast::PatternId>;
/// Pattern cells, and the alternatives they take at chosen or-patterns.
type Row = (Vec<Cell>, std::rc::Rc<ast::PatternChoice>);
/// The disjoint key intervals of a scalar domain that every named interval
/// either contains or misses, keeping one interval per distinct set of rows
/// that cover it (equal sets specialize identically). Inside `asked` only,
/// when the query names an interval.
fn scalar_segments(
    primitive: Primitive,
    intervals: &[Option<(u128, u128)>],
    asked: Option<(u128, u128)>,
) -> Vec<(u128, u128)> {
    let mut cuts: Vec<u128> = intervals
        .iter()
        .flatten()
        .chain(asked.iter())
        .flat_map(|(low, high)| [Some(*low), high.checked_add(1)])
        .flatten()
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut seen = std::collections::BTreeSet::new();
    let mut segments = vec![];
    for (from, to) in primitive.pattern_domain().unwrap_or_default() {
        let (from, to) = match asked {
            Some((low, high)) => (from.max(low), to.min(high)),
            None => (from, to),
        };
        if from > to {
            continue;
        }
        let mut low = from;
        loop {
            let high = cuts
                .iter()
                .find(|c| **c > low && **c <= to)
                .map_or(to, |c| c - 1);
            let covering: Vec<usize> = intervals
                .iter()
                .enumerate()
                .filter(|(_, i)| i.is_some_and(|(a, b)| a <= low && high <= b))
                .map(|(row, _)| row)
                .collect();
            if seen.insert(covering) {
                segments.push((low, high));
            }
            if high == to {
                break;
            }
            low = high + 1;
        }
    }
    segments
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum Tag {
    Bool(bool),
    Tuple,
    Record,
    Variant(u32),
    /// An interval of scalar keys that each named interval contains or misses.
    Scalar(u128, u128),
}

/// An or-pattern occurrence, its enclosing choices, and alternatives that
/// precede that occurrence along the enclosing path.
type RefiningAlternation = (
    ast::PatternId,
    Vec<ast::PatternId>,
    ast::PatternChoice,
    Vec<std::rc::Rc<ast::PatternChoice>>,
);
