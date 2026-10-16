use super::builder::{Builder, Value};
use super::*;
use hirsel_syntax::PatternId;
use hirsel_tir::*;
use hirsel_types::{ContextualName, PatternDecision, PatternDecisionKind, ResolvedName, UseKind};
impl Builder<'_, '_> {
    fn pattern(&self, id: PatternId) -> Result<PatternDecision> {
        let Some(node) = self.input.tree.patterns.get(id.0 as usize) else {
            return self.error(Span::default(), "pattern index out of range");
        };
        let mut rows = self.decisions.patterns.get(&id).iter().copied();
        let Some(row) = rows.next() else {
            return self.error(node.origin.span, "missing pattern decision");
        };
        if rows.next().is_some() {
            return self.error(node.origin.span, "duplicate pattern decision");
        }
        if self.types.get(row.ty).is_none() {
            return self.error(node.origin.span, "foreign pattern type");
        }
        Ok(row.clone())
    }
    fn inspected_pattern_place(&mut self, row: &PatternDecision, mut place: PlaceId) -> PlaceId {
        if matches!(
            row.kind,
            PatternDecisionKind::Binding(_) | PatternDecisionKind::Wildcard
        ) {
            return place;
        }
        // #98 uses the existing physical Field(0), including nested boxes.
        let mut ty = self.body.places[place.0 as usize].ty;
        while let hirsel_types::TypeKind::Box(part) = self.types.get(ty).unwrap().kind {
            let mut payload = self.types.get(part).unwrap().clone();
            if !self.input.ownership.plain_types.contains(&part) {
                payload.modes.relation = payload
                    .modes
                    .relation
                    .union(&self.types.get(ty).unwrap().modes.relation);
            }
            ty = self.types.push(payload);
            place = self.project(place, Projection::Field(0), ty);
        }
        place
    }
    pub fn bind_pattern(
        &mut self,
        id: PatternId,
        place: PlaceId,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<()> {
        self.bind_pattern_stage(id, place, bodies, None, false)
    }
    pub fn guard_pattern(
        &mut self,
        id: PatternId,
        place: PlaceId,
        bodies: &mut Vec<Option<Body>>,
        test: bool,
    ) -> Result<()> {
        self.bind_pattern_stage(id, place, bodies, Some(test), false)
    }
    pub fn bind_fresh_pattern(
        &mut self,
        id: PatternId,
        place: PlaceId,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<()> {
        self.bind_pattern_stage(id, place, bodies, None, true)
    }
    fn bind_pattern_stage(
        &mut self,
        id: PatternId,
        place: PlaceId,
        bodies: &mut Vec<Option<Body>>,
        stage: Option<bool>,
        fresh: bool,
    ) -> Result<()> {
        let row = self.pattern(id)?;
        let place = self.inspected_pattern_place(&row, place);
        let span = self.input.tree.patterns[id.0 as usize].origin.span;
        match row.kind {
            PatternDecisionKind::Wildcard | PatternDecisionKind::Literal => {}
            PatternDecisionKind::Error => {
                return self.error(span, "recovery pattern reached lowering");
            }
            PatternDecisionKind::Binding(local) => {
                let Some(binding) = self
                    .decisions
                    .bindings
                    .get(&NameSite::Pattern(id))
                    .iter()
                    .copied()
                    .find(|b| {
                        b.site == NameSite::Pattern(id)
                            && b.owner == self.owner
                            && b.binding == ResolvedName::Local(local)
                    })
                    .cloned()
                else {
                    return self.error(span, "missing pattern binding row");
                };
                if binding.usage != row.usage
                    || !hirsel_types::shape_equal(
                        &self.input.checked.types,
                        binding.ty,
                        &self.input.checked.types,
                        row.ty,
                    )
                    .unwrap_or(false)
                {
                    return self.error(span, "binding and pattern decisions disagree");
                }
                let usage = if let Some(test) = stage {
                    if let Some(uses) = self.decisions.guards.get(&id).iter().copied().next() {
                        if uses.commit != row.usage
                            || matches!(uses.test, UseKind::Take | UseKind::Mut)
                        {
                            return self.error(span, "inconsistent guarded-pattern decisions");
                        }
                        if test { uses.test } else { uses.commit }
                    } else if row.usage == UseKind::Take {
                        return self.error(span, "missing guarded-pattern test/commit decisions");
                    } else {
                        row.usage
                    }
                } else {
                    row.usage
                };
                // The initializer was already evaluated into an owned staging
                // slot. Relocating its whole value or an introduced field transfers
                // that identity even when the source-level value is Plain.
                let usage = if stage != Some(true)
                    && usage == UseKind::Copy
                    // Indexed storage is revisited by the loop. Only an owned
                    // staging value or its introduced fields can be relocated;
                    // Copy is still Copy for an element of that value.
                    && !self.body.places[place.0 as usize]
                        .projection.iter().any(|p| matches!(p, Projection::Index(_)))
                    && (fresh
                        || self.body.places[place.0 as usize].projection.is_empty()
                        || self.body.places[place.0 as usize]
                            .binding
                            .as_ref()
                            .is_some_and(|b| b.fields.is_empty()))
                    && self.body.locals[self.root(place).0 as usize].storage == Storage::Temporary
                    && self.owned[self.root(place).0 as usize]
                {
                    UseKind::Take
                } else {
                    usage
                };
                if stage == Some(true) || matches!(usage, UseKind::Borrow | UseKind::Mut) {
                    // Borrowed bindings are views of their source place. Their
                    // scope loan ends without creating an owned alias slot;
                    // repeated loop iterations can bind the same view safely.
                    let mut value = Value {
                        checked_ty: None,
                        place,
                        usage,
                    };
                    let _ = self.loan(&mut value, span);
                    let origin = Self::binding_origin(binding.owner, &binding.binding).unwrap();
                    let place =
                        self.bind_origin(value.place, origin, OriginBindingKind::Alias, span);
                    self.retain_view(place);
                    self.bindings.push((binding.owner, binding.binding, place));
                    return Ok(());
                }
                let mut value = self.transport_typed(
                    Value {
                        checked_ty: None,
                        place,
                        usage,
                    },
                    binding.ty,
                    Storage::Local,
                    span,
                );
                let origin = Self::binding_origin(binding.owner, &binding.binding).unwrap();
                value.place = self.bind_origin(
                    value.place,
                    origin,
                    if fresh {
                        OriginBindingKind::Fresh
                    } else {
                        OriginBindingKind::Alias
                    },
                    span,
                );
                self.bindings
                    .push((binding.owner, binding.binding, value.place));
            }
            PatternDecisionKind::Tuple | PatternDecisionKind::Constructor(_) => {
                let resource_fields = if stage != Some(true) && row.usage == UseKind::Take {
                    self.decisions
                        .resource_patterns
                        .get(&id)
                        .first()
                        .map(|(_, fields)| fields.clone())
                } else {
                    None
                };
                let defused = resource_fields.is_some();
                if defused {
                    // LANG-13 / #97: committing the whole pattern consumes the
                    // parent's finalizer before any field is transferred.
                    self.emit(InstructionKind::Defuse { place }, span);
                }
                let omitted: Vec<_> = resource_fields
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .filter(|(ordinal, _)| {
                        !row.fields.iter().any(|(field, child)| {
                            field.0 as usize == *ordinal
                                && !matches!(
                                    self.pattern(*child).map(|p| p.kind),
                                    Ok(PatternDecisionKind::Wildcard)
                                )
                        })
                    })
                    .collect();
                let staging = stage != Some(true)
                    && self.body.locals[self.root(place).0 as usize].storage == Storage::Temporary;
                let mut children = Vec::new();
                let mut fields = Vec::new();
                for (field, child) in row.fields {
                    let child_row = self.pattern(child)?;
                    let projection = match &row.kind {
                        PatternDecisionKind::Constructor(ContextualName::Variant {
                            variant,
                            ..
                        }) => Projection::VariantField {
                            variant: variant.0,
                            field: field.0,
                        },
                        _ => Projection::Field(field.0),
                    };
                    let projected = self.project(place, projection, child_row.ty);
                    if staging {
                        if let PatternDecisionKind::Binding(local) = child_row.kind {
                            let origin =
                                Self::binding_origin(self.owner, &ResolvedName::Local(local))
                                    .unwrap();
                            self.body.places[projected.0 as usize].binding = Some(origin);
                            fields.push(projected);
                        }
                    }
                    children.push((child, projected));
                }
                if !fields.is_empty() {
                    self.emit(
                        InstructionKind::BindFields {
                            source: place,
                            fields,
                        },
                        span,
                    );
                }
                for (child, projected) in children {
                    // Field binders were introduced together; transport retains
                    // those identities rather than minting them a second time.
                    self.bind_pattern_stage(
                        child,
                        projected,
                        bodies,
                        stage,
                        if staging { false } else { fresh },
                    )?;
                }
                // Stage every omission before invoking any finalizer. Reverse
                // introduction order makes failure cleanup's ordinary reverse
                // evaluation order equal declaration order for these owners.
                let mut discarded = Vec::new();
                for (field, ty) in omitted.into_iter().rev() {
                    let projection = match &row.kind {
                        PatternDecisionKind::Constructor(ContextualName::Variant {
                            variant,
                            ..
                        }) => Projection::VariantField {
                            variant: variant.0,
                            field: field as u32,
                        },
                        _ => Projection::Field(field as u32),
                    };
                    let source = self.project(place, projection, ty);
                    let omitted = self.transport_typed(
                        Value {
                            checked_ty: None,
                            place: source,
                            usage: UseKind::Take,
                        },
                        ty,
                        Storage::Temporary,
                        span,
                    );
                    discarded.push(omitted.place);
                }
                if defused {
                    // The distribution is complete before control can join.
                    // Retire the empty shell before an omitted-field finalizer
                    // can introduce a normal/failure continuation join.
                    self.emit(
                        InstructionKind::Drop {
                            place,
                            if_live: false,
                        },
                        span,
                    );
                    if self.body.places[place.0 as usize].projection.is_empty() {
                        self.state.live.remove(&self.root(place).0);
                    }
                }
                // #97: omissions finalize immediately, in declaration order.
                for place in discarded.into_iter().rev() {
                    self.emit(
                        InstructionKind::Drop {
                            place,
                            if_live: false,
                        },
                        span,
                    );
                    self.state.live.remove(&self.root(place).0);
                }
            }
        }
        let _ = bodies;
        Ok(())
    }
    pub fn table_pattern(
        &mut self,
        id: PatternId,
        place: PlaceId,
        patterns: &mut Vec<MatchPattern>,
        guarded: bool,
    ) -> Result<MatchPatternId> {
        let row = self.pattern(id)?;
        let place = self.inspected_pattern_place(&row, place);
        let span = self.input.tree.patterns[id.0 as usize].origin.span;
        let mut fields = vec![];
        for (field, child) in &row.fields {
            let projection = match &row.kind {
                PatternDecisionKind::Constructor(ContextualName::Variant { variant, .. }) => {
                    Projection::VariantField {
                        variant: variant.0,
                        field: field.0,
                    }
                }
                _ => Projection::Field(field.0),
            };
            let child_row = self.pattern(*child)?;
            let child_place = self.project(place, projection, child_row.ty);
            let node = self.table_pattern(*child, child_place, patterns, guarded)?;
            fields.push((field.0, node));
        }
        let kind = match row.kind {
            PatternDecisionKind::Wildcard => MatchPatternKind::Wildcard,
            PatternDecisionKind::Binding(_) => {
                let guard_usage = if guarded {
                    match self.decisions.guards.get(&id).iter().copied().next() {
                        Some(r)
                            if r.commit == row.usage
                                && !matches!(r.test, UseKind::Take | UseKind::Mut) =>
                        {
                            r.test
                        }
                        None if row.usage != UseKind::Take => row.usage,
                        _ => {
                            return self.error(span, "missing or inconsistent guarded-pattern use");
                        }
                    }
                } else {
                    row.usage
                };
                MatchPatternKind::Binding {
                    usage: row.usage,
                    guard_usage,
                }
            }
            PatternDecisionKind::Literal => {
                let mut rows = self
                    .decisions
                    .literals
                    .get(&id)
                    .iter()
                    .copied()
                    .filter(|(p, _)| *p == id);
                let Some((_, literal)) = rows.next() else {
                    return self.error(span, "missing checked pattern literal");
                };
                if rows.next().is_some() {
                    return self.error(span, "duplicate checked pattern literal");
                }
                MatchPatternKind::Literal(super::literal::constant(
                    literal,
                    &self
                        .types
                        .get(self.body.places[place.0 as usize].ty)
                        .unwrap()
                        .kind,
                    span,
                )?)
            }
            PatternDecisionKind::Tuple => MatchPatternKind::Tuple(fields),
            PatternDecisionKind::Constructor(ContextualName::Constructor(definition)) => {
                MatchPatternKind::Record { definition, fields }
            }
            PatternDecisionKind::Constructor(ContextualName::Variant { owner, variant }) => {
                MatchPatternKind::Variant {
                    definition: owner,
                    variant: variant.0,
                    fields,
                }
            }
            _ => return self.error(span, "invalid checked match pattern"),
        };
        let node = MatchPatternId(patterns.len() as u32);
        patterns.push(MatchPattern { place, kind });
        Ok(node)
    }
}
