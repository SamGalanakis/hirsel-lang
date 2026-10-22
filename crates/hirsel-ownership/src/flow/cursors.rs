//! Finite observations of storage prefixes, separate from content and loans.
//! A path leaves the observed trie once; its descendant summary is absorbing.
use super::*;
type Path = Vec<(u8, u32, u32)>;
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct Anchor {
    root: usize,
    path: Path,
    descendant: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Views {
    anchors: BTreeSet<Anchor>,
    // Must-lineage for a declaration whose initial selection is summarized.
    lineages: BTreeSet<usize>,
}
impl Views {
    pub fn cursor_derived(&self) -> bool {
        !self.lineages.is_empty()
    }
    pub fn root(root: usize) -> Self {
        Self {
            anchors: BTreeSet::from([Anchor {
                root,
                path: vec![],
                descendant: false,
            }]),
            lineages: BTreeSet::new(),
        }
    }
    pub fn join(&mut self, other: &Self) {
        if self.anchors.is_empty() {
            *self = other.clone();
            return;
        }
        if other.anchors.is_empty() {
            return;
        }
        self.anchors.extend(other.anchors.iter().cloned());
        self.lineages.retain(|root| other.lineages.contains(root));
    }
    pub fn project(&mut self, path: &[Projection], observations: &Observations) {
        self.anchors = self
            .anchors
            .iter()
            .map(|anchor| {
                let mut anchor = anchor.clone();
                for step in path {
                    if anchor.descendant {
                        break;
                    }
                    let next = projection_key(step);
                    let mut extended = anchor.path.clone();
                    extended.push(next);
                    if observations.paths.contains(&extended) {
                        anchor.path = extended;
                    } else {
                        anchor.descendant = true;
                    }
                }
                anchor
            })
            .collect();
    }
    fn within(&self, cover: &Self) -> bool {
        !self.anchors.is_empty()
            && self.anchors.iter().all(|anchor| {
                cover.anchors.iter().any(|allowed| {
                    allowed.root == anchor.root
                        && anchor.path.starts_with(&allowed.path)
                        && (!allowed.descendant
                            || self
                                .lineages
                                .iter()
                                .any(|root| cover.lineages.contains(root)))
                })
            })
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Contract {
    cover: Views,
}
impl Contract {
    pub fn join(&mut self, other: &Self) {
        self.cover
            .anchors
            .retain(|anchor| other.cover.anchors.contains(anchor));
        self.cover
            .lineages
            .retain(|root| other.cover.lineages.contains(root));
    }
}
pub(super) struct Observations {
    paths: BTreeSet<Path>,
    pub active: bool,
}
impl Observations {
    pub fn new(body: &CheckedBody, t: &TypeInterner, properties: &PropertyTable) -> Self {
        let active = body.bindings.iter().any(|binding| {
            binding.mutable && {
                let p = properties[binding.ty];
                p.data && p.copyable && !p.plain
            }
        });
        if !active {
            return Self {
                paths: BTreeSet::new(),
                active,
            };
        }
        let mut paths = BTreeSet::from([vec![]]);
        let mut add = |path: Path| {
            for n in 0..=path.len() {
                paths.insert(path[..n].to_vec());
            }
        };
        for place in &body.places {
            add(place
                .projections
                .iter()
                .map(|step| match step.kind {
                    SourceProjectionKind::Field(field) => (0, field.0, 0),
                    SourceProjectionKind::VariantField { variant, field } => {
                        (1, variant.0, field.0)
                    }
                    SourceProjectionKind::Index(_) => (2, 0, 0),
                })
                .collect());
        }
        let rows: BTreeMap<_, _> = body.patterns.iter().map(|row| (row.pattern, row)).collect();
        for row in &body.patterns {
            let mut pending = vec![(row.pattern, vec![])];
            while let Some((id, mut prefix)) = pending.pop() {
                let row = rows[&id];
                let mut ty = row.ty;
                if !row.fields.is_empty() {
                    while let TypeKind::Box(inner) = t.get(ty).unwrap().kind {
                        prefix.push((0, 0, 0));
                        ty = inner;
                    }
                }
                add(prefix.clone());
                for (field, child) in &row.fields {
                    let mut path = prefix.clone();
                    path.push(match &row.kind {
                        PatternDecisionKind::Constructor(ContextualName::Variant {
                            variant,
                            ..
                        }) => (1, variant.0, field.0),
                        _ => (0, field.0, 0),
                    });
                    pending.push((*child, path));
                }
            }
        }
        for binding in &body.bindings {
            for reach in t.get(binding.ty).unwrap().modes.relation.reaches() {
                if let Reach::Place(origin) = reach {
                    add(origin.fields.iter().map(|field| (0, field.0, 0)).collect());
                }
            }
        }
        Self { paths, active }
    }
}
impl Checker<'_, '_> {
    pub(super) fn receiving_context(&self, state: &State, context: Context) -> Context {
        match context {
            Context::Replacement(binding) => {
                if state.slots[binding].cursor.is_some() {
                    Context::View
                } else {
                    Context::Store
                }
            }
            _ => context,
        }
    }
    pub(super) fn cursor_contract(
        &mut self,
        state: &State,
        index: usize,
        part: &mut Value,
        annotation: Option<(WrittenModes, TypeId)>,
        root_pattern: hirsel_syntax::PatternId,
        source: ExprId,
    ) -> Option<Contract> {
        let binding = &self.body.bindings[index];
        let property = self.properties[binding.ty];
        if !binding.mutable
            || binding.owner != self.graph.owner
            || !self.let_patterns.contains(&root_pattern)
            || !property.data
            || !property.copyable
            || property.plain
        {
            return None;
        }
        let written_cover = annotation
            .filter(|(written, _)| written.relation)
            .and_then(|_| {
                self.tree
                    .statements
                    .iter()
                    .find_map(|statement| match statement.kind {
                        hirsel_syntax::StmtKind::Let { pattern, ty, .. }
                            if pattern == root_pattern =>
                        {
                            ty
                        }
                        _ => None,
                    })
            });
        if !part.borrowed_only || part.aliased().is_none() {
            // J defers `env` on potential mutable data cursors until actual
            // borrowed-only content is known. An owned receipt is still an
            // ordinary binding, with the original MODE-1 diagnostic.
            if let Some(site) = written_cover
                && let hirsel_syntax::TypeExprKind::Qualified {
                    relation: Some(relation),
                    ..
                } = &self.tree.types[site.0 as usize].kind
                && relation
                    .places
                    .contains(&hirsel_syntax::RelationPlace::Environment)
            {
                self.diagnostic(
                    const { RuleId::of("MODE-1") },
                    relation.origin.span,
                    "`env` is a closure or function type's environment: a binding is not related to it",
                );
                part.invalid = true;
            }
            return None;
        }
        let mut cover = part.cursor_views.clone();
        let mut explicit = None;
        if annotation.is_some_and(|(written, _)| written.relation) {
            // J erases body-local lending relations to `env`. Recover the
            // written names, then independently check their live value places.
            let mut syntax = written_cover;
            let mut paths = vec![];
            let mut valid = true;
            while let Some(site) = syntax {
                let hirsel_syntax::TypeExprKind::Qualified {
                    base,
                    brand,
                    relation,
                    ..
                } = &self.tree.types[site.0 as usize].kind
                else {
                    break;
                };
                syntax = Some(*base);
                if let Some(relation) = relation {
                    let mut names = self
                        .body
                        .resolution
                        .paths
                        .iter()
                        .filter(|(s, _)| *s == NameSite::Type(site))
                        .map(|(_, name)| name);
                    if brand.is_some() {
                        names.next();
                    }
                    for place in &relation.places {
                        match place {
                            hirsel_syntax::RelationPlace::Path(path) => {
                                if let Some(name) = names.next() {
                                    paths.push((
                                        name.clone(),
                                        path.segments.iter().skip(1).copied().collect::<Vec<_>>(),
                                    ));
                                } else {
                                    valid = false;
                                }
                            }
                            _ => valid = false,
                        }
                    }
                }
            }
            valid &= !paths.is_empty();
            let mut origins = vec![];
            let mut allowed = Views::default();
            for (name, path) in paths {
                let named = self
                    .source_rows
                    .binding(self.body, Some(self.graph.owner), &name);
                let Some(named) = named else {
                    valid = false;
                    continue;
                };
                let outer = self
                    .graph
                    .lifetimes
                    .bindings
                    .get(&named)
                    .copied()
                    .unwrap_or(super::super::cfg::LifetimeScopes::FRAME);
                let inner = self
                    .graph
                    .lifetimes
                    .bindings
                    .get(&index)
                    .copied()
                    .unwrap_or(super::super::cfg::LifetimeScopes::FRAME);
                let earlier = named != index
                    && state.slots[named].available == LIVE
                    && self.graph.lifetimes.contains(outer, inner);
                valid &= earlier;
                let root = match name {
                    ResolvedName::Local(local) => OriginRoot::Local(self.body.lexical, local),
                    ResolvedName::Parameter(parameter) => OriginRoot::Parameter(
                        ParameterRef::body(self.body.lexical.site(self.graph.owner), parameter),
                    ),
                    _ => {
                        valid = false;
                        continue;
                    }
                };
                let mut origin = Origin {
                    root,
                    fields: Default::default(),
                };
                let mut ty = self.body.bindings[named].ty;
                let mut fields = vec![];
                let mut projections = vec![];
                for label in path {
                    let expanded = self.signatures.prove_flow(|memo| {
                        hirsel_type_analysis::representation::expand_representation(
                            self.t,
                            self.signatures.m,
                            ty,
                            &mut |_, definition, arguments| {
                                Ok(memo.signature(definition)?.map(|signature| {
                                    (signature, vec![AuthoritySummary::Unknown; arguments.len()])
                                }))
                            },
                            None,
                        )
                    });
                    let expanded = match expanded {
                        Ok(expanded) => expanded,
                        Err(error) => {
                            if let hirsel_type_analysis::representation::RepresentationFailure::Query(query) = error.failure {
                                *self.proof_error.borrow_mut() = Some(query);
                            }
                            valid = false;
                            break;
                        }
                    };
                    let TypeKind::Record(parts) = &self.t.get(expanded).unwrap().kind else {
                        valid = false;
                        break;
                    };
                    let Some((field, part)) = parts
                        .iter()
                        .enumerate()
                        .find(|(_, part)| part.name == label)
                    else {
                        valid = false;
                        break;
                    };
                    ty = part.ty;
                    origin.fields.push(FieldId(field as u32));
                    projections.push(Projection::Field(field as u32));
                    fields.push(ty);
                }
                let mut view = if state.slots[named].value.aliased().is_some()
                    && state.slots[named].value.cursor_views != Views::default()
                {
                    state.slots[named].value.cursor_views.clone()
                } else {
                    Views::root(named)
                };
                view.project(&projections, &self.cursor_observations);
                allowed.anchors.extend(view.anchors);
                origins.push((origin, fields));
            }
            if !valid {
                self.diagnostic(
                    const { RuleId::of("LANG-15") },
                    self.span_of(source),
                    "a cursor cover must name earlier enclosing value places",
                );
                part.invalid = true;
                return None;
            }
            cover = allowed;
            explicit = Some(origins);
        }
        if part.invalid {
            return None;
        }
        if !part.cursor_views.within(&cover) {
            self.diagnostic(
                const { RuleId::of("MODE-1") },
                self.span_of(source),
                "the initial view is outside the cursor's fixed origin cover",
            );
            part.invalid = true;
            return None;
        }
        part.cursor_views.lineages.insert(index);
        cover.lineages.insert(index);
        if self.emit {
            self.facts
                .borrow_cursors
                .push(hirsel_types::BorrowCursorDecision {
                    binding: self.body.bindings[index].site,
                    cover: explicit,
                });
        }
        Some(Contract { cover })
    }
    pub(super) fn rebind_cursor(
        &mut self,
        state: &mut State,
        place: &Place,
        expression: ExprId,
        rhs: ExprId,
        operation: Span,
        compound: bool,
    ) -> bool {
        let Some(contract) = state.slots[place.binding].cursor.clone() else {
            return false;
        };
        if !place.projections.is_empty() || compound {
            return false;
        }
        if !self.body.bindings[place.binding].mutable {
            self.diagnostic(
                const { RuleId::of("LANG-5") },
                operation,
                "an immutable binding cannot be reassigned",
            );
            return true;
        }
        // Ordinary capture/storage loans still conflict with replacing the name.
        if !self.unlent(place, state, operation, false) {
            return true;
        }
        let mut value = self.value(state, rhs);
        if value.invalid {
            return true;
        }
        if !value.borrowed_only || value.usage != UseKind::Borrow || value.aliased().is_none() {
            if self.emit {
                let (name, declaration) = self.binding_site(place.binding);
                let source = value.site.unwrap_or(rhs);
                let mut labels = vec![
                    hirsel_diag::Label {
                        span: declaration,
                        message: format!("`{name}` was declared as a read-only borrow cursor here"),
                    },
                    hirsel_diag::Label {
                        span: self.span_of(source),
                        message: "this expression produces an owned value, not a read-only view"
                            .into(),
                    },
                ];
                if let ExprKind::Call { callee, .. } = self.tree.expressions[source.0 as usize].kind
                {
                    let callee_name = self
                        .spelled(callee)
                        .unwrap_or_else(|| "this callable".into());
                    labels.push(hirsel_diag::Label {
                        span: self.cursor_result_site(callee),
                        message: format!("`{callee_name}` returns an owned value; a result relation does not turn an owned return into a read-only view"),
                    });
                }
                if let Some(diagnostic) = self.diagnostic(
                    const { RuleId::of("LANG-15") },
                    operation,
                    "a borrow cursor requires a read-only view, not an owned refill",
                ) {
                    diagnostic.secondary = labels;
                }
            }
            return true;
        }
        if !value.cursor_views.within(&contract.cover) {
            if self.emit {
                let (name, declaration) = self.binding_site(place.binding);
                let roots: BTreeSet<_> = contract
                    .cover
                    .anchors
                    .iter()
                    .map(|anchor| self.binding_site(anchor.root).0)
                    .collect();
                let mut labels = vec![hirsel_diag::Label {
                    span: declaration,
                    message: format!(
                        "`{name}` fixes its origin cover here (rooted in {})",
                        roots
                            .into_iter()
                            .map(|root| format!("`{root}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                }];
                let conflicts: BTreeSet<_> = value
                    .cursor_views
                    .anchors
                    .iter()
                    .filter(|anchor| {
                        !Views {
                            anchors: BTreeSet::from([(*anchor).clone()]),
                            lineages: value.cursor_views.lineages.clone(),
                        }
                        .within(&contract.cover)
                    })
                    .map(|anchor| anchor.root)
                    .collect();
                let mut boundaries = BTreeSet::new();
                for root in conflicts {
                    let (origin, span) = self.binding_site(root);
                    labels.push(hirsel_diag::Label {
                        span,
                        message: format!(
                            "this incoming view of `{origin}` is outside `{name}`'s fixed cover"
                        ),
                    });
                    if let Some(source) = self.cursor_pattern_source(root)
                        && super::super::cfg::place(self.body, source, self.source_rows).is_none()
                    {
                        boundaries.insert(source);
                    }
                }
                if labels.len() == 1 {
                    labels.push(hirsel_diag::Label {
                        span: self.span_of(rhs),
                        message: "this incoming view requires an origin outside the fixed cover"
                            .into(),
                    });
                }
                for boundary in boundaries {
                    labels.push(hirsel_diag::Label {
                        span: self.span_of(boundary),
                        message: "this computed value supplies a distinct storage origin; covering its inputs does not cover the temporary".into(),
                    });
                }
                if let Some(diagnostic) = self.diagnostic(
                    const { RuleId::of("MODE-1") },
                    operation,
                    "this view is outside the cursor's fixed origin cover",
                ) {
                    diagnostic.secondary = labels;
                    // Override MODE-1's generic result-relation repair: it is
                    // not a verified repair for a cursor's fixed cover.
                    diagnostic.notes.push("help: The cursor's origin cover is fixed at its declaration; later assignments cannot enlarge it.".into());
                }
            }
            return true;
        }
        let mut required = self
            .binding_modes
            .get(&place.binding)
            .cloned()
            .unwrap_or_else(|| {
                self.t
                    .get(self.body.bindings[place.binding].ty)
                    .unwrap()
                    .modes
                    .clone()
            });
        // Origin admission is the fixed cover check above; all other axes
        // retain the binding's ordinary receiving requirements.
        required.relation = value.modes.relation.clone();
        if !self.required(
            state,
            &value,
            &required,
            rhs,
            operation,
            Fact::new(Requirement::AnnotatedBinding, operation, &[]),
        ) {
            return true;
        }
        self.receiving_scope(
            &value,
            self.graph
                .lifetimes
                .bindings
                .get(&place.binding)
                .copied()
                .unwrap_or(0),
            operation,
            Some(place.binding),
        );
        value.cursor_views.lineages.insert(place.binding);
        state.slots[place.binding].value = value;
        state.slots[place.binding].available = LIVE;
        if self.emit {
            self.facts.cursor_rebindings.push(expression);
        }
        true
    }
}
