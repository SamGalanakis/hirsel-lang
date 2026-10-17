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
                            self.report(
                                const { RuleId::of("LANG-7") },
                                p.origin.span,
                                "literal pattern has the wrong type",
                            );
                        }
                        self.plans.pattern_literals.push((id, l.clone()));
                        PatternDecisionKind::Literal
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
    /// Matrix specialization handles nested products and variants, rather than
    /// treating any occurrence of a variant tag as complete payload coverage.
    pub(super) fn exhaustive(
        &mut self,
        root: TypeId,
        arms: &[ast::MatchArm],
    ) -> Result<bool, QueryError> {
        let rows: Vec<Vec<Option<ast::PatternId>>> = arms
            .iter()
            .filter(|a| a.guard.is_none())
            .map(|a| vec![Some(a.pattern)])
            .collect();
        let mut todo = vec![(vec![root], rows)];
        while let Some((types, rows)) = todo.pop() {
            if rows.iter().any(|r| r.iter().all(|p| self.wild(*p))) {
                continue;
            }
            if rows.is_empty() {
                return Ok(false);
            }
            if types.is_empty() {
                continue;
            }
            let first = types[0];
            let rest = &types[1..];
            let inspected = self.box_payload(first);
            let (_, repr) = self.representation(inspected)?;
            let choices: Option<Vec<(Tag, Vec<TypeId>)>> = match self.kind(repr) {
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
            if let Some(choices) = choices {
                for (tag, fields) in choices {
                    let mut specialized = vec![];
                    for row in &rows {
                        let head = row[0];
                        let mut prefix = if self.wild(head) {
                            vec![None; fields.len()]
                        } else if self.tag(head) == Some(tag.clone()) {
                            let p = self
                                .body
                                .patterns
                                .iter()
                                .find(|p| Some(p.pattern) == head)
                                .unwrap();
                            let mut cs = vec![None; fields.len()];
                            for (f, child) in &p.fields {
                                if let Some(c) = cs.get_mut(f.0 as usize) {
                                    *c = Some(*child);
                                }
                            }
                            cs
                        } else {
                            continue;
                        };
                        prefix.extend_from_slice(&row[1..]);
                        specialized.push(prefix);
                    }
                    let mut next = fields;
                    next.extend_from_slice(rest);
                    todo.push((next, specialized));
                }
            } else {
                let defaults = rows
                    .into_iter()
                    .filter(|r| self.wild(r[0]))
                    .map(|r| r[1..].to_vec())
                    .collect();
                todo.push((rest.to_vec(), defaults));
            }
        }
        Ok(true)
    }
    fn wild(&self, p: Option<ast::PatternId>) -> bool {
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
    fn tag(&self, p: Option<ast::PatternId>) -> Option<Tag> {
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
#[derive(Clone, Debug, Eq, PartialEq)]
enum Tag {
    Bool(bool),
    Tuple,
    Record,
    Variant(u32),
}
