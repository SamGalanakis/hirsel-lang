use super::*;
impl Checker<'_, '_> {
    pub(super) fn mutable_map_pattern(&mut self, root: ast::PatternId) {
        let mut pending = vec![(root, false)];
        while let Some((pattern, key)) = pending.pop() {
            let Some(row) = self.body.patterns.iter().find(|p| p.pattern == pattern) else {
                continue;
            };
            let fields = row.fields.clone();
            let whole = pattern == root;
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
                    .map(|(field, child)| (child, key || whole && field.0 == 0)),
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
        let root_span = self.input.tree.patterns[root.0 as usize].origin.span;
        let mut todo = vec![(root, ty)];
        let mut refutable = false;
        while let Some((id, ty)) = todo.pop() {
            let p = self.input.tree.patterns[id.0 as usize].clone();
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
                    ast::PatternKind::Tuple(ps) => children.extend(ps.iter().map(|p| (*p, ty))),
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
                                .find(|(_, v)| v.name == self.name(*name) && v.fields.is_empty())
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
                                TypeKind::Variant(vs) => {
                                    vs.iter().enumerate().find(|(_, v)| v.name == name).map(
                                        |(i, v)| {
                                            (
                                                ContextualName::Variant {
                                                    owner: owner.clone(),
                                                    variant: VariantId(i as u32),
                                                },
                                                v.fields.to_vec(),
                                                vs.len() > 1,
                                            )
                                        },
                                    )
                                }
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
                                    &slots.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>(),
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
                            self.report(
                                const { RuleId::of("LANG-13") },
                                p.origin.span,
                                "constructor pattern needs a nominal type",
                            );
                            for f in fs {
                                let e = self.error();
                                children.push((f.pattern, e));
                            }
                            PatternDecisionKind::Error
                        }
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
        if irrefutable && refutable {
            self.report(
                const { RuleId::of("LANG-13") },
                root_span,
                "binding pattern must be irrefutable",
            );
        }
        Ok(())
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
        match &self.input.tree.patterns[id.0 as usize].kind {
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
    /// LANG-13: a range pattern no value reaches, because earlier arms match every
    /// value it matches, is rejected. Other unreachable arms are not judged here.
    pub(super) fn unreachable_ranges(
        &mut self,
        root: TypeId,
        arms: &[ast::MatchArm],
    ) -> Result<(), QueryError> {
        let mut rows = vec![];
        for arm in arms {
            if let Some(range) = self.first_range(arm.pattern)
                && !self.useful(root, rows.clone(), vec![Some(arm.pattern)])?
            {
                self.report(
                    const { RuleId::of("LANG-13") },
                    self.input.tree.patterns[range.0 as usize].origin.span,
                    "range pattern can never match: earlier arms match every value it matches",
                );
            }
            if arm.guard.is_none() {
                rows.push(vec![Some(arm.pattern)]);
            }
        }
        Ok(())
    }
    fn first_range(&self, root: ast::PatternId) -> Option<ast::PatternId> {
        let mut todo = vec![root];
        while let Some(id) = todo.pop() {
            let row = self.body.patterns.iter().find(|p| p.pattern == id)?;
            if matches!(row.kind, PatternDecisionKind::Range) {
                return Some(id);
            }
            todo.extend(row.fields.iter().rev().map(|(_, child)| *child));
        }
        None
    }
    /// Usefulness by matrix specialization: whether some value matches `query`
    /// and no row. Nested products and variants split by constructor, rather
    /// than treating any occurrence of a variant tag as complete payload
    /// coverage; integer and char columns split their domain at the bounds
    /// their literal and range patterns name.
    fn useful(
        &mut self,
        root: TypeId,
        rows: Vec<Vec<Cell>>,
        query: Vec<Cell>,
    ) -> Result<bool, QueryError> {
        let mut todo = vec![(vec![root], rows, query)];
        while let Some((types, rows, query)) = todo.pop() {
            if rows.iter().any(|r| r.iter().all(|p| self.wild(*p))) {
                continue;
            }
            if rows.is_empty() {
                return Ok(true);
            }
            if types.is_empty() {
                continue;
            }
            let first = types[0];
            let rest = &types[1..];
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
                    .map(|r| self.scalar_interval(r[0], primitive))
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
            let Some(choices) = choices else {
                // An open domain always has a value no literal names.
                if !self.wild(query[0]) {
                    let spelling = self.input.tree.patterns[query[0].unwrap().0 as usize]
                        .kind
                        .clone();
                    let kept = rows
                        .into_iter()
                        .filter(|r| {
                            self.wild(r[0])
                                || self.input.tree.patterns[r[0].unwrap().0 as usize].kind
                                    == spelling
                        })
                        .map(|r| r[1..].to_vec())
                        .collect();
                    todo.push((rest.to_vec(), kept, query[1..].to_vec()));
                    continue;
                }
                let defaults = rows
                    .into_iter()
                    .filter(|r| self.wild(r[0]))
                    .map(|r| r[1..].to_vec())
                    .collect();
                todo.push((rest.to_vec(), defaults, query[1..].to_vec()));
                continue;
            };
            for (tag, fields) in choices {
                let Some(asked) = self.specialize(&query, &tag, fields.len(), scalar) else {
                    continue;
                };
                let specialized = rows
                    .iter()
                    .filter_map(|row| self.specialize(row, &tag, fields.len(), scalar))
                    .collect();
                let mut next = fields;
                next.extend_from_slice(rest);
                todo.push((next, specialized, asked));
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
            PatternDecisionKind::Literal => match self.input.tree.patterns[id.0 as usize].kind {
                ast::PatternKind::Literal(ast::Literal::Bool(v)) => Some(Tag::Bool(v)),
                _ => None,
            },
            _ => None,
        }
    }
}
type Cell = Option<ast::PatternId>;
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
