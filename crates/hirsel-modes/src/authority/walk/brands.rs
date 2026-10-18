use super::*;

impl Checker<'_> {
    /// The arenas a relation admits: the arenas it names and those named by
    /// the relation of each binding it names (v22 R3, "within S"). A frame
    /// relation (`env`) alone stands for bindings in scope, which every arena
    /// parameter outlives, so the caller admits any arena there.
    pub(super) fn admitted_arenas(&self, relation: &Relation, named: Relation) -> Relation {
        let mut admitted = named;
        for reach in relation.reaches() {
            let Reach::Place(origin) = reach else {
                continue;
            };
            if self.arena_place(origin) {
                continue;
            }
            let Some(binding) = self.origin_binding(origin) else {
                continue;
            };
            // A field path holds what its field was constructed with, when
            // that is known (#82), and otherwise what the binding holds.
            let field = origin.fields.first().and_then(|field| {
                self.field_holdings
                    .get(&binding_key(binding))
                    .and_then(|fields| fields.get(field))
            });
            let own = match field {
                Some(held) => held,
                None => &self.out.body.types.get(binding.ty).unwrap().modes.relation,
            };
            admitted = admitted
                .union(&own.retain(|r| matches!(r, Reach::Place(o) if self.arena_place(o))));
        }
        admitted
    }
    /// The body binding a place's root names.
    pub(super) fn origin_binding(&self, origin: &Origin) -> Option<&BindingDecision> {
        self.out
            .body
            .bindings
            .iter()
            .find(|b| match (&origin.root, &b.binding) {
                (OriginRoot::Local(l), ResolvedName::Local(m)) => l == m,
                (
                    OriginRoot::Parameter(ParameterRef::Body { scope, parameter }),
                    ResolvedName::Parameter(p),
                ) => *scope == b.owner && parameter == p,
                _ => false,
            })
    }
    /// The arenas a value read from a field path of a binding with known
    /// per-field holdings holds (#82), which its read row, placed as the
    /// whole owner, does not narrow.
    pub(super) fn read_holdings(&self, value: ExprId) -> Option<Relation> {
        let mut root = value;
        while let ExprKind::Unary { value, .. } = self.tree.expressions[root.0 as usize].kind {
            root = value;
        }
        if !self.source_rows.places.contains_key(&root) {
            return None;
        }
        let place = self.source_place(root);
        let SourceRoot::Binding { owner, binding } = &place.root else {
            return None;
        };
        let SourceProjectionKind::Field(field) = place.projections.first()?.kind else {
            return None;
        };
        let binding = self.root_binding(*owner, binding)?;
        self.field_holdings
            .get(&binding_key(binding))?
            .get(&field)
            .cloned()
    }
    /// Whether a relation names a binding of this body that is not an arena.
    pub(super) fn names_bindings(&self, relation: &Relation) -> bool {
        relation
            .reaches()
            .iter()
            .any(|r| matches!(r, Reach::Place(o) if self.binding_place(o)))
    }
    /// Whether a relation place is a whole binding of this body, not an arena.
    pub(super) fn binding_place(&self, origin: &Origin) -> bool {
        !self.arena_place(&Origin {
            root: origin.root.clone(),
            fields: vec![].into(),
        }) && matches!(
            origin.root,
            OriginRoot::Local(_) | OriginRoot::Parameter(ParameterRef::Body { .. })
        )
    }
    /// Whether a body relation place names an Arena-typed binding: such a
    /// place sets placement rather than bounding a lifetime (PROV-4).
    pub(super) fn arena_place(&self, origin: &Origin) -> bool {
        origin.fields.is_empty()
            && self.out.body.bindings.iter().any(|b| {
                let root = match (&origin.root, &b.binding) {
                    (OriginRoot::Local(l), ResolvedName::Local(m)) => l == m,
                    (
                        OriginRoot::Parameter(ParameterRef::Body { scope, parameter }),
                        ResolvedName::Parameter(p),
                    ) => *scope == b.owner && parameter == p,
                    _ => false,
                };
                root && self.arena_type(b.ty)
            })
    }
    pub(super) fn arena_type(&self, ty: TypeId) -> bool {
        self.signature
            .metadata
            .roles
            .heap
            .as_ref()
            .is_some_and(|heap| is_arena_type(&self.out.body.types, ty, &heap.arena))
    }
    /// A body row's lifetime sources: every reach except arena places.
    pub(super) fn lifetime(&self, relation: &Relation) -> Relation {
        relation.retain(|r| !matches!(r, Reach::Place(o) if self.arena_place(o)))
    }
    /// A body row's placement: its arena places and regions only.
    pub(super) fn placement(&self, relation: &Relation) -> Relation {
        relation.retain(|r| match r {
            Reach::Place(o) => self.arena_place(o),
            Reach::Region(_) => true,
            Reach::Environment => false,
        })
    }
    /// Whether a body row's relation places its blocks in an arena or region.
    pub(super) fn placed(&self, relation: &Relation) -> bool {
        relation.scoped(|o| self.arena_place(o))
    }
    /// Whether `holder`, at its root or any part, is an unplaced local join of
    /// scoped content `value` holds at the same position (PROV-4). The content
    /// is then bounded by the frame that made the join, as its placement was.
    fn unplaced_scoped(&self, holder: TypeId, value: TypeId) -> bool {
        let types = &self.out.body.types;
        let (Some(h), Some(v)) = (types.get(holder), types.get(value)) else {
            return false;
        };
        if !self.placed(&h.modes.relation)
            && self.lifetime(&h.modes.relation).has_environment()
            && self.placed(&v.modes.relation)
        {
            return true;
        }
        let (hs, vs) = (h.kind.value_parts(), v.kind.value_parts());
        hs.len() == vs.len()
            && hs
                .iter()
                .zip(&vs)
                .any(|(h, v)| self.unplaced_scoped(*h, *v))
    }
    /// The lifetime sources of a callable's result relation: every reach
    /// except the callable's own Arena parameters and body arenas (PROV-4).
    pub(super) fn signature_lifetime(
        &self,
        function: &FunctionType,
        relation: &Relation,
    ) -> Relation {
        let arena = self
            .signature
            .metadata
            .roles
            .heap
            .as_ref()
            .map(|h| &h.arena);
        relation.retain(|r| {
            !matches!(r, Reach::Place(o)
                if self.arena_place(o)
                    || arena.is_some_and(|arena| arena_parameter(&self.out.body.types, function, o, 0, arena)))
        })
    }
    pub(super) fn source_place(&self, e: ExprId) -> &SourcePlace {
        work::count(Counter::AuthoritySourceRows);
        &self.out.body.places[self
            .source_rows
            .places
            .get(&e)
            .copied()
            .expect("H source place")]
    }
    pub(super) fn place_root_identity(&self, e: ExprId) -> Option<Origin> {
        match &self.source_place(e).root {
            SourceRoot::Binding { owner, binding } => {
                let b = self.root_binding(*owner, binding)?;
                if let Some(o) = self.identities.get(&binding_key(b)) {
                    return Some(o.whole.clone());
                }
                let root = match binding {
                    ResolvedName::Local(l) => OriginRoot::Local(*l),
                    ResolvedName::Parameter(p) => {
                        OriginRoot::Parameter(ParameterRef::body(owner.clone(), *p))
                    }
                    _ => unreachable!("checked binding root"),
                };
                Some(Origin {
                    root,
                    fields: vec![].into(),
                })
            }
            SourceRoot::Definition(d) => Some(Origin {
                root: OriginRoot::Definition(d.clone()),
                fields: vec![].into(),
            }),
            SourceRoot::Value(v) => {
                Some(self.expression_origins.get(v).cloned().unwrap_or(Origin {
                    root: OriginRoot::Expression(*v),
                    fields: vec![].into(),
                }))
            }
        }
    }
    pub(super) fn value_partition(&self, e: ExprId) -> Option<Identity> {
        if let Some(partition) = self.expression_partitions.get(&e) {
            return Some(partition.clone());
        }
        let place = self.source_place(e);
        if place.access == SourceAccess::Unique
            || place
                .projections
                .iter()
                .any(|p| matches!(p.kind, SourceProjectionKind::Index(_)))
        {
            return self.identity(e).map(Identity::new);
        }
        self.place_partition(e)
    }
    fn place_partition(&self, e: ExprId) -> Option<Identity> {
        let place = self.source_place(e);
        let root = self.place_root_identity(e)?;
        let base = match &place.root {
            SourceRoot::Binding { owner, binding } => self
                .root_binding(*owner, binding)
                .and_then(|b| self.identities.get(&binding_key(b)))
                .cloned()
                .unwrap_or_else(|| Identity::new(root)),
            // A transparent block keeps the partition of the storage it names.
            SourceRoot::Value(v) => self
                .expression_partitions
                .get(v)
                .cloned()
                .unwrap_or_else(|| Identity::new(root)),
            _ => Identity::new(root),
        };
        let fields: Vec<_> = place
            .projections
            .iter()
            .map_while(|p| match p.kind {
                SourceProjectionKind::Field(f)
                | SourceProjectionKind::VariantField { field: f, .. } => Some(f),
                SourceProjectionKind::Index(_) => None,
            })
            .collect();
        Some(base.projected(&fields))
    }
    pub(super) fn parameter_origins(
        &mut self,
        root: TypeId,
        arguments: &BTreeMap<ParameterRef, OriginArgument>,
        actuals: &BTreeMap<ParameterRef, TypeId>,
        partitions: &BTreeMap<ParameterRef, Identity>,
    ) -> TypeId {
        let root = map_brand_origins(&mut self.out.body.types, root, |origin, depth| {
            let OriginRoot::Parameter(ParameterRef::Bound {
                depth: d,
                parameter,
            }) = origin.root
            else {
                return Ok(origin.clone());
            };
            if d < depth || origin.fields.is_empty() {
                return Ok(origin.clone());
            }
            Ok(partitions
                .get(&ParameterRef::Bound {
                    depth: d - depth,
                    parameter,
                })
                .map(|identity| identity.at(&origin.fields.iter().copied().collect::<Vec<_>>()))
                .unwrap_or_else(|| origin.clone()))
        })
        .expect("checked field-prefix selectors");
        super::facts::origins(&mut self.out.body.types, root, arguments, actuals)
    }
    /// Mutating any element ends evidence on its enclosing sequence. Value
    /// identities still distinguish indexed reads from their storage owner.
    pub(super) fn mutation_identity(&self, expression: ExprId) -> Option<Origin> {
        Some(self.place_partition(expression)?.whole)
    }
    pub(super) fn identity(&self, e: ExprId) -> Option<Origin> {
        if let Some(o) = self.expression_origins.get(&e) {
            return Some(o.clone());
        }
        let place = self.source_place(e);
        if place.access == SourceAccess::Unique {
            return Some(Origin {
                root: OriginRoot::Expression(e),
                fields: vec![].into(),
            });
        }
        if !place
            .projections
            .iter()
            .any(|p| matches!(p.kind, SourceProjectionKind::Index(_)))
        {
            return Some(self.value_partition(e)?.whole);
        }
        let mut origin = self.place_root_identity(e)?;
        for step in &place.projections {
            match step.kind {
                SourceProjectionKind::Field(field)
                | SourceProjectionKind::VariantField { field, .. } => origin.fields.push(field),
                // Indexed value identities stay independent of the sequence's
                // storage identity; mutation expiry has its own interpretation.
                SourceProjectionKind::Index(_) => {
                    origin = Origin {
                        root: OriginRoot::Expression(step.expression),
                        fields: vec![].into(),
                    }
                }
            }
        }
        Some(origin)
    }
    pub(super) fn arena_construction(&mut self, expression: ExprId) {
        let Some(t) = self.ty(expression) else { return };
        if self
            .out
            .body
            .types
            .get(t)
            .unwrap()
            .modes
            .relation
            .reaches()
            .iter()
            .any(|r| matches!(r, Reach::Place(o) if self.arena_place(o)))
            && self.contains_resource(t)
        {
            let arena = self
                .named_arena(&self.out.body.types.get(t).unwrap().modes.relation)
                .and_then(|origin| self.origin_site(&origin).1)
                .unwrap_or(self.span(expression));
            self.explained(
                const { RuleId::of("MEM-4") },
                self.span(expression),
                "an arena construction cannot hold resources",
                Explanation::default()
                    .provided(Fact::new(Cause::Resource, self.span(expression), &[]))
                    .required(Fact::new(Requirement::NoResources, arena, &[])),
            );
        }
    }
    /// Record `e`'s partition as `value`'s. Partitions stay out of the
    /// recursive walk's frame, whose depth the compiler stack budget bounds.
    pub(super) fn keep_partition(&mut self, e: ExprId, value: ExprId) {
        if let Some(partition) = self.value_partition(value) {
            self.expression_partitions.insert(e, partition);
        }
    }
    // Only the selected variant's fields carry value provenance. Generic
    // arguments describing absent fields are contracts, not fabricated values.
    pub(super) fn brandless_variant(
        &mut self,
        expression: ExprId,
        expected: TypeId,
    ) -> Option<ExprId> {
        if !matches!(
            self.out.body.types.get(expected)?.kind,
            TypeKind::Named { .. } | TypeKind::Variant(_)
        ) {
            return None;
        }
        let leaf = match self.tree.expressions[expression.0 as usize].kind.clone() {
            ExprKind::Block {
                tail: Some(tail), ..
            } => return self.brandless_variant(tail, expected),
            ExprKind::Unary {
                op: UnaryOp::Take | UnaryOp::Copy,
                value,
            } => return self.brandless_variant(value, expected),
            ExprKind::Path(_) => {
                let binding = self.binding(expression)?;
                if binding.mutable {
                    return None;
                }
                let NameSite::Pattern(pattern) = binding.site else {
                    return None;
                };
                let value =
                    self.tree
                        .statements
                        .iter()
                        .find_map(|statement| match statement.kind {
                            StmtKind::Let {
                                pattern: candidate,
                                value,
                                ..
                            } if candidate == pattern => Some(value),
                            _ => None,
                        })?;
                return self.brandless_variant(value, expected);
            }
            ExprKind::Call { .. } => expression,
            _ => return None,
        };
        if !matches!(
            self.call_row(leaf)?.target,
            CallTarget::Constructor(ContextualName::Variant { .. })
        ) {
            return None;
        }
        let fields = self.constructor_fields(leaf, expected)?;
        let mut pending: Vec<_> = fields.iter().map(|field| field.ty).collect();
        let mut seen = BTreeSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let row = self.out.body.types.get(ty)?;
            if !row.brand.alternatives.is_empty() {
                return None;
            }
            pending.extend(row.kind.value_parts());
        }
        Some(leaf)
    }
    pub(super) fn join_expression(&mut self, e: ExprId, branches: &[ExprId]) {
        let Some(id) = self.ty(e) else { return };
        let branches: Vec<_> = branches
            .iter()
            .copied()
            .filter(|e| self.falls_through(*e))
            .collect();
        let present: Vec<_> = branches
            .iter()
            .filter_map(|branch| {
                if self.brandless_variant(*branch, id).is_some() {
                    None
                } else {
                    self.ty(*branch)
                }
            })
            .collect();
        let all: Vec<_> = branches.iter().filter_map(|e| self.ty(*e)).collect();
        if all.is_empty() {
            return;
        }
        // Retain all root modes/brands; only phantom payload arguments receive
        // the constraints of the paths where that payload actually exists.
        let payload = self.join_types(id, if present.is_empty() { &all } else { &present });
        let mut contextual = Vec::new();
        for branch in branches {
            let ty = self.ty(branch).unwrap();
            if let Some(leaf) = self.brandless_variant(branch, payload) {
                let leaf_ty = self.ty(leaf).unwrap();
                let refined = self.part_brands(leaf_ty, payload);
                self.set_ty(leaf, refined);
                let refined = self.part_brands(ty, payload);
                self.set_ty(branch, refined);
                contextual.push(refined);
            } else {
                contextual.push(ty);
            }
        }
        let ty = self.join_types(id, &contextual);
        self.set_ty(e, ty);
    }
    pub(super) fn join_types(&mut self, base: TypeId, ids: &[TypeId]) -> TypeId {
        let mut t = self.out.body.types.get(base).unwrap().clone();
        let ts: Vec<_> = ids
            .iter()
            .filter_map(|id| self.out.body.types.get(*id))
            .cloned()
            .collect();
        // A branch joins the environments actually produced on its continuing
        // paths, never H's uninstantiated callee-relative result selector.
        if let Some(first) = ts.first() {
            t.erased_captures = if ts
                .iter()
                .all(|row| row.erased_captures == first.erased_captures)
            {
                first.erased_captures.clone()
            } else if ts
                .iter()
                .all(|row| matches!(row.erased_captures, Some(ErasedCaptures::Known(_))))
            {
                Some(ErasedCaptures::Known(ts.iter().fold(
                    AuthoritySummary::inert(),
                    |authority, row| {
                        authority.join(row.erased_captures.as_ref().unwrap().authority())
                    },
                )))
            } else {
                None
            };
        }
        // An unbranded path gives no identity: the join can promise none. A
        // brand may be forgotten, never invented for the unbranded value.
        t.brand = if ts.iter().any(|t| t.brand.alternatives.is_empty()) {
            Brand::default()
        } else {
            ts.iter()
                .fold(Brand::default(), |b, t| brand_join(&b, &t.brand))
        };
        let modes = ts.iter().fold(
            Modes {
                relation: Relation::global(),
                sendability: Sendability::Sendable,
                ..t.modes.clone()
            },
            |modes, row| modes.join(&row.modes),
        );
        // J's declared-row join widens conditional facts even when equal.
        // An empty join preserves the body checker's original relation.
        // PROV-4 (v22 R3): a join holds the union of its arms' placements, so
        // arena content joined with global content stays in the arena.
        if !ts.is_empty() {
            let placement = self.placement(&modes.relation);
            t.modes.relation = if self.lifetime(&modes.relation).is_global() {
                placement
            } else {
                Relation::environment().union(&placement)
            };
        }
        t.modes.uniqueness = modes.uniqueness;
        t.modes.multiplicity = modes.multiplicity;
        if modes.sendability != Sendability::Sendable {
            t.modes.sendability = Sendability::NotSendable;
        }
        let edges: Vec<_> = ts.iter().map(|t| t.kind.value_parts()).collect();
        let mut edge_index = 0;
        let mut join = |id| {
            let ids: Vec<_> = edges
                .iter()
                .filter_map(|e| e.get(edge_index).copied())
                .collect();
            edge_index += 1;
            self.join_types(id, &ids)
        };
        match &mut t.kind {
            TypeKind::Tuple(ts) => {
                for t in ts {
                    *t = join(*t)
                }
            }
            TypeKind::Record(fs) => {
                for f in fs {
                    f.ty = join(f.ty)
                }
            }
            TypeKind::Variant(vs) => {
                for v in vs {
                    for f in &mut v.fields {
                        f.1 = join(f.1)
                    }
                }
            }
            TypeKind::Box(t) => *t = join(*t),
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                for a in arguments {
                    if let GenericArgument::Type(t) = a {
                        *t = join(*t)
                    }
                }
            }
            TypeKind::Function(f) => {
                for t in &ts {
                    if let TypeKind::Function(other) = &t.kind {
                        f.authority = f.authority.clone().join(other.authority.clone());
                        if other.allocation == Allocation::MayAllocate {
                            f.allocation = Allocation::MayAllocate;
                        }
                        f.uses = match (&f.uses, &other.uses) {
                            (Uses::Only(a), Uses::Only(b)) => Uses::Only(a.clone().join(b.clone())),
                            _ => Uses::Unrestricted,
                        };
                    }
                }
            }
            _ => (),
        }
        let mut result = self.out.body.types.push(t);
        // The rule holds at every depth: an aggregate holding a part that lost
        // its scoped placement is as local as that part.
        if !self
            .lifetime(&self.out.body.types.get(result).unwrap().modes.relation)
            .has_environment()
            && ids.iter().any(|id| self.unplaced_scoped(result, *id))
        {
            let mut row = self.out.body.types.get(result).unwrap().clone();
            row.modes.relation =
                Relation::environment().union(&self.placement(&row.modes.relation));
            result = self.out.body.types.push(row);
        }
        if let Some(span) = ids.iter().find_map(|id| self.invalid.get(id)).copied() {
            self.invalid.insert(result, span);
        }
        result
    }
    /// Agreeing aliases survive; a divergent binding starts a fresh join identity.
    pub(super) fn join_identities(
        &mut self,
        site: NameSite,
        before: &Identities,
        branch: &Identities,
    ) {
        let left = self.identities.clone();
        let changed = before.changed_keys(&[&left, branch]);
        self.identities.retain_agreeing(branch);
        let span = match site {
            NameSite::Expression(e) => self.span(e),
            NameSite::Statement(s) => self.tree.statements[s.0 as usize].origin.span,
            _ => unreachable!("control join"),
        };
        for key in changed.iter().filter(|key| before.get(key).is_some()) {
            let Some(binding) = self
                .out
                .body
                .bindings
                .iter()
                .find(|b| {
                    work::count(Counter::AuthoritySourceRows);
                    binding_key(b) == *key
                })
                .cloned()
            else {
                continue;
            };
            let root = match binding.binding {
                ResolvedName::Local(l) => OriginRoot::Local(l),
                ResolvedName::Parameter(p) => {
                    OriginRoot::Parameter(ParameterRef::body(binding.owner.clone(), p))
                }
                _ => continue,
            };
            let fresh = Origin {
                root,
                fields: vec![].into(),
            };
            let fallback = Identity::new(fresh.clone());
            let a = left.get(key).unwrap_or(&fallback);
            let b = branch.get(key).unwrap_or(&fallback);
            let held = a.held.union(&b.held).cloned().collect();
            if a.whole == b.whole
                && self
                    .identity_paths
                    .iter()
                    .all(|path| a.at(path) == b.at(path))
            {
                self.identities
                    .insert(key.clone(), Identity { held, ..a.clone() });
            } else {
                self.invalidate_origin(&fresh, span);
                self.identity_changes.insert(fresh.clone(), span);
                let parts = self
                    .identity_paths
                    .iter()
                    .map(|path| {
                        let left = a.at(path);
                        let right = b.at(path);
                        let origin = if left == right {
                            left
                        } else {
                            let mut o = fresh.clone();
                            o.fields.extend(path);
                            o
                        };
                        (path.clone(), origin)
                    })
                    .collect();
                let changed_paths: Vec<_> = self
                    .identity_paths
                    .iter()
                    .filter(|path| a.at(path) != b.at(path))
                    .cloned()
                    .collect();
                self.identities.insert(
                    key.clone(),
                    Identity {
                        whole: fresh.clone(),
                        parts,
                        held,
                    },
                );
                for path in changed_paths {
                    let mut ty = binding.ty;
                    let mut prefix = vec![];
                    let mut projections = vec![];
                    for field in &path {
                        let Some(fields) = self.declared_fields(ty) else {
                            break;
                        };
                        let Some(part) = fields.get(field.0 as usize) else {
                            break;
                        };
                        let partition = self.identities.get(key).unwrap().clone();
                        let siblings = fields
                            .iter()
                            .enumerate()
                            .map(|(ordinal, _)| {
                                let mut p = prefix.clone();
                                p.push(FieldId(ordinal as u32));
                                (FieldId(ordinal as u32), partition.at(&p))
                            })
                            .collect();
                        let part =
                            super::facts::siblings(&mut self.out.body.types, part.ty, &siblings);
                        // I places the declared part under the binding's final row.
                        ty = self.projected_item(ty, part);
                        prefix.push(*field);
                        projections.push((*field, part));
                    }
                    if projections.len() == path.len() {
                        let mut origin = fresh.clone();
                        origin.fields.extend(&path);
                        self.out.identity_fields.retain(|(at, owner, old, _)| {
                            *at != site || *owner != binding.owner || *old != origin
                        });
                        self.out.identity_fields.push((
                            site.clone(),
                            binding.owner,
                            origin,
                            projections,
                        ));
                    }
                }
                self.out
                    .identity_joins
                    .push((site.clone(), binding.owner, fresh));
            }
        }
    }
    pub(super) fn expired_type(&self, root: TypeId) -> bool {
        self.ended(root).is_some()
    }
    /// Where an origin named anywhere in this value's type ended.
    pub(super) fn ended(&self, root: TypeId) -> Option<Span> {
        let mut todo = vec![root];
        let mut seen = BTreeSet::new();
        while let Some(ty) = todo.pop() {
            if !seen.insert(ty) {
                continue;
            }
            if let Some(span) = self.invalid.get(&ty) {
                return Some(*span);
            }
            if let Some(row) = self.out.body.types.get(ty) {
                todo.extend(row.kind.value_parts());
            }
        }
        None
    }
    /// `actual` with the brands of `expected`'s parts at every depth where
    /// the two have the same shape; the root brand stays `actual`'s.
    pub(super) fn part_brands(&mut self, actual: TypeId, expected: TypeId) -> TypeId {
        let (Some(a), Some(b)) = (
            self.out.body.types.get(actual).cloned(),
            self.out.body.types.get(expected).cloned(),
        ) else {
            return actual;
        };
        if std::mem::discriminant(&a.kind) != std::mem::discriminant(&b.kind) {
            return actual;
        }
        let (ap, bp) = (a.kind.value_parts(), b.kind.value_parts());
        if ap.len() != bp.len() || ap.is_empty() {
            return actual;
        }
        let mut changed = false;
        let mut map = BTreeMap::new();
        for (part, contract) in ap.iter().zip(&bp) {
            let mut branded = self.part_brands(*part, *contract);
            let row = self.out.body.types.get(branded).unwrap();
            let brand = &self.out.body.types.get(*contract).unwrap().brand;
            if row.brand != *brand {
                let mut row = row.clone();
                row.brand = brand.clone();
                branded = self.out.body.types.push(row);
            }
            changed |= branded != *part;
            map.insert(*part, branded);
        }
        if !changed {
            return actual;
        }
        let mut row = a;
        let swap = |t: &mut TypeId| *t = map.get(t).copied().unwrap_or(*t);
        match &mut row.kind {
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                let mut list: Vec<_> = arguments.iter().cloned().collect();
                for argument in &mut list {
                    if let GenericArgument::Type(t) = argument {
                        swap(t);
                    }
                }
                *arguments = list.into();
            }
            TypeKind::Tuple(parts) => {
                let mut list: Vec<_> = parts.iter().copied().collect();
                list.iter_mut().for_each(swap);
                *parts = list.into();
            }
            TypeKind::Box(part) => swap(part),
            _ => return actual,
        }
        self.out.body.types.push(row)
    }
    /// `instance` with `source`'s single brands wherever it has none yet, at
    /// every depth where the two have the same shape.
    pub(super) fn adopt_brands(&mut self, instance: TypeId, source: TypeId) -> TypeId {
        let (Some(a), Some(b)) = (
            self.out.body.types.get(instance).cloned(),
            self.out.body.types.get(source).cloned(),
        ) else {
            return instance;
        };
        if std::mem::discriminant(&a.kind) != std::mem::discriminant(&b.kind) {
            return instance;
        }
        let mut row = a.clone();
        if row.brand.alternatives.is_empty() && b.brand.alternatives.len() == 1 {
            row.brand = b.brand.clone();
        }
        let (ap, bp) = (a.kind.value_parts(), b.kind.value_parts());
        if ap.len() == bp.len() && !ap.is_empty() {
            let mut map = BTreeMap::new();
            for (part, source) in ap.iter().zip(&bp) {
                map.insert(*part, self.adopt_brands(*part, *source));
            }
            let swap = |t: &mut TypeId| *t = map.get(t).copied().unwrap_or(*t);
            match &mut row.kind {
                TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                    let mut list: Vec<_> = arguments.iter().cloned().collect();
                    for argument in &mut list {
                        if let GenericArgument::Type(t) = argument {
                            swap(t);
                        }
                    }
                    *arguments = list.into();
                }
                TypeKind::Tuple(parts) => {
                    let mut list: Vec<_> = parts.iter().copied().collect();
                    list.iter_mut().for_each(swap);
                    *parts = list.into();
                }
                TypeKind::Box(part) => swap(part),
                _ => {}
            }
        }
        if row == a {
            instance
        } else {
            self.out.body.types.push(row)
        }
    }
    pub(super) fn placed_item(&mut self, owner: TypeId, part: TypeId) -> TypeId {
        let mut store = std::mem::take(&mut self.out.body.types);
        let generics = self.declarations();
        let part = placed_part_type(&mut store, owner, part, &generics, &mut |path| {
            Ok::<_, hirsel_types::QueryError>(self.read(path))
        })
        .expect("checked projection graph");
        self.out.body.types = store;

        part
    }
    pub(super) fn projected_item(&mut self, owner: TypeId, part: TypeId) -> TypeId {
        let part = self.placed_item(owner, part);
        let mut store = std::mem::take(&mut self.out.body.types);
        let generics = self.declarations();
        let part = projected_type(&mut store, owner, part, &generics, &mut |path| {
            Ok::<_, hirsel_types::QueryError>(self.read(path))
        })
        .expect("checked projection graph");
        self.out.body.types = store;
        part
    }
    /// A projection used as another field's declared brand is an indivisible owner.
    pub(super) fn protected_field(&mut self, mut e: ExprId) -> bool {
        while let ExprKind::Unary { value, .. } = self.tree.expressions[e.0 as usize].kind {
            e = value;
        }
        let Some(origin) = self.identity(e) else {
            return false;
        };
        let Some(field) = origin.fields.last() else {
            return false;
        };
        let Some(binding) =
            self.binding(e)
                .or_else(|| match self.tree.expressions[e.0 as usize].kind {
                    ExprKind::Field { base, .. } => self.binding(base),
                    _ => None,
                })
        else {
            return false;
        };
        let Some(ty) = self.binding_types.get(&binding_key(&binding)).copied() else {
            return false;
        };
        let Some(fields) = self.fields(ty) else {
            return false;
        };
        fields.iter().any(|f| {
            self.out.body.types.get(f.ty).is_some_and(|t| {
                t.brand
                    .alternatives
                    .iter()
                    .any(|o| o.root == OriginRoot::Sibling(*field))
            })
        })
    }

    /// A body's temporaries end before its result reaches the caller, so the
    /// result cannot be branded by one of them (spec issue #60).
    pub(super) fn frame_temporaries(&mut self, body: ExprId, from: usize) {
        let span = self.span(body);
        self.end_temporaries(from, span);
        if self.valid(body) && self.ty(body).and_then(|ty| self.ended(ty)) == Some(span) {
            self.ended_use(
                self.result_span(body),
                span,
                "the result is branded by a temporary that ends with the function",
            );
        }
    }
    pub(super) fn invalidate(&mut self, e: ExprId, span: Span, mutation: bool) {
        if mutation
            && self.ty(e).is_some_and(|t| {
                matches!(
                    self.out.body.types.get(t).unwrap().kind,
                    TypeKind::Builtin {
                        kind: BuiltinType::Slots,
                        ..
                    }
                )
            })
        {
            return;
        }
        let iteration_source = match &self.source_place(e).root {
            SourceRoot::Binding { owner, binding } => self
                .root_binding(*owner, binding)
                .and_then(|binding| self.iteration_sources.get(&binding_key(binding)))
                .copied(),
            _ => None,
        };
        if let Some(source) = iteration_source {
            // Renew the storage owner through its ordinary copy-detachment
            // rules as well as the yielded value's independent name below.
            self.invalidate(source, span, true);
        }
        if let Some(o) = self.mutation_identity(e) {
            let binding = match &self.source_place(e).root {
                SourceRoot::Binding { owner, binding } => {
                    self.root_binding(*owner, binding).cloned()
                }
                _ => None,
            };
            if let Some(b) = binding {
                let own_root = match b.binding {
                    ResolvedName::Local(l) => OriginRoot::Local(l),
                    ResolvedName::Parameter(p) => {
                        OriginRoot::Parameter(ParameterRef::body(b.owner, p))
                    }
                    _ => o.root.clone(),
                };
                if b.usage != UseKind::Mut {
                    let key = binding_key(&b);
                    let old = self.identities.get(&key).cloned().unwrap_or_else(|| {
                        Identity::new(Origin {
                            root: own_root.clone(),
                            fields: vec![].into(),
                        })
                    });
                    let path: Vec<_> = self
                        .source_place(e)
                        .projections
                        .iter()
                        .map_while(|p| match p.kind {
                            SourceProjectionKind::Field(f)
                            | SourceProjectionKind::VariantField { field: f, .. } => Some(f),
                            SourceProjectionKind::Index(_) => None,
                        })
                        .collect();
                    // A transferred owner ends its old prefix. A copied owner
                    // detaches without expiring the original value's brands.
                    if b.usage == UseKind::Take {
                        self.invalidate_origin(&o, span);
                        // So do the owners moved into the transferred part.
                        for held in old.held_under(&path).cloned().collect::<Vec<_>>() {
                            self.invalidate_origin(&held, span);
                        }
                        // A written part's brand names the binding itself
                        // (#78); the transfer ends that name too (MEM-14).
                        let mut named = Origin {
                            root: own_root.clone(),
                            fields: vec![].into(),
                        };
                        named.fields.extend(&path);
                        self.invalidate_origin(&named, span);
                        // A written part a brand names, and such descendants,
                        // take the binding's own name, as lowering binds
                        // them; the whole keeps its transferred identity (#78).
                        if !path.is_empty() {
                            let parts = self
                                .identity_paths
                                .iter()
                                .map(|p| {
                                    let value =
                                        if p.starts_with(&path) && self.branded_paths.contains(p) {
                                            let mut o = Origin {
                                                root: own_root.clone(),
                                                fields: vec![].into(),
                                            };
                                            o.fields.extend(p);
                                            o
                                        } else {
                                            old.at(p)
                                        };
                                    (p.clone(), value)
                                })
                                .collect();
                            self.identities.insert(
                                key,
                                Identity {
                                    whole: old.whole.clone(),
                                    parts,
                                    held: old.held_beside(&path),
                                },
                            );
                        }
                        return;
                    }
                    let fresh = Origin {
                        root: own_root,
                        fields: vec![].into(),
                    };
                    let parts = self
                        .identity_paths
                        .iter()
                        .map(|p| {
                            let value = if p.starts_with(&path) || path.starts_with(p) {
                                let mut o = fresh.clone();
                                o.fields.extend(p);
                                o
                            } else {
                                old.at(p)
                            };
                            (p.clone(), value)
                        })
                        .collect();
                    self.identities.insert(
                        key,
                        Identity {
                            whole: fresh.clone(),
                            parts,
                            held: old.held_beside(&path),
                        },
                    );
                    let mut changed = fresh;
                    changed.fields.extend(&path);
                    self.invalidate_origin(&changed, span);
                    self.identity_changes.insert(changed, span);
                    return;
                }
            }
            self.invalidate_origin(&o, span);
        }
    }
    fn index_brand_inputs(&mut self) {
        // Immutable published rows retain their prefix through every append-only
        // working graph replacement. Index each new row exactly once.
        for (id, row) in self.out.body.types.iter().skip(self.indexed_brand_rows) {
            work::count(Counter::AuthorityInvalidationRows);
            let roots: BTreeSet<_> = row
                .brand
                .alternatives
                .iter()
                .map(|origin| origin.root.clone())
                .collect();
            for root in roots {
                self.brand_inputs.entry(root).or_default().push(id);
            }
        }
        self.indexed_brand_rows = self.out.body.types.len();
    }
    pub(super) fn invalidate_origin(&mut self, origin: &Origin, span: Span) {
        self.index_brand_inputs();
        let candidates = self
            .brand_inputs
            .get(&origin.root)
            .cloned()
            .unwrap_or_default();
        for id in candidates {
            let ty = self.out.body.types.get(id).unwrap();
            work::count(Counter::AuthorityInvalidationRows);
            if ty.brand.alternatives.iter().any(|o| {
                o.root == origin.root
                    && (o.fields.starts_with(&origin.fields)
                        || origin.fields.starts_with(&o.fields))
            }) {
                self.invalid.insert(id, span);
            }
        }
        // A mut parameter is handed back to the caller, never dropped here,
        // so the operation that ends a brand it carries is the failed use.
        let returned: Vec<_> = self
            .mut_parameters
            .iter()
            .cloned()
            .filter(|key| {
                work::count(Counter::AuthorityInvalidationRows);
                !self.returned_ended.contains(key)
            })
            .filter(|key| {
                self.binding_types
                    .get(key)
                    .is_some_and(|ty| self.ended(*ty) == Some(span))
            })
            .collect();
        for key in returned {
            self.returned_ended.insert(key);
            self.ended_use(
                span,
                span,
                "this ends a brand carried by a mut parameter, which its caller receives back",
            );
        }
    }
    /// A temporary lives to the end of its statement. One that lent its
    /// identity to a brand and was not kept by a binding ends there.
    pub(super) fn end_temporaries(&mut self, from: usize, span: Span) {
        for root in self.temporaries.split_off(from) {
            if !self.identities.held(&root) {
                self.invalidate_origin(
                    &Origin {
                        root,
                        fields: vec![].into(),
                    },
                    span,
                );
            }
        }
    }

    /// OWN-8/OWN-1: a non-plain part of a borrowed capture is in the
    /// capture's locality, as a part of a borrowed parameter is.
    pub(super) fn localize_view_part(&mut self, expression: ExprId, root: Option<BindingKey>) {
        let view = root.is_some_and(|key| self.capture_views.contains(&key));
        let Some(ty) = self.ty(expression) else {
            return;
        };
        if !view
            || self
                .lifetime(&self.out.body.types.get(ty).unwrap().modes.relation)
                .is_environment()
            || self.property(ty, Bound::Plain)
        {
            return;
        }
        let mut row = self.out.body.types.get(ty).unwrap().clone();
        row.modes.relation = Relation::environment().union(&self.placement(&row.modes.relation));
        let local = self.out.body.types.push(row);
        self.set_ty(expression, local);
    }
    pub(super) fn refine_projection(&mut self, expression: ExprId, declared: TypeId) {
        let Some(actual) = self.ty(expression) else {
            return;
        };
        let mut row = self.out.body.types.get(actual).unwrap().clone();
        let provenance = self.out.body.types.get(declared).unwrap();
        if !provenance.brand.alternatives.is_empty() {
            row.brand = provenance.brand.clone();
        }
        let mut lifetime = self.lifetime(&row.modes.relation);
        let mut placement = self.placement(&row.modes.relation);
        if !self.placement(&provenance.modes.relation).is_global() {
            placement = self.placement(&provenance.modes.relation);
        }
        // H leaves dependent call locality symbolic until J instantiates the
        // receiver. A projection must resolve that removed call binder before
        // its occurrence can become an inferred aggregate child contract.
        if lifetime.named_parameters().is_some() {
            lifetime = self.lifetime(&provenance.modes.relation);
        }
        row.modes.relation = lifetime.union(&placement);
        // Retain the projected kind, other modes and cleanup evidence while
        // applying flow-sensitive provenance from the selected checked field.
        if self.out.body.types.get(actual) != Some(&row) {
            let refined = self.out.body.types.push(row);
            self.set_ty(expression, refined);
        }
    }
    pub(super) fn projection_fields(
        &mut self,
        root: TypeId,
        projection: &SourceProjectionKind,
    ) -> Option<(Vec<Field>, usize)> {
        match projection {
            SourceProjectionKind::Field(field) => Some((self.fields(root)?, field.0 as usize)),
            SourceProjectionKind::VariantField { variant, field } => {
                let payload = match &self.out.body.types.get(root)?.kind {
                    TypeKind::Builtin {
                        kind: BuiltinType::Packed,
                        arguments,
                    } => match arguments.first()? {
                        GenericArgument::Type(t) => *t,
                        _ => return None,
                    },
                    _ => root,
                };
                let TypeKind::Variant(variants) = self.layout(payload)? else {
                    return None;
                };
                let fields = variants
                    .get(variant.0 as usize)?
                    .fields
                    .iter()
                    .map(|(name, ty)| Field {
                        name: name.clone().unwrap_or_default(),
                        ty: self.placed_item(root, *ty),
                        growable: false,
                    })
                    .collect();
                Some((fields, field.0 as usize))
            }
            SourceProjectionKind::Index(_) => None,
        }
    }
    pub(super) fn fields(&mut self, root: TypeId) -> Option<Vec<Field>> {
        let mut fields = self.declared_fields(root)?;
        for field in &mut fields {
            field.ty = self.placed_item(root, field.ty);
        }
        Some(fields)
    }
    /// The fields as declared, before placement under `root`.
    pub(super) fn declared_fields(&mut self, root: TypeId) -> Option<Vec<Field>> {
        let fields = match self.layout(root)? {
            TypeKind::Record(fields) => Some(fields),
            TypeKind::Tuple(ts) => Some(
                ts.into_iter()
                    .enumerate()
                    .map(|(i, ty)| Field {
                        name: i.to_string(),
                        ty,
                        growable: false,
                    })
                    .collect(),
            ),
            _ => None,
        }?;
        Some(fields.to_vec())
    }
    pub(super) fn layout(&mut self, root: TypeId) -> Option<TypeKind> {
        // Expansion appends to the body's store itself: a clone left behind
        // in the body would make the first append copy every row.
        let mut store = std::mem::take(&mut self.out.body.types);
        let result = expand_representation(
            &mut store,
            root,
            &mut |store, path, arguments| {
                let Some(signature) = self.read(path) else {
                    return Ok(None);
                };
                let authorities = arguments
                    .iter()
                    .map(|argument| match argument {
                        GenericArgument::Type(ty) => {
                            let summary = self.raw_authority(store, *ty);
                            self.scoped_authority_in(store, summary)
                        }
                        _ => AuthoritySummary::inert(),
                    })
                    .collect();
                Ok(Some((signature, authorities)))
            },
            None,
        );
        self.out.body.types = store;
        match result {
            Ok(root) => self.out.body.types.get(root).map(|row| row.kind.clone()),
            Err(RepresentationError {
                failure: RepresentationFailure::Query(error),
                ..
            }) => self.proof_failure(error, None),
            Err(_) => None,
        }
    }
    /// The origin the `index`th resolved path of a type site names, with the
    /// type of the value it names when that is a binding or one of its
    /// fields. A type site resolves its brand, then its relation places.
    fn typed_origin_path(
        &mut self,
        site: NameSite,
        path: &hirsel_syntax::Path,
        index: usize,
    ) -> Option<(Origin, Option<TypeId>)> {
        let resolved = self
            .out
            .body
            .resolution
            .paths
            .iter()
            .filter(|(s, _)| s == &site)
            .nth(index)
            .map(|(_, n)| n.clone());
        let owner = self
            .out
            .body
            .resolution
            .reference_scopes
            .iter()
            .filter(|(s, _)| s == &site)
            .nth(index)
            .map(|(_, o)| o.clone())
            .or_else(|| self.frames.last().map(|f| f.owner.clone()))?;
        let name = path
            .segments
            .first()
            .and_then(|name| self.symbols.resolve(*name))
            .map(str::to_owned);
        let binding = resolved
            .as_ref()
            .and_then(|n| {
                self.out.body.bindings.iter().find(|b| {
                    work::count(Counter::AuthoritySourceRows);
                    &b.binding == n && b.owner == owner
                })
            })
            .cloned()
            .or_else(|| {
                let name = name.as_ref()?;
                self.out
                    .body
                    .bindings
                    .iter()
                    .find(|b| match &b.binding {
                        ResolvedName::Parameter(p) => {
                            let syntax = match b.owner {
                                NameSite::Item(i) => {
                                    self.tree
                                        .items
                                        .get(i.0 as usize)
                                        .and_then(|i| match &i.kind {
                                            ItemKind::Function { signature, .. } => Some(signature),
                                            _ => None,
                                        })
                                }
                                NameSite::Expression(e) => {
                                    self.tree.expressions.get(e.0 as usize).and_then(|e| {
                                        match &e.kind {
                                            ExprKind::Closure { signature, .. } => Some(signature),
                                            _ => None,
                                        }
                                    })
                                }
                                NameSite::Statement(s) => {
                                    self.tree.statements.get(s.0 as usize).and_then(|s| {
                                        match &s.kind {
                                            StmtKind::Function { signature, .. } => Some(signature),
                                            _ => None,
                                        }
                                    })
                                }
                                _ => None,
                            };
                            syntax
                                .and_then(|s| s.parameters.get(p.0 as usize))
                                .and_then(|p| p.name)
                                .and_then(|p| self.symbols.resolve(p))
                                .is_some_and(|n| n == name)
                                && b.owner == owner
                        }
                        ResolvedName::Local(_) => match b.site {
                            NameSite::Pattern(p) => {
                                match self.tree.patterns.get(p.0 as usize).map(|p| &p.kind) {
                                    Some(hirsel_syntax::PatternKind::Binding(s)) => {
                                        self.symbols.resolve(*s).is_some_and(|n| n == name)
                                    }
                                    _ => false,
                                }
                            }
                            _ => false,
                        },
                        _ => false,
                    })
                    .cloned()
            });
        let (partition, mut ty) = if let Some(b) = binding {
            let root = match b.binding {
                ResolvedName::Parameter(p) => OriginRoot::Parameter(ParameterRef::body(b.owner, p)),
                ResolvedName::Local(l) => OriginRoot::Local(l),
                _ => return None,
            };
            (
                self.identities
                    .get(&binding_key(&b))
                    .cloned()
                    .unwrap_or_else(|| {
                        Identity::new(Origin {
                            root,
                            fields: vec![].into(),
                        })
                    }),
                Some(b.ty),
            )
        } else if let Some(ResolvedName::Definition(d)) = resolved {
            (
                Identity::new(Origin {
                    root: OriginRoot::Definition(d),
                    fields: vec![].into(),
                }),
                None,
            )
        } else {
            return None;
        };
        let mut path_fields = vec![];
        for field in path.segments.iter().skip(1) {
            let name = self.symbols.resolve(*field)?.to_owned();
            let fields = self.fields(ty?)?;
            let ordinal = fields.iter().position(|f| f.name == name)?;
            path_fields.push(FieldId(ordinal as u32));
            ty = Some(fields[ordinal].ty);
        }
        let origin = partition.at(&path_fields);
        Some((origin, ty))
    }
    pub(super) fn annotation(&mut self, ty: TypeId, syntax: hirsel_syntax::TypeExprId) -> TypeId {
        let node = self.tree.types[syntax.0 as usize].kind.clone();
        let mut row = self.out.body.types.get(ty).unwrap().clone();
        row.sendability_requirement = SendabilityRequirement::None;
        if matches!(node, hirsel_syntax::TypeExprKind::Named { .. }) {
            let declaration = self
                .out
                .body
                .resolution
                .types
                .iter()
                .find(|(id, _)| *id == syntax)
                .and_then(|(_, name)| match name {
                    ResolvedName::Definition(path) => Some(path.clone()),
                    _ => None,
                });
            if let Some(signature) = declaration.and_then(|path| self.read(&path)) {
                if signature.metadata.alias {
                    if let SignatureKind::Type(declaration) = &signature.kind {
                        row.sendability_requirement = declaration
                            .representation
                            .and_then(|root| signature.types.get(root))
                            .map_or(SendabilityRequirement::None, |row| {
                                row.sendability_requirement
                            });
                    }
                }
            }
        }
        let ty = self.out.body.types.push(row);
        match node {
            hirsel_syntax::TypeExprKind::Function(signature) => {
                // #58: a written function type is preserved exactly, at any
                // depth. The row it refines is the initializer's, which may
                // carry derived evidence (`uses {}`, `@noalloc`) the
                // annotation does not promise, or lack what it does.
                let mut row = self.out.body.types.get(ty).unwrap().clone();
                let TypeKind::Function(function) = &mut row.kind else {
                    return ty;
                };
                let mut function = function.clone();
                let parameters: Vec<_> = function
                    .parameters
                    .iter()
                    .cloned()
                    .zip(&signature.parameters)
                    .map(|(mut parameter, written)| {
                        parameter.ty = self.annotation(parameter.ty, written.ty);
                        parameter
                    })
                    .collect();
                if parameters.len() == function.parameters.len() {
                    function.parameters = parameters.into();
                }
                if let Some(result) = signature.result {
                    function.result = self.annotation(function.result, result);
                }
                // CLO-4: a written binding type supplies the escaping
                // context and its declared capture-authority restriction.
                match &signature.uses {
                    Some(paths) => {
                        let uses = self.syntax_uses(NameSite::Type(syntax), paths);
                        if let Uses::Only(authority) = &uses {
                            function.authority = authority.clone();
                        }
                        function.uses = uses;
                    }
                    None => {
                        function.uses = Uses::Unrestricted;
                        function.authority = AuthoritySummary::Unknown;
                    }
                }
                function.allocation = if signature
                    .attributes
                    .iter()
                    .any(|a| self.symbols.resolve(a.name) == Some("noalloc"))
                {
                    Allocation::NoAlloc
                } else {
                    Allocation::MayAllocate
                };
                // MODE-4: it crosses tasks only where it is written
                // `sendable`, which the qualifier around it restores.
                row.modes.sendability = Sendability::NotSendable;
                row.kind = TypeKind::Function(function);
                self.out.body.types.push(row)
            }
            hirsel_syntax::TypeExprKind::Named {
                arguments: syntax_arguments,
                ..
            } => {
                let mut row = self.out.body.types.get(ty).unwrap().clone();
                // MODE-4: a written erased-state type, like a written callable,
                // keeps its declared environment promise rather than the
                // initializer's stronger summary; only `sendable` restores it.
                if matches!(
                    row.kind,
                    TypeKind::Builtin {
                        kind: BuiltinType::Iter,
                        ..
                    }
                ) {
                    row.modes.sendability = Sendability::NotSendable;
                }
                match &mut row.kind {
                    TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                        let mut refined = arguments.iter().cloned().collect::<Vec<_>>();
                        for (actual, syntax) in refined.iter_mut().zip(syntax_arguments) {
                            if let (
                                GenericArgument::Type(t),
                                hirsel_syntax::TypeArgument::Type(s),
                            ) = (actual, syntax)
                            {
                                *t = self.annotation(*t, s);
                            }
                        }
                        *arguments = refined.into();
                    }
                    _ => (),
                }
                self.out.body.types.push(row)
            }
            hirsel_syntax::TypeExprKind::Tuple(syntax) => {
                let mut row = self.out.body.types.get(ty).unwrap().clone();
                if let TypeKind::Tuple(parts) = &mut row.kind {
                    *parts = parts
                        .iter()
                        .copied()
                        .zip(syntax)
                        .map(|(t, s)| self.annotation(t, s))
                        .collect::<Vec<_>>()
                        .into();
                }
                self.out.body.types.push(row)
            }
            hirsel_syntax::TypeExprKind::Box(syntax) => {
                let mut row = self.out.body.types.get(ty).unwrap().clone();
                if let TypeKind::Box(part) = &mut row.kind {
                    *part = self.annotation(*part, syntax);
                }
                self.out.body.types.push(row)
            }
            hirsel_syntax::TypeExprKind::Qualified {
                base,
                modifiers,
                brand,
                relation,
            } => {
                let ty = self.annotation(ty, base);
                let mut row = self.out.body.types.get(ty).unwrap().clone();
                let site = NameSite::Type(syntax);
                let mut index = 0;
                if let Some(b) = brand.as_ref().filter(|b| !b.sibling) {
                    if let Some((o, _)) = self.typed_origin_path(site.clone(), &b.path, 0) {
                        row.brand = Brand {
                            alternatives: vec![o].into(),
                        };
                    }
                    index = 1;
                }
                if let Some(relation) = relation {
                    // An arena names where blocks live (PROV-4); other places
                    // bound the lifetime, which the binding's contract holds.
                    let mut placed = vec![];
                    for place in &relation.places {
                        let hirsel_syntax::RelationPlace::Path(path) = place else {
                            continue;
                        };
                        let named = self.typed_origin_path(site.clone(), path, index);
                        index += 1;
                        let arena = self.signature.metadata.roles.heap.as_ref();
                        if let Some((o, Some(ty))) = named {
                            if o.fields.is_empty()
                                && matches!(
                                    &self.out.body.types.get(ty).unwrap().kind,
                                    TypeKind::Named { definition, .. }
                                        if arena.is_some_and(|h| &h.arena == definition)
                                )
                            {
                                placed.push(o);
                            }
                        }
                    }
                    if relation.places == [hirsel_syntax::RelationPlace::Heap] || !placed.is_empty()
                    {
                        row.modes.relation = self.lifetime(&row.modes.relation);
                    }
                    for o in placed {
                        row.modes.relation = row.modes.relation.union(&Relation::place(o));
                    }
                }
                if modifiers.contains(&hirsel_syntax::TypeModifier::Once) {
                    row.modes.multiplicity = Multiplicity::Once
                }
                // MODE-4: a written `sendable` is the type's crossing promise,
                // which every value it receives must keep.
                if modifiers.contains(&hirsel_syntax::TypeModifier::Sendable) {
                    row.sendability_requirement = SendabilityRequirement::Sendable;
                }
                self.out.body.types.push(row)
            }
            _ => ty,
        }
    }
    /// A let annotation's row with the binding places its relation names,
    /// which bound what the binding receives (v22 R3, "within S"). Body rows
    /// lower them to the frame bound (`env`), which admits any arena. A field
    /// path `z.f` names its binding `z`. A relation naming anything other than
    /// bindings, their field paths and arenas keeps the frame bound alone.
    pub(super) fn within_bindings(
        &mut self,
        ty: TypeId,
        syntax: hirsel_syntax::TypeExprId,
    ) -> TypeId {
        let hirsel_syntax::TypeExprKind::Qualified {
            brand,
            relation: Some(relation),
            ..
        } = self.tree.types[syntax.0 as usize].kind.clone()
        else {
            return ty;
        };
        let site = NameSite::Type(syntax);
        let mut index = usize::from(brand.as_ref().is_some_and(|b| !b.sibling));
        let (mut places, mut bindings) = (Relation::global(), false);
        for place in &relation.places {
            let hirsel_syntax::RelationPlace::Path(path) = place else {
                return ty;
            };
            // A field path is a place of its binding: it is within what that
            // field holds (#82).
            let field = path.segments.len() > 1;
            let named = self
                .relation_binding(&site, index)
                .filter(|o| !field || self.binding_place(o))
                .filter(|_| !field || self.typed_origin_path(site.clone(), path, index).is_some())
                .and_then(|mut origin| {
                    if field {
                        let mut ty = self.origin_binding(&origin)?.ty;
                        let mut fields = vec![];
                        for segment in path.segments.iter().skip(1) {
                            let name = self.symbols.resolve(*segment)?.to_owned();
                            let declared = self.fields(ty)?;
                            let ordinal = declared.iter().position(|f| f.name == name)?;
                            fields.push(FieldId(ordinal as u32));
                            ty = declared[ordinal].ty;
                        }
                        origin.fields = fields.into();
                    }
                    Some(origin)
                });
            index += 1;
            match named {
                Some(o) if self.binding_place(&o) || self.arena_place(&o) => {
                    bindings |= self.binding_place(&o);
                    places = places.union(&Relation::place(o));
                }
                _ => return ty,
            }
        }
        if !bindings {
            return ty;
        }
        // The value is within S alone, not the initializer's placement.
        let mut row = self.out.body.types.get(ty).unwrap().clone();
        row.modes.relation = self.lifetime(&row.modes.relation).union(&places);
        self.out.body.types.push(row)
    }
    /// The binding the `index`th path written at `site` names, as a place.
    fn relation_binding(&self, site: &NameSite, index: usize) -> Option<Origin> {
        let resolution = &self.out.body.resolution;
        let name = resolution
            .paths
            .iter()
            .filter(|(s, _)| s == site)
            .nth(index)?;
        let owner = resolution
            .reference_scopes
            .iter()
            .filter(|(s, _)| s == site)
            .nth(index)
            .map(|(_, o)| o.clone())
            .or_else(|| self.frames.last().map(|f| f.owner.clone()))?;
        let binding = self
            .out
            .body
            .bindings
            .iter()
            .find(|b| b.binding == name.1 && b.owner == owner)?;
        let root = match binding.binding {
            ResolvedName::Parameter(p) => {
                OriginRoot::Parameter(ParameterRef::body(binding.owner, p))
            }
            ResolvedName::Local(l) => OriginRoot::Local(l),
            _ => return None,
        };
        Some(Origin {
            root,
            fields: vec![].into(),
        })
    }
}
