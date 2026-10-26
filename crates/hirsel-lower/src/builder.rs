use super::*;
use hirsel_syntax::ExprId;
use hirsel_tir::*;
use hirsel_types::{ResolvedName, SourceRoot, UseKind};
use std::collections::{BTreeMap, BTreeSet};

// Sparse content dependencies, keyed by storage root. Projections keep sibling
// holders separate; aliases retain the same physical content row.
type HeldLoans = Vec<(Vec<Projection>, BTreeSet<u32>)>;

#[derive(Clone, Default)]
pub(super) struct State {
    pub live: super::live::Live,
    pub loans: Vec<(LoanId, ScopeId)>,
    retained: BTreeMap<u32, HeldLoans>,
    views: Vec<(PlaceId, ScopeId)>,
}
#[derive(Clone, Copy)]
pub(super) struct Value {
    pub place: PlaceId,
    pub usage: UseKind,
    /// The expression judgment can differ from its source storage declaration.
    pub checked_ty: Option<TypeId>,
}
#[derive(Clone)]
pub(super) struct Loop {
    pub statement: hirsel_syntax::StmtId,
    pub next: BlockId,
    pub exit: BlockId,
    pub scope: ScopeId,
    pub exits: Vec<State>,
    pub feedback: Vec<State>,
}
/// A labelled block being lowered (LANG-11): its result slot, the scope it
/// opens inside `outer`, and the states its breaks reach `exit` with.
pub(super) struct LabelledBlock {
    pub block: ExprId,
    pub label: hirsel_intern::Symbol,
    pub destination: PlaceId,
    pub lent: Option<ScopeId>,
    pub outer: ScopeId,
    pub scope: Option<ScopeId>,
    pub exit: BlockId,
    pub exits: Vec<State>,
}
pub(super) struct Builder<'a, 'b> {
    pub input: &'b Input<'a>,
    pub decisions: &'b decisions::Decisions<'a>,
    pub owner: NameSite,
    pub body: Body,
    projection_paths: hirsel_types::PathInterner<Projection>,
    pub types: &'a hirsel_types::TypeInterner,
    pub current: Option<BlockId>,
    pub scope: ScopeId,
    pub state: State,
    pub bindings: super::bindings::Bindings,
    pub owned: Vec<bool>,
    plain_locals: Vec<bool>,
    /// Scope nesting depth of each local; cleanup releases nest by it (DROP-2).
    local_depth: Vec<u32>,
    /// The greatest depth in `local_depth`.
    max_depth: u32,
    cleanup_epoch: usize,
    pub roots: Vec<PlaceId>,
    pub loops: Vec<Loop>,
    pub blocks: Vec<LabelledBlock>,
    pub loan_count: u32,
    loan_sources: Vec<(PlaceId, bool)>,
    /// For a call result holding argument loans, by destination place: the
    /// parameters whose argument loans it holds and whether it holds the
    /// callee environment's contents, and whether it also holds the callee's
    /// source access loan (an owned erased environment can lend its payload).
    pub call_retention: BTreeMap<u32, (BTreeSet<u32>, bool, bool)>,
    /// For a call result related only to a field path of an argument, by
    /// destination place and parameter: the access through the read loan on
    /// that field place, which the result holds instead of the argument (#77).
    pub call_field_loans: BTreeMap<u32, BTreeMap<u32, PlaceId>>,
    /// Composed arguments before invocation access is added, by result and
    /// parameter. Last-use replay must retain the same source permissions as
    /// construction, without introducing a loan on the local carrier (MODE-1).
    pub call_composed_arguments: BTreeMap<u32, BTreeMap<u32, Operand>>,
    /// Roots whose type has user-visible destruction, with every loan their
    /// content may hold: their cleanup drops end those loans after them.
    pub destroyed_holders: BTreeMap<u32, BTreeSet<u32>>,
    /// Borrowed protocol cursors have no observable destruction despite being
    /// noncopyable. Their source loans follow uses, rather than cursor cleanup.
    pub borrowed_cursor_types: std::collections::HashSet<TypeId>,
    /// Lowering a value that a binding receives: a borrowed argument temporary
    /// the result keeps is released like a named local declared just before
    /// that binding, on every exit (DROP-2).
    pub keeping: bool,
    /// Where that binding's declaration starts: a kept temporary's declaration.
    pub keep_span: Span,
    /// The next expression's own result is borrowed by its caller and kept.
    pub keep_result: bool,
    pub conditional_loans: BTreeSet<u32>,
    pub evaluation_holders: Vec<Operand>,
    cleanup_nodes: std::collections::HashMap<CleanupNode, CleanupNodeId>,
    cleanup_roots: std::collections::HashMap<Option<CleanupNodeId>, CleanupId>,
    pub snapshots: Vec<(ExprId, PlaceId)>,
    pub contract_result: Option<PlaceId>,
    pub clauses: Vec<(ExprId, PlaceId)>,
    /// While binding one way an irrefutable pattern's alternatives match: the
    /// slot every way moves or copies each bound local into (LANG-13).
    pub joining: Option<BTreeMap<hirsel_types::LocalId, PlaceId>>,
}
impl<'a, 'b> Builder<'a, 'b> {
    pub fn new(
        input: &'b Input<'a>,
        decisions: &'b decisions::Decisions<'a>,
        owner: NameSite,
        result: TypeId,
    ) -> Result<Self> {
        let types = input.types;
        for ty in [result, input.unit, input.boolean] {
            if types.get(ty).is_none() {
                return Err(LowerError {
                    site: Span::default(),
                    message: "foreign or missing type".into(),
                });
            }
        }
        let mut this = Self {
            input,
            decisions,
            owner,
            types,
            projection_paths: Default::default(),
            body: Body {
                arena_scope_grant: None,
                scoped_parameters: Vec::new(),
                parameter_scope: decisions.lexical.site(owner),
                locals: vec![],
                places: vec![],
                scopes: vec![Scope {
                    parent: None,
                    span: Span::default(),
                }],
                blocks: vec![],
                rvalues: vec![],
                cleanup_nodes: vec![],
                cleanups: vec![Cleanup::default()],
                entry: BlockId(0),
                parameters: vec![],
                result,
                borrow_cursors: vec![],
                tail_sites: vec![],
                matches: vec![],
            },
            current: None,
            scope: ScopeId(0),
            state: State::default(),
            bindings: Default::default(),
            owned: vec![],
            plain_locals: vec![],
            local_depth: vec![],
            max_depth: 0,
            cleanup_epoch: 0,
            roots: vec![],
            loops: vec![],
            blocks: vec![],
            loan_count: 0,
            loan_sources: vec![],
            call_retention: BTreeMap::new(),
            call_field_loans: BTreeMap::new(),
            call_composed_arguments: BTreeMap::new(),
            destroyed_holders: BTreeMap::new(),
            borrowed_cursor_types: std::collections::HashSet::new(),
            keeping: false,
            keep_span: Span::default(),
            keep_result: false,
            conditional_loans: BTreeSet::new(),
            evaluation_holders: vec![],
            cleanup_nodes: Default::default(),
            cleanup_roots: [(None, CleanupId(0))].into(),
            snapshots: vec![],
            contract_result: None,
            clauses: vec![],
            joining: None,
        };
        this.current = Some(this.block(Span::default()));
        if let NameSite::Expression(literal) = owner {
            for call in &input.checked.calls {
                let hirsel_types::CallTarget::Definition(path) = &call.target else {
                    continue;
                };
                let Some(slot) = hirsel_types::core_roles::arena_scope_callback(path) else {
                    continue;
                };
                if call.arguments.iter().any(|argument| {
                    argument.expression == literal
                        && argument.parameter.as_ref().is_some_and(|(p, _)| *p == slot)
                }) {
                    this.body.arena_scope_grant = Some(ArenaScopeGrant {
                        intrinsic: path.clone(),
                        callback_parameter: hirsel_types::ParamId(slot),
                        arena_parameter: hirsel_types::ParamId(0),
                    });
                }
            }
        }
        if input.plans.evaluation_frame && owner == input.owner {
            return Ok(this);
        }
        let mut parameters: Vec<_> = decisions
            .owners
            .get(&this.owner)
            .iter()
            .copied()
            .filter(|b| b.owner == this.owner && matches!(b.binding, ResolvedName::Parameter(_)))
            .cloned()
            .collect();
        parameters.sort_by_key(|b| match b.binding {
            ResolvedName::Parameter(p) => p.0,
            _ => unreachable!(),
        });
        for (ordinal, b) in parameters.into_iter().enumerate() {
            if b.binding != ResolvedName::Parameter(hirsel_types::ParamId(ordinal as u32)) {
                return this.error(Span::default(), "parameter binding rows are incomplete");
            }
            let own = !matches!(b.usage, UseKind::Borrow | UseKind::Mut);
            let place = this.slot(b.ty, Storage::Parameter, Span::default(), own);
            this.state.live.insert(this.root(place).0);
            this.body.parameters.push(this.root(place));
            if !own
                && matches!(
                    this.types.get(b.ty).unwrap().kind,
                    hirsel_types::TypeKind::Function(_)
                )
            {
                this.body.scoped_parameters.push(ScopedParameter {
                    parameter: ordinal as u32,
                    fields: Vec::new(),
                });
            }
            this.body.places[place.0 as usize].binding =
                Self::binding_origin(decisions.lexical, b.owner, &b.binding);
            this.bindings.push((b.owner, b.binding, place));
        }
        if let NameSite::Expression(literal) = owner {
            let reader = input.checked.calls.iter().any(|call| {
                matches!(&call.target, hirsel_types::CallTarget::Definition(path) if path.is_core("region.with"))
                    && call.arguments.iter().any(|a| a.expression == literal
                        && a.parameter.as_ref().is_some_and(|(p, _)| *p == 1))
            });
            if reader && !this.body.scoped_parameters.iter().any(|p| p.parameter == 0) {
                this.body.scoped_parameters.insert(
                    0,
                    ScopedParameter {
                        parameter: 0,
                        fields: Vec::new(),
                    },
                );
            }
        }
        // H's implicit finalizer self is a resolver local, but the runtime
        // supplies it at entry just like a borrowed parameter. Consume the
        // producer row rather than assigning a new resolver identity or type.
        if let NameSite::Item = this.owner {
            if matches!(
                input.syntax.item.kind,
                hirsel_syntax::ItemKind::Type { drop: Some(_), .. }
            ) {
                let mut binders = decisions
                    .owners
                    .get(&this.owner)
                    .iter()
                    .copied()
                    .filter(|b| {
                        b.owner == this.owner
                            && b.site == this.owner
                            && matches!(b.binding, ResolvedName::Local(_))
                    });
                let Some(b) = binders.next() else {
                    return this.error(Span::default(), "missing finalizer self binding");
                };
                if binders.next().is_some()
                    || b.usage != UseKind::Borrow
                    || !this.body.parameters.is_empty()
                {
                    return this.error(Span::default(), "inconsistent finalizer self binding");
                }
                let place = this.slot(b.ty, Storage::Parameter, Span::default(), false);
                this.state.live.insert(this.root(place).0);
                this.body.parameters.push(this.root(place));
                let origin = Self::binding_origin(decisions.lexical, b.owner, &b.binding).unwrap();
                let place =
                    this.bind_origin(place, origin, OriginBindingKind::Alias, Span::default());
                this.bindings.push((b.owner, b.binding.clone(), place));
            }
        }
        Ok(this)
    }
    pub fn finish(mut self) -> Result<Body> {
        self.end_loans_at_last_use();
        self.complete_loan_flow();
        // A semantic origin id is not a body parameter ordinal. Resolve only
        // roots whose storage is needed, retaining all checked type rows intact.
        let mut captured_origins = BTreeSet::new();
        let mut visited = std::collections::HashSet::new();
        let mut pending: Vec<_> = self
            .body
            .locals
            .iter()
            .filter(|local| local.storage == Storage::Capture)
            .map(|local| local.ty)
            .collect();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let ty = self.types.get(id).unwrap();
            captured_origins.extend(ty.brand.alternatives.iter().cloned());
            captured_origins.extend(ty.modes.relation.reaches().iter().filter_map(|r| match r {
                hirsel_types::Reach::Place(origin) => Some(origin.clone()),
                _ => None,
            }));
            pending.extend(ty.kind.graph_edges().into_iter().map(|edge| edge.ty));
        }
        // Scoped captures retain their declaring frame's semantic origins.
        // Their complete checked graphs need no duplicate owner/arena slots in
        // the child; V independently checks transport from the parent operand.
        captured_origins.retain(|origin| {
            matches!(
                origin.root,
                hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body { scope, .. })
                    if scope != self.decisions.lexical.site(self.owner)
            )
        });
        for local in &self.body.locals {
            let ty = self.types.get(local.ty).unwrap();
            for reach in ty.modes.relation.reaches().iter() {
                if let hirsel_types::Reach::Place(origin) = reach {
                    // A parameter contract may name an enclosing declaration
                    // without retaining its value. Keep that scoped name;
                    // only an actual value use needs a runtime capture slot.
                    let declared_enclosing = match origin.root {
                        hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body {
                            scope,
                            parameter,
                        }) if scope != self.decisions.lexical.site(self.owner) => {
                            self.input.checked.bindings.iter().any(|binding| {
                                self.decisions.lexical.site(binding.owner) == scope
                                    && binding.binding == ResolvedName::Parameter(parameter)
                            })
                        }
                        _ => false,
                    };
                    if !captured_origins.contains(origin) && !declared_enclosing {
                        self.origin_place(origin, local.span)?;
                    }
                }
            }
            if !self.lifetime(&ty.modes.relation).is_global() {
                for origin in &ty.brand.alternatives {
                    // A declaration can refer to an enclosing parameter's
                    // brand without capturing its value. Callers prove that
                    // fixed name; this frame needs no runtime owner slot.
                    let enclosing_parameter = matches!(origin.root,
                        hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body { scope, .. })
                            if scope != self.decisions.lexical.site(self.owner));
                    if !captured_origins.contains(origin) && !enclosing_parameter {
                        self.origin_place(origin, local.span)?;
                    }
                }
            }
        }
        Ok(self.body)
    }
    /// Whether a relation place names an Arena-typed binding of this body:
    /// such a place sets placement rather than bounding a lifetime.
    pub fn arena_place(&self, origin: &hirsel_types::Origin) -> bool {
        let Some(arena) = &self.input.arena else {
            return false;
        };
        if !origin.fields.is_empty() {
            return false;
        }
        let local = match &origin.root {
            hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body {
                scope,
                parameter,
            }) if *scope == self.body.parameter_scope => {
                self.body.parameters.get(parameter.0 as usize).copied()
            }
            _ => None,
        }
        .or_else(|| {
            self.body
                .places
                .iter()
                .find(|p| {
                    p.projection.is_empty()
                        && p.binding
                            .as_ref()
                            .is_some_and(|b| b.root == origin.root && b.fields.is_empty())
                })
                .map(|p| p.root)
        });
        local
            .and_then(|l| self.body.locals.get(l.0 as usize))
            .and_then(|l| self.types.get(l.ty))
            .is_some_and(|t| {
                matches!(&t.kind, hirsel_types::TypeKind::Named { definition, .. } if definition.path() == arena)
            })
    }
    /// A row's lifetime sources: every reach except arena places.
    pub fn lifetime(&self, relation: &hirsel_types::Relation) -> hirsel_types::Relation {
        relation.retain(|r| !matches!(r, hirsel_types::Reach::Place(o) if self.arena_place(o)))
    }
    /// The construction destination a row's relation selects in this body.
    pub fn row_destination(&self, ty: TypeId) -> hirsel_types::Destination {
        match hirsel_types::destination(&self.types.get(ty).unwrap().modes.relation, |o| {
            self.arena_place(o)
        }) {
            hirsel_types::Destination::Caller(_) => hirsel_types::Destination::Inferred,
            d => d,
        }
    }
    fn origin_parameter(&self, id: hirsel_types::ParameterRef, span: Span) -> Result<PlaceId> {
        match id {
            hirsel_types::ParameterRef::Body { scope, parameter }
                if scope.owner == self.decisions.lexical =>
            {
                self.lookup(&ResolvedName::Parameter(parameter), &scope.site, span)
            }
            hirsel_types::ParameterRef::Body { .. } => {
                self.error(span, "another definition's parameter in body storage")
            }
            hirsel_types::ParameterRef::Bound { .. } => {
                self.error(span, "unbound signature parameter in body storage")
            }
        }
    }
    fn origin_place(&self, origin: &hirsel_types::Origin, span: Span) -> Result<Option<PlaceId>> {
        match origin.root {
            hirsel_types::OriginRoot::Parameter(p) => self.origin_parameter(p, span).map(Some),
            // Non-parameter identities are already preserved by checked types;
            // no slot ordinal is substituted into their semantic namespace.
            _ => Ok(None),
        }
    }
    pub fn error<T>(&self, site: Span, message: impl Into<String>) -> Result<T> {
        Err(LowerError {
            site,
            message: message.into(),
        })
    }
    pub fn span(&self, id: ExprId) -> Result<Span> {
        self.input
            .syntax
            .tree
            .expressions
            .get(id.0 as usize)
            .map(|e| e.origin.span)
            .ok_or_else(|| LowerError {
                site: Span::default(),
                message: "expression index out of range".into(),
            })
    }
    pub fn decision(&self, id: ExprId) -> Result<&hirsel_types::Elaboration> {
        let mut rows = self.decisions.expressions.get(&id).iter().copied();
        let row = rows.next().ok_or_else(|| LowerError {
            site: self.span(id).unwrap_or_default(),
            message: format!("missing expression decision {id:?}"),
        })?;
        if rows.next().is_some() {
            return self.error(self.span(id)?, "duplicate expression decision");
        }
        if self.types.get(row.ty).is_none() {
            return self.error(self.span(id)?, "foreign expression type");
        }
        Ok(row)
    }
    pub fn root(&self, p: PlaceId) -> LocalId {
        self.body.places[p.0 as usize].root
    }
    pub fn ty(&self, p: PlaceId) -> TypeId {
        self.body.places[p.0 as usize].ty
    }
    pub fn slot(&mut self, ty: TypeId, storage: Storage, span: Span, owned: bool) -> PlaceId {
        let root = LocalId(self.body.locals.len() as u32);
        self.body.locals.push(Local {
            ty,
            storage,
            scope: self.scope,
            span,
        });
        self.owned.push(owned);
        self.plain_locals.push(self.plain(ty));
        let mut depth = 0;
        let mut scope = self.scope;
        while let Some(parent) = self.body.scopes[scope.0 as usize].parent {
            depth += 1;
            scope = parent;
        }
        self.local_depth.push(depth);
        self.max_depth = self.max_depth.max(depth);
        let place = self.place(root, vec![], ty);
        self.roots.push(place);
        place
    }
    pub fn place(
        &mut self,
        root: LocalId,
        projection: impl Into<hirsel_types::SharedPath<Projection>>,
        ty: TypeId,
    ) -> PlaceId {
        let id = PlaceId(self.body.places.len() as u32);
        self.body.places.push(Place {
            loan: None,
            binding: None,
            root,
            projection: self.projection_paths.intern(projection.into()),
            ty,
        });
        id
    }
    pub fn binding_origin(
        lexical: hirsel_types::LexicalOwner,
        owner: NameSite,
        binding: &ResolvedName,
    ) -> Option<hirsel_types::Origin> {
        use hirsel_types::{Origin, OriginRoot, ParameterRef};
        let root = match binding {
            ResolvedName::Local(l) => OriginRoot::Local(lexical, *l),
            ResolvedName::Parameter(p) => {
                OriginRoot::Parameter(ParameterRef::body(lexical.site(owner), *p))
            }
            _ => return None,
        };
        Some(Origin {
            root,
            fields: vec![].into(),
        })
    }
    pub fn publish_identity_joins(&mut self, site: &NameSite, span: Span) -> Result<()> {
        let joins: Vec<_> = self
            .decisions
            .joins
            .get(site)
            .iter()
            .copied()
            .filter(|(at, _, _)| at == site)
            .cloned()
            .collect();
        for (_, owner, origin) in joins {
            let name = match origin.root {
                hirsel_types::OriginRoot::Local(_, l) => ResolvedName::Local(l),
                hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body {
                    parameter,
                    ..
                }) => ResolvedName::Parameter(parameter),
                _ => return self.error(span, "join identity is not a checked binding"),
            };
            let place = self.lookup(&name, &owner, span)?;
            let root = self.root(place);
            let cursor = self.body.borrow_cursors.iter().any(|c| c.carrier == root);
            let place = self.bind_origin(
                if cursor {
                    self.roots[root.0 as usize]
                } else {
                    place
                },
                origin,
                if cursor {
                    OriginBindingKind::Alias
                } else {
                    OriginBindingKind::Fresh
                },
                span,
            );
            self.bindings.set(owner, &name, place);
        }
        let fields: Vec<_> = self
            .decisions
            .fields
            .get(site)
            .iter()
            .copied()
            .filter(|(at, _, _, _)| at == site)
            .cloned()
            .collect();
        for (_, owner, origin, projections) in fields {
            let name = match origin.root {
                hirsel_types::OriginRoot::Local(_, l) => ResolvedName::Local(l),
                hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body {
                    parameter,
                    ..
                }) => ResolvedName::Parameter(parameter),
                _ => return self.error(span, "join field identity is not a checked binding"),
            };
            let mut place = self.lookup(&name, &owner, span)?;
            for (field, ty) in projections {
                place = self.project(place, Projection::Field(field.0), ty);
            }
            self.bind_origin(place, origin, OriginBindingKind::Fresh, span);
        }
        Ok(())
    }
    pub fn bind_origin(
        &mut self,
        place: PlaceId,
        origin: hirsel_types::Origin,
        kind: OriginBindingKind,
        span: Span,
    ) -> PlaceId {
        let source = place;
        let mut access = self.body.places[place.0 as usize].clone();
        access.binding = Some(origin);
        let place = PlaceId(self.body.places.len() as u32);
        self.body.places.push(access);
        self.emit(
            InstructionKind::BindOrigin {
                source,
                place,
                kind,
            },
            span,
        );
        place
    }
    pub fn project(&mut self, base: PlaceId, projection: Projection, ty: TypeId) -> PlaceId {
        let p = self.body.places[base.0 as usize].clone();
        let mut path = p.projection;
        path.push(projection.clone());
        let place = self.place(p.root, path, ty);
        self.body.places[place.0 as usize].loan = p.loan;
        self.body.places[place.0 as usize].binding = p.binding.and_then(|mut origin| {
            match projection {
                Projection::Field(field) | Projection::VariantField { field, .. } => {
                    origin.fields.push(hirsel_types::FieldId(field))
                }
                Projection::Index(_) => return None,
            }
            Some(origin)
        });
        place
    }
    pub fn block(&mut self, span: Span) -> BlockId {
        let id = BlockId(self.body.blocks.len() as u32);
        self.body.blocks.push(Block {
            instructions: vec![],
            terminator: Terminator::Unreachable,
            span,
        });
        id
    }
    pub fn terminate(&mut self, term: Terminator, span: Span) {
        if let Some(id) = self.current.take() {
            let b = &mut self.body.blocks[id.0 as usize];
            b.terminator = term;
            b.span = span;
        }
    }
    /// OWN-8: a nonplain part read through its owner keeps the owner's bounds.
    pub(super) fn part_type(&mut self, owner: TypeId, part: TypeId) -> TypeId {
        if self.input.ownership.plain_types.contains(&part) {
            return part;
        }
        let mut row = self.types.get(part).unwrap().clone();
        row.modes.relation = row
            .modes
            .relation
            .union(&self.types.get(owner).unwrap().modes.relation);
        self.types.intern(row)
    }
    pub(super) fn plain(&self, ty: TypeId) -> bool {
        self.input.ownership.plain_types.contains(&ty)
            || hirsel_types::structural_property(
                self.types,
                ty,
                hirsel_types::StructuralProperty::Plain,
                &self.input.checked.resolution.generic_declarations,
                &mut |_| Ok::<_, hirsel_types::QueryError>(None),
            )
            .unwrap()
    }
    pub fn retire_plain(&mut self, slots: &BTreeSet<u32>, span: Span) {
        for &root in slots {
            if self.plain_locals[root as usize] {
                self.emit(
                    InstructionKind::StorageDead {
                        place: self.roots[root as usize],
                        if_live: true,
                    },
                    span,
                );
            }
        }
    }
    pub fn cleanup_for(&mut self, slots: &BTreeSet<u32>, loans: &[(LoanId, ScopeId)]) -> CleanupId {
        let mut head = None;
        // Build in reverse execution order so old scopes remain shared suffixes:
        // an exit releases inner scopes first, each scope's temporaries before
        // its names (DROP-2).
        let mut order: Vec<_> = slots
            .iter()
            .map(|&root| {
                let temporary = self.body.locals[root as usize].storage == Storage::Temporary;
                (self.local_depth[root as usize], temporary, root)
            })
            .collect();
        order.sort();
        for (_, _, root) in order {
            work::count(Counter::LoweringCleanupRootRows);
            if self.owned[root as usize] && !self.plain_locals[root as usize] {
                head = Some(self.cleanup_node(
                    CleanupAction::Drop {
                        place: self.roots[root as usize],
                        if_live: true,
                    },
                    head,
                ));
            }
        }
        for (loan, _) in loans {
            head = Some(self.cleanup_node(
                CleanupAction::EndLoan {
                    loan: *loan,
                    if_active: self.conditional_loans.contains(&loan.0),
                },
                head,
            ));
        }
        self.cleanup_root(head)
    }

    pub(super) fn cleanup_node(
        &mut self,
        action: CleanupAction,
        parent: Option<CleanupNodeId>,
    ) -> CleanupNodeId {
        let node = CleanupNode { action, parent };
        *self.cleanup_nodes.entry(node.clone()).or_insert_with(|| {
            let id = CleanupNodeId(self.body.cleanup_nodes.len() as u32);
            self.body.cleanup_nodes.push(node);
            id
        })
    }
    pub(super) fn cleanup_root(&mut self, head: Option<CleanupNodeId>) -> CleanupId {
        if let Some(id) = self.cleanup_roots.get(&head) {
            return *id;
        }
        let id = CleanupId(self.body.cleanups.len() as u32);
        self.body.cleanups.push(Cleanup { head });
        self.cleanup_roots.insert(head, id);
        id
    }
    pub fn receive_ownership(&mut self, root: usize, owned: bool) {
        if owned && !self.owned[root] {
            self.owned[root] = true;
            if !self.plain_locals[root] {
                self.cleanup_epoch += 1;
            }
        }
    }
    pub fn cleanup(&mut self) -> CleanupId {
        let live = self.state.live.clone();
        let mut head = None;
        // Reverse execution order: outer scopes first, each scope's names
        // before its temporaries, so the chain releases inner scopes first.
        for depth in 0..=self.max_depth {
            for temporary in [false, true] {
                head = live.compile(
                    depth * 2 + temporary as u32,
                    self.cleanup_epoch,
                    head,
                    &mut |root, parent| {
                        if self.owned[root as usize]
                            && !self.plain_locals[root as usize]
                            && self.local_depth[root as usize] == depth
                            && (self.body.locals[root as usize].storage == Storage::Temporary)
                                == temporary
                        {
                            Some(self.cleanup_node(
                                CleanupAction::Drop {
                                    place: self.roots[root as usize],
                                    if_live: true,
                                },
                                parent,
                            ))
                        } else {
                            parent
                        }
                    },
                );
            }
        }
        for (loan, _) in self.state.loans.clone() {
            head = Some(self.cleanup_node(
                CleanupAction::EndLoan {
                    loan,
                    if_active: self.conditional_loans.contains(&loan.0),
                },
                head,
            ));
        }
        self.cleanup_root(head)
    }
    /// One compile-time budget unit for this source call or loop iteration (CT-4).
    pub fn charge(&mut self, kind: hirsel_tir::SourceEventKind, source: u32, span: Span) {
        self.emit(
            InstructionKind::Charge(hirsel_tir::SourceEvent { kind, source }),
            span,
        );
    }
    pub fn emit(&mut self, kind: InstructionKind, span: Span) {
        let on_failure = self.cleanup();
        self.emit_with(kind, span, on_failure);
    }
    pub fn emit_with(&mut self, kind: InstructionKind, span: Span, on_failure: CleanupId) {
        let kind = match kind {
            InstructionKind::Drop { place, if_live }
                if self.plain(self.body.places[place.0 as usize].ty) =>
            {
                InstructionKind::StorageDead { place, if_live }
            }
            kind => kind,
        };
        if let Some(id) = self.current {
            self.body.blocks[id.0 as usize]
                .instructions
                .push(Instruction {
                    kind,
                    span,
                    on_failure,
                });
        }
    }
    pub fn operand_loans(&self, operand: &Operand) -> HeldLoans {
        let place = &self.body.places[operand.place.0 as usize];
        // Copy creates an owned share; its storage lifetime no longer depends
        // on the read permission. Scoped callable environments remain views.
        if operand.usage == UseKind::Copy
            && !matches!(
                self.types.get(place.ty).unwrap().kind,
                hirsel_types::TypeKind::Function(_)
            )
        {
            return vec![];
        }
        let mut held = self.content_loans(operand.place);
        if matches!(operand.usage, UseKind::Borrow | UseKind::Mut) {
            let mut loan = place.loan;
            let mut loans = BTreeSet::new();
            while let Some(id) = loan {
                if !loans.insert(id.0) {
                    break;
                }
                loan = self.body.places[self.loan_sources[id.0 as usize].0.0 as usize].loan;
            }
            if !loans.is_empty() {
                held.push((vec![], loans));
            }
        }
        held
    }
    pub fn content_loans(&self, place: PlaceId) -> HeldLoans {
        let place = &self.body.places[place.0 as usize];
        let mut held = vec![];
        if let Some(rows) = self.state.retained.get(&place.root.0) {
            for (path, loans) in rows {
                if path.len() >= place.projection.len()
                    && place
                        .projection
                        .iter()
                        .eq(path.iter().take(place.projection.len()))
                {
                    held.push((path[place.projection.len()..].to_vec(), loans.clone()));
                } else if place.projection.starts_with(path) {
                    held.push((vec![], loans.clone()));
                }
            }
        }
        held
    }
    pub fn transitive_operand_loans(&self, operand: &Operand) -> BTreeSet<u32> {
        let held = self
            .operand_loans(operand)
            .into_iter()
            .flat_map(|(_, loans)| loans)
            .collect();
        self.source_dependencies(held)
    }
    fn source_dependencies(&self, mut held: BTreeSet<u32>) -> BTreeSet<u32> {
        let mut pending: Vec<_> = held.iter().copied().collect();
        while let Some(loan) = pending.pop() {
            let root = self.root(self.loan_sources[loan as usize].0).0;
            if self.body.locals[root as usize].storage == Storage::Temporary
                && self.state.live.contains(&root)
            {
                if let Some(rows) = self.state.retained.get(&root) {
                    for dependency in rows.iter().flat_map(|(_, loans)| loans) {
                        if held.insert(*dependency) {
                            pending.push(*dependency);
                        }
                    }
                }
            }
        }
        held
    }
    pub fn set_held_loans(&mut self, destination: PlaceId, held: HeldLoans) {
        let place = &self.body.places[destination.0 as usize];
        let rows = self.state.retained.entry(place.root.0).or_default();
        rows.retain(|(path, _)| {
            !(path.len() >= place.projection.len()
                && place
                    .projection
                    .iter()
                    .eq(path.iter().take(place.projection.len())))
        });
        for (path, loans) in held {
            let path = place
                .projection
                .iter()
                .cloned()
                .chain(path)
                .collect::<Vec<_>>();
            if let Some((_, previous)) = rows.iter_mut().find(|(p, _)| *p == path) {
                previous.extend(loans);
            } else if !loans.is_empty() {
                rows.push((path, loans));
            }
        }
        if rows.is_empty() {
            self.state.retained.remove(&place.root.0);
        }
    }
    pub fn retain_rvalue(&mut self, destination: PlaceId, value: &Rvalue) {
        let mut held = vec![];
        match value {
            Rvalue::Use(value) => held = self.operand_loans(value),
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value,
            } if matches!(
                self.types.get(self.ty(destination)).unwrap().kind,
                hirsel_types::TypeKind::Function(_) | hirsel_types::TypeKind::Parameter(_)
            ) =>
            {
                held = self.operand_loans(value)
            }
            Rvalue::Construct {
                constructor,
                fields,
                ..
            } => {
                for field in fields {
                    let prefix = match constructor {
                        Constructor::Variant { variant, .. } => Some(Projection::VariantField {
                            variant: *variant,
                            field: field.field,
                        }),
                        Constructor::Tuple | Constructor::Record(_) => {
                            Some(Projection::Field(field.field))
                        }
                        _ => None,
                    };
                    held.extend(self.operand_loans(&field.value).into_iter().map(
                        |(path, loans)| (prefix.iter().cloned().chain(path).collect(), loans),
                    ));
                }
            }
            Rvalue::Closure { captures, .. } => {
                held.extend(captures.iter().flat_map(|value| self.operand_loans(value)));
            }
            _ => {}
        }
        if self.owned[self.root(destination).0 as usize] && self.plain(self.ty(destination)) {
            held.clear();
        }
        // Extraction transfers a projected holder as well as its dependencies.
        // Whole moves are tracked by the existing storage live set.
        let operands: Vec<&Operand> = match value {
            Rvalue::Use(value) | Rvalue::Unary { value, .. } => vec![value],
            Rvalue::Binary { left, right, .. } => vec![left, right],
            Rvalue::Construct {
                constructor: Constructor::SortedMap { by },
                fields,
                ..
            } => std::iter::once(by)
                .chain(fields.iter().map(|field| &field.value))
                .collect(),
            Rvalue::Construct { fields, .. } => fields.iter().map(|field| &field.value).collect(),
            Rvalue::Closure { captures, .. } => captures.iter().collect(),
            _ => vec![],
        };
        for operand in operands {
            self.retire_operand_content(operand);
        }
        self.set_held_loans(destination, held);
    }
    pub fn retire_operand_content(&mut self, operand: &Operand) {
        let place = &self.body.places[operand.place.0 as usize];
        if operand.usage == UseKind::Take && !place.projection.is_empty() {
            if let Some(rows) = self.state.retained.get_mut(&place.root.0) {
                rows.retain(|(path, _)| {
                    !(path.len() >= place.projection.len()
                        && place
                            .projection
                            .iter()
                            .eq(path.iter().take(place.projection.len())))
                });
            }
        }
    }
    pub fn view_named_within(&self, place: PlaceId, scope: ScopeId) -> bool {
        self.state
            .views
            .iter()
            .any(|(named, home)| *named == place && self.within(*home, scope))
    }
    pub fn retain_view(&mut self, place: PlaceId) {
        self.state.views.push((place, self.scope));
    }
    pub fn named_view_holds_root(&self, root: u32) -> bool {
        // A named borrow can refer to a nonowning branch staging slot. It has
        // no loan on that slot, but keeps its selected source permissions.
        // Its evaluated index snapshots also remain part of the address.
        self.state.views.iter().any(|(place, _)| {
            let place = &self.body.places[place.0 as usize];
            place.root.0 == root
                || place.projection.iter().any(
                    |projection| matches!(projection, Projection::Index(index) if index.0 == root),
                )
        })
    }
    /// End permissions after displaced values and statement staging values have
    /// gone. The installed content is already present, so replacing a view by
    /// another view of the same source keeps its permission active.
    pub fn end_unheld_loans(&mut self, span: Span) {
        self.state
            .retained
            .retain(|root, _| self.state.live.contains(root));
        // Temporary sources kept solely by a permission are dependencies of
        // real holders, not roots of liveness. Otherwise a closure source keeps
        // its capture loans alive after the last result holder is replaced.
        let source_roots: BTreeSet<_> = self
            .state
            .loans
            .iter()
            .map(|(loan, _)| self.root(self.loan_sources[loan.0 as usize].0).0)
            .collect();
        let mut independent: BTreeSet<_> = self
            .state
            .retained
            .keys()
            .copied()
            .filter(|root| {
                self.body.locals[*root as usize].storage != Storage::Temporary
                    || !source_roots.contains(root)
            })
            .collect();
        independent.extend(
            self.evaluation_holders
                .iter()
                .map(|value| self.root(value.place).0),
        );
        independent.extend(
            self.state
                .views
                .iter()
                .map(|(place, _)| self.root(*place).0),
        );
        let mut held = BTreeSet::new();
        for root in &independent {
            if let Some(rows) = self.state.retained.get(root) {
                held.extend(rows.iter().flat_map(|(_, loans)| loans));
            }
        }
        for operand in &self.evaluation_holders {
            held.extend(
                self.operand_loans(operand)
                    .into_iter()
                    .flat_map(|(_, loans)| loans),
            );
        }
        for (place, _) in &self.state.views {
            held.extend(
                self.operand_loans(&Operand {
                    place: *place,
                    usage: UseKind::Borrow,
                })
                .into_iter()
                .flat_map(|(_, loans)| loans),
            );
        }
        let held = self.source_dependencies(held);
        let supplied: BTreeSet<_> = held
            .iter()
            .map(|loan| self.root(self.loan_sources[*loan as usize].0).0)
            .collect();
        let end: Vec<_> = self
            .state
            .loans
            .iter()
            .rev()
            .filter_map(|(loan, _)| (!held.contains(&loan.0)).then_some(*loan))
            .collect();
        for loan in end {
            let source = self.root(self.loan_sources[loan.0 as usize].0).0;
            self.end_loan(Some(loan), span);
            if self.body.locals[source as usize].storage == Storage::Temporary
                && self.state.live.contains(&source)
                && !independent.contains(&source)
                && !supplied.contains(&source)
                && !self.loan_supplies_root(source)
            {
                let slots = [source].into();
                let cleanup = self.cleanup_for(&slots, &[]);
                self.state.live.remove(&source);
                self.emit(InstructionKind::RunCleanup(cleanup), span);
                self.retire_plain(&slots, span);
                self.state.retained.remove(&source);
            }
        }
    }
    pub fn consume(&mut self, value: Value) -> Operand {
        let p = &self.body.places[value.place.0 as usize];
        if value.usage == UseKind::Take && p.projection.is_empty() {
            self.state.live.remove(&p.root.0);
        }
        Operand {
            place: value.place,
            usage: value.usage,
        }
    }
    pub fn loan(&mut self, value: &mut Value, span: Span) -> Option<LoanId> {
        if matches!(value.usage, UseKind::Borrow | UseKind::Mut) {
            if value.usage == UseKind::Borrow
                && self.body.places[value.place.0 as usize].loan.is_some()
            {
                // A read through a scoped view retains its existing permission.
                // A narrower field reborrow would hide sibling identity proofs
                // supplied by the aggregate's original loan.
                return None;
            }
            let root = self.root(value.place);
            if self.body.locals[root.0 as usize].storage == Storage::Temporary
                && !self.owned[root.0 as usize]
            {
                // A transported borrowed view retains the source loans. Borrowing
                // its compiler slot would incorrectly shorten the view lifetime.
                return None;
            }

            let id = LoanId(self.loan_count);
            self.loan_sources
                .push((value.place, value.usage == UseKind::Mut));
            self.loan_count += 1;
            self.emit(
                InstructionKind::BeginLoan {
                    id,
                    place: value.place,
                    mutable: value.usage == UseKind::Mut,
                    if_inactive: false,
                },
                span,
            );
            self.state.loans.push((id, self.scope));
            let mut access = self.body.places[value.place.0 as usize].clone();
            access.loan = Some(id);
            value.place = PlaceId(self.body.places.len() as u32);
            self.body.places.push(access);
            return Some(id);
        }
        None
    }
    /// A read loan on the borrowed argument `value`'s source place extended
    /// by `path`, whose type ownership published for `argument` (#77). The
    /// call result holds it instead of the whole argument. Returns the access
    /// through it.
    pub fn field_loan(
        &mut self,
        argument: ExprId,
        value: &Value,
        path: &[hirsel_types::FieldId],
        span: Span,
    ) -> Option<PlaceId> {
        let (_, ty) = *self
            .input
            .ownership
            .field_loans
            .iter()
            .find(|(e, _)| *e == argument)?;
        let mut source = value.place;
        if let Some(loan) = self.body.places[source.0 as usize].loan {
            source = self.loan_source(loan);
        }
        let base = self.body.places[source.0 as usize].clone();
        if base.loan.is_some() {
            return None;
        }
        let mut projection = base.projection.clone();
        for field in path {
            projection.push(Projection::Field(field.0));
        }
        let place = self.place(base.root, projection, ty);
        self.body.places[place.0 as usize].binding = base.binding.map(|mut origin| {
            for field in path {
                origin.fields.push(*field);
            }
            origin
        });
        let mut field = Value {
            place,
            usage: UseKind::Borrow,
            checked_ty: None,
        };
        self.loan(&mut field, span)?;
        Some(field.place)
    }
    pub(super) fn loan_source(&self, loan: LoanId) -> PlaceId {
        self.loan_sources[loan.0 as usize].0
    }
    pub(super) fn has_mutable_loan_on(&self, root: LocalId) -> bool {
        self.state.loans.iter().any(|(loan, _)| {
            let (source, mutable) = self.loan_sources[loan.0 as usize];
            mutable && self.root(source) == root
        })
    }
    pub fn loan_supplies_root(&self, root: u32) -> bool {
        self.state.loans.iter().any(|(loan, _)| {
            work::count(Counter::LoweringRetirementRows);
            self.root(self.loan_sources[loan.0 as usize].0).0 == root
        })
    }
    pub fn new_loans_since(&self, first: u32) -> Vec<(LoanId, ScopeId)> {
        self.state
            .loans
            .iter()
            .copied()
            .filter(|(loan, _)| loan.0 >= first)
            .collect()
    }
    pub fn end_new_loans(&mut self, first: u32, span: Span) {
        for (loan, _) in self.new_loans_since(first).into_iter().rev() {
            self.end_loan(Some(loan), span);
        }
    }
    pub fn end_loan(&mut self, loan: Option<LoanId>, span: Span) {
        if let Some(loan) = loan {
            self.state.loans.retain(|(id, _)| *id != loan);
            self.emit(
                InstructionKind::EndLoan {
                    loan,
                    if_active: self.conditional_loans.contains(&loan.0),
                },
                span,
            );
        }
    }
    pub fn replace(&mut self, destination: PlaceId, value: Value, content: TypeId, span: Span) {
        self.replace_with_liveness(destination, value, content, false, span);
    }
    pub fn replace_with_liveness(
        &mut self,
        destination: PlaceId,
        value: Value,
        content: TypeId,
        if_live: bool,
        span: Span,
    ) {
        // The old content's uniqueness is flow knowledge, not the place's
        // declaration: a copy taken since the place was written still holds
        // it (MODE-2), so the displaced temporary claims no uniqueness.
        let content = if self.types.get(content).unwrap().modes.uniqueness
            == hirsel_types::Uniqueness::Unique
        {
            let mut row = self.types.get(content).unwrap().clone();
            row.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
            self.types.intern(row)
        } else {
            content
        };
        let displaced = self
            .exchange(destination, value, content, if_live, span)
            .place;
        self.emit(
            InstructionKind::Drop {
                place: displaced,
                if_live,
            },
            span,
        );
        self.state.live.remove(&self.root(displaced).0);
    }
    /// Exchange live storage without finalizing the displaced owner.
    pub fn exchange(
        &mut self,
        destination: PlaceId,
        value: Value,
        content: TypeId,
        if_live: bool,
        span: Span,
    ) -> Value {
        let displaced = self.slot(content, Storage::Temporary, span, true);
        self.emit(InstructionKind::StorageLive(self.root(displaced)), span);
        let failure = self.cleanup();
        let incoming = self.operand_loans(&Operand {
            place: value.place,
            usage: value.usage,
        });
        let outgoing = self.operand_loans(&Operand {
            place: destination,
            usage: UseKind::Take,
        });
        let value = self.consume(value);
        self.emit_with(
            InstructionKind::Replace {
                destination,
                value,
                displaced,
                if_live,
            },
            span,
            failure,
        );
        self.set_held_loans(destination, incoming);
        self.set_held_loans(displaced, outgoing);
        self.state.live.insert(self.root(displaced).0);
        self.state.live.insert(self.root(destination).0);
        Value {
            checked_ty: None,
            place: displaced,
            usage: UseKind::Take,
        }
    }
    pub fn assign(&mut self, destination: PlaceId, value: Rvalue, span: Span) {
        self.retain_rvalue(destination, &value);
        let id = RvalueId(self.body.rvalues.len() as u32);
        self.body.rvalues.push(value);
        self.emit(
            InstructionKind::Assign {
                destination,
                value: id,
            },
            span,
        );
        self.state.live.insert(self.root(destination).0);
    }
    pub fn value_owned(&self, value: Value) -> bool {
        match value.usage {
            UseKind::Borrow | UseKind::Mut => false,
            UseKind::Copy => true,
            // Plain bits taken from storage this body does not own (a `mut`
            // parameter) are still an owned copy of those bits.
            UseKind::Take => {
                self.owned[self.root(value.place).0 as usize] || self.plain(self.ty(value.place))
            }
        }
    }
    pub fn transport(&mut self, value: Value, storage: Storage, span: Span) -> Value {
        self.transport_typed(
            value,
            value.checked_ty.unwrap_or_else(|| self.ty(value.place)),
            storage,
            span,
        )
    }
    pub fn transport_typed(
        &mut self,
        mut value: Value,
        ty: TypeId,
        storage: Storage,
        span: Span,
    ) -> Value {
        let _ = self.loan(&mut value, span);
        let owned = self.value_owned(value);
        let dest = self.slot(ty, storage, span, owned);
        self.emit(InstructionKind::StorageLive(self.root(dest)), span);
        // Failure can happen before or after extracting an operand. Conditional
        // drops retain the pre-extraction roots on this instruction's edge.
        let failure = self.cleanup();
        let operand = self.consume(value);
        let rv = RvalueId(self.body.rvalues.len() as u32);
        self.retain_rvalue(dest, &Rvalue::Use(operand.clone()));
        self.body.rvalues.push(Rvalue::Use(operand));
        self.emit_with(
            InstructionKind::Assign {
                destination: dest,
                value: rv,
            },
            span,
            failure,
        );
        self.state.live.insert(self.root(dest).0);
        Value {
            checked_ty: None,
            place: dest,
            usage: if owned {
                UseKind::Take
            } else if value.usage == UseKind::Mut {
                UseKind::Mut
            } else {
                UseKind::Borrow
            },
        }
    }
    pub fn rvalue(&mut self, ty: TypeId, value: Rvalue, span: Span) -> Value {
        let dest = self.slot(ty, Storage::Temporary, span, true);
        self.emit(InstructionKind::StorageLive(self.root(dest)), span);
        self.assign(dest, value, span);
        Value {
            checked_ty: None,
            place: dest,
            usage: UseKind::Take,
        }
    }
    pub fn unit(&mut self, span: Span) -> Value {
        self.rvalue(self.input.unit, Rvalue::Constant(Constant::Unit), span)
    }
    pub fn return_value(
        &mut self,
        mut value: Value,
        span: Span,
        bodies: &mut Vec<Option<Body>>,
    ) -> Result<()> {
        if self
            .decisions
            .contracts
            .get(&self.owner)
            .iter()
            .copied()
            .any(|c| c.owner == self.owner && c.kind == hirsel_syntax::ContractKind::Ensures)
        {
            value = self.transport(value, Storage::Temporary, span);
            self.contract_result = Some(value.place);
            self.exit_checks(bodies)?;
            self.contract_result = None;
        }
        // Return extracts first; its cleanup is the post-extraction state.
        let _ = self.loan(&mut value, span);
        let operand = self.consume(value);
        let cleanup = self.cleanup();
        self.terminate(
            Terminator::Return {
                value: operand,
                cleanup,
            },
            span,
        );
        Ok(())
    }
    pub fn lookup(&self, name: &ResolvedName, owner: &NameSite, span: Span) -> Result<PlaceId> {
        self.bindings.get(*owner, name).ok_or_else(|| LowerError {
            site: span,
            message: format!("missing scoped binding {owner:?}/{name:?}"),
        })
    }
    /// A loop that assigns a binding which is moved when the loop is entered
    /// leaves it live on the back edge: from the loop's first instruction on,
    /// the binding may hold a value and every cleanup must release it.
    pub fn revive_in_loop(&mut self, span: Span) {
        for (place, _, live) in &self.input.ownership.replacements {
            if *live == Some(true) {
                continue;
            }
            let inside = self
                .input
                .syntax
                .tree
                .expressions
                .get(place.0 as usize)
                .is_some_and(|e| {
                    e.origin.span.anchor == span.anchor
                        && span.start <= e.origin.span.start
                        && e.origin.span.end <= span.end
                });
            if !inside {
                continue;
            }
            let Ok(source) = self.source_place(*place) else {
                continue;
            };
            let SourceRoot::Binding { owner, binding } = &source.root else {
                continue;
            };
            if !source.projections.is_empty() {
                continue;
            }
            // A binding the loop itself declares is not yet in scope here.
            if let Ok(slot) = self.lookup(binding, owner, span) {
                let root = self.root(slot).0;
                if self.owned[root as usize] && !self.plain_locals[root as usize] {
                    self.state.live.insert(root);
                }
            }
        }
    }
    pub fn resolved_place(&self, id: ExprId) -> Result<PlaceId> {
        let span = self.span(id)?;
        let source = self.source_place(id)?;
        let SourceRoot::Binding { owner, binding } = &source.root else {
            return self.error(span, "checked source root is not a binding");
        };
        self.lookup(binding, owner, span)
    }
    pub fn exit_scope(&mut self, scope: ScopeId, span: Span) {
        // Active lexical scopes are allocated in preorder. At their exit all
        // scopes allocated since this one are descendants; future siblings do
        // not exist yet. Local.scope never changes after slot allocation.
        let slots: BTreeSet<_> = self
            .state
            .live
            .roots_in_scopes(scope.0, self.body.scopes.len() as u32, &|root| {
                self.body.locals[root as usize].scope.0
            })
            .into_iter()
            .collect();
        let loans: Vec<_> = self
            .state
            .loans
            .iter()
            .copied()
            .filter(|(_, s)| self.within(*s, scope))
            .collect();
        let cleanup = self.cleanup_for(&slots, &loans);
        // Cleanup failures continue its list; the failure edge cleans remaining
        // outer scopes, and must not repeat actions already in the list.
        for root in &slots {
            self.state.live.remove(root);
        }
        self.state.loans.retain(|l| !loans.contains(l));
        let scopes = &self.body.scopes;
        self.state.views.retain(|(_, home)| {
            let mut home = *home;
            loop {
                if home == scope {
                    return false;
                }
                match scopes[home.0 as usize].parent {
                    Some(parent) => home = parent,
                    None => return true,
                }
            }
        });
        self.emit(InstructionKind::RunCleanup(cleanup), span);
        self.emit(InstructionKind::EndScope(scope), span);
    }
    pub fn within(&self, mut child: ScopeId, parent: ScopeId) -> bool {
        loop {
            if child == parent {
                return true;
            }
            match self.body.scopes[child.0 as usize].parent {
                Some(p) => child = p,
                None => return false,
            }
        }
    }
    pub fn merge(&mut self, states: &[State]) -> State {
        let mut result = states.first().cloned().unwrap_or_default();
        for state in states.iter().skip(1) {
            result.live.union(&state.live);
            for (root, rows) in &state.retained {
                let joined = result.retained.entry(*root).or_default();
                for (path, loans) in rows {
                    if let Some((_, previous)) = joined.iter_mut().find(|(p, _)| p == path) {
                        previous.extend(loans);
                    } else {
                        joined.push((path.clone(), loans.clone()));
                    }
                }
            }
            for view in &state.views {
                if !result.views.contains(view) {
                    result.views.push(*view);
                }
            }
            for loan in &state.loans {
                if let Some((_, home)) = result.loans.iter_mut().find(|(id, _)| *id == loan.0) {
                    // A live view can promote its source permission to an
                    // enclosing home. Joining paths retains that enclosing
                    // lifetime, rather than adding a second earlier expiry.
                    while !self.within(loan.1, *home) {
                        *home = self.body.scopes[home.0 as usize].parent.unwrap();
                    }
                } else {
                    result.loans.push(*loan);
                }
            }
        }
        result.loans.sort_by_key(|(loan, _)| loan.0);
        for (loan, _) in &result.loans {
            if states
                .iter()
                .any(|s| !s.loans.iter().any(|(id, _)| id == loan))
            {
                self.conditional_loans.insert(loan.0);
            }
        }
        result
    }
}
