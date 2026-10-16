use super::facts::*;
use super::identities::Identities;
use super::type_bindings::BindingTypes;
use super::*;
use hirsel_diag::{Label, Obligation, RuleId};
use hirsel_syntax::{Argument, ExprKind, ItemKind, StmtKind, UnaryOp};
use std::collections::{BTreeMap, BTreeSet};
mod causes;
use crate::explain::{Cause, Explanation, Fact, Requirement};

#[derive(Clone)]
struct Frame {
    owner: NameSite,
    allowed: Uses,
    result: Option<TypeId>,
    restriction: &'static str,
}
/// What receives the value whose brand or placement is being judged, for
/// the required side of a PROV explanation.
#[derive(Clone)]
struct Receiving {
    /// "a parameter of `f`", "the annotation of `q`", "the result".
    description: String,
    span: Span,
    value: Option<ExprId>,
    /// The annotated binding, whose brand can be dropped (probe 5a).
    annotated: Option<String>,
    argument: bool,
    result: bool,
}
impl Receiving {
    fn new(description: impl Into<String>, span: Span) -> Self {
        Self {
            description: description.into(),
            span,
            value: None,
            annotated: None,
            argument: false,
            result: false,
        }
    }
}
#[derive(Clone)]
struct FreeUse {
    binding: BindingDecision,
    first: ExprId,
    take: bool,
}

/// Prerequisite proof for one expression, independent of diagnostic storage.
struct Verdict {
    valid: bool,
    /// Call prerequisites begin after callable lookup, as distinct from expression validity.
    call_prerequisites: Option<bool>,
    span: Span,
    branded_use: bool,
}

pub(super) struct Checker<'a> {
    tree: &'a SyntaxTree,
    item: ItemId,
    signature: &'a Signature,
    provider: &'a mut dyn SignatureProvider,
    intrinsics: &'a Intrinsics,
    symbols: &'a hirsel_intern::Interner,
    receiving: Option<Receiving>,
    expression_origins: BTreeMap<ExprId, Origin>,
    out: AuthorityOutput,
    signatures: BTreeMap<DefPath, Option<SignatureRef>>,
    elaborations: BTreeMap<ExprId, usize>,
    resolved: BTreeMap<ExprId, ResolvedName>,
    source_rows: super::source_rows::SourceRows,
    closures: super::closure_rows::ClosureRows,
    captures: super::closure_rows::OrderedRows<CaptureTable>,
    facts: BTreeMap<ExprId, AuthoritySummary>,
    /// Immutable receipts retain the authority of their value derivation in
    /// the creating frame. Capturing that receipt still charges its own type.
    derivations: BTreeMap<BindingKey, (NameSite, AuthoritySummary)>,
    binding_types: BindingTypes,
    /// Borrowed captures the closure bodies being walked see as borrowed
    /// parameters (CLO-1).
    capture_views: BTreeSet<BindingKey>,
    identities: Identities,
    identity_paths: BTreeSet<Vec<FieldId>>,
    /// The identity paths a brand or relation names, directly or under a
    /// projected prefix.
    branded_paths: BTreeSet<Vec<FieldId>>,
    expression_partitions: BTreeMap<ExprId, Identity>,
    /// The arena placement of each part a constructor received.
    part_holdings: BTreeMap<ExprId, BTreeMap<FieldId, Relation>>,
    /// Per-field arena holdings of a record binding declared without `mut`
    /// from a constructor; such a binding is never written or lent `mut`, so
    /// each field keeps what it was constructed with (#82).
    field_holdings: BTreeMap<BindingKey, BTreeMap<FieldId, Relation>>,
    invalid: BTreeMap<TypeId, Span>,
    brand_inputs: BTreeMap<OriginRoot, Vec<TypeId>>,
    indexed_brand_rows: usize,
    mut_parameters: Vec<BindingKey>,
    /// Place expressions visited as a write target or as the base of a larger
    /// place; the read of an ended value is judged at the whole place read.
    unread: BTreeSet<ExprId>,
    /// Reads of ended values, reported unless the enclosing call reports
    /// the ended owner at its own branded use.
    ended_reads: Vec<Diagnostic>,
    verdicts: Vec<Verdict>,
    reported: BTreeSet<(hirsel_span::FileId, u32, u32, String)>,
    /// Temporaries that lent their identity to a brand in the current statement.
    temporaries: Vec<OriginRoot>,
    /// Mut parameters already reported as handed back with an ended brand.
    returned_ended: BTreeSet<BindingKey>,
    identity_changes: BTreeMap<Origin, Span>,
    frames: Vec<Frame>,
    task_values: BTreeMap<ExprId, BTreeSet<ExprId>>,
    task_bindings: BTreeMap<BindingKey, BTreeSet<ExprId>>,
    pending_tasks: BTreeMap<ExprId, Span>,
    loop_task_exits: Vec<BTreeMap<ExprId, Span>>,
    cursor_loops: BTreeMap<hirsel_syntax::StmtId, CursorPlan>,
    /// The callee path of the direct call being entered, which is not a value.
    direct_callee: Option<ExprId>,
    clauses: Vec<(ExprId, TypeId)>,
    in_clause: bool,
}
pub(super) type BindingKey = (NameSite, u8, u32);
fn binding_key(b: &BindingDecision) -> BindingKey {
    match b.binding {
        ResolvedName::Parameter(p) => (b.owner.clone(), 0, p.0),
        ResolvedName::Local(l) => (b.owner.clone(), 1, l.0),
        _ => (b.site.clone(), 2, 0),
    }
}
/// A whole value and its statically observed field prefixes can have different
/// identities after a projected write. Copies retain this partition by value.
/// `held` lists, by field path, the identities of owners a constructor moved
/// into the value: their evidence travels with the field (PROV-2, Take
/// transports existing evidence), so a transfer or write of that field ends
/// them (PROV-1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Identity {
    whole: Origin,
    parts: BTreeMap<Vec<FieldId>, Origin>,
    held: BTreeSet<(Vec<FieldId>, Origin)>,
}
impl Identity {
    fn new(whole: Origin) -> Self {
        Self {
            whole,
            parts: BTreeMap::new(),
            held: BTreeSet::new(),
        }
    }
    /// Held identities at or below `path`.
    fn held_under<'s>(&'s self, path: &'s [FieldId]) -> impl Iterator<Item = &'s Origin> + 's {
        self.held
            .iter()
            .filter(move |(p, _)| p.starts_with(path))
            .map(|(_, origin)| origin)
    }
    /// The held identities beside `path`, which a write at `path` keeps.
    fn held_beside(&self, path: &[FieldId]) -> BTreeSet<(Vec<FieldId>, Origin)> {
        self.held
            .iter()
            .filter(|(p, _)| !p.starts_with(path))
            .cloned()
            .collect()
    }
    fn at(&self, fields: &[FieldId]) -> Origin {
        if fields.is_empty() {
            return self.whole.clone();
        }
        if let Some(origin) = self.parts.get(fields) {
            return origin.clone();
        }
        let mut origin = self.whole.clone();
        origin.fields.extend(fields);
        origin
    }
    pub(super) fn roots(&self) -> impl Iterator<Item = &OriginRoot> {
        std::iter::once(&self.whole)
            .chain(self.parts.values())
            .map(|origin| &origin.root)
    }
    fn projected(&self, fields: &[FieldId]) -> Self {
        Self {
            whole: self.at(fields),
            parts: self
                .parts
                .iter()
                .filter_map(|(path, origin)| {
                    path.strip_prefix(fields)
                        .filter(|tail| !tail.is_empty())
                        .map(|tail| (tail.to_vec(), origin.clone()))
                })
                .collect(),
            held: self
                .held
                .iter()
                .filter_map(|(path, origin)| {
                    path.strip_prefix(fields)
                        .map(|tail| (tail.to_vec(), origin.clone()))
                })
                .collect(),
        }
    }
}
impl<'a> Checker<'a> {
    pub(super) fn new(
        tree: &'a SyntaxTree,
        item: ItemId,
        signature: &'a Signature,
        mut body: CheckedBody,
        provider: &'a mut dyn SignatureProvider,
        intrinsics: &'a Intrinsics,
        symbols: &'a hirsel_intern::Interner,
    ) -> Self {
        let source_rows = super::source_rows::SourceRows::new(&body);
        let closures = super::closure_rows::ClosureRows::new(std::mem::take(&mut body.closures));
        let elaborations = body
            .expressions
            .iter()
            .enumerate()
            .map(|(i, e)| (e.expression, i))
            .collect();
        let resolved = body.resolution.expressions.iter().cloned().collect();
        let binding_types = BindingTypes::new(body.bindings.iter().map(|b| (binding_key(b), b.ty)));
        let identities = Identities::new(body.bindings.iter().map(binding_key));
        let mut_parameters = body
            .bindings
            .iter()
            .filter(|binding| {
                work::count(Counter::AuthorityInvalidationRows);
                binding.usage == UseKind::Mut
                    && matches!(binding.binding, ResolvedName::Parameter(_))
            })
            .map(binding_key)
            .collect();
        let reads = body.resolution.reads.clone();
        let mut identity_paths = BTreeSet::new();
        fn add(paths: &mut BTreeSet<Vec<FieldId>>, fields: &[FieldId]) {
            if !fields.is_empty() {
                paths.insert(fields.to_vec());
            }
        }
        for (_, row) in body.types.iter() {
            for origin in &row.brand.alternatives {
                add(
                    &mut identity_paths,
                    &origin.fields.iter().copied().collect::<Vec<_>>(),
                );
            }
            for reach in row.modes.relation.reaches().iter() {
                if let Reach::Place(origin) = reach {
                    add(
                        &mut identity_paths,
                        &origin.fields.iter().copied().collect::<Vec<_>>(),
                    );
                }
            }
        }
        let selectors = identity_paths.clone();
        for place in &body.places {
            let fields: Vec<_> = place
                .projections
                .iter()
                .map_while(|p| match p.kind {
                    SourceProjectionKind::Field(f)
                    | SourceProjectionKind::VariantField { field: f, .. } => Some(f),
                    SourceProjectionKind::Index(_) => None,
                })
                .collect();
            add(&mut identity_paths, &fields);
        }
        // Callable selectors are relative to their operands. A projected
        // argument or copied subrecord needs those paths in its source root's
        // partition as well as in the received value's partition.
        let mut prefixes = BTreeSet::new();
        for place in &body.places {
            let prefix: Vec<_> = place
                .projections
                .iter()
                .map_while(|p| match p.kind {
                    SourceProjectionKind::Field(f)
                    | SourceProjectionKind::VariantField { field: f, .. } => Some(f),
                    SourceProjectionKind::Index(_) => None,
                })
                .collect();
            if !prefix.is_empty() {
                prefixes.insert(prefix);
            }
        }
        let mut branded_paths = selectors.clone();
        for prefix in prefixes {
            for selector in &selectors {
                let mut path = prefix.clone();
                path.extend(selector);
                identity_paths.insert(path.clone());
                branded_paths.insert(path);
            }
        }
        Self {
            tree,
            item,
            signature,
            provider,
            intrinsics,
            symbols,
            receiving: None,
            expression_origins: BTreeMap::new(),
            out: AuthorityOutput {
                body,
                captures: vec![],
                call_allocations: vec![],
                result_types: vec![],
                identity_joins: vec![],
                identity_fields: vec![],
                cursor_identities: BTreeMap::new(),
                diagnostics: vec![],
                dependencies: vec![],
                reads,
                failure: None,
            },
            signatures: BTreeMap::new(),
            elaborations,
            resolved,
            source_rows,
            closures,
            captures: super::closure_rows::OrderedRows::new(),
            facts: BTreeMap::new(),
            derivations: BTreeMap::new(),
            binding_types,
            capture_views: BTreeSet::new(),
            identities,
            identity_paths,
            branded_paths,
            expression_partitions: BTreeMap::new(),
            part_holdings: BTreeMap::new(),
            field_holdings: BTreeMap::new(),
            invalid: BTreeMap::new(),
            brand_inputs: BTreeMap::new(),
            indexed_brand_rows: 0,
            mut_parameters,
            unread: BTreeSet::new(),
            ended_reads: vec![],
            verdicts: vec![],
            reported: BTreeSet::new(),
            temporaries: vec![],
            returned_ended: BTreeSet::new(),
            identity_changes: BTreeMap::new(),
            frames: vec![],
            task_values: BTreeMap::new(),
            task_bindings: BTreeMap::new(),
            pending_tasks: BTreeMap::new(),
            loop_task_exits: vec![],
            cursor_loops: BTreeMap::new(),
            direct_callee: None,
            clauses: vec![],
            in_clause: false,
        }
    }
    pub(super) fn with_cursor_loops(mut self, loops: &[CursorPlan]) -> Self {
        self.cursor_loops = loops.iter().map(|l| (l.statement, l.clone())).collect();
        self
    }
    pub(super) fn with_clauses(mut self, clauses: &[(ExprId, TypeId)]) -> Self {
        self.clauses = clauses.to_vec();
        self
    }
    pub(super) fn run(mut self) -> AuthorityOutput {
        let item = &self.tree.items[self.item.0 as usize];
        let (body, allowed, result) = match (&item.kind, &self.signature.kind) {
            (ItemKind::Function { body, .. }, SignatureKind::Function(f)) => {
                let result = import_type(&self.signature.types, f.result, &mut self.out.body.types)
                    .ok()
                    .and_then(|t| {
                        bind_parameter_scopes(
                            &mut self.out.body.types,
                            t,
                            &[NameSite::Item(self.item)],
                        )
                        .ok()
                    });
                // Provision comes from parameters, whereas uses limits captured/used authority.
                if !self.validate_header(&self.signature.types, f, item.origin.span) {
                    // A declaration identity error invalidates the body result
                    // contract. Do not diagnose uses under that invalid promise.
                    return self.out;
                }
                let allowed = if let Uses::Only(a) = &f.uses {
                    let parameter_types: Vec<_> = f
                        .parameters
                        .iter()
                        .filter_map(|p| {
                            import_type(&self.signature.types, p.ty, &mut self.out.body.types).ok()
                        })
                        .collect();
                    let parameters: Vec<_> = parameter_types
                        .into_iter()
                        .map(|ty| {
                            let authority = self.authority(ty);
                            if authority == AuthoritySummary::Unknown {
                                AuthoritySummary::inert()
                            } else {
                                authority
                            }
                        })
                        .collect();
                    let generics: Vec<_> = f
                        .generics
                        .iter()
                        .enumerate()
                        .map(|(i, _)| {
                            AuthoritySummary::Known(
                                vec![AuthorityTerm::TypeParameter(TypeParameterRef::bound(
                                    TypeParamId(i as u32),
                                ))]
                                .into(),
                            )
                        })
                        .collect();
                    Uses::Only(substitute_authority(a, &parameters, &generics))
                } else {
                    Uses::Unrestricted
                };
                (*body, allowed, result)
            }
            (ItemKind::Const { value, .. }, _) => (Some(*value), Uses::Unrestricted, None),
            (
                ItemKind::Type {
                    drop: Some(drop), ..
                },
                SignatureKind::Type(t),
            ) => (
                Some(drop.body),
                Uses::Only(
                    t.implementation_authority
                        .as_ref()
                        .unwrap_or(&t.authority)
                        .clone(),
                ),
                None,
            ),
            _ => (None, Uses::Unrestricted, None),
        };
        self.frames.push(Frame {
            owner: NameSite::Item(self.item),
            allowed,
            result,
            restriction: "CAP-1",
        });

        if let Some(result) = result {
            self.out
                .result_types
                .push((NameSite::Item(self.item), result));
        }
        if let Some(body) = body {
            let temporaries = self.temporaries.len();
            self.expr(
                body,
                Some((
                    CaptureContext::Escaping,
                    self.frames.last().and_then(|f| f.result),
                    "CLO-4",
                )),
            );
            self.frame_temporaries(body, temporaries);
            self.return_value(body, self.result_span(body));
            self.check_pending_tasks();
        }
        for (root, function) in self.clauses.clone() {
            let TypeKind::Function(function) =
                self.out.body.types.get(function).unwrap().kind.clone()
            else {
                continue;
            };
            self.frames.push(Frame {
                owner: NameSite::Expression(root),
                allowed: Uses::Only(AuthoritySummary::inert()),
                result: Some(function.result),
                restriction: "FACT-1",
            });
            self.in_clause = true;
            self.expr(root, None);
            self.in_clause = false;
            self.frames.pop();
            self.out
                .result_types
                .push((NameSite::Expression(root), function.result));
        }
        self.out.call_allocations.sort_by_key(|(e, _)| *e);
        self.out.call_allocations.dedup_by_key(|(e, _)| *e);
        self.out.identity_joins.sort();
        self.out.identity_joins.dedup();
        self.out.identity_fields.sort();
        self.out.result_types.sort_by(|(a, _), (b, _)| a.cmp(b));
        self.out.captures = self.captures.into_sorted();
        self.out.body.closures = self.closures.into_sorted();
        self.out
            .dependencies
            .sort_by(|a, b| a.definition.cmp(&b.definition));
        self.out.dependencies.dedup();
        self.out.body.resolution.reads = self.out.reads.clone();
        let reads = std::mem::take(&mut self.ended_reads);
        self.out.diagnostics.extend(reads);
        // This pass is one source unit. Driver sorts source units by manifest path.
        self.out.diagnostics.sort_by_key(|d| {
            let (p, n) = d.rule.as_str().split_once('-').unwrap();
            (
                d.primary.start,
                d.primary.end,
                p.to_owned(),
                n.parse::<u32>().unwrap(),
            )
        });
        self.out.diagnostics.dedup();
        self.out
    }
    /// The value a chain of transparent blocks yields (spec #50): the
    /// innermost tail, which the consumer receives as if written directly.
    fn transparent_value(&self, mut e: ExprId) -> ExprId {
        while let ExprKind::Block {
            tail: Some(tail), ..
        } = self.tree.expressions[e.0 as usize].kind
        {
            e = tail;
        }
        e
    }
    fn result_span(&self, e: ExprId) -> Span {
        self.span(self.transparent_value(e))
    }
    fn callback_result_span(&self, call: ExprId) -> Span {
        if let ExprKind::Call { arguments, .. } = &self.tree.expressions[call.0 as usize].kind {
            if let Some(argument) = arguments.last() {
                if let ExprKind::Closure { body, .. } =
                    self.tree.expressions[argument.value.0 as usize].kind
                {
                    return self.result_span(body);
                }
            }
        }
        self.span(call)
    }
    /// The arenas a relation admits: the arenas it names and those named by
    /// the relation of each binding it names (v22 R3, "within S"). A frame
    /// relation (`env`) alone stands for bindings in scope, which every arena
    /// parameter outlives, so the caller admits any arena there.
    fn admitted_arenas(&self, relation: &Relation, named: Relation) -> Relation {
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
    fn origin_binding(&self, origin: &Origin) -> Option<&BindingDecision> {
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
    fn read_holdings(&self, value: ExprId) -> Option<Relation> {
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
    fn names_bindings(&self, relation: &Relation) -> bool {
        relation
            .reaches()
            .iter()
            .any(|r| matches!(r, Reach::Place(o) if self.binding_place(o)))
    }
    /// Whether a relation place is a whole binding of this body, not an arena.
    fn binding_place(&self, origin: &Origin) -> bool {
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
    fn arena_place(&self, origin: &Origin) -> bool {
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
    fn arena_type(&self, ty: TypeId) -> bool {
        self.signature
            .metadata
            .roles
            .heap
            .as_ref()
            .is_some_and(|heap| is_arena_type(&self.out.body.types, ty, &heap.arena))
    }
    /// A body row's lifetime sources: every reach except arena places.
    fn lifetime(&self, relation: &Relation) -> Relation {
        relation.retain(|r| !matches!(r, Reach::Place(o) if self.arena_place(o)))
    }
    /// A body row's placement: its arena places and regions only.
    fn placement(&self, relation: &Relation) -> Relation {
        relation.retain(|r| match r {
            Reach::Place(o) => self.arena_place(o),
            Reach::Region(_) => true,
            Reach::Environment => false,
        })
    }
    /// Whether a body row's relation places its blocks in an arena or region.
    fn placed(&self, relation: &Relation) -> bool {
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
    fn signature_lifetime(&self, function: &FunctionType, relation: &Relation) -> Relation {
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
    fn span(&self, e: ExprId) -> Span {
        self.tree.expressions[e.0 as usize].origin.span
    }
    fn error(&mut self, rule: &str, span: Span, message: &str) {
        if self.out.failure.is_none() && self.signatures.values().all(Option::is_some) {
            let purity = self.in_clause && (rule.starts_with("CAP-") || rule == "CLO-1");
            let span = if purity {
                self.clauses
                    .iter()
                    .find_map(|(root, _)| {
                        self.frames
                            .iter()
                            .any(|f| f.owner == NameSite::Expression(*root))
                            .then_some(self.span(*root))
                    })
                    .unwrap_or(span)
            } else {
                span
            };
            let rule = if purity { "FACT-1" } else { rule };
            self.emit(Diagnostic::error(RuleId::new(rule).unwrap(), span, message));
        }
    }
    fn emit(&mut self, diagnostic: Diagnostic) {
        if let Some(verdict) = self.verdicts.last_mut() {
            verdict.valid = false;
            if let Some(valid) = &mut verdict.call_prerequisites {
                *valid = false;
            }
            verdict.branded_use |=
                diagnostic.primary == verdict.span && diagnostic.rule.as_str() == "PROV-1";
        }
        let span = diagnostic.primary;
        self.reported.insert((
            span.file,
            span.start,
            span.end,
            diagnostic.rule.as_str().to_owned(),
        ));
        self.out.diagnostics.push(diagnostic);
    }
    fn read(&mut self, d: &DefPath) -> Option<SignatureRef> {
        if d == &self.signature.definition {
            return Some(std::sync::Arc::new(self.signature.clone()));
        }
        if let Some(s) = self.signatures.get(d) {
            return s.clone();
        }
        let result = self.provider.signature(d);
        self.out.reads.push(DependencyRead::Signature {
            definition: d.clone(),
            fingerprint: result.as_ref().ok().map(|s| s.fingerprint),
        });
        let value = match result {
            Ok(s) => {
                self.out.dependencies.push(SignatureDependency {
                    definition: d.clone(),
                    fingerprint: s.fingerprint,
                });
                Some(s)
            }
            Err(QueryError::Diagnostics(ds)) => {
                for diagnostic in ds {
                    self.emit(diagnostic);
                }
                None
            }
            Err(e) => {
                self.out.failure.get_or_insert(e);
                None
            }
        };
        self.signatures.insert(d.clone(), value.clone());
        value
    }
    fn ty(&self, e: ExprId) -> Option<TypeId> {
        self.elaborations
            .get(&e)
            .map(|i| self.out.body.expressions[*i].ty)
    }
    fn set_ty(&mut self, e: ExprId, ty: TypeId) {
        if let Some(i) = self.elaborations.get(&e).copied() {
            self.out.body.expressions[i].ty = ty
        }
    }
    /// A fresh or explicitly detached value owns its blocks alone. Plain bits
    /// own none: their rows keep the one canonical spelling.
    fn set_unique(&mut self, e: ExprId) {
        let Some(t) = self.ty(e) else { return };
        if self.property(t, Bound::Plain) {
            return;
        }
        let mut row = self.out.body.types.get(t).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let t = self.out.body.types.push(row.clone());
        let declarations = self.declarations();
        // The shared handle ends before the next push, which would otherwise
        // copy every row of the body's store.
        let store = self.out.body.types.clone();
        row.modes.sendability =
            structural_sendability(&store, t, &declarations, &mut |path| Ok(self.read(path)))
                .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
        drop(store);
        let t = self.out.body.types.push(row);
        self.set_ty(e, t);
    }
    fn valid(&self, e: ExprId) -> bool {
        self.ty(e).is_some_and(|t| {
            self.out
                .body
                .types
                .get(t)
                .is_some_and(|t| !matches!(t.kind, TypeKind::Error))
        })
    }
    fn binding(&self, e: ExprId) -> Option<BindingDecision> {
        let name = self.resolved.get(&e)?;
        work::count(Counter::AuthoritySourceRows);
        let scope = self
            .source_rows
            .scopes
            .get(&NameSite::Expression(e))
            .map(|i| self.out.body.resolution.reference_scopes[*i].1);
        let i = self.source_rows.binding(&self.out.body, name, scope)?;
        Some(self.out.body.bindings[i].clone())
    }
    fn root_binding(&self, owner: NameSite, name: &ResolvedName) -> Option<&BindingDecision> {
        let i = self
            .source_rows
            .binding(&self.out.body, name, Some(owner))?;
        Some(&self.out.body.bindings[i])
    }
    fn call_row(&self, e: ExprId) -> Option<&CheckedCall> {
        work::count(Counter::AuthoritySourceRows);
        self.source_rows
            .calls
            .get(&e)
            .map(|i| &self.out.body.calls[*i])
    }
    fn pattern_row(&self, p: hirsel_syntax::PatternId) -> Option<&PatternDecision> {
        work::count(Counter::AuthoritySourceRows);
        self.source_rows
            .patterns
            .get(&p)
            .map(|i| &self.out.body.patterns[*i])
    }
    fn pattern_row_mut(&mut self, p: hirsel_syntax::PatternId) -> Option<&mut PatternDecision> {
        work::count(Counter::AuthoritySourceRows);
        let i = *self.source_rows.patterns.get(&p)?;
        self.out.body.patterns.get_mut(i)
    }
    fn binding_site_index(&self, p: NameSite) -> Option<usize> {
        work::count(Counter::AuthoritySourceRows);
        self.source_rows.sites.get(&p).copied()
    }
    fn task_handles(&self, e: ExprId) -> BTreeSet<ExprId> {
        if let Some(handles) = self.task_values.get(&e) {
            return handles.clone();
        }
        if let ExprKind::Unary { value, .. } = self.tree.expressions[e.0 as usize].kind {
            return self.task_handles(value);
        }
        self.binding(e)
            .and_then(|b| self.task_bindings.get(&binding_key(&b)).cloned())
            .unwrap_or_default()
    }
    fn check_pending_tasks(&mut self) {
        for span in self.pending_tasks.values().copied().collect::<Vec<_>>() {
            self.error(
                "CONC-2",
                span,
                "every task handle must be awaited or explicitly detached",
            );
        }
    }
    fn falls_through(&self, e: ExprId) -> bool {
        match &self.tree.expressions[e.0 as usize].kind {
            ExprKind::Return(_) | ExprKind::Break | ExprKind::Continue => false,
            ExprKind::Block { statements, tail } => {
                statements
                    .iter()
                    .all(|s| match self.tree.statements[s.0 as usize].kind {
                        StmtKind::Expr(e) => self.falls_through(e),
                        _ => true,
                    })
                    && tail.is_none_or(|e| self.falls_through(e))
            }
            ExprKind::If {
                then_branch,
                else_branch: Some(other),
                ..
            } => self.falls_through(*then_branch) || self.falls_through(*other),
            ExprKind::Match { arms, .. } => arms.iter().any(|a| self.falls_through(a.body)),
            _ => true,
        }
    }
    fn source_place(&self, e: ExprId) -> &SourcePlace {
        work::count(Counter::AuthoritySourceRows);
        &self.out.body.places[self
            .source_rows
            .places
            .get(&e)
            .copied()
            .expect("H source place")]
    }
    fn place_root_identity(&self, e: ExprId) -> Option<Origin> {
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
    fn value_partition(&self, e: ExprId) -> Option<Identity> {
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
    fn parameter_origins(
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
    fn mutation_identity(&self, expression: ExprId) -> Option<Origin> {
        Some(self.place_partition(expression)?.whole)
    }
    fn identity(&self, e: ExprId) -> Option<Origin> {
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
    fn validate_header(&mut self, store: &TypeStore, f: &FunctionType, span: Span) -> bool {
        let mut valid = true;
        for id in f.parameters.iter().map(|p| p.ty).chain([f.result]) {
            valid &= self.header_type(store, id, f, span, &mut BTreeSet::new());
        }
        valid
    }
    fn header_type(
        &mut self,
        store: &TypeStore,
        id: TypeId,
        f: &FunctionType,
        span: Span,
        seen: &mut BTreeSet<TypeId>,
    ) -> bool {
        if !seen.insert(id) {
            return true;
        }
        let Some(t) = store.get(id) else { return true };
        let mut valid = true;
        if t.brand.alternatives.iter().any(|o| matches!(o.root, OriginRoot::Parameter(ParameterRef::Bound { depth: 0, parameter: p }) if f.parameters.get(p.0 as usize).is_some_and(|p| p.passing == PassingMode::Take))) {
            self.explained(
                "PROV-1",
                span,
                "a brand cannot name a taken parameter",
                Explanation::default()
                    .provided(Fact::new(Cause::TakenOwner, span, &["the parameter"]))
                    .required(Fact::new(Requirement::OwnerOutlives, span, &[])),
            );
            valid = false;
        }
        // A taken arena ends with the call, so nothing the caller still sees
        // can be declared to live in it ("a value in a cannot outlive a").
        // The conservative reading covers every position, as for brands.
        let arena = self
            .signature
            .metadata
            .roles
            .heap
            .as_ref()
            .map(|h| h.arena.clone());
        if t.modes.relation.reaches().iter().any(|r| matches!(r, Reach::Place(o)
            if arena.as_ref().is_some_and(|arena| arena_parameter(store, f, o, 0, arena))
                && matches!(o.root, OriginRoot::Parameter(ParameterRef::Bound { depth: 0, parameter: p }) if f.parameters.get(p.0 as usize).is_some_and(|p| p.passing == PassingMode::Take))))
        {
            self.explained(
                "PROV-4",
                span,
                "a placement cannot name a taken parameter",
                Explanation::default()
                    .provided(Fact::new(Cause::TakenOwner, span, &["the arena parameter"]))
                    .required(Fact::new(Requirement::OwnerOutlives, span, &[])),
            );
            valid = false;
        }
        for child in t.kind.value_parts() {
            valid &= self.header_type(store, child, f, span, seen);
        }
        valid
    }
    /// CAP-2 charges a branded resource at use. Derive this from declarations,
    /// including new core resources, rather than maintaining a handle-name list.
    fn capability_call_rule(
        &mut self,
        intrinsic: Option<Intrinsic>,
        f: &FunctionType,
        missing: &BTreeSet<ParameterRef>,
    ) -> &'static str {
        match intrinsic {
            Some(Intrinsic::UnsafeOperation) => "MEM-9",
            Some(
                Intrinsic::Synchronization
                | Intrinsic::MutexCreate
                | Intrinsic::MutexWith
                | Intrinsic::AtomicCreate,
            ) => "CAP-4",
            Some(
                Intrinsic::HandleOperation
                | Intrinsic::ChannelSend
                | Intrinsic::TaskAwait
                | Intrinsic::TaskDetach,
            ) => "CAP-2",
            _ => {
                let branded_resource = f.parameters.iter().any(|p| {
                    let Some(row) = self.out.body.types.get(p.ty) else {
                        return false;
                    };
                    // A resource charges CAP-2 only for its governing argument.
                    // An unrelated missing capability remains CAP-1 even when
                    // the function also accepts a branded resource.
                    if !row.brand.alternatives.iter().any(|origin| {
                        matches!(&origin.root,
                            OriginRoot::Parameter(p) | OriginRoot::ArgumentBrand(p)
                                if missing.contains(p))
                    }) {
                        return false;
                    }
                    let TypeKind::Named { definition, .. } = &row.kind else {
                        return false;
                    };
                    let definition = definition.clone();
                    self.read(&definition)
                        .is_some_and(|s| matches!(&s.kind, SignatureKind::Type(t) if t.resource))
                });
                if branded_resource { "CAP-2" } else { "CAP-1" }
            }
        }
    }
    fn capability_definition(&mut self, ty: TypeId) -> Option<DefPath> {
        let TypeKind::Named { definition, .. } = self.out.body.types.get(ty)?.kind.clone() else {
            return None;
        };
        let signature = self.read(&definition)?;
        matches!(&signature.kind, SignatureKind::Type(t) if t.capability.is_some())
            .then_some(definition)
    }
    fn scoped_authority(&mut self, summary: AuthoritySummary) -> AuthoritySummary {
        let store = self.out.body.types.clone();
        self.scoped_authority_in(&store, summary)
    }
    /// `scoped_authority`, reading the body's parameter rows from `store`,
    /// which holds every row of the body's store.
    fn scoped_authority_in(
        &mut self,
        store: &TypeStore,
        summary: AuthoritySummary,
    ) -> AuthoritySummary {
        fn expand(
            checker: &mut Checker<'_>,
            store: &TypeStore,
            summary: AuthoritySummary,
            active: &mut BTreeSet<ParameterRef>,
        ) -> AuthoritySummary {
            let AuthoritySummary::Known(terms) = summary else {
                return AuthoritySummary::Unknown;
            };
            terms
                .into_iter()
                .fold(AuthoritySummary::inert(), |a, term| {
                    let next = if let AuthorityTerm::Parameter(
                        p @ ParameterRef::Body { scope, parameter },
                    ) = term
                    {
                        if !active.insert(p) {
                            return AuthoritySummary::Unknown;
                        }
                        let ty = checker
                            .out
                            .body
                            .bindings
                            .iter()
                            .find(|b| {
                                b.owner == scope && b.binding == ResolvedName::Parameter(parameter)
                            })
                            .map(|b| b.ty);
                        let summary = ty
                            .map(|t| match &store.get(t).unwrap().kind {
                                TypeKind::Function(f) => f.authority.clone(),
                                _ => checker.raw_authority(store, t),
                            })
                            .unwrap_or(AuthoritySummary::Unknown);
                        // A preservation parameter's self capture names the
                        // incoming envelope, not a recursively captured value.
                        let expanded = if summary
                            == AuthoritySummary::Known(vec![AuthorityTerm::Parameter(p)].into())
                        {
                            summary
                        } else {
                            expand(checker, store, summary, active)
                        };
                        active.remove(&p);
                        expanded
                    } else {
                        AuthoritySummary::Known(vec![term].into())
                    };
                    a.join(next)
                })
        }
        expand(self, store, summary, &mut BTreeSet::new())
    }
    fn authority(&mut self, id: TypeId) -> AuthoritySummary {
        self.authority_in(&self.out.body.types.clone(), id)
    }
    fn authority_in(&mut self, store: &TypeStore, id: TypeId) -> AuthoritySummary {
        let summary = self.raw_authority(store, id);
        self.scoped_authority(summary)
    }
    fn raw_authority(&mut self, store: &TypeStore, id: TypeId) -> AuthoritySummary {
        let roles = self.signature.metadata.roles.clone();
        let declarations = self.declarations();
        structural_authority(store, id, &roles, &declarations, &mut |_, path| {
            Ok::<_, hirsel_types::QueryError>(path.and_then(|path| self.read(path)))
        })
        .unwrap()
    }
    fn proof_failure<T>(&mut self, error: QueryError, fallback: T) -> T {
        if self.out.failure.is_none() {
            self.out.failure = Some(error);
        }
        fallback
    }
    fn contains_resource(&mut self, root: TypeId) -> bool {
        let mut store = self.out.body.types.clone();
        let mut pending = vec![(root, false)];
        let mut seen = BTreeSet::new();
        let mut nominal_roots = hirsel_types::NominalExpansions::default();
        let mut checked_closures = BTreeSet::new();
        while let Some((t, exit)) = pending.pop() {
            if exit {
                nominal_roots.leave(t, 0);
                continue;
            }
            if !seen.insert(t) {
                continue;
            }
            let Some(row) = store.get(t).cloned() else {
                return true;
            };
            match row.kind {
                TypeKind::Error => return true,
                TypeKind::Named {
                    definition,
                    arguments,
                } => {
                    let Some(signature) = self.read(&definition) else {
                        return true;
                    };
                    let SignatureKind::Type(declaration) = &signature.kind else {
                        return true;
                    };
                    if declaration.resource {
                        return true;
                    }
                    if declaration.bounds.iter().any(|b| {
                        matches!(
                            b,
                            Bound::Plain | Bound::Copyable | Bound::Data | Bound::Flat
                        )
                    }) {
                        continue;
                    }
                    if signature.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque {
                        return true;
                    }
                    if !nominal_roots
                        .enter(
                            t,
                            0,
                            |a, b| semantic_equal(&store, a, &store, b).unwrap_or(false),
                            |a, b| hirsel_types::nominal_arguments_grow(&store, a, &store, b),
                        )
                        .unwrap_or_else(|error| self.proof_failure(error, false))
                    {
                        continue;
                    }
                    pending.push((t, true));
                    let Some(representation) = declaration.representation else {
                        return true;
                    };
                    let representation = if arguments.is_empty() {
                        import_type(&signature.types, representation, &mut store)
                    } else {
                        substitute_type(
                            &signature.types,
                            representation,
                            &mut store,
                            Substitution {
                                binder: Some(hirsel_types::GenericBinder::Bound(0)),
                                roles: &self.signature.metadata.roles,
                                arguments: &arguments,
                                authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                            },
                        )
                    };
                    let Ok(representation) = representation else {
                        return true;
                    };
                    pending.push((representation, false));
                }
                TypeKind::Parameter(_) => {
                    if !self.property_in(store.clone(), t, Bound::Copyable, None) {
                        return true;
                    }
                }
                TypeKind::Function(_) => {
                    // Revision 1 erases captures from function types. Known
                    // constructions retain their complete capture layout;
                    // otherwise a once environment has no resource-free proof.
                    let matching: Vec<_> = self
                        .closures
                        .iter()
                        .filter(|closure| {
                            semantic_equal(&store, t, &self.out.body.types, closure.ty)
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect();
                    if matching.is_empty() && row.modes.multiplicity == Multiplicity::Once {
                        return true;
                    }
                    for closure in matching {
                        if checked_closures.insert(closure.expression) {
                            for capture in closure.captures {
                                let Ok(t) =
                                    import_type(&self.out.body.types, capture.ty, &mut store)
                                else {
                                    return true;
                                };
                                pending.push((t, false));
                            }
                        }
                    }
                }
                TypeKind::Builtin {
                    kind:
                        BuiltinType::Sender
                        | BuiltinType::Receiver
                        | BuiltinType::Task
                        | BuiltinType::Region
                        | BuiltinType::Iter,
                    ..
                } => return true,
                TypeKind::Builtin {
                    kind:
                        BuiltinType::Shared
                        | BuiltinType::SlotId
                        | BuiltinType::Ptr
                        | BuiltinType::FnPtr,
                    ..
                } => (),
                kind => pending.extend(kind.value_parts().into_iter().rev().map(|t| (t, false))),
            }
        }
        false
    }
    fn arena_construction(&mut self, expression: ExprId) {
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
                "MEM-4",
                self.span(expression),
                "an arena construction cannot hold resources",
                Explanation::default()
                    .provided(Fact::new(Cause::Resource, self.span(expression), &[]))
                    .required(Fact::new(Requirement::NoResources, arena, &[])),
            );
        }
    }

    fn declarations(&self) -> GenericDeclarations {
        self.out.body.resolution.generic_declarations.clone()
    }
    /// A declared result at one call: its allocator, origins, lifetime,
    /// authority and erased captures come from the actual arguments.
    fn result_at(
        &mut self,
        declared: TypeId,
        allocation_targets: &BTreeMap<ParameterRef, AllocationTarget>,
        origins: &BTreeMap<ParameterRef, OriginArgument>,
        actuals: &BTreeMap<ParameterRef, TypeId>,
        authorities: &BTreeMap<ParameterRef, AuthoritySummary>,
        captures: &BTreeMap<RelationPlace, CaptureProperties>,
        partitions: &BTreeMap<ParameterRef, Identity>,
    ) -> TypeId {
        let result =
            instantiate_allocation_targets(&mut self.out.body.types, declared, allocation_targets)
                .expect("checked allocator result");
        let result = self.parameter_origins(result, origins, actuals, partitions);
        let types = &mut self.out.body.types;
        let result = instantiate_parameter_authorities(types, result, authorities)
            .expect("checked result authority");
        instantiate_parameter_captures(types, result, captures).expect("checked result captures")
    }
    /// A named function used as a value is the capture-free closure that
    /// forwards its parameters to the definition. No source expression
    /// states that call, so its result is typed here, at the value's own
    /// parameters, and published as the closure's body result.
    fn forwarding_closure(&mut self, e: ExprId) {
        let Some(ty) = self.ty(e) else { return };
        let TypeKind::Function(f) = self.out.body.types.get(ty).unwrap().kind.clone() else {
            return;
        };
        let scope = NameSite::Expression(e);
        let mut allocation_targets = BTreeMap::new();
        let mut origins = BTreeMap::new();
        let mut actuals = BTreeMap::new();
        let mut authorities = BTreeMap::new();
        let mut captures = BTreeMap::new();
        for (ordinal, parameter) in f.parameters.iter().enumerate() {
            let Ok(actual) =
                bind_parameter_scopes(&mut self.out.body.types, parameter.ty, &[scope])
            else {
                return;
            };
            let declared = ParameterRef::bound(ParamId(ordinal as u32));
            let row = self.out.body.types.get(actual).unwrap().clone();
            if let Some(target) = row.allocation_target {
                allocation_targets.insert(declared, target);
            }
            origins.insert(
                declared,
                OriginArgument {
                    identity: Origin {
                        root: OriginRoot::Parameter(ParameterRef::body(
                            scope,
                            ParamId(ordinal as u32),
                        )),
                        fields: vec![].into(),
                    },
                    brand: row.brand,
                    relation: if self.arena_type(actual) {
                        // An Arena argument bounds a result that names it by
                        // its own place: fresh blocks go there (PROV-4).
                        Relation::place(Origin {
                            root: OriginRoot::Parameter(ParameterRef::body(
                                scope,
                                ParamId(ordinal as u32),
                            )),
                            fields: vec![].into(),
                        })
                    } else {
                        row.modes.relation
                    },
                },
            );
            actuals.insert(declared, actual);
            let authority = self.authority(actual);
            authorities.insert(declared, authority.clone());
            let store = self.out.body.types.clone();
            let generics = self.declarations();
            let sendability = structural_sendability(&store, actual, &generics, &mut |path| {
                Ok::<_, hirsel_types::QueryError>(self.read(path))
            })
            .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
            captures.insert(
                declared.into(),
                CaptureProperties::from_type(
                    self.out.body.types.get(actual).unwrap(),
                    authority,
                    sendability,
                ),
            );
        }
        let result = self.result_at(
            f.result,
            &allocation_targets,
            &origins,
            &actuals,
            &authorities,
            &captures,
            &BTreeMap::new(),
        );

        self.closures.insert(
            e,
            ClosureInfo {
                result_evidence: Some(result),
                expression: e,
                captures: vec![],
                ty,
            },
        );
    }
    /// Instantiate a declared graph already in the body store with a call's
    /// generic arguments. Substituted nominal rows take their canonical plain
    /// spelling from their declarations; ownership derives summaries later,
    /// in each row's own generic scope.
    fn substitute(
        &mut self,
        declared: TypeId,
        arguments: &[GenericArgument],
        authorities: &[AuthoritySummary],
    ) -> Result<TypeId, GraphError> {
        // Substitution appends to the body's store itself: a clone left behind
        // in the body would make the first append copy every row.
        let mut store = std::mem::take(&mut self.out.body.types);
        let mark = store.mark();
        let substituted = match substitute_type_in_place(
            &mut store,
            declared,
            Substitution {
                binder: Some(hirsel_types::GenericBinder::Bound(0)),
                roles: &self.signature.metadata.roles,
                arguments,
                authorities,
            },
        ) {
            Ok(substituted) => substituted,
            Err(error) => {
                store.truncate_to(mark);
                self.out.body.types = store;
                return Err(error);
            }
        };
        let canonical = canonical_plain_type(&mut store, substituted, &mut |p| {
            Ok::<_, hirsel_types::QueryError>(self.read(p))
        })
        .unwrap_or_else(|error| self.proof_failure(error, substituted));
        self.out.body.types = store;
        Ok(canonical)
    }
    fn property(&mut self, id: TypeId, bound: Bound) -> bool {
        self.property_in(self.out.body.types.clone(), id, bound, None)
    }
    fn property_in(
        &mut self,
        store: TypeStore,
        id: TypeId,
        bound: Bound,
        declaration_generics: Option<&GenericDeclarations>,
    ) -> bool {
        if bound == Bound::Serializable {
            let generics = declaration_generics
                .cloned()
                .unwrap_or_else(|| self.declarations());
            return structural_serializable(&store, id, &generics, &mut |path| {
                Ok::<_, hirsel_types::QueryError>(self.read(path))
            })
            .unwrap_or_else(|error| self.proof_failure(error, false));
        }
        let property = match bound {
            Bound::Plain => Some(StructuralProperty::Plain),
            Bound::Copyable => Some(StructuralProperty::Copyable),
            Bound::Sendable => Some(StructuralProperty::Sendable),
            _ => None,
        };
        if let Some(property) = property {
            let generics = declaration_generics
                .cloned()
                .unwrap_or_else(|| self.declarations());
            return structural_property(&store, id, property, &generics, &mut |path| {
                Ok::<_, hirsel_types::QueryError>(self.read(path))
            })
            .unwrap_or_else(|error| self.proof_failure(error, false));
        }
        debug_assert_eq!(bound, Bound::Flat);
        let generics = declaration_generics
            .cloned()
            .unwrap_or_else(|| self.declarations());
        structural_flat(&store, id, &generics, &mut |path| {
            Ok::<_, hirsel_types::QueryError>(self.read(path))
        })
        .unwrap_or_else(|error| self.proof_failure(error, false))
    }
    fn scoped(&mut self, id: TypeId) -> bool {
        self.scoped_in(self.out.body.types.clone(), id, &mut BTreeSet::new())
    }
    fn scoped_in(&mut self, store: TypeStore, id: TypeId, active: &mut BTreeSet<DefPath>) -> bool {
        // Specialize generic and erased authority before opening a shape. A
        // nominal generic holder must not hide the scoped seal of its actual.
        if let AuthoritySummary::Known(terms) = self.authority_in(&store, id) {
            for term in terms {
                if let AuthorityTerm::Capability(path) = term {
                    if self.read(&path).is_some_and(|s|
                        matches!(&s.kind, SignatureKind::Type(d) if d.capability == Some(CapabilityKind::Scoped))) {
                        return true;
                    }
                }
            }
            return false;
        }
        let Some(t) = store.get(id) else { return false };
        if t.is_heap_target(&self.signature.metadata.roles) {
            return false;
        }
        if let TypeKind::Named {
            definition,
            arguments,
        } = &t.kind
        {
            let Some(s) = self.read(definition) else {
                return false;
            };
            let SignatureKind::Type(d) = &s.kind else {
                return false;
            };
            if d.capability == Some(CapabilityKind::Scoped) {
                return true;
            }
            if !active.insert(definition.clone()) {
                return false;
            }
            // Opaque authority also exposes contained scoped capabilities.
            if let AuthoritySummary::Known(terms) = &d.authority {
                for term in terms {
                    if let AuthorityTerm::Capability(cap) = term {
                        if self.read(cap).is_some_and(|s| matches!(&s.kind, SignatureKind::Type(t) if t.capability == Some(CapabilityKind::Scoped))) { active.remove(definition); return true }
                    }
                }
            }
            let r = d
                .representation
                .is_some_and(|r| self.scoped_in(s.types.clone(), r, active))
                || arguments.iter().any(|a| match a {
                    GenericArgument::Type(actual) => self.scoped_in(store.clone(), *actual, active),
                    _ => false,
                });
            active.remove(definition);
            return r;
        }
        t.kind
            .value_parts()
            .into_iter()
            .any(|t| self.scoped_in(store.clone(), t, active))
    }
    fn sendable(&mut self, id: TypeId) -> bool {
        self.property(id, Bound::Sendable)
    }
    /// Whether a row's sendability is evidence about what it captures
    /// (MODE-4: "computed where it is built and is part of its type") rather
    /// than a summary of its parts.
    fn carries_crossing_evidence(row: &Type) -> bool {
        matches!(
            row.kind,
            TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                }
        )
    }
    /// MODE-4 for what a value holds: its crossing evidence apart from its
    /// own lifetime, which MODE-1 judges. Placement still counts: an arena
    /// value does not cross.
    fn content_sendable(&mut self, id: TypeId) -> bool {
        let mut store = self.out.body.types.clone();
        let mut row = store.get(id).unwrap().clone();
        row.modes.relation = self.placement(&row.modes.relation);
        let root = store.push(row);
        self.property_in(store, root, Bound::Sendable, None)
    }
    /// A symbolic crossing promise is a conjunction: the actual may require
    /// only dependencies that the receiving type already requires.
    fn receives_sendability(&mut self, actual: TypeId, expected: &Sendability) -> bool {
        let row = self.out.body.types.get(actual).unwrap().clone();
        if row.modes.sendability == Sendability::NotSendable
            && row.modes.relation.named_parameters().is_some()
        {
            return false;
        }
        if row.modes.sendability == Sendability::Sendable {
            return self.content_sendable(actual);
        }
        let mut store = self.out.body.types.clone();
        let mut row = row;
        row.modes.relation = self.placement(&row.modes.relation);
        let actual = store.push(row);
        let generics = self.declarations();
        let derived =
            content_sendability(&store, actual, &generics, &mut |path| Ok(self.read(path)))
                .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
        Self::crossing_satisfies(&derived, expected)
    }
    fn crossing_satisfies(actual: &Sendability, expected: &Sendability) -> bool {
        match (actual, expected) {
            (_, Sendability::NotSendable) | (Sendability::Sendable, _) => true,
            (Sendability::DependsOn(actual), Sendability::DependsOn(expected)) => {
                actual.iter().all(|p| expected.contains(p))
            }
            _ => false,
        }
    }
    /// Spec issue #60 (provisional): a value whose brand owner has ended may
    /// only be dropped. Reading it, as a whole place, is PROV-1 at the read.
    /// A call that itself reports the ended owner at a branded use owns that
    /// failure (section 23), so its arguments' reads are not reported again.
    fn expr(
        &mut self,
        e: ExprId,
        context: Option<(CaptureContext, Option<TypeId>, &'static str)>,
    ) -> bool {
        let span = self.span(e);
        self.verdicts.push(Verdict {
            valid: true,
            call_prerequisites: None,
            span,
            branded_use: false,
        });
        let reads = self.ended_reads.len();
        self.walk(e, context);
        match self.tree.expressions[e.0 as usize].kind {
            ExprKind::Path(_) | ExprKind::Field { .. } | ExprKind::Index { .. } => {
                if !self.unread.remove(&e) && self.valid(e) {
                    if let Some((ty, ended)) = self
                        .ty(e)
                        .and_then(|ty| self.ended(ty).map(|ended| (ty, ended)))
                    {
                        // Inspect variant producers only when an identity has
                        // actually ended. Ordinary reads need no payload proof.
                        let inactive = self
                            .out
                            .body
                            .types
                            .get(ty)
                            .unwrap()
                            .brand
                            .alternatives
                            .is_empty()
                            && self.brandless_variant(e, ty).is_some();
                        if !inactive {
                            let mut diagnostic = Diagnostic::error(
                                RuleId::new("PROV-1").unwrap(),
                                span,
                                "the value's brand owner has ended",
                            );
                            self.ended_explanation(ended, span).render(&mut diagnostic);
                            self.ended_reads.push(diagnostic);
                        }
                    }
                }
            }
            ExprKind::Call { ref arguments, .. } => {
                if self.verdicts.last().unwrap().branded_use {
                    self.ended_reads.truncate(reads);
                }
                // A written `take` hands the callee the only owner: a result
                // branded by that argument has no owner when the call returns.
                for argument in arguments.clone() {
                    let ExprKind::Unary {
                        op: UnaryOp::Take,
                        value: taken,
                    } = self.tree.expressions[argument.value.0 as usize].kind
                    else {
                        continue;
                    };
                    // Result substitution uses the evaluated argument's identity.
                    // A take can detach a copied local from its prior owner;
                    // the operand read still snapshots that earlier identity.
                    let Some(origin) = self
                        .identity(argument.value)
                        .or_else(|| self.identity(taken))
                    else {
                        continue;
                    };
                    let at = self.span(argument.value);
                    self.invalidate_origin(&origin, at);
                    if self.valid(e) && self.ty(e).and_then(|ty| self.ended(ty)) == Some(at) {
                        self.ended_use(
                            span,
                            at,
                            "the result is branded by an argument the call consumed",
                        );
                    }
                }
            }
            _ => (),
        }
        let verdict = self.verdicts.pop().unwrap();
        if let Some(parent) = self.verdicts.last_mut() {
            parent.valid &= verdict.valid;
            if let Some(valid) = &mut parent.call_prerequisites {
                *valid &= verdict.valid;
            }
            parent.branded_use |= verdict.branded_use && parent.span == verdict.span;
        }
        verdict.valid
    }
    fn ended_use(&mut self, span: Span, ended: Span, message: &'static str) {
        let mut diagnostic = Diagnostic::error(RuleId::new("PROV-1").unwrap(), span, message);
        self.ended_explanation(ended, span).render(&mut diagnostic);
        self.emit(diagnostic);
    }
    /// A brand whose owner's identity ended; uses of one ended identity fold.
    fn ended_explanation(&self, ended: Span, used: Span) -> Explanation {
        Explanation {
            root: Some(crate::explain::root_key(
                &self.signature.definition,
                ("ended", ended.start, ended.end),
            )),
            ..Explanation::default()
        }
        .provided(Fact::new(Cause::IdentityEnded, ended, &[]))
        .required(Fact::new(Requirement::BrandedUse, used, &[]))
    }
    fn walk(&mut self, e: ExprId, context: Option<(CaptureContext, Option<TypeId>, &'static str)>) {
        if !self.valid(e) {
            return;
        }
        // These rows snapshot an operand's identity at evaluation time. A
        // backedge reevaluates the expression against its current binding.
        self.expression_origins.remove(&e);
        self.expression_partitions.remove(&e);
        let kind = self.tree.expressions[e.0 as usize].kind.clone();
        if let ExprKind::Closure { body, .. } = kind {
            self.closure(e, body, context);
            return;
        }
        if let ExprKind::Call { callee, arguments } = kind {
            self.call(
                e,
                callee,
                &arguments,
                context.and_then(|(_, expected, _)| expected),
            );
            return;
        }
        let mut used = AuthoritySummary::inert();
        if matches!(
            kind,
            ExprKind::Literal(_) | ExprKind::Sequence(_) | ExprKind::Tuple(_)
        ) {
            if let Some((_, Some(expected), _)) = &context {
                let placement =
                    self.placement(&self.out.body.types.get(*expected).unwrap().modes.relation);
                let mut ty = self
                    .out
                    .body
                    .types
                    .get(self.ty(e).unwrap())
                    .unwrap()
                    .clone();
                if !placement.is_global() && self.placement(&ty.modes.relation).is_global() {
                    ty.modes.relation = ty.modes.relation.union(&placement);
                    let ty = self.out.body.types.push(ty);
                    self.set_ty(e, ty);
                }
                // PROV-1: a sequence's element brands state who each element
                // belongs to. The expected element type supplies them, and
                // every element is then kept against it, so `[p]` transports
                // p's evidence and `[]`, holding no element, satisfies any.
                if matches!(kind, ExprKind::Sequence(_)) {
                    let actual = self.ty(e).unwrap();
                    let branded = self.part_brands(actual, *expected);
                    if branded != actual {
                        self.set_ty(e, branded);
                    }
                }
            }
        }
        match kind {
            ExprKind::Block { statements, tail } => {
                let local_patterns: Vec<_> = statements
                    .iter()
                    .filter_map(|s| match self.tree.statements[s.0 as usize].kind {
                        StmtKind::Let { pattern, .. } => Some(NameSite::Pattern(pattern)),
                        _ => None,
                    })
                    .collect();
                let mut reachable = true;
                for stmt in statements {
                    if !reachable {
                        break;
                    }
                    let temporaries = self.temporaries.len();
                    used = used.join(self.statement(stmt));
                    self.end_temporaries(
                        temporaries,
                        self.tree.statements[stmt.0 as usize].origin.span,
                    );
                    if let StmtKind::Expr(value) = self.tree.statements[stmt.0 as usize].kind {
                        reachable = self.falls_through(value);
                    }
                }
                if let Some(tail) = tail.filter(|_| reachable) {
                    self.expr(tail, context);
                    used = used.join(self.used(tail));
                    self.copy_value_type(e, tail);
                    self.task_values.insert(e, self.task_handles(tail));
                    // A block is transparent (spec issue #50): one that names
                    // storage declared outside it has that storage's identity.
                    // The storage is named, not computed: a binding declared
                    // outside the block (whatever identity it holds), or an
                    // inner block that itself names one. A taken tail names no
                    // storage: its value moves out and consumes the identity.
                    // Lowering applies the same test (`names_outer_storage`).
                    let place = self.source_place(tail);
                    let outer = match &place.root {
                        SourceRoot::Binding { binding, .. } => {
                            !matches!(place.access, SourceAccess::Unique | SourceAccess::Take)
                                && !place
                                    .projections
                                    .iter()
                                    .any(|p| matches!(p.kind, SourceProjectionKind::Index(_)))
                                && !self.out.body.bindings.iter().any(|b| {
                                    local_patterns.contains(&b.site) && b.binding == *binding
                                })
                        }
                        _ => self.expression_origins.contains_key(&tail),
                    };
                    if let Some(origin) = self.identity(tail).filter(|_| outer) {
                        self.expression_origins.insert(e, origin);
                        self.keep_partition(e, tail);
                    }
                }
                // Origins of locals leaving a block have ended: the binding's
                // own name, and the identity of a computed value it was the
                // only holder of.
                let leaving: Vec<_> = self
                    .out
                    .body
                    .bindings
                    .iter()
                    .filter(|b| local_patterns.contains(&b.site))
                    .cloned()
                    .collect();
                let keys: BTreeSet<_> = leaving.iter().map(binding_key).collect();
                for b in &leaving {
                    let held: BTreeSet<_> = self
                        .identities
                        .get(&binding_key(b))
                        .into_iter()
                        .flat_map(Identity::roots)
                        .filter(|root| matches!(root, OriginRoot::Expression(_)))
                        .filter(|root| !self.identities.held_outside(root, &keys))
                        .cloned()
                        .collect();
                    let own = match b.binding {
                        ResolvedName::Local(l) => Some(OriginRoot::Local(l)),
                        _ => None,
                    };
                    for root in own.into_iter().chain(held) {
                        self.invalidate_origin(
                            &Origin {
                                root,
                                fields: vec![].into(),
                            },
                            self.span(e),
                        );
                    }
                }
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expr(condition, None);
                used = used.join(self.used(condition));
                let before = self.binding_types.clone();
                let invalid_before = self.invalid.clone();
                let identities_before = self.identities.clone();
                let changes_before = self.identity_changes.clone();
                let pending_before = self.pending_tasks.clone();
                self.expr(then_branch, context.clone());
                used = used.join(self.used(then_branch));
                let then_types = self.binding_types.clone();
                let then_invalid = self.invalid.clone();
                let then_identities = self.identities.clone();
                let then_changes = self.identity_changes.clone();
                let then_live = self.falls_through(then_branch);
                let pending_then = if then_live {
                    self.pending_tasks.clone()
                } else {
                    BTreeMap::new()
                };
                self.pending_tasks = pending_before;
                self.binding_types = before.clone();
                self.invalid = invalid_before;
                self.identities = identities_before.clone();
                self.identity_changes = changes_before.clone();
                let else_live = if let Some(other) = else_branch {
                    self.expr(other, context);
                    used = used.join(self.used(other));
                    self.falls_through(other)
                } else {
                    true
                };
                if !else_live {
                    self.pending_tasks.clear();
                    self.binding_types = then_types.clone();
                    self.invalid = then_invalid.clone();
                } else if then_live {
                    // Joined rows are immutable: expire their inputs before
                    // publishing them, so the new rows retain either path's expiry.
                    self.invalid.extend(then_invalid);
                }
                if !else_live {
                    self.identities = if then_live {
                        then_identities
                    } else {
                        identities_before
                    };
                    self.identity_changes = if then_live {
                        then_changes
                    } else {
                        changes_before
                    };
                } else if then_live {
                    self.join_identities(
                        NameSite::Expression(e),
                        &identities_before,
                        &then_identities,
                    );
                    self.identity_changes.extend(then_changes);
                }
                self.pending_tasks.extend(pending_then);
                if let Some(other) = else_branch {
                    self.join_expression(e, &[then_branch, other]);
                }
                if then_live && else_live {
                    self.join_bindings(&before, &then_types);
                }
            }
            ExprKind::Match { value, arms } => {
                self.expr(value, None);
                used = used.join(self.used(value));
                let before = self.binding_types.clone();
                let before_invalid = self.invalid.clone();
                let before_identities = self.identities.clone();
                let before_changes = self.identity_changes.clone();
                let mut branch_identities = Vec::new();
                let mut changes = before_changes.clone();
                let mut branch_types = Vec::new();
                let mut invalid = BTreeMap::new();
                let mut results = Vec::new();
                let pending_before = self.pending_tasks.clone();
                let mut pending_after = BTreeMap::new();
                for arm in arms {
                    self.pending_tasks = pending_before.clone();
                    self.binding_types = before.clone();
                    self.invalid = before_invalid.clone();
                    self.identities = before_identities.clone();
                    self.identity_changes = before_changes.clone();
                    if let Some(actual) = self.ty(value) {
                        self.bind_pattern(arm.pattern, actual, value);
                    }
                    if let Some(guard) = arm.guard {
                        self.expr(guard, None);
                        used = used.join(self.used(guard));
                    }
                    self.expr(arm.body, context.clone());
                    used = used.join(self.used(arm.body));
                    results.push(arm.body);
                    if self.falls_through(arm.body) {
                        pending_after.extend(self.pending_tasks.clone());
                        branch_types.push(self.binding_types.clone());
                        branch_identities.push(self.identities.clone());
                        changes.extend(self.identity_changes.clone());
                        invalid.extend(self.invalid.clone());
                    }
                }
                self.pending_tasks = pending_after;
                self.identity_changes = changes;
                self.identities = before_identities.clone();
                let mut identities = branch_identities.into_iter();
                if let Some(first) = identities.next() {
                    self.identities = first;
                    for branch in identities {
                        self.join_identities(NameSite::Expression(e), &before_identities, &branch);
                    }
                }
                self.invalid = before_invalid;
                self.invalid.extend(invalid);
                self.join_expression(e, &results);
                self.binding_types = before.clone();
                let mut branches = branch_types.into_iter();
                if let Some(first) = branches.next() {
                    for key in before.changed_keys(&[&first]) {
                        if let Some(actual) = first.get(&key) {
                            self.binding_types.insert(key, *actual);
                        }
                    }
                    for branch in branches {
                        self.join_bindings(&before, &branch);
                    }
                }
            }
            ExprKind::Path(_) => {
                if let Some(b) = self.binding(e) {
                    if let Some(mut ty) = self.binding_types.get(&binding_key(&b)).copied() {
                        let projections = self.source_place(e).projections.clone();
                        let projected = !projections.is_empty();
                        let mut complete = true;
                        let mut origin = self.place_root_identity(e);
                        for projection in projections {
                            let Some((fields, index)) =
                                self.projection_fields(ty, &projection.kind)
                            else {
                                complete = false;
                                break;
                            };
                            let replacements = origin
                                .as_ref()
                                .map(|o| {
                                    fields
                                        .iter()
                                        .enumerate()
                                        .map(|(i, _)| {
                                            let mut field = o.clone();
                                            field.fields.push(FieldId(i as u32));
                                            (FieldId(i as u32), field)
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            ty = super::facts::siblings(
                                &mut self.out.body.types,
                                fields[index].ty,
                                &replacements,
                            );
                            if let Some(o) = &mut origin {
                                o.fields.push(FieldId(index as u32));
                            }
                        }
                        if !projected {
                            self.set_ty(e, ty);
                        } else if complete {
                            self.refine_projection(e, ty);
                            self.localize_view_part(e, Some(binding_key(&b)));
                        }
                        if complete {
                            if let Some(o) = origin {
                                self.expression_origins.insert(
                                    e,
                                    if self
                                        .source_place(e)
                                        .projections
                                        .iter()
                                        .any(|p| matches!(p.kind, SourceProjectionKind::Index(_)))
                                    {
                                        o
                                    } else {
                                        self.mutation_identity(e).unwrap_or(o)
                                    },
                                );
                            }
                        }
                    }
                }
                used = if matches!(self.resolved.get(&e), Some(ResolvedName::Definition(_)))
                    && self.ty(e).is_some_and(|t| {
                        matches!(
                            self.out.body.types.get(t).unwrap().kind,
                            TypeKind::Function(_)
                        )
                    }) {
                    // A direct callee names its definition; only a value
                    // use is a closure.
                    if self.direct_callee != Some(e) {
                        self.forwarding_closure(e);
                    }
                    AuthoritySummary::inert()
                } else {
                    self.ty(e)
                        .map(|t| self.authority(t))
                        .unwrap_or(AuthoritySummary::Unknown)
                };
                if let Some(binding) = self.binding(e) {
                    if !self.source_place(e).projections.is_empty() {
                        // Dotted source paths are a single Path node, whereas
                        // postfix projections have a Field node. Both derive
                        // their value from the same entrusted root.
                        used = self.authority(binding.ty);
                    }
                    if let Some((owner, derived)) = self.derivations.get(&binding_key(&binding)) {
                        if self
                            .frames
                            .last()
                            .is_some_and(|frame| frame.owner == *owner)
                        {
                            used = derived.clone();
                        }
                    }
                }
            }
            ExprKind::Tuple(es) | ExprKind::Sequence(es) => {
                let sequence = matches!(
                    self.tree.expressions[e.0 as usize].kind,
                    ExprKind::Sequence(_)
                );
                let parts = self.ty(e).map(|t| {
                    let parts = self.out.body.types.get(t).unwrap().kind.value_parts();
                    (t, parts)
                });
                // A tuple typed by its operands follows their refined rows.
                let mut refined = parts.clone().filter(|_| !sequence);
                for (ordinal, child) in es.into_iter().enumerate() {
                    let written = self.ty(child);
                    let expected_part = parts.as_ref().and_then(|(_, parts)| {
                        parts.get(if sequence { 0 } else { ordinal }).copied()
                    });
                    let contextual = expected_part
                        .filter(|part| {
                            self.out.body.types.get(*part).is_some_and(|t| {
                                matches!(t.kind, TypeKind::Function(_))
                                    && crossing_requirement(t) == Sendability::Sendable
                            })
                        })
                        .map(|part| (CaptureContext::Escaping, Some(part), "CLO-4"));
                    self.expr(child, contextual);
                    used = used.join(self.used(child));
                    if let (Some((_, parts)), Some(now)) = (&mut refined, self.ty(child)) {
                        if parts.get(ordinal).copied() == written {
                            parts[ordinal] = self.construction_part_type(child).unwrap_or(now);
                        }
                    }
                    // Every element of a sequence has the one element type.
                    let part = parts.as_ref().and_then(|(owner, parts)| {
                        let part = if sequence {
                            parts.first()
                        } else {
                            refined
                                .as_ref()
                                .and_then(|(_, parts)| parts.get(ordinal))
                                .or_else(|| parts.get(ordinal))
                        };
                        part.map(|part| (*owner, *part))
                    });
                    if let (Some((owner, part)), Some(actual)) = (part, self.ty(child)) {
                        let expected = self.placed_item(owner, part);
                        // A literal tuple or sequence row already carries
                        // its destination's locality.
                        let local_owner = self.ty(e).is_some_and(|t| {
                            !self
                                .lifetime(&self.out.body.types.get(t).unwrap().modes.relation)
                                .is_global()
                        });
                        self.kept(
                            actual,
                            expected,
                            self.span(e),
                            child,
                            false,
                            Receiving::new("this aggregate", self.span(e)),
                            local_owner,
                        );
                    }
                }
                if let Some((owner, refined)) =
                    refined.filter(|refined| Some(refined) != parts.as_ref())
                {
                    let mut row = self.out.body.types.get(owner).unwrap().clone();
                    row.kind = TypeKind::Tuple(refined.into());
                    let ty = self.out.body.types.push(row);
                    self.set_ty(e, ty);
                }
                if let Some(actual) = self.ty(e) {
                    let evidence = self
                        .out
                        .body
                        .types
                        .get(actual)
                        .unwrap()
                        .modes
                        .sendability
                        .clone();
                    let receipt = declared_receipt_evidence(&mut self.out.body.types, actual)
                        .expect("checked literal parts");
                    let mut row = self.out.body.types.get(receipt).unwrap().clone();
                    row.modes.sendability = evidence;
                    let receipt = self.out.body.types.push(row);
                    self.set_ty(e, receipt);
                }
                // PROV-2: a literal is a new value. The expected type it was
                // checked against cannot give it an identity it never had.
                if let Some(ty) = self.ty(e) {
                    let mut row = self.out.body.types.get(ty).unwrap().clone();
                    if !row.brand.alternatives.is_empty() {
                        row.brand = Brand::default();
                        let ty = self.out.body.types.push(row);
                        self.set_ty(e, ty);
                    }
                }
            }
            ExprKind::Unary { op, value } => {
                let inferred_box = op == UnaryOp::Box
                    && self.ty(e).is_some_and(|owner| {
                        matches!(self.out.body.types.get(owner).unwrap().kind,
                            TypeKind::Box(part) if Some(part) == self.ty(value))
                    });
                // An ownership marker passes its operand on unchanged, so a
                // marked callback literal is still built for the parameter.
                let marker = matches!(op, UnaryOp::Take | UnaryOp::Copy | UnaryOp::Mut);
                self.expr(value, context.filter(|_| marker));
                used = self.used(value);
                if inferred_box {
                    let payload = self.construction_part_type(value).unwrap();
                    let mut owner = self
                        .out
                        .body
                        .types
                        .get(self.ty(e).unwrap())
                        .unwrap()
                        .clone();
                    owner.kind = TypeKind::Box(payload);
                    let owner = self.out.body.types.push(owner);
                    self.set_ty(e, owner);
                }
                // Copies preserve provenance but counted copies lose uniqueness.
                if matches!(op, UnaryOp::Take | UnaryOp::Copy | UnaryOp::Mut) {
                    self.copy_value_type(e, value);
                    if op == UnaryOp::Copy {
                        if let Some(t) = self.ty(e) {
                            let mut row = self.out.body.types.get(t).unwrap().clone();
                            row.sendability_requirement = SendabilityRequirement::None;
                            let t = self.out.body.types.push(row);
                            self.set_ty(e, t);
                        }
                        // MEM-11: copying a Shared root acquires an atomic
                        // count on an immutable, never region-placed root, so
                        // the copy is global wherever the root was read.
                        if let Some(t) = self.ty(e) {
                            let row = self.out.body.types.get(t).unwrap();
                            if !self.lifetime(&row.modes.relation).is_global()
                                && matches!(
                                    row.kind,
                                    TypeKind::Builtin {
                                        kind: BuiltinType::Shared,
                                        ..
                                    }
                                )
                            {
                                let mut row = row.clone();
                                row.modes.relation = self.placement(&row.modes.relation);
                                let t = self.out.body.types.push(row);
                                self.set_ty(e, t);
                            }
                        }
                        if let Some(t) = self.ty(e) {
                            if !self.property(t, Bound::Plain) && !self.property(t, Bound::Flat) {
                                let mut ty = self.out.body.types.get(t).unwrap().clone();
                                ty.modes.uniqueness = Uniqueness::Aliased;
                                let root = self.out.body.types.push(ty.clone());
                                let store = self.out.body.types.clone();
                                let declarations = self.declarations();
                                ty.modes.sendability = sendability_after_sharing(
                                    &store,
                                    root,
                                    &declarations,
                                    &mut |path| Ok(self.read(path)),
                                )
                                .unwrap_or_else(|error| {
                                    self.proof_failure(error, Sendability::NotSendable)
                                });
                                let t = self.out.body.types.push(ty);
                                self.set_ty(e, t);
                                self.weaken(value);
                            }
                        }
                    }
                }
                if op == UnaryOp::Unique {
                    self.copy_value_type(e, value);
                    self.set_unique(e);
                    // A unique value is independent of the lifetime of the
                    // operand it was extracted from. One extracted in an
                    // arena or region is bounded by that placement (MEM-5),
                    // which PROV-4 judges where the value is kept.
                    if let Some(t) = self.ty(e) {
                        let mut ty = self.out.body.types.get(t).unwrap().clone();
                        if self.placed(&ty.modes.relation)
                            && self.lifetime(&ty.modes.relation).is_environment()
                        {
                            ty.modes.relation = self.placement(&ty.modes.relation);
                            let t = self.out.body.types.push(ty);
                            self.set_ty(e, t);
                        }
                    }
                }
                if op == UnaryOp::Take {
                    self.invalidate(value, self.span(e), false);
                }
            }
            ExprKind::Field { base, .. } => {
                self.unread.insert(base);
                self.expr(base, None);
                used = self.used(base);
                if let Some(base_ty) = self.ty(base) {
                    if let Some(fields) = self.fields(base_ty) {
                        let ordinal = self
                            .source_place(e)
                            .projections
                            .iter()
                            .find(|p| {
                                work::count(Counter::AuthoritySourceRows);
                                p.expression == e
                            })
                            .and_then(|p| match p.kind {
                                SourceProjectionKind::Field(field)
                                | SourceProjectionKind::VariantField { field, .. } => Some(field),
                                SourceProjectionKind::Index(_) => None,
                            });
                        if let Some(ordinal) = ordinal {
                            if let Some(field) = fields.get(ordinal.0 as usize) {
                                let mut siblings = BTreeMap::new();
                                if let Some(base_origin) = self.identity(base) {
                                    for i in 0..fields.len() {
                                        let mut o = base_origin.clone();
                                        o.fields.push(FieldId(i as u32));
                                        siblings.insert(FieldId(i as u32), o);
                                    }
                                }
                                let ty = super::facts::siblings(
                                    &mut self.out.body.types,
                                    field.ty,
                                    &siblings,
                                );
                                let ty = self.projected_item(base_ty, ty);
                                self.refine_projection(e, ty);
                                self.localize_view_part(
                                    e,
                                    self.binding(base).map(|b| binding_key(&b)),
                                );
                            }
                        }
                    }
                }
            }
            ExprKind::Index { base, index } => {
                self.unread.insert(base);
                self.expr(base, None);
                self.expr(index, None);
                used = self.used(base).join(self.used(index));
                // An element read keeps the brands its sequence's element
                // type names (PROV-1), which a written binding type supplies.
                if let (Some(sequence), Some(element)) = (self.ty(base), self.ty(e)) {
                    if let TypeKind::Builtin {
                        kind: BuiltinType::List | BuiltinType::Array | BuiltinType::Inline,
                        arguments,
                    } = &self.out.body.types.get(sequence).unwrap().kind
                    {
                        if let Some(GenericArgument::Type(contract)) = arguments.first() {
                            let contract = *contract;
                            let branded = self.part_brands(element, contract);
                            let row = self.out.body.types.get(branded).unwrap();
                            let brand = &self.out.body.types.get(contract).unwrap().brand;
                            if row.brand.alternatives.is_empty() && brand.alternatives.len() == 1 {
                                let mut row = row.clone();
                                row.brand = brand.clone();
                                let branded = self.out.body.types.push(row);
                                self.set_ty(e, branded);
                            } else if branded != element {
                                self.set_ty(e, branded);
                            }
                            // PROV-4, OWN-8: an element lives where its
                            // sequence does, as a field of a record does.
                            let placed = self.projected_item(sequence, contract);
                            let mut row = self.out.body.types.get(placed).unwrap().clone();
                            row.brand = Brand::default();
                            let placed = self.out.body.types.push(row);
                            self.refine_projection(e, placed);
                        }
                    }
                }
                self.localize_view_part(e, self.binding(base).map(|b| binding_key(&b)));
                if self.ty(base).is_some_and(|t| {
                    matches!(&self.out.body.types.get(t).unwrap().kind,
                        TypeKind::Builtin { kind: BuiltinType::List | BuiltinType::Array | BuiltinType::Inline, arguments }
                        if arguments.first().is_some_and(|a| matches!(a, GenericArgument::Type(t)
                            if matches!(self.out.body.types.get(*t).unwrap().kind,
                                TypeKind::Builtin { kind: BuiltinType::Packed, .. }))))
                }) {
                    self.explained(
                        "MEM-13",
                        self.span(e),
                        "a sequence of Packed values is sequential and cannot be indexed",
                        Explanation::default()
                    .provided(Fact::new(Cause::Packed, self.span(base), &[]))
                    .required(Fact::new(Requirement::Index, self.span(e), &[])),
                    );
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.expr(left, None);
                self.expr(right, None);
                used = self.used(left).join(self.used(right));
            }
            ExprKind::Try {
                value,
                ref map_error,
            } => {
                self.expr(value, None);
                used = self.used(value);
                if map_error.is_some() && self.valid(value) {
                    self.error_mapper(e, value);
                }
                // The callee result has now been substituted into the caller's
                // origin namespace. Propagation returns that exact success edge.
                if let Some(source) = self.ty(value) {
                    if let TypeKind::Named { arguments, .. } =
                        &self.out.body.types.get(source).unwrap().kind
                    {
                        if let Some(GenericArgument::Type(success)) = arguments.first() {
                            self.set_ty(e, *success);
                        }
                    }
                }
            }
            ExprKind::Before(_) if self.in_clause => (),
            ExprKind::Before(value) => {
                self.expr(value, None);
                used = self.used(value);
            }
            ExprKind::Comptime(value) => {
                self.expr(value, None);
                used = self.used(value);
                // O evaluates this child against the wrapper's checked result;
                // preserve the complete call-refined occurrence, not H's template.
                self.copy_value_type(e, value);
            }
            ExprKind::Break | ExprKind::Continue => {
                if let Some(exits) = self.loop_task_exits.last_mut() {
                    exits.extend(std::mem::take(&mut self.pending_tasks));
                }
            }
            ExprKind::Return(value) => {
                if let Some(value) = value {
                    let expected = self.frames.last().and_then(|f| f.result);
                    self.expr(value, Some((CaptureContext::Escaping, expected, "CLO-4")));
                    used = self.used(value);
                    self.return_value(value, self.span(e));
                }
                self.check_pending_tasks();
                self.pending_tasks.clear();
            }
            _ => (),
        }
        let fresh = match &self.tree.expressions[e.0 as usize].kind {
            ExprKind::Literal(
                hirsel_syntax::Literal::Bytes(_) | hirsel_syntax::Literal::String(_),
            ) => true,
            ExprKind::Sequence(es) | ExprKind::Tuple(es) => {
                es.iter().all(|e| self.unique_construction_part(*e))
            }
            ExprKind::Unary {
                op: UnaryOp::Box,
                value,
            } => self.unique_construction_part(*value),
            _ => false,
        };
        if fresh {
            self.set_unique(e);
        }
        let constructed_parts = match &self.tree.expressions[e.0 as usize].kind {
            ExprKind::Sequence(parts) | ExprKind::Tuple(parts) => Some(parts.clone()),
            ExprKind::Unary {
                op: UnaryOp::Box,
                value,
            } => Some(vec![*value]),
            _ => None,
        };
        if let Some(parts) = constructed_parts {
            self.construction_sendability(e, &parts);
        }
        if matches!(
            self.tree.expressions[e.0 as usize].kind,
            ExprKind::Sequence(_)
                | ExprKind::Tuple(_)
                | ExprKind::Unary {
                    op: UnaryOp::Box,
                    ..
                }
        ) {
            self.arena_construction(e);
        }
        let ty = self.ty(e).unwrap();
        // Preserve nominal provider attempts and their failure policy.
        let _ = self.authority(ty);
        self.facts.insert(e, used);
        self.keep_partition(e, e);
    }
    /// Record `e`'s partition as `value`'s. Partitions stay out of the
    /// recursive walk's frame, whose depth the compiler stack budget bounds.
    fn keep_partition(&mut self, e: ExprId, value: ExprId) {
        if let Some(partition) = self.value_partition(value) {
            self.expression_partitions.insert(e, partition);
        }
    }
    fn used(&self, e: ExprId) -> AuthoritySummary {
        self.facts
            .get(&e)
            .cloned()
            .unwrap_or_else(AuthoritySummary::inert)
    }
    /// Fresh owned storage crosses when each transferred part crosses, even
    /// when its erased environment proof grants no deep-unique permission.
    fn construction_sendability(&mut self, expression: ExprId, parts: &[ExprId]) {
        let Some(root) = self.ty(expression) else {
            return;
        };
        let row = self.out.body.types.get(root).unwrap();
        if !row.modes.relation.is_global() {
            return;
        }
        let proved = parts.iter().all(|part| {
            self.construction_part_type(*part)
                .is_some_and(|ty| self.sendable(ty))
        });
        let mut row = self.out.body.types.get(root).unwrap().clone();
        row.modes.sendability = if proved {
            Sendability::Sendable
        } else {
            Sendability::NotSendable
        };
        let root = self.out.body.types.push(row);
        self.set_ty(expression, root);
    }
    /// MODE-2 construction owns its parts. Plain bits, flat copies and Shared
    /// boundaries need no exclusive source; other counted parts must be taken
    /// from current unique evidence, not copied from a unique borrowed place.
    fn unique_construction_part(&mut self, expression: ExprId) -> bool {
        let Some(ty) = self.ty(expression) else {
            return false;
        };
        if self.property(ty, Bound::Plain) {
            return true;
        }
        let row = self.out.body.types.get(ty).unwrap();
        if matches!(
            row.kind,
            TypeKind::Builtin {
                kind: BuiltinType::Shared,
                ..
            }
        ) {
            return true;
        }
        let unique = row.modes.uniqueness == Uniqueness::Unique;
        let source = self.source_place(expression);
        let owned = matches!(source.root, SourceRoot::Value(_)) && source.projections.is_empty()
            || matches!(source.access, SourceAccess::Take | SourceAccess::Unique)
            || source.projections.is_empty()
                && self
                    .binding(expression)
                    .is_some_and(|binding| binding.usage == UseKind::Take);
        let copying =
            source.access == SourceAccess::Copy || source.access == SourceAccess::Read && !owned;
        if copying && self.property(ty, Bound::Flat) {
            return true;
        }
        unique && owned && !copying
    }
    /// Inferred aggregate parts describe their receiving values. A shared read
    /// cannot retain the source field's exclusive physical storage contract.
    fn construction_part_type(&mut self, expression: ExprId) -> Option<TypeId> {
        let ty = self.ty(expression)?;
        if self.unique_construction_part(expression) {
            return Some(ty);
        }
        let source = self.source_place(expression);
        let owned = matches!(source.root, SourceRoot::Value(_)) && source.projections.is_empty()
            || matches!(source.access, SourceAccess::Take | SourceAccess::Unique)
            || source.projections.is_empty()
                && self
                    .binding(expression)
                    .is_some_and(|binding| binding.usage == UseKind::Take);
        if owned && source.access != SourceAccess::Copy {
            return Some(ty);
        }
        let mut row = self.out.body.types.get(ty)?.clone();
        row.modes.uniqueness = Uniqueness::Aliased;
        let shared = self.out.body.types.push(row.clone());
        let store = self.out.body.types.clone();
        let generics = self.declarations();
        row.modes.sendability =
            sendability_after_sharing(&store, shared, &generics, &mut |path| Ok(self.read(path)))
                .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
        Some(self.out.body.types.push(row))
    }
    fn copy_value_type(&mut self, e: ExprId, value: ExprId) {
        if let (Some(_), Some(src)) = (self.ty(e), self.ty(value)) {
            // A value marker preserves the complete occurrence graph, including
            // origins substituted into nominal arguments and aggregate payloads.
            let ty = self.out.body.types.get(src).unwrap().clone();
            let t = self.out.body.types.push(ty);

            if let Some(stale) = self.invalid.get(&src).copied() {
                self.invalid.insert(t, stale);
            }
            self.set_ty(e, t);
        }
    }
    // Only the selected variant's fields carry value provenance. Generic
    // arguments describing absent fields are contracts, not fabricated values.
    fn brandless_variant(&mut self, expression: ExprId, expected: TypeId) -> Option<ExprId> {
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
    fn join_expression(&mut self, e: ExprId, branches: &[ExprId]) {
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
    fn join_types(&mut self, base: TypeId, ids: &[TypeId]) -> TypeId {
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
    fn join_bindings(&mut self, before: &BindingTypes, branch: &BindingTypes) {
        for site in before.changed_keys(&[branch, &self.binding_types]) {
            if let (Some(base), Some(a), Some(b)) = (
                before.get(&site),
                branch.get(&site),
                self.binding_types.get(&site),
            ) {
                let t = self.join_types(*base, &[*a, *b]);
                self.binding_types.insert(site, t);
            }
        }
    }
    /// Agreeing aliases survive; a divergent binding starts a fresh join identity.
    fn join_identities(&mut self, site: NameSite, before: &Identities, branch: &Identities) {
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
                        let Some(fields) = self.fields(ty) else {
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
                        ty = self.projected_item(ty, part);
                        prefix.push(*field);
                        projections.push((*field, ty));
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
    /// Replay the existing transfer rules until the loop-header judgments agree.
    /// Type rows are immutable and may be freshly allocated during replay, so
    /// convergence compares semantic types and expiration, not arena indices.
    fn loop_body(&mut self, site: NameSite, body: ExprId, condition: Option<ExprId>) {
        let (falls, continues) = self.loop_control(body);
        let incoming = self.identities.clone();
        loop {
            let before: BTreeMap<_, _> = self
                .binding_types
                .iter()
                .map(|(key, ty)| (key.clone(), (*ty, self.expired_type(*ty))))
                .collect();
            let identities = self.identities.clone();
            let identity_changes = self.identity_changes.clone();
            self.expr(body, None);
            if !falls && !continues {
                break;
            }
            self.join_identities(site.clone(), &incoming, &incoming);
            if let Some(condition) = condition {
                self.expr(condition, None);
            }
            let unchanged = identities == self.identities
                && identity_changes == self.identity_changes
                && before.len() == self.binding_types.len()
                && before.iter().all(|(key, (old, expired))| {
                    self.binding_types.get(key).is_some_and(|new| {
                        *expired == self.expired_type(*new)
                            && semantic_equal(
                                &self.out.body.types,
                                *old,
                                &self.out.body.types,
                                *new,
                            )
                            .unwrap_or(false)
                    })
                });
            if unchanged {
                break;
            }
        }
    }
    fn expired_type(&self, root: TypeId) -> bool {
        self.ended(root).is_some()
    }
    /// Where an origin named anywhere in this value's type ended.
    fn ended(&self, root: TypeId) -> Option<Span> {
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
    /// Fallthrough and continue reach this header; nested loops and callable
    /// bodies own their own break/continue destinations.
    fn loop_control(&self, expression: ExprId) -> (bool, bool) {
        fn sequence(
            checker: &Checker<'_>,
            expressions: impl IntoIterator<Item = ExprId>,
        ) -> (bool, bool) {
            let (mut next, mut back) = (true, false);
            for expression in expressions {
                if !next {
                    break;
                }
                let flow = checker.loop_control(expression);
                next = flow.0;
                back |= flow.1;
            }
            (next, back)
        }
        match &self.tree.expressions[expression.0 as usize].kind {
            ExprKind::Continue => (false, true),
            ExprKind::Break => (false, false),
            ExprKind::Return(value) => {
                (false, value.is_some_and(|value| self.loop_control(value).1))
            }
            ExprKind::Closure { .. } => (true, false),
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let prefix = self.loop_control(*condition);
                if !prefix.0 {
                    return prefix;
                }
                let a = self.loop_control(*then_branch);
                let b = else_branch.map_or((true, false), |branch| self.loop_control(branch));
                (a.0 || b.0, prefix.1 || a.1 || b.1)
            }
            ExprKind::Match { value, arms } => {
                let prefix = self.loop_control(*value);
                if !prefix.0 {
                    return prefix;
                }
                arms.iter().fold((false, prefix.1), |flow, arm| {
                    let branch =
                        sequence(self, arm.guard.into_iter().chain(std::iter::once(arm.body)));
                    (flow.0 || branch.0, flow.1 || branch.1)
                })
            }
            ExprKind::Block { statements, tail } => {
                let (mut next, mut back) = (true, false);
                for statement in statements {
                    if !next {
                        break;
                    }
                    let flow = match &self.tree.statements[statement.0 as usize].kind {
                        StmtKind::Expr(value) | StmtKind::Let { value, .. } => {
                            self.loop_control(*value)
                        }
                        StmtKind::Assign { place, value, .. } => sequence(self, [*place, *value]),
                        StmtKind::For { source, .. } => self.loop_control(*source),
                        StmtKind::While { condition, .. } => self.loop_control(*condition),
                        StmtKind::Function { .. } => (true, false),
                    };
                    next = flow.0;
                    back |= flow.1;
                }
                if next {
                    if let Some(tail) = tail {
                        let flow = self.loop_control(*tail);
                        next = flow.0;
                        back |= flow.1;
                    }
                }
                (next, back)
            }
            _ => sequence(self, self.expression_children(expression)),
        }
    }
    fn statement(&mut self, s: hirsel_syntax::StmtId) -> AuthoritySummary {
        let stmt = self.tree.statements[s.0 as usize].clone();
        match stmt.kind {
            StmtKind::Let {
                pattern, ty, value, ..
            } => {
                let written_locality = self
                    .pattern_row(pattern)
                    .and_then(|p| p.annotation)
                    .is_some_and(|written| written.relation);
                let expected = self.pattern_row(pattern).map(|p| p.ty);
                let expected = match (expected, ty) {
                    (Some(expected), Some(annotation)) => {
                        let mut row = self.out.body.types.get(expected).unwrap().clone();
                        row.brand = Brand::default();
                        let expected = self.out.body.types.push(row);
                        let expected = self.annotation(expected, annotation);
                        Some(self.within_bindings(expected, annotation))
                    }
                    _ => expected,
                };
                let context = if ty.is_some()
                    && expected.is_some_and(|t| {
                        matches!(
                            self.out.body.types.get(t).map(|t| &t.kind),
                            Some(TypeKind::Function(_))
                        )
                    }) {
                    Some((CaptureContext::Escaping, expected, "CLO-4"))
                } else {
                    // An inferred function binding supplies scope, not a
                    // declared callable/result promise. Keep literal inference
                    // and its own written uses clause independent of that row.
                    let expected = expected.filter(|t| {
                        !matches!(
                            self.out.body.types.get(*t).unwrap().kind,
                            TypeKind::Function(_)
                        )
                    });
                    Some((CaptureContext::Scoped, expected, "CAP-3"))
                };
                self.expr(value, context);
                if let Some(actual) = self.ty(value) {
                    if ty.is_some() {
                        if let Some(expected) = expected {
                            let (name, site) = match self.tree.patterns[pattern.0 as usize].kind {
                                hirsel_syntax::PatternKind::Binding(symbol) => (
                                    self.sites().name(symbol),
                                    self.tree.patterns[pattern.0 as usize].origin.span,
                                ),
                                _ => (
                                    "_".into(),
                                    self.tree.patterns[pattern.0 as usize].origin.span,
                                ),
                            };
                            let receiving = Receiving {
                                annotated: Some(name.clone()),
                                ..Receiving::new(format!("the annotation of `{name}`"), site)
                            };
                            self.kept(
                                actual,
                                expected,
                                self.span(value),
                                value,
                                false,
                                receiving,
                                false,
                            );
                        }
                    }
                    // An explicit binding annotation fixes its root contract.
                    // Extra initializer provenance is forgotten by reception;
                    // inferred bindings retain the initializer's brand.
                    let actual = if let Some(expected) = expected.filter(|_| ty.is_some()) {
                        let mut row = self.out.body.types.get(actual).unwrap().clone();
                        let contract = self.out.body.types.get(expected).unwrap();
                        row.sendability_requirement = contract.sendability_requirement;
                        row.brand = contract.brand.clone();
                        if contract.modes.multiplicity == Multiplicity::Once {
                            row.modes.multiplicity = Multiplicity::Once;
                        }
                        // The written type permits the cleanup it states,
                        // whatever this initializer proves (MEM-5).
                        row.drop_allocation = contract.drop_allocation.clone();
                        if written_locality {
                            // Written receiving locality is a storage contract,
                            // independently of the initializer's stronger fact.
                            // The bindings a let relation names bound what
                            // it receives, not the storage (v22 R3).
                            row.modes.relation = self.placement(&row.modes.relation).union(
                                &self.lifetime(&contract.modes.relation).retain(
                                    |r| !matches!(r, Reach::Place(o) if self.binding_place(o)),
                                ),
                            );
                        }
                        let mutable = self
                            .out
                            .body
                            .bindings
                            .iter()
                            .any(|b| b.site == NameSite::Pattern(pattern) && b.mutable);
                        if let (TypeKind::Function(held), TypeKind::Function(written)) =
                            (&mut row.kind, &contract.kind)
                        {
                            // Multiplicity is the receiving permission, even
                            // when this initializer can be called many times.
                            row.modes.multiplicity = contract.modes.multiplicity;
                            // The binding promises what its type states, not
                            // what this initializer happens to prove: a later
                            // write or `mut` hand-back is held to the same.
                            // The authority and crossing mode it holds are
                            // fixed by its value (CLO-1, MODE-4) unless a
                            // write may replace it.
                            held.uses = written.uses.clone();
                            held.parameters = written.parameters.clone();
                            held.result = written.result;
                            held.error = written.error;
                            held.allocation = written.allocation;
                            if mutable {
                                held.authority = written.authority.clone();
                                if contract.sendability_requirement
                                    != SendabilityRequirement::Sendable
                                {
                                    row.modes.sendability = contract.modes.sendability.clone();
                                }
                            }
                        } else {
                            row.kind = contract.kind.clone();
                            // An erased state likewise holds what its written
                            // type states, unknown captures (no syntax names
                            // them), once a write or `mut` hand-back may
                            // replace it (spec #91).
                            if mutable {
                                row.erased_captures = None;
                                if Self::carries_crossing_evidence(&row) {
                                    if contract.sendability_requirement
                                        != SendabilityRequirement::Sendable
                                    {
                                        row.modes.sendability = contract.modes.sendability.clone();
                                    }
                                }
                            }
                        }
                        let evidence = row.modes.sendability.clone();
                        let received = self.out.body.types.push(row);
                        let received =
                            declared_receipt_evidence(&mut self.out.body.types, received)
                                .expect("checked binding receipt");
                        let mut row = self.out.body.types.get(received).unwrap().clone();
                        row.modes.sendability = evidence;
                        self.out.body.types.push(row)
                    } else {
                        let mut row = self.out.body.types.get(actual).unwrap().clone();
                        row.sendability_requirement = SendabilityRequirement::None;
                        self.out.body.types.push(row)
                    };
                    self.bind_pattern(pattern, actual, value);
                    if let Some(holdings) = self.part_holdings.get(&value).cloned() {
                        if let Some(binding) = self
                            .out
                            .body
                            .bindings
                            .iter()
                            .find(|b| b.site == NameSite::Pattern(pattern) && !b.mutable)
                        {
                            self.field_holdings.insert(binding_key(binding), holdings);
                        }
                    }
                }
                self.used(value)
            }
            StmtKind::Assign { place, value, .. } => {
                self.unread.insert(place);
                self.expr(place, None);
                self.expr(
                    value,
                    Some((CaptureContext::Escaping, self.ty(place), "CLO-4")),
                );
                if let Some(t) = self.ty(value) {
                    if self.scoped(t) {
                        self.explained(
                            "MODE-6",
                            stmt.origin.span,
                            "a scoped capability cannot be stored",
                            Explanation::default()
                                .provided(Fact::new(Cause::ScopedCapability, self.span(value), &[]))
                                .required(Fact::new(
                                    Requirement::StoreValue,
                                    self.span(place),
                                    &[],
                                )),
                        );
                    }
                }
                if self.protected_field(place) {
                    let name = self.value_name(Some(place));
                    self.explained(
                        "PROV-5",
                        stmt.origin.span,
                        "a sibling brand owner cannot be replaced separately from its record",
                        Explanation::default()
                            .provided(Fact::new(Cause::SiblingOwner, self.span(place), &[]))
                            .required(Fact::new(Requirement::Write, stmt.origin.span, &[&name])),
                    );
                }
                if self
                    .frames
                    .last()
                    .is_some_and(|f| f.restriction == "CONC-5")
                {
                    if let (Some(place_type), Some(value_type)) = (self.ty(place), self.ty(value)) {
                        if self
                            .out
                            .body
                            .types
                            .get(place_type)
                            .unwrap()
                            .modes
                            .uniqueness
                            == Uniqueness::Unique
                            && !self.property(value_type, Bound::Plain)
                            && self
                                .out
                                .body
                                .types
                                .get(value_type)
                                .unwrap()
                                .modes
                                .uniqueness
                                != Uniqueness::Unique
                        {
                            self.explained(
                                "CONC-5",
                                stmt.origin.span,
                                "values stored into a mutex payload must be unique",
                                self.boundary_explanation(
                                    Some(value),
                                    self.span(place),
                                    Cause::Aliased,
                                    Requirement::Unique,
                                ),
                            );
                        }
                    }
                }
                let whole_binding = self.source_place(place).projections.is_empty()
                    && matches!(self.source_place(place).root, SourceRoot::Binding { .. });
                let expected = if whole_binding {
                    self.binding(place).map(|binding| binding.ty)
                } else {
                    self.ty(place)
                };
                if let (Some(actual), Some(expected)) = (self.ty(value), expected) {
                    let receiving = match self.binding(place) {
                        Some(binding) => {
                            let (name, site) = self.sites().binding(&binding);
                            let annotated = match binding.site {
                                NameSite::Pattern(pattern) => self
                                    .pattern_row(pattern)
                                    .is_some_and(|p| p.annotation.is_some()),
                                _ => false,
                            };
                            Receiving {
                                annotated: annotated.then(|| name.clone()),
                                ..Receiving::new(format!("the declared type of `{name}`"), site)
                            }
                        }
                        None => Receiving::new("the assigned place", self.span(place)),
                    };
                    self.kept(
                        actual,
                        expected,
                        stmt.origin.span,
                        value,
                        false,
                        receiving,
                        false,
                    );
                }
                self.invalidate(place, stmt.origin.span, false);
                // The write ends brands naming the storage it replaces, the
                // stored value's own included: it would be kept, not dropped.
                if self.ty(value).is_some_and(|ty| {
                    self.invalid.get(&ty) == Some(&stmt.origin.span)
                        || self.ended(ty) == Some(stmt.origin.span)
                }) {
                    self.ended_use(
                        stmt.origin.span,
                        stmt.origin.span,
                        "the assignment ends the brand of the value it stores",
                    );
                }
                if let (true, Some(b), Some(t)) =
                    (whole_binding, self.binding(place), self.ty(value))
                {
                    // Whole replacement introduces the receiving storage's
                    // namespace, even when its old value was a transferred
                    // owner. Projected writes retain the enclosing identity.
                    if b.usage == UseKind::Take {
                        let root = match b.binding {
                            ResolvedName::Local(local) => OriginRoot::Local(local),
                            ResolvedName::Parameter(parameter) => {
                                OriginRoot::Parameter(ParameterRef::body(b.owner, parameter))
                            }
                            _ => unreachable!("checked binding storage"),
                        };
                        let fresh = Origin {
                            root,
                            fields: vec![].into(),
                        };
                        self.identities
                            .insert(binding_key(&b), Identity::new(fresh.clone()));
                        self.identity_changes.insert(fresh, stmt.origin.span);
                    }
                    if b.usage != UseKind::Mut {
                        let mut row = self.out.body.types.get(t).unwrap().clone();
                        if let Some(expected) = expected {
                            let contract = self.out.body.types.get(expected).unwrap();
                            row.sendability_requirement = contract.sendability_requirement;
                            row.brand = contract.brand.clone();
                            if contract.modes.multiplicity == Multiplicity::Once {
                                row.modes.multiplicity = Multiplicity::Once;
                            }
                            if let (TypeKind::Function(held), TypeKind::Function(written)) =
                                (&mut row.kind, &contract.kind)
                            {
                                row.modes.multiplicity = contract.modes.multiplicity;
                                // The binding still promises what its type
                                // states, whatever this value proves.
                                held.uses = written.uses.clone();
                                held.authority = written.authority.clone();
                                held.allocation = written.allocation;
                            } else {
                                row.kind = contract.kind.clone();
                            }
                        }
                        let received = self.out.body.types.push(row);

                        self.binding_types.insert(binding_key(&b), received);
                    }
                    self.task_bindings
                        .insert(binding_key(&b), self.task_handles(value));
                }
                self.used(place).join(self.used(value))
            }
            StmtKind::Expr(e) => {
                self.expr(e, None);
                self.used(e)
            }
            StmtKind::While { condition, body } => {
                self.expr(condition, None);
                let before = self.binding_types.clone();
                let pending = self.pending_tasks.clone();
                self.loop_task_exits.push(BTreeMap::new());
                self.loop_body(NameSite::Statement(s), body, Some(condition));
                self.pending_tasks
                    .extend(self.loop_task_exits.pop().unwrap());
                self.pending_tasks.extend(pending);
                self.join_bindings(&before, &before);
                self.used(condition).join(self.used(body))
            }
            StmtKind::For {
                source,
                body,
                pattern,
            } => {
                self.expr(source, None);
                if let Some(plan) = self.cursor_loops.get(&s).cloned() {
                    self.cursor_loop(plan, pattern);
                } else if let Some(actual) = self.ty(source) {
                    if let Some(TypeKind::Builtin { kind, arguments }) = self.layout(actual) {
                        if let Some(fields) = (TypeKind::Builtin {
                            kind,
                            arguments: arguments.clone(),
                        })
                        .map_entry_fields()
                        {
                            let Some(pattern_row) = self.pattern_row(pattern) else {
                                return AuthoritySummary::Unknown;
                            };
                            let mut row = self.out.body.types.get(pattern_row.ty).unwrap().clone();
                            row.kind = TypeKind::Tuple(fields.to_vec().into());
                            let tuple = self.out.body.types.push(row);
                            let item = self.projected_item(actual, tuple);
                            self.bind_pattern(pattern, item, source);
                        } else if matches!(
                            kind,
                            BuiltinType::Array
                                | BuiltinType::List
                                | BuiltinType::Inline
                                | BuiltinType::Simd
                                | BuiltinType::Iter
                        ) {
                            if let Some(GenericArgument::Type(item)) = arguments.first() {
                                let item = if kind == BuiltinType::Iter {
                                    *item
                                } else {
                                    self.projected_item(actual, *item)
                                };
                                self.bind_pattern(pattern, item, source);
                            }
                        }
                    }
                }
                let pending = self.pending_tasks.clone();
                self.loop_task_exits.push(BTreeMap::new());
                self.loop_body(NameSite::Statement(s), body, None);
                self.pending_tasks
                    .extend(self.loop_task_exits.pop().unwrap());
                self.pending_tasks.extend(pending);
                self.used(source).join(self.used(body))
            }
            StmtKind::Function {
                body, signature, ..
            } => {
                if let Some(binding) = self
                    .out
                    .body
                    .bindings
                    .iter()
                    .find(|b| {
                        b.site == NameSite::Statement(s)
                            && matches!(b.binding, ResolvedName::Local(_))
                    })
                    .cloned()
                {
                    let original = self.out.body.types.get(binding.ty).unwrap().clone();
                    self.function_value(
                        body,
                        body,
                        NameSite::Statement(s),
                        original,
                        Some(signature),
                        None,
                    );
                } else {
                    self.out.failure.get_or_insert(QueryError::NotImplemented);
                }
                // Declaration creates a local environment; invocation effects are
                // checked in the function's own frame, not charged to its parent.
                self.captures
                    .get(&body)
                    .map(|c| c.authority.clone())
                    .unwrap_or(AuthoritySummary::Unknown)
            }
        }
    }
    fn cursor_result(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
        origins: &BTreeMap<ParameterRef, OriginArgument>,
        actuals: &BTreeMap<ParameterRef, TypeId>,
    ) -> Option<TypeId> {
        let authorities: Vec<_> = arguments
            .iter()
            .map(|a| match a {
                GenericArgument::Type(t) => self.authority(*t),
                _ => AuthoritySummary::inert(),
            })
            .collect();
        let mut store = std::mem::take(&mut self.out.body.types);
        let roles = self.signature.metadata.roles.clone();
        let generics = self.declarations();
        let result = super::cursors::result(
            &mut store,
            &roles,
            &generics,
            path,
            arguments,
            &authorities,
            origins,
            actuals,
            &mut |path| Ok(self.read(path)),
        );
        self.out.body.types = store;
        match result {
            Ok(result) => Some(result),
            Err(error) => {
                self.out.failure.get_or_insert(error);
                None
            }
        }
    }
    fn cursor_loop(&mut self, mut plan: CursorPlan, pattern: hirsel_syntax::PatternId) {
        let Some(source) = self.ty(plan.source) else {
            return;
        };
        let Some(identity) = self.identity(plan.source) else {
            return;
        };
        self.out
            .cursor_identities
            .insert(plan.source, identity.clone());
        if let Err(error) = super::cursors::refine(self, &mut plan, source, &identity) {
            self.out.failure.get_or_insert(error);
            return;
        }
        self.bind_pattern(pattern, plan.step.item, plan.source);
    }
    /// `actual` with the brands of `expected`'s parts at every depth where
    /// the two have the same shape; the root brand stays `actual`'s.
    fn part_brands(&mut self, actual: TypeId, expected: TypeId) -> TypeId {
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
    fn adopt_brands(&mut self, instance: TypeId, source: TypeId) -> TypeId {
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
    fn placed_item(&mut self, owner: TypeId, part: TypeId) -> TypeId {
        let mut store = std::mem::take(&mut self.out.body.types);
        let generics = self.declarations();
        let part = placed_part_type(&mut store, owner, part, &generics, &mut |path| {
            Ok::<_, hirsel_types::QueryError>(self.read(path))
        })
        .expect("checked projection graph");
        self.out.body.types = store;

        part
    }
    fn projected_item(&mut self, owner: TypeId, part: TypeId) -> TypeId {
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
    fn bind_pattern(&mut self, pattern: hirsel_syntax::PatternId, actual: TypeId, value: ExprId) {
        let source = self.source_place(value);
        let owned = matches!(source.access, SourceAccess::Take | SourceAccess::Unique)
            || matches!(source.root, SourceRoot::Value(_))
            || self
                .binding(value)
                .is_some_and(|binding| binding.usage == UseKind::Take);
        let deep_unique = owned
            && self.out.body.types.get(actual).unwrap().modes.uniqueness == Uniqueness::Unique;
        self.bind_pattern_value(pattern, actual, value, deep_unique, false);
    }
    fn bind_pattern_value(
        &mut self,
        pattern: hirsel_syntax::PatternId,
        actual: TypeId,
        value: ExprId,
        deep_unique: bool,
        part: bool,
    ) {
        let Some(decision) = self.pattern_row(pattern).cloned() else {
            return;
        };
        let mut ty = self.out.body.types.get(decision.ty).unwrap().clone();
        let a = self.out.body.types.get(actual).unwrap().clone();
        ty.sendability_requirement = a.sendability_requirement;
        ty.brand = a.brand.clone();
        ty.modes = a.modes.clone();
        ty.kind = a.kind.clone();
        ty.erased_captures = a.erased_captures.clone();
        ty.allocation_target = a.allocation_target;
        // MEM-5: a cleanup summary travels with the value it describes.
        ty.drop_allocation = a.drop_allocation.clone();
        let refined = self.out.body.types.push(ty);

        if let Some(stale) = self.invalid.get(&actual).copied() {
            self.invalid.insert(refined, stale);
        }
        if let Some(row) = self.pattern_row_mut(pattern) {
            row.ty = refined;
        }
        let boundary = matches!(
            self.out.body.types.get(refined).unwrap().kind,
            TypeKind::Builtin {
                kind: BuiltinType::Shared,
                ..
            }
        );
        let deep_unique = deep_unique && !boundary;
        let received = if part && !boundary && !self.property(refined, Bound::Plain) {
            let mut row = self.out.body.types.get(refined).unwrap().clone();
            row.modes.uniqueness = if deep_unique {
                Uniqueness::Unique
            } else {
                Uniqueness::Aliased
            };
            self.out.body.types.push(row)
        } else {
            refined
        };
        let task_handles = self.task_handles(value);
        let value_partition = self.value_partition(value);
        // A taken value, and a copy of a computed value that ends with this
        // statement, make the binding a new owner; a copy of a place keeps
        // that place's identity (PROV-1). Blocks are transparent (spec #50):
        // `{take x}` is `take x`. Lowering applies the same test.
        let value_identity =
            match self.tree.expressions[self.transparent_value(value).0 as usize].kind {
                ExprKind::Unary {
                    op: UnaryOp::Take, ..
                } => None,
                ExprKind::Unary {
                    op: UnaryOp::Copy,
                    value: copied,
                } if self
                    .identity(copied)
                    .is_some_and(|o| o.root == OriginRoot::Expression(copied)) =>
                {
                    None
                }
                _ => self.identity(value),
            };
        // Call results are owned receipts, even when local to an argument
        // (spec #43). Their source elaboration's Borrow is a consumer view,
        // not a copied alias that detaches when this binding is mutated.
        let owned_call = !part
            && matches!(
                self.tree.expressions[value.0 as usize].kind,
                ExprKind::Call { .. }
            );
        let derivation = self
            .frames
            .last()
            .map(|frame| (frame.owner, self.used(value)));
        if let Some(index) = self.binding_site_index(NameSite::Pattern(pattern)) {
            let row = &mut self.out.body.bindings[index];
            row.ty = received;
            if owned_call {
                row.usage = UseKind::Take;
            }
            self.binding_types.insert(binding_key(row), received);
            self.task_bindings.insert(binding_key(row), task_handles);
            if !row.mutable {
                if let Some(derivation) = derivation {
                    self.derivations.insert(binding_key(row), derivation);
                }
            }
            if let Some(o) = value_identity {
                self.identities.insert(
                    binding_key(row),
                    value_partition.unwrap_or_else(|| Identity::new(o)),
                );
            }
        }
        let mut inspected = actual;
        if !matches!(
            decision.kind,
            PatternDecisionKind::Binding(_) | PatternDecisionKind::Wildcard
        ) {
            while let Some(TypeKind::Box(payload)) = self.layout(inspected) {
                inspected = self.projected_item(inspected, payload);
            }
        }
        let actual_fields: Vec<_> = match self.layout(inspected) {
            Some(TypeKind::Record(fields)) => fields.into_iter().map(|field| field.ty).collect(),
            Some(TypeKind::Tuple(fields)) => fields.to_vec(),
            Some(TypeKind::Variant(variants)) => match &decision.kind {
                PatternDecisionKind::Constructor(ContextualName::Variant { variant, .. }) => {
                    variants
                        .get(variant.0 as usize)
                        .map(|variant| variant.fields.iter().map(|field| field.1).collect())
                        .unwrap_or_default()
                }
                _ => vec![],
            },
            Some(kind) => kind.value_parts(),
            None => vec![],
        };
        let roots: BTreeMap<_, _> = decision
            .fields
            .iter()
            .filter_map(|(field, child)| {
                let b = self.out.body.bindings.iter().find(|b| {
                    work::count(Counter::AuthoritySourceRows);
                    b.site == NameSite::Pattern(*child)
                })?;
                let root = match b.binding {
                    ResolvedName::Local(l) => OriginRoot::Local(l),
                    _ => return None,
                };
                Some((
                    *field,
                    Origin {
                        root,
                        fields: vec![].into(),
                    },
                ))
            })
            .collect();
        for (field, child) in decision.fields {
            if let Some(part) = actual_fields.get(field.0 as usize) {
                let part = super::facts::siblings(&mut self.out.body.types, *part, &roots);
                let part = self.projected_item(refined, part);
                self.bind_pattern_value(child, part, value, deep_unique, true);
                if let Some(b) = self.out.body.bindings.iter().find(|b| {
                    work::count(Counter::AuthoritySourceRows);
                    b.site == NameSite::Pattern(child)
                }) {
                    self.identities.remove(&binding_key(b));
                }
            }
        }
    }
    /// A projection used as another field's declared brand is an indivisible owner.
    fn protected_field(&mut self, mut e: ExprId) -> bool {
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
    fn frame_temporaries(&mut self, body: ExprId, from: usize) {
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
    fn return_value(&mut self, value: ExprId, span: Span) {
        let Some(t) = self.ty(value) else { return };
        if self.scoped(t) {
            let result = self.sites().result(self.frames.last().unwrap().owner);
            self.explained(
                "MODE-6",
                span,
                "a scoped capability cannot be returned",
                Explanation::default()
                    .provided(Fact::new(Cause::ScopedCapability, self.span(value), &[]))
                    .required(Fact::new(Requirement::ReturnValue, result, &[])),
            );
            return;
        }
        if let Some(frame) = self.frames.last() {
            let relation = self.out.body.types.get(t).unwrap().modes.relation.clone();
            for origin in relation.reaches().iter().filter_map(|r| match r {
                Reach::Place(o) if self.arena_place(o) => Some(o),
                _ => None,
            }) {
                if matches!(frame.owner, NameSite::Expression(_))
                    && matches!(origin.root, OriginRoot::Parameter(ParameterRef::Body { scope, .. }) if scope == frame.owner)
                {
                    let (arena, site) = self.origin_site(origin);
                    let x = self.value_name(Some(value));
                    let callback = self.sites().owner_span(frame.owner);
                    self.explained(
                        "PROV-4",
                        span,
                        "a value cannot escape the arena introduced by this callback",
                        Explanation::default()
                            .provided(Fact::new(
                                Cause::Placed,
                                site.unwrap_or(span),
                                &[
                                    &x,
                                    &arena,
                                    &crate::explain::qualifier(crate::explain::Qualifier::In(&[
                                        &arena,
                                    ])),
                                ],
                            ))
                            .required(Fact::new(Requirement::Arena, callback, &[])),
                    );
                    return;
                }
            }
        }
        if let Some(expected) = self.frames.last().and_then(|f| f.result) {
            let owner = self.frames.last().unwrap().owner;
            let receiving = Receiving {
                result: true,
                ..Receiving::new("the result", self.sites().result(owner))
            };
            self.kept(t, expected, span, value, false, receiving, false);
        }
    }
    /// A mapper receives the owned error payload, not the enclosing Result.
    /// Its occurrence requirements still apply even though no ordinary call
    /// expression or argument appears in the source.
    fn error_mapper(&mut self, expression: ExprId, value: ExprId) {
        let Some(path) = self
            .out
            .body
            .resolution
            .paths
            .iter()
            .find_map(|(site, name)| {
                if *site == NameSite::Expression(expression)
                    && let ResolvedName::Definition(path) = name
                {
                    Some(path.clone())
                } else {
                    None
                }
            })
        else {
            return;
        };
        let Some(signature) = self.read(&path) else {
            return;
        };
        let SignatureKind::Function(function) = &signature.kind else {
            return;
        };
        let [parameter] = function.parameters.as_slice() else {
            return;
        };
        let Some(source) = self.ty(value) else { return };
        let TypeKind::Named { arguments, .. } = &self.out.body.types.get(source).unwrap().kind
        else {
            return;
        };
        let [_, GenericArgument::Type(error)] = arguments.as_slice() else {
            return;
        };
        let declared_error = *error;
        let error = self.projected_item(source, declared_error);
        let error = if !self.property(error, Bound::Plain)
            && !matches!(
                self.out.body.types.get(error).unwrap().kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Shared,
                    ..
                }
            ) {
            // The failure branch takes a part, just like a taken pattern.
            // Current deep uniqueness comes from its owning Result, rather
            // than the error type argument's written holder contract.
            let mut row = self.out.body.types.get(error).unwrap().clone();
            row.modes.uniqueness = self.out.body.types.get(source).unwrap().modes.uniqueness;
            self.out.body.types.push(row)
        } else {
            error
        };
        let arguments = self
            .out
            .body
            .instantiations
            .iter()
            .find(|row| row.expression == expression && row.callee == path)
            .map(|row| row.arguments.clone())
            .unwrap_or_default();
        let authorities: Vec<_> = arguments
            .iter()
            .map(|argument| match argument {
                GenericArgument::Type(ty) => self.authority(*ty),
                _ => AuthoritySummary::inert(),
            })
            .collect();
        let Some(expected) =
            import_type(&signature.types, parameter.ty, &mut self.out.body.types).ok()
        else {
            return;
        };
        let expected = if arguments.is_empty() {
            expected
        } else {
            self.substitute(expected, &arguments, &authorities)
                .expect("checked mapper instantiation")
        };
        let span = self.span(expression);
        let saved = self.receiving.replace(Receiving {
            argument: true,
            ..Receiving::new(
                format!("the error parameter of `{}`", path.0.name),
                self.span(expression),
            )
        });
        let actual_modes = self.out.body.types.get(error).unwrap().modes.clone();
        let required_modes = self.out.body.types.get(expected).unwrap().modes.clone();
        if required_modes.uniqueness == Uniqueness::Unique
            && actual_modes.uniqueness != Uniqueness::Unique
            && !self.property(error, Bound::Plain)
        {
            self.explained(
                "MODE-2",
                span,
                "the error mapper requires a unique error payload",
                Explanation::default()
                    .provided(Fact::new(Cause::Aliased, self.span(value), &[]))
                    .required(Fact::new(Requirement::Unique, span, &[])),
            );
        }
        self.compatible(error, expected, span, None, false);
        self.receiving = saved;
        // H checks ordinary structural bounds; crossing and authority bounds
        // use the actual error's evidence here, as for explicit calls.
        for (formal, actual) in function.generics.iter().zip(&arguments) {
            let (GenericParameter::Type { bounds, .. }, GenericArgument::Type(actual)) =
                (formal, actual)
            else {
                continue;
            };
            for bound in bounds {
                let (valid, rule) = match bound {
                    Bound::Sendable => (
                        self.receives_sendability(*actual, &Sendability::Sendable),
                        "MODE-4",
                    ),
                    Bound::Inert => (
                        subset(&self.authority(*actual), &AuthoritySummary::inert()),
                        "CAP-2",
                    ),
                    _ => continue,
                };
                if !valid {
                    self.explained(
                        rule,
                        span,
                        "error mapper actual lacks its declared bound",
                        Explanation::default()
                            .provided(Fact::new(
                                if *bound == Bound::Sendable {
                                    Cause::NotSendable
                                } else {
                                    Cause::Authority
                                },
                                self.span(value),
                                &[],
                            ))
                            .required(Fact::new(
                                if *bound == Bound::Sendable {
                                    Requirement::Sendable
                                } else {
                                    Requirement::Uses
                                },
                                self.span(expression),
                                &[],
                            )),
                    );
                }
            }
        }
    }
    fn callable(&mut self, callee: ExprId) -> Option<(FunctionType, Option<DefPath>)> {
        if let Some(ResolvedName::Definition(d)) = self.resolved.get(&callee).cloned() {
            let s = self.read(&d)?;
            if let SignatureKind::Function(f) = &s.kind {
                let mut imported = f.clone();
                for p in &mut imported.parameters {
                    p.ty = import_type(&s.types, p.ty, &mut self.out.body.types).ok()?;
                }
                imported.result = import_type(&s.types, f.result, &mut self.out.body.types).ok()?;
                return Some((imported, Some(d)));
            }
        }
        // A dotted callee (`h.action`) resolves to its root binding, whose
        // row is the holder's: only a whole binding is called by its row.
        let whole = self
            .source_rows
            .places
            .get(&callee)
            .is_none_or(|i| self.out.body.places[*i].projections.is_empty());
        let t = self
            .binding(callee)
            .filter(|_| whole)
            .and_then(|b| self.binding_types.get(&binding_key(&b)).copied())
            .or_else(|| self.ty(callee))?;
        if let TypeKind::Function(f) = &self.out.body.types.get(t)?.kind {
            Some((f.clone(), None))
        } else {
            None
        }
    }
    fn argument_construction_type(
        &mut self,
        expected: TypeId,
        arguments: &[Argument],
        expression: ExprId,
    ) -> TypeId {
        if matches!(
            self.out.body.types.get(expected).unwrap().kind,
            TypeKind::Function(_)
        ) {
            return expected;
        }
        let partitions: BTreeMap<_, _> = arguments
            .iter()
            .enumerate()
            .filter_map(|(i, argument)| {
                self.value_partition(argument.value)
                    .map(|partition| (ParameterRef::bound(ParamId(i as u32)), partition))
            })
            .collect();
        let mut origins = BTreeMap::new();
        let mut actuals = BTreeMap::new();
        let mut targets = BTreeMap::new();
        for (i, argument) in arguments.iter().enumerate() {
            let parameter = ParameterRef::bound(ParamId(i as u32));
            let Some(ty) = self.ty(argument.value) else {
                continue;
            };
            let row = self.out.body.types.get(ty).unwrap().clone();
            actuals.insert(parameter, ty);
            if let Some(target) = row.allocation_target {
                targets.insert(parameter, target);
            }
            if let Some(identity) = self.identity(argument.value) {
                // An Arena argument bounds a result that names it by its own
                // place: fresh blocks go there (PROV-4).
                let relation = if self.arena_type(ty) {
                    Relation::place(identity.clone())
                } else {
                    row.modes.relation
                };
                origins.insert(
                    parameter,
                    OriginArgument {
                        identity,
                        brand: row.brand,
                        relation,
                    },
                );
            }
        }
        // A copied or taken constructor call is built under the same context.
        let mut expression = expression;
        while let ExprKind::Unary {
            op: UnaryOp::Copy | UnaryOp::Take,
            value,
        } = self.tree.expressions[expression.0 as usize].kind
        {
            expression = value;
        }
        if let ExprKind::Call { callee, .. } = self.tree.expressions[expression.0 as usize].kind {
            if self.callable(callee).is_none() {
                if let Some(actual) = self.ty(expression) {
                    // H may have seeded nested construction contracts from an
                    // uninstantiated call parameter. Rewrite only that namespace;
                    // caller-owned generic type arguments and operand rows stay intact.
                    let actual =
                        instantiate_allocation_targets(&mut self.out.body.types, actual, &targets)
                            .expect("checked constructor allocator context");
                    let actual = self.parameter_origins(actual, &origins, &actuals, &partitions);
                    self.set_ty(expression, actual);
                }
            }
        }
        let expected = instantiate_allocation_targets(&mut self.out.body.types, expected, &targets)
            .expect("checked construction allocator context");
        let expected = self.parameter_origins(expected, &origins, &actuals, &partitions);
        expected
    }
    fn call(
        &mut self,
        e: ExprId,
        callee: ExprId,
        arguments: &[Argument],
        expected: Option<TypeId>,
    ) {
        let Some(call) = self.call_row(e).cloned() else {
            self.out
                .failure
                .get_or_insert(QueryError::Operational(OperationalFailure {
                    operation: "authority checked call".into(),
                    message: "missing H call mapping".into(),
                }));
            return;
        };
        if matches!(call.target, CallTarget::Constructor(_)) {
            self.constructor(e, callee, arguments, expected);
            return;
        }
        let Some((f, definition)) = self.callable(callee) else {
            for arg in arguments {
                self.expr(arg.value, None);
            }
            return;
        };
        let intrinsic = definition
            .as_ref()
            .and_then(|d| self.intrinsics.get(d))
            .copied();
        self.verdicts.last_mut().unwrap().call_prerequisites = Some(true);
        let source_arguments = arguments;
        let mut ordered: Vec<_> = call
            .arguments
            .iter()
            .filter_map(|a| {
                let (ordinal, _) = a.parameter.as_ref()?;
                source_arguments
                    .iter()
                    .find(|arg| arg.value == a.expression)
                    .map(|arg| (*ordinal as usize, arg.clone()))
            })
            .collect();
        ordered.sort_by_key(|(p, _)| *p);
        let ordered_arguments: Vec<_> = ordered.iter().map(|(_, a)| a.clone()).collect();
        let arguments = ordered_arguments.as_slice();
        if matches!(call.target, CallTarget::Definition(_)) {
            self.direct_callee = Some(callee);
        }
        self.expr(callee, None);
        self.direct_callee = None;
        // A called value's type is complete only after the value is evaluated:
        // a closure literal's inferred result is its body's. The call row's
        // declared result follows the callee, for ownership and lowering.
        let f = if matches!(call.target, CallTarget::Definition(_)) {
            f
        } else {
            let f = self.callable(callee).map_or(f, |(f, _)| f);
            if let Some(row) = self
                .out
                .body
                .calls
                .iter_mut()
                .find(|row| row.expression == e)
            {
                row.result = f.result;
            }
            f
        };
        let mut used = self.used(callee);
        for arg in source_arguments {
            let parameter = call
                .arguments
                .iter()
                .find(|a| a.expression == arg.value)
                .and_then(|a| a.parameter.as_ref().map(|(_, p)| p));
            let authority_arguments: BTreeMap<_, _> = ordered
                .iter()
                .filter_map(|(i, a)| {
                    self.ty(a.value)
                        .map(|t| (ParameterRef::bound(ParamId(*i as u32)), self.authority(t)))
                })
                .collect();
            let expected = parameter.map(|p| {
                instantiate_parameter_authorities(
                    &mut self.out.body.types,
                    p.ty,
                    &authority_arguments,
                )
                .expect("checked callback authority")
            });
            let expected =
                expected.map(|ty| self.argument_construction_type(ty, arguments, arg.value));
            let context = expected.and_then(|t| {
                let ty = self.out.body.types.get(t)?;
                let TypeKind::Function(_) = &ty.kind else {
                    return Some((CaptureContext::Escaping, Some(t), "CLO-4"));
                };
                let ctx = if matches!(
                    intrinsic,
                    Some(Intrinsic::TaskAsync | Intrinsic::TaskIsolate)
                ) {
                    CaptureContext::Task
                } else if !ty.modes.relation.is_global()
                    && ty.modes.multiplicity == Multiplicity::Once
                {
                    CaptureContext::OwningScoped
                } else if parameter.unwrap().passing != PassingMode::Take {
                    CaptureContext::Scoped
                } else {
                    CaptureContext::Escaping
                };
                let callback = if let TypeKind::Function(f) = &ty.kind {
                    f.clone()
                } else {
                    unreachable!()
                };
                let rule = if intrinsic == Some(Intrinsic::MutexWith) {
                    "CONC-5"
                } else if callback.parameters.iter().any(|p| self.scoped(p.ty)) {
                    "CAP-3"
                } else {
                    "CLO-2"
                };
                Some((ctx, expected, rule))
            });
            self.expr(arg.value, context);
            used = used.join(self.used(arg.value));
        }
        if arguments.iter().any(|a| !self.valid(a.value)) {
            return;
        }
        if call.state == CallState::Recovery {
            return;
        }
        if call.state == CallState::MissingCapabilities {
            let supplied: BTreeSet<_> = call
                .arguments
                .iter()
                .filter_map(|a| a.parameter.as_ref().map(|(ordinal, _)| *ordinal))
                .collect();
            let missing = f
                .parameters
                .iter()
                .enumerate()
                .filter_map(|(ordinal, p)| {
                    (!supplied.contains(&(ordinal as u32))
                        && self.capability_definition(p.ty).is_some())
                    .then_some(ParameterRef::bound(ParamId(ordinal as u32)))
                })
                .collect();
            let rule = self.capability_call_rule(intrinsic, &f, &missing);
            self.error(
                rule,
                self.span(e),
                "the operation requires its explicit typed capability argument",
            );
            for a in &call.arguments {
                if let Some((_, p)) = &a.parameter {
                    if matches!(p.passing, PassingMode::Mut | PassingMode::Take) {
                        self.invalidate(a.expression, self.span(e), p.passing == PassingMode::Mut);
                    }
                }
            }
            // Supplied operands still have access effects. Judgments depending on
            // the invalid invocation/result are suppressed under section 23.
            if let Some(ty) = self.ty(e) {
                let mut row = self.out.body.types.get(ty).unwrap().clone();
                row.kind = TypeKind::Error;
                let error = self.out.body.types.push(row);
                self.set_ty(e, error);
            }
            return;
        }
        let args: Vec<_> = arguments
            .iter()
            .map(|a| self.ty(a.value).unwrap())
            .collect();
        let authorities: Vec<_> = args.iter().map(|t| self.authority(*t)).collect();
        let authority_arguments: BTreeMap<_, _> = authorities
            .iter()
            .enumerate()
            .map(|(i, a)| (ParameterRef::bound(ParamId(i as u32)), a.clone()))
            .collect();
        let mut capture_arguments: BTreeMap<_, _> = args
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let store = self.out.body.types.clone();
                let generics = self.declarations();
                let sendability = structural_sendability(&store, *t, &generics, &mut |path| {
                    Ok::<_, hirsel_types::QueryError>(self.read(path))
                })
                .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
                (
                    ParameterRef::bound(ParamId(i as u32)).into(),
                    CaptureProperties::from_type(
                        self.out.body.types.get(*t).unwrap(),
                        authorities[i].clone(),
                        sendability,
                    ),
                )
            })
            .collect();
        for (state, callback, slot) in mutable_capture_states(&self.out.body.types, &f) {
            let store = self.out.body.types.clone();
            let proof = mutable_capture_state_sendability(
                &store,
                args[callback],
                slot,
                &self.declarations(),
                &mut |path| Ok(self.read(path)),
            )
            .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
            if let Some(capture) = capture_arguments.get_mut(&hirsel_types::RelationPlace {
                parameter: ParameterRef::bound(ParamId(state as u32)),
                fields: vec![].into(),
            }) {
                capture.sendability = combine_crossing_evidence(capture.sendability.clone(), proof);
            }
        }
        let mut allocation = f.allocation;
        for (parameter, actual) in f.parameters.iter().zip(&args) {
            if parameter.passing != PassingMode::Take {
                if let TypeKind::Function(callback) =
                    &self.out.body.types.get(*actual).unwrap().kind
                {
                    if callback.allocation == Allocation::MayAllocate {
                        allocation = Allocation::MayAllocate;
                    }
                }
            }
        }
        self.out.call_allocations.retain(|(site, _)| *site != e);
        self.out.call_allocations.push((e, allocation));
        let mut generic_arguments = call.generic_arguments.clone();
        let mut pending: Vec<_> = f
            .parameters
            .iter()
            .zip(&args)
            .map(|(p, a)| (p.ty, *a, 0))
            .collect();
        while let Some((formal, actual, depth)) = pending.pop() {
            match self.out.body.types.get(formal).unwrap().kind.clone() {
                TypeKind::Parameter(p) if p.binder() == GenericBinder::Bound(depth) => {
                    if let Some(GenericArgument::Type(argument)) =
                        generic_arguments.get_mut(p.parameter().0 as usize)
                    {
                        *argument = refine_inferred_value_contract(
                            &mut self.out.body.types,
                            *argument,
                            actual,
                        )
                        .expect("checked generic occurrence");
                    }
                }
                TypeKind::Function(f) => {
                    if let TypeKind::Function(a) = &self.out.body.types.get(actual).unwrap().kind {
                        pending.extend(
                            f.parameters
                                .iter()
                                .map(|p| p.ty)
                                .chain([f.result])
                                .zip(a.parameters.iter().map(|p| p.ty).chain([a.result]))
                                .map(|(a, b)| (a, b, depth + 1)),
                        );
                    }
                }
                kind => pending.extend(
                    kind.value_parts()
                        .into_iter()
                        .zip(self.out.body.types.get(actual).unwrap().kind.value_parts())
                        .map(|(a, b)| (a, b, depth)),
                ),
            }
        }
        // GEN-1/PROV-1: a type argument carries the brands of the values it
        // is instantiated at. The first parameter in declaration order that
        // supplies a single brand fixes it, and every other argument is then
        // checked against it, so a value of another owner cannot enter through
        // a generic parameter. A type argument no parameter mentions types no
        // value the callee receives, so it can produce none: the expected
        // result type instantiates it, and any brand holds vacuously.
        let mut mentioned = BTreeSet::new();
        for parameter in &f.parameters {
            let mut pending = vec![(parameter.ty, 0)];
            while let Some((ty, depth)) = pending.pop() {
                let row = self.out.body.types.get(ty).unwrap();
                if let TypeKind::Parameter(p) = &row.kind {
                    if p.binder() == GenericBinder::Bound(depth) {
                        mentioned.insert(p.parameter().0 as usize);
                    }
                }
                pending.extend(
                    row.kind
                        .graph_edges()
                        .into_iter()
                        .map(|edge| (edge.ty, depth + edge.role.parameter_binders())),
                );
            }
        }
        let supplied = f
            .parameters
            .iter()
            .map(|p| p.ty)
            .zip(args.iter().copied())
            .map(|pair| (pair, true))
            .chain(
                expected
                    .map(|expected| ((f.result, expected), false))
                    .into_iter(),
            );
        for ((formal, actual), argument) in supplied.collect::<Vec<_>>() {
            let mut pending = vec![(formal, actual)];
            while let Some((formal, actual)) = pending.pop() {
                let (Some(formal_row), Some(actual_row)) = (
                    self.out.body.types.get(formal).cloned(),
                    self.out.body.types.get(actual).cloned(),
                ) else {
                    continue;
                };
                match formal_row.kind {
                    TypeKind::Parameter(p) if p.binder() == GenericBinder::Bound(0) => {
                        let index = p.parameter().0 as usize;
                        if argument != mentioned.contains(&index) {
                            continue;
                        }
                        if let Some(GenericArgument::Type(instance)) =
                            generic_arguments.get_mut(index)
                        {
                            *instance = self.adopt_brands(*instance, actual);
                        }
                    }
                    TypeKind::Function(_) => {}
                    kind => {
                        if std::mem::discriminant(&kind) == std::mem::discriminant(&actual_row.kind)
                        {
                            pending.extend(
                                kind.value_parts()
                                    .into_iter()
                                    .zip(actual_row.kind.value_parts()),
                            );
                        }
                    }
                }
            }
        }
        if let Some(row) = self
            .out
            .body
            .instantiations
            .iter_mut()
            .find(|i| i.expression == e && definition.as_ref() == Some(&i.callee))
        {
            row.arguments = generic_arguments.clone();
        }
        let mut refined_parameters = call.arguments.clone();
        if generic_arguments != call.generic_arguments {
            let authorities = generic_arguments
                .iter()
                .map(|a| match a {
                    GenericArgument::Type(t) => self.authority(*t),
                    _ => AuthoritySummary::inert(),
                })
                .collect::<Vec<_>>();
            for argument in &mut refined_parameters {
                if let Some((ordinal, parameter)) = &mut argument.parameter {
                    parameter.ty = self
                        .substitute(
                            f.parameters[*ordinal as usize].ty,
                            &generic_arguments,
                            &authorities,
                        )
                        .expect("refined checked parameter");
                }
            }
        }
        let row = self
            .out
            .body
            .calls
            .iter_mut()
            .find(|row| row.expression == e)
            .unwrap();
        row.generic_arguments = generic_arguments.clone();
        row.arguments = refined_parameters;
        let generic_authorities: Vec<_> = generic_arguments
            .iter()
            .map(|a| match a {
                GenericArgument::Type(t) => self.authority(*t),
                _ => AuthoritySummary::inert(),
            })
            .collect();
        let allocation_targets: BTreeMap<_, _> = args
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                self.out
                    .body
                    .types
                    .get(*t)
                    .unwrap()
                    .allocation_target
                    .map(|a| (ParameterRef::bound(ParamId(i as u32)), a))
            })
            .collect();
        let actuals: BTreeMap<_, _> = args
            .iter()
            .enumerate()
            .map(|(i, t)| (ParameterRef::bound(ParamId(i as u32)), *t))
            .collect();
        let partitions: BTreeMap<_, _> = arguments
            .iter()
            .enumerate()
            .filter_map(|(i, argument)| {
                self.value_partition(argument.value)
                    .map(|partition| (ParameterRef::bound(ParamId(i as u32)), partition))
            })
            .collect();
        let origins: BTreeMap<_, _> = arguments
            .iter()
            .enumerate()
            .filter_map(|(i, a)| {
                let parameter = ParameterRef::bound(ParamId(i as u32));
                self.identity(a.value).map(|origin| {
                    (
                        parameter,
                        OriginArgument {
                            brand: self
                                .ty(a.value)
                                .map(|t| self.out.body.types.get(t).unwrap().brand.clone())
                                .unwrap_or_default(),
                            relation: match self.ty(a.value) {
                                // An Arena argument bounds a result that names
                                // it by its own place (PROV-4).
                                Some(t) if self.arena_type(t) => Relation::place(origin.clone()),
                                Some(t) => {
                                    self.out.body.types.get(t).unwrap().modes.relation.clone()
                                }
                                None => Relation::environment(),
                            },
                            identity: origin,
                        },
                    )
                })
            })
            .collect();
        for (a, origin) in arguments.iter().zip(0u32..).filter_map(|(a, i)| {
            origins
                .get(&ParameterRef::bound(ParamId(i)))
                .map(|o| (a, o.identity.root.clone()))
        }) {
            if matches!(&origin, OriginRoot::Expression(x) if *x == a.value
                || self.source_place(a.value).root == SourceRoot::Value(*x))
                && !self.temporaries.contains(&origin)
            {
                self.temporaries.push(origin);
            }
        }
        let mut required = self.scoped_authority(substitute_authority(
            &f.authority,
            &authorities,
            &generic_authorities,
        ));
        let mut transparent = false;
        for (i, p) in f.parameters.iter().enumerate() {
            let pty = self.out.body.types.get(p.ty).unwrap();
            if matches!(pty.kind, TypeKind::Function(_)) && p.passing == PassingMode::Borrow {
                transparent = true;
                if let Some(a) = authorities.get(i) {
                    required = required.join(a.clone());
                }
            }
        }
        // Calling an unrestricted scoped function parameter has unknown authority.
        if definition.is_none()
            && self.ty(callee).is_some_and(|t| {
                !self
                    .out
                    .body
                    .types
                    .get(t)
                    .unwrap()
                    .modes
                    .relation
                    .is_global()
            })
        {
            required = match &f.uses {
                Uses::Unrestricted => AuthoritySummary::Unknown,
                Uses::Only(a) => self.scoped_authority(substitute_authority(
                    a,
                    &authorities,
                    &generic_authorities,
                )),
            };
            transparent = true;
        }
        let callee_authority = if definition.is_none() {
            self.ty(callee)
                .map(|t| self.authority(t))
                .unwrap_or(AuthoritySummary::inert())
        } else {
            AuthoritySummary::inert()
        };
        let provided = authorities
            .iter()
            .cloned()
            .fold(callee_authority, |a, b| a.join(b));
        let missing = f
            .parameters
            .iter()
            .enumerate()
            .filter_map(|(index, parameter)| {
                let capability = self.capability_definition(parameter.ty)?;
                let supplied = args
                    .get(index)
                    .and_then(|actual| self.capability_definition(*actual));
                (supplied.as_ref() != Some(&capability))
                    .then_some(ParameterRef::bound(ParamId(index as u32)))
            })
            .collect();
        let provision_rule = self.capability_call_rule(intrinsic, &f, &missing);
        let span = self.span(e);
        let mut obligation_valid = self.verdicts.last().unwrap().call_prerequisites.unwrap();
        if obligation_valid {
            // A handle's retained authority is not provision of its governing
            // capability. Missing capability operands belong to CAP before
            // derivative arity/type/brand failures (v20 section 23).
            for (index, parameter) in f.parameters.iter().enumerate() {
                if let Some(capability) = self.capability_definition(parameter.ty) {
                    let supplied = args
                        .get(index)
                        .and_then(|actual| self.capability_definition(*actual));
                    if supplied.as_ref() != Some(&capability) {
                        self.error(
                            provision_rule,
                            span,
                            "the operation requires its explicit typed capability argument",
                        );
                        obligation_valid = false;
                        break;
                    }
                }
            }
        }
        // CAP-1: what the callee needs beyond its operands must be handed in
        // as one. An operand's own authority is supplied by that operand,
        // known or not (an open `mut` function parameter relayed onwards), and
        // an operand of unknown authority hides none of the others.
        let ambient = self.scoped_authority(substitute_authority(
            &f.authority,
            &vec![AuthoritySummary::inert(); f.parameters.len()],
            &generic_authorities,
        ));
        let supplied = authorities
            .iter()
            .chain([&provided])
            .filter(|a| **a != AuthoritySummary::Unknown)
            .cloned()
            .fold(AuthoritySummary::inert(), |a, b| a.join(b));
        if obligation_valid
            && definition.is_some()
            && !(transparent && required == AuthoritySummary::Unknown)
            && !subset(&required, &provided)
            && !subset(&ambient, &supplied)
        {
            self.error(
                provision_rule,
                span,
                "the declared governing capability must be supplied as a typed argument",
            );
            obligation_valid = false;
        }
        if obligation_valid {
            for (i, p) in f.parameters.iter().enumerate() {
                let Some(actual) = args.get(i) else { continue };
                // Rewrite the callee binder before inserting caller-owned generic
                // values: their Parameter roots belong to the caller, not this call.
                let declared = instantiate_allocation_targets(
                    &mut self.out.body.types,
                    p.ty,
                    &allocation_targets,
                )
                .expect("checked allocator arguments");
                let declared = self.parameter_origins(declared, &origins, &actuals, &partitions);
                let declared = instantiate_parameter_authorities(
                    &mut self.out.body.types,
                    declared,
                    &authority_arguments,
                )
                .expect("checked call authority");
                let declared = instantiate_parameter_captures(
                    &mut self.out.body.types,
                    declared,
                    &capture_arguments,
                )
                .expect("checked call captures");
                let expected = if generic_arguments.is_empty() {
                    declared
                } else {
                    self.substitute(declared, &generic_arguments, &generic_authorities)
                        .unwrap_or(declared)
                };
                let receiving_expected = expected;
                // A proved whole-Iter preservation contract is quantified over
                // the actual incoming envelope, even when the formal is narrower.
                let expected = if p.passing == PassingMode::Mut
                    && f.state_promises.iter().any(|promise| {
                        promise.selector.parameter == ParameterRef::bound(ParamId(i as u32))
                            && promise.selector.fields.is_empty()
                            && promise.preserve_captures
                    })
                    && matches!(
                        self.out.body.types.get(*actual).unwrap().kind,
                        TypeKind::Builtin {
                            kind: BuiltinType::Iter,
                            ..
                        }
                    ) {
                    let incoming = self.out.body.types.get(*actual).unwrap();
                    let mut row = self.out.body.types.get(expected).unwrap().clone();
                    row.erased_captures = incoming.erased_captures.clone();
                    row.drop_allocation = incoming.drop_allocation.clone();
                    self.out.body.types.push(row)
                } else {
                    expected
                };
                let actual = self.receive_literal_result(arguments[i].value, *actual, expected);
                let actual = &actual;
                // A taken value is the callee's to keep, so an unplaced take
                // parameter is storage like any other; a borrow cannot keep it.
                let saved = self.receiving.replace(Receiving {
                    description: format!(
                        "a parameter of `{}`",
                        self.spelled(callee).unwrap_or_else(|| "the callee".into())
                    ),
                    span: self.span(callee),
                    value: Some(arguments[i].value),
                    annotated: None,
                    argument: true,
                    result: false,
                });
                let kept = p.passing != PassingMode::Take || {
                    let argument = self.span(arguments[i].value);
                    self.keeps_placement(*actual, expected, argument, false)
                };
                let valid = kept
                    && self.compatible(*actual, receiving_expected, span, intrinsic, false)
                    && (p.passing != PassingMode::Mut
                        || self.keeps_handback_promises(
                            *actual,
                            expected,
                            span,
                            arguments[i].value,
                            callee,
                        ))
                    && self.keeps_nested_brands(*actual, expected, p.passing, arguments[i].value)
                    && self.keeps_nested_uniqueness(
                        *actual,
                        expected,
                        p.passing != PassingMode::Mut,
                        self.span(arguments[i].value),
                    );
                self.receiving = saved;
                if !valid {
                    obligation_valid = false;
                    break;
                }
            }
        }
        if obligation_valid {
            self.generic_obligations(e, &f, arguments, intrinsic);
            // Literal reception may replace its header row. Construction
            // evidence stays indexed by the received row, not this old snapshot.
            let received_args: Vec<_> = arguments.iter().filter_map(|a| self.ty(a.value)).collect();
            self.static_intrinsic(e, intrinsic, &received_args, &f);
            if matches!(
                intrinsic,
                Some(Intrinsic::TaskAwait | Intrinsic::TaskDetach)
            ) {
                if let Some(handle) = arguments.get(1) {
                    for task in self.task_handles(handle.value) {
                        self.pending_tasks.remove(&task);
                    }
                }
            }
            if let Some(frame) = self.frames.last().cloned() {
                if let Uses::Only(allowed) = frame.allowed {
                    // CAP-2/CLO-2, spec #58: captures were checked when the
                    // frame was created. Parameters and calls may derive new
                    // capability values inside it; their operation's input
                    // authority is checked by provision, not as ambient uses.
                    let ambient = if self.in_clause
                        || matches!(frame.owner, NameSite::Item(_))
                        || transparent && required == AuthoritySummary::Unknown
                    {
                        required.clone()
                    } else if definition.is_none()
                        && self.ty(callee).is_some_and(|ty| {
                            !self
                                .out
                                .body
                                .types
                                .get(ty)
                                .unwrap()
                                .modes
                                .relation
                                .is_global()
                        })
                    {
                        match &f.uses {
                            Uses::Unrestricted => AuthoritySummary::Unknown,
                            Uses::Only(_) => AuthoritySummary::inert(),
                        }
                    } else {
                        self.scoped_authority(substitute_authority(
                            &f.authority,
                            &vec![AuthoritySummary::inert(); f.parameters.len()],
                            &generic_authorities,
                        ))
                    };
                    // An explicitly selected callback parameter supplies its
                    // own unknown authority. Preserve that exact receipt instead
                    // of treating its invocation as unknown ambient authority.
                    let selected_parameter = matches!(frame.owner, NameSite::Item(owner) if owner == self.item)
                        && definition.is_none()
                        && self.binding(callee).is_some_and(|binding| {
                            let ResolvedName::Parameter(parameter) = binding.binding else { return false; };
                            if binding.owner != frame.owner { return false; }
                            let SignatureKind::Function(header) = &self.signature.kind else { return false; };
                            matches!(&header.uses, Uses::Only(AuthoritySummary::Known(terms)) if terms.contains(&AuthorityTerm::Parameter(ParameterRef::bound(parameter))))
                        });
                    // A receipt covers only the callee's own environment, not
                    // a callback passed to it. Those invocation effects keep
                    // their ordinary independent uses obligation.
                    let additional_callbacks = arguments.iter().any(|argument| {
                        self.ty(argument.value).is_some_and(|ty| {
                            matches!(
                                self.out.body.types.get(ty).unwrap().kind,
                                TypeKind::Function(_)
                            ) && !subset(&self.authority(ty), &allowed)
                        })
                    });
                    // CAP-5/CLO-2: an owner projection may derive Dir from an
                    // authorized Seal parameter. Prove the actual input values,
                    // never a type-level Dir <= Seal relation. Environment
                    // authority required beyond those inputs remains ambient.
                    let derived_inputs = !self.in_clause
                        && required != AuthoritySummary::Unknown
                        && matches!(frame.owner, NameSite::Item(_))
                        && arguments
                            .iter()
                            .all(|a| subset(&self.used(a.value), &allowed))
                        && (definition.is_some() || subset(&self.used(callee), &allowed))
                        && subset(
                            &self.scoped_authority(substitute_authority(
                                &f.authority,
                                &vec![AuthoritySummary::inert(); f.parameters.len()],
                                &generic_authorities,
                            )),
                            &allowed,
                        );
                    if !(ambient == AuthoritySummary::Unknown
                        && selected_parameter
                        && !additional_callbacks)
                        && !derived_inputs
                        && !subset(&ambient, &allowed)
                    {
                        let rule = if matches!(frame.restriction, "CONC-5" | "CAP-3") {
                            frame.restriction
                        } else if transparent {
                            "CLO-2"
                        } else {
                            frame.restriction
                        };
                        if rule == "CONC-5" {
                            self.explained(
                                rule,
                                span,
                                "call authority exceeds the declared uses restriction",
                                Explanation::default()
                                    .provided(Fact::new(Cause::Authority, self.span(callee), &[]))
                                    .required(Fact::new(
                                        Requirement::Uses,
                                        self.sites().owner_span(frame.owner),
                                        &[],
                                    )),
                            );
                        } else {
                            self.error(
                                rule,
                                span,
                                "call authority exceeds the declared uses restriction",
                            );
                        }
                    }
                }
            }
        }
        let declared_result = self.result_at(
            f.result,
            &allocation_targets,
            &origins,
            &actuals,
            &authority_arguments,
            &capture_arguments,
            &partitions,
        );
        let result = if generic_arguments.is_empty() {
            declared_result
        } else {
            self.substitute(declared_result, &generic_arguments, &generic_authorities)
                .unwrap_or(declared_result)
        };
        let result = declared_receipt_evidence(&mut self.out.body.types, result)
            .expect("declared call result");
        self.set_ty(e, result);
        if obligation_valid
            && intrinsic == Some(Intrinsic::TaskAsync)
            && matches!(
                self.out.body.types.get(result).unwrap().kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Task,
                    ..
                }
            )
        {
            self.pending_tasks.insert(e, span);
            self.task_values.insert(e, BTreeSet::from([e]));
        }
        // Brand invalidation happens after checking all argument obligations. Container
        // operations that mutate Slots preserve identity; a move/replacement does not.
        for (i, p) in f.parameters.iter().enumerate() {
            if let Some((_, a)) = ordered.iter().find(|(ordinal, _)| *ordinal == i) {
                if matches!(p.passing, PassingMode::Mut | PassingMode::Take) {
                    if self.protected_field(a.value) {
                        self.explained(
                            "PROV-5",
                            span,
                            "a sibling brand owner cannot be moved or passed mut separately from its record",
                            Explanation::default()
                    .provided(Fact::new(Cause::SiblingOwner, self.span(a.value), &[]))
                    .required(Fact::new(Requirement::Owned, span, &[])),
                        );
                    }
                    let advances = p.passing == PassingMode::Mut
                        && f.state_promises.iter().any(|promise| {
                            promise.selector.parameter == ParameterRef::bound(ParamId(i as u32))
                                && promise.selector.fields.is_empty()
                                && promise.advance_only
                        });
                    if !advances {
                        self.invalidate(a.value, span, p.passing == PassingMode::Mut);
                    }
                }
            }
        }
        // MODE-2: a counted local result may retain parts of its selected
        // borrowed sources. The share survives the result's evaluation loan.
        let shared_result = matches!(
            self.out.body.types.get(result).unwrap().kind,
            TypeKind::Builtin {
                kind: BuiltinType::Shared,
                ..
            }
        );
        if !shared_result
            && !self.property(result, Bound::Plain)
            && !self.property(result, Bound::Flat)
        {
            let lifetime = self.signature_lifetime(
                &f,
                &self.out.body.types.get(f.result).unwrap().modes.relation,
            );
            if let Some(sources) = lifetime.named_parameters() {
                for source in &sources {
                    match source {
                        ParameterRef::Bound {
                            depth: 0,
                            parameter,
                        } => {
                            if let Some((_, argument)) = ordered
                                .iter()
                                .find(|(ordinal, _)| *ordinal == parameter.0 as usize)
                            {
                                self.weaken(argument.value);
                            }
                        }
                        ParameterRef::Body { scope, parameter } => {
                            if let Some(binding) = self
                                .out
                                .body
                                .bindings
                                .iter()
                                .find(|b| {
                                    b.owner == *scope
                                        && b.binding == ResolvedName::Parameter(*parameter)
                                })
                                .cloned()
                            {
                                self.weaken_binding(&binding);
                            }
                        }
                        _ => (),
                    }
                }
            }
            // A counted result of calling a closure may share what its
            // environment borrows, whatever its relation, so its borrowed
            // captures stop being unique as ownership judges (#80).
            self.weaken(callee);
        }
        // Preserve nominal provider attempts and their failure policy.
        let _ = self.authority(result);
        self.facts.insert(e, used.join(required));
    }
    /// An unheld literal can choose its result promise at the receiving call.
    /// Keep its executable result and complete bound evidence independently.
    fn receive_literal_result(
        &mut self,
        expression: ExprId,
        actual: TypeId,
        expected: TypeId,
    ) -> TypeId {
        if !matches!(&self.tree.expressions[expression.0 as usize].kind,
            ExprKind::Closure { signature, .. } if signature.result.is_none())
        {
            return actual;
        }
        let mut row = self.out.body.types.get(actual).unwrap().clone();
        let (TypeKind::Function(function), TypeKind::Function(promise)) = (
            &mut row.kind,
            &self.out.body.types.get(expected).unwrap().kind,
        ) else {
            return actual;
        };
        let (result, expected_result) = (function.result, promise.result);
        let generics = self.declarations().nested(&function.generics);
        let mut store = self.out.body.types.clone();
        let compatible = compatible_view_type_in(
            &mut store,
            expected_result,
            result,
            TypeRelation::Value,
            &generics,
            &mut |path| Ok::<_, hirsel_types::QueryError>(self.read(path)),
        )
        .unwrap_or_else(|error| self.proof_failure(error, false));
        if !compatible {
            return actual;
        }
        function.result = expected_result;
        function.error = result_error(
            &self.out.body.types,
            expected_result,
            &self.signature.metadata.roles,
        );
        let received = self.out.body.types.push(row);
        self.set_ty(expression, received);
        if let Some(mut closure) = self.closures.get_mut(&expression) {
            closure.ty = received;
        }
        received
    }
    fn compatible(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        span: Span,
        intrinsic: Option<Intrinsic>,
        sibling: bool,
    ) -> bool {
        let actual_id = actual;
        let a = self.out.body.types.get(actual).unwrap().clone();
        let b = self.out.body.types.get(expected).unwrap().clone();
        if matches!(a.kind, TypeKind::Error) || matches!(b.kind, TypeKind::Error) {
            return false;
        }
        if !b.brand.alternatives.is_empty() {
            let stale = self.invalid.get(&actual).copied();
            if let Some(invalidation) = stale {
                let mut d = Diagnostic::error(
                    RuleId::new(if sibling { "PROV-5" } else { "PROV-1" }).unwrap(),
                    span,
                    "the brand's owner was invalidated",
                );
                let wanted = b
                    .brand
                    .alternatives
                    .first()
                    .map(|o| self.origin_site(o).0)
                    .unwrap_or_default();
                Explanation {
                    root: Some(crate::explain::root_key(
                        &self.signature.definition,
                        ("ended", invalidation.start, invalidation.end),
                    )),
                    ..Explanation::default()
                }
                .provided(Fact::new(Cause::IdentityEnded, invalidation, &[]))
                .required(Fact::new(
                    Requirement::Brand,
                    self.receiving.as_ref().map_or(span, |r| r.span),
                    &[
                        &wanted,
                        &crate::explain::qualifier(crate::explain::Qualifier::Brand(&wanted)),
                        &self
                            .receiving
                            .as_ref()
                            .map_or_else(|| "this use".into(), |r| r.description.clone()),
                    ],
                ))
                .render(&mut d);
                self.emit(d);
                return false;
            }
            if a.brand.alternatives.len() != 1 || b.brand.alternatives.len() != 1 {
                let explanation = self.unbranded_explanation(&b.brand, span);
                self.explained(
                    if sibling { "PROV-5" } else { "PROV-2" },
                    span,
                    "a union or erased origin cannot satisfy a branded use",
                    explanation,
                );
                return false;
            }
            if brand_join(&a.brand, &Brand::default()) != brand_join(&b.brand, &Brand::default()) {
                let mut diagnostic = Diagnostic::error(
                    RuleId::new(if sibling { "PROV-5" } else { "PROV-1" }).unwrap(),
                    span,
                    "the value belongs to a different origin",
                );
                self.brand_explanation(&a.brand.alternatives[0], &b.brand.alternatives[0], span)
                    .render(&mut diagnostic);
                if let Some(invalidation) = b.brand.alternatives.iter().find_map(|required| {
                    self.identity_changes.iter().find_map(|(changed, span)| {
                        (changed.root == required.root
                            && (required.fields.starts_with(&changed.fields)
                                || changed.fields.starts_with(&required.fields)))
                        .then_some(*span)
                    })
                }) {
                    diagnostic.secondary.push(Label {
                        span: invalidation,
                        message: "identity ended here".into(),
                    });
                }
                self.emit(diagnostic);
                return false;
            }
        }
        let arenas =
            |r: &Relation| r.retain(|r| matches!(r, Reach::Place(o) if self.arena_place(o)));
        // `in heap` is the empty relation: a global value meets an arena
        // relation; a bounded value elsewhere does not (PROV-4).
        let (expected, mut actual) = (arenas(&b.modes.relation), arenas(&a.modes.relation));
        // v22 R3: a value is within S when its arena loans are on arenas S
        // names or that the relation of a binding S names names.
        let expected = self.admitted_arenas(&b.modes.relation, expected);
        // A frame bound alone admits any arena; a relation naming bindings
        // admits only the arenas they hold (v22 R3), perhaps none.
        let within = self.names_bindings(&b.modes.relation);
        if within {
            if let Some(held) = self
                .receiving
                .as_ref()
                .and_then(|r| r.value)
                .filter(|value| self.ty(*value) == Some(actual_id))
                .and_then(|value| self.read_holdings(value))
            {
                actual = held;
            }
        }
        if (!expected.is_global() || within)
            && (within || !b.modes.relation.has_environment())
            && !a.modes.relation.is_global()
            && !actual
                .reaches()
                .iter()
                .all(|r| expected.reaches().contains(r))
        {
            if within {
                let (message, explanation) =
                    self.within_explanation(&actual, &expected, &b.modes.relation, span);
                self.explained("PROV-4", span, &message, explanation);
                return false;
            }
            // Name an arena the value holds outside the relation.
            let escaping = actual.retain(|r| !expected.reaches().contains(r));
            let explanation = self.placement_explanation(&escaping, &b.modes.relation, span);
            self.explained(
                "PROV-4",
                span,
                "the value's placement does not outlive the required arena",
                explanation,
            );
            return false;
        }
        // The receiving requirement applies to every type. Current content
        // supplies the proof; locality remains an independent obligation.
        if crossing_requirement(&b) != Sendability::NotSendable
            && !self.receives_sendability(actual_id, &crossing_requirement(&b))
        {
            let value = self.received_span(span);
            let (receiver, mut description) = self.receiving.as_ref().map_or_else(
                || (span, "this place".to_owned()),
                |r| (r.span, r.description.clone()),
            );
            let whole = self.receiving.as_ref().and_then(|r| r.value);
            if whole.and_then(|value| self.ty(value)) != Some(actual_id) {
                description = format!("a part of {description}");
            }
            let owned_local = self
                .receiving
                .as_ref()
                .is_some_and(|receiving| !receiving.result)
                && !self.lifetime(&a.modes.relation).is_global()
                && !matches!(
                    a.kind,
                    TypeKind::Function(_)
                        | TypeKind::Builtin {
                            kind: BuiltinType::Iter,
                            ..
                        }
                )
                && !self.property(actual_id, Bound::Flat)
                && whole.is_some_and(|value| {
                    matches!(self.source_place(value).root, SourceRoot::Value(_))
                });
            if owned_local {
                self.explained(
                    "MODE-1",
                    span,
                    "an owned result retains a pre-existing local source bound",
                    Explanation::default()
                        .provided(Fact::new(Cause::LocalValue, value, &[]))
                        .required(Fact::new(
                            Requirement::GlobalBinding,
                            receiver,
                            &[&description, "in"],
                        )),
                );
                return false;
            }
            self.explained(
                "MODE-4",
                span,
                "a value that is not sendable cannot be kept where its type is `sendable`",
                Explanation::default()
                    .provided(Fact::new(Cause::NotSendable, value, &[]))
                    .required(Fact::new(
                        Requirement::SendableType,
                        receiver,
                        &[&description],
                    )),
            );
            return false;
        }
        if let (TypeKind::Function(af), TypeKind::Function(bf)) = (&a.kind, &b.kind) {
            if let Uses::Only(allowed) = &bf.uses {
                let allowed = self.scoped_authority(allowed.clone());
                let valid = match &af.uses {
                    Uses::Only(actual) => subset(&self.scoped_authority(actual.clone()), &allowed),
                    Uses::Unrestricted => false,
                };
                if !valid {
                    let value = self.received_span(span);
                    let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                    self.explained(
                        if intrinsic == Some(Intrinsic::MutexWith) {
                            "CONC-5"
                        } else if b.modes.relation.is_global() {
                            "CLO-4"
                        } else {
                            "CAP-3"
                        },
                        span,
                        "function value authority exceeds the required uses restriction",
                        Explanation::default()
                            .provided(Fact::new(Cause::Authority, value, &[]))
                            .required(Fact::new(Requirement::Uses, receiver, &[])),
                    );
                    return false;
                }
            }
            // MEM-5: the allocation contract is part of a function type, so
            // `@noalloc` is received only from a value that promises it.
            if bf.allocation == Allocation::NoAlloc && af.allocation != Allocation::NoAlloc {
                let value = self.received_span(span);
                let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                self.explained(
                    "MEM-5",
                    span,
                    "the function value may allocate where the function type is `@noalloc`",
                    Explanation::default()
                        .provided(Fact::new(Cause::Header, value, &[]))
                        .required(Fact::new(Requirement::Header, receiver, &[])),
                );
                return false;
            }
            // Contract 6: each expected state guarantee needs the same
            // selector on the value; a stronger promise may satisfy it.
            if !state_promises_satisfy(&af.state_promises, &bf.state_promises) {
                let value = self.received_span(span);
                let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                let advance = bf.state_promises.iter().any(|expected| {
                    expected.advance_only
                        && !af.state_promises.iter().any(|actual| {
                            actual.selector == expected.selector && actual.advance_only
                        })
                });
                self.explained(
                    if advance { "MEM-14" } else { "CLO-4" },
                    span,
                    "the function value does not promise the state guarantees the function type requires",
                    Explanation::default()
                        .provided(Fact::new(Cause::Header, value, &[]))
                        .required(Fact::new(Requirement::Header, receiver, &[])),
                );
                return false;
            }
            if !self.captures_promised(af, bf) {
                let value = self.received_span(span);
                let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                self.explained(
                    "CLO-4",
                    span,
                    "the function value does not promise the creation inputs the function type selects",
                    Explanation::default()
                        .provided(Fact::new(Cause::Header, value, &[]))
                        .required(Fact::new(Requirement::Header, receiver, &[])),
                );
                return false;
            }
            let nested = af
                .parameters
                .iter()
                .map(|p| p.ty)
                .zip(bf.parameters.iter().map(|p| p.ty))
                .chain([(af.result, bf.result)])
                .zip(Variance::Covariant.edges(&b.kind));
            for ((actual, expected), variance) in nested {
                if let Some((rule, message)) = self.header_difference(actual, expected, variance) {
                    let value = self.received_span(span);
                    let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                    self.explained(
                        if sibling && matches!(rule, "PROV-1" | "PROV-2") {
                            "PROV-5"
                        } else {
                            rule
                        },
                        span,
                        message,
                        Explanation::default()
                            .provided(Fact::new(Cause::Header, value, &[]))
                            .required(Fact::new(Requirement::Header, receiver, &[])),
                    );
                    return false;
                }
            }
        }
        let parts = a
            .kind
            .graph_edges()
            .into_iter()
            .zip(b.kind.graph_edges())
            .zip(Variance::Covariant.edges(&b.kind))
            .filter(|((_, b), _)| b.role.parameter_binders() == 0);
        for ((a, b), variance) in parts {
            if !self.keeps_placement(a.ty, b.ty, span, false)
                || variance == Variance::Exact && !self.keeps_exact_promises(a.ty, b.ty, span)
                || !self.compatible(a.ty, b.ty, span, intrinsic, sibling)
            {
                return false;
            }
        }
        true
    }
    /// PROV-4: "a value in a cannot outlive a". Storage, a result or a part
    /// declared without a placement promises nothing about an arena, so an
    /// arena value cannot be kept there; a heap value can. A `local(x, ..)`
    /// result is bounded by its sources instead, the root of an argument or
    /// of an annotated binding receives its actual placement, and a plain
    /// value's placement is inert. A local arena value kept where a global is
    /// required is PROV-4 too: docs/spec/diagnostics.md orders placement before required modes,
    /// and ownership judges a read's locality by current content, which may
    /// be Global under a written local storage contract. Only a part of a
    /// Local aggregate checked where it is constructed defers to that owner:
    /// the declared field row does not know the owner's lifetime, which is
    /// checked where the owner is kept.
    fn keeps_placement(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        span: Span,
        local_owner_part: bool,
    ) -> bool {
        // A plain value is copied bytes: its placement is inert.
        if self.property(actual, Bound::Plain) {
            return true;
        }
        let types = &self.out.body.types;
        let (Some(a), Some(b)) = (types.get(actual), types.get(expected)) else {
            return true;
        };
        let (al, bl) = (
            self.lifetime(&a.modes.relation),
            self.lifetime(&b.modes.relation),
        );
        // A let relation naming bindings admits the arenas they hold, and
        // no other (v22 R3, "within S").
        let within = bl.named_parameters().is_none() && self.names_bindings(&b.modes.relation);
        if within && !self.placed(&b.modes.relation) && self.placed(&a.modes.relation) {
            let allowed =
                self.admitted_arenas(&b.modes.relation, self.placement(&b.modes.relation));
            let held = self
                .receiving
                .as_ref()
                .and_then(|r| r.value)
                .filter(|value| self.ty(*value) == Some(actual))
                .and_then(|value| self.read_holdings(value))
                .unwrap_or_else(|| self.placement(&a.modes.relation));
            if !held.contained_in(&allowed)
                && !matches!(a.kind, TypeKind::Error)
                && !matches!(b.kind, TypeKind::Error)
            {
                let relation = b.modes.relation.clone();
                let (message, explanation) =
                    self.within_explanation(&held, &allowed, &relation, span);
                self.explained("PROV-4", span, &message, explanation);
                return false;
            }
            return true;
        }
        if !self.placed(&b.modes.relation)
            && bl.named_parameters().is_none()
            && (!local_owner_part || al.is_global() || !bl.is_global())
            && self.placed(&a.modes.relation)
            && !matches!(a.kind, TypeKind::Error)
            && !matches!(b.kind, TypeKind::Error)
        {
            let (actual, expected) = (a.modes.relation.clone(), b.modes.relation.clone());
            let explanation = self.placement_explanation(&actual, &expected, span);
            self.explained(
                "PROV-4",
                span,
                "an arena value cannot be kept where no placement is declared",
                explanation,
            );
            return false;
        }
        // A relation naming parameters admits the arenas those parameters'
        // own relations name, and no other (PROV-4, "within S").
        if let Some(parameters) = bl
            .named_parameters()
            .filter(|_| self.placed(&a.modes.relation))
        {
            let mut allowed = self.placement(&b.modes.relation);
            for parameter in parameters {
                let ParameterRef::Body { scope, parameter } = parameter else {
                    return true;
                };
                let Some(binding) = self.out.body.bindings.iter().find(|binding| {
                    binding.owner == scope && binding.binding == ResolvedName::Parameter(parameter)
                }) else {
                    return true;
                };
                let row = &self.out.body.types.get(binding.ty).unwrap().modes.relation;
                allowed = allowed.union(&self.placement(row));
            }
            if !self.placement(&a.modes.relation).contained_in(&allowed)
                && !matches!(a.kind, TypeKind::Error)
                && !matches!(b.kind, TypeKind::Error)
            {
                let explanation =
                    self.placement_explanation(&a.modes.relation, &b.modes.relation, span);
                self.explained(
                    "PROV-4",
                    span,
                    "an arena value cannot be kept where no placement is declared",
                    explanation,
                );
                return false;
            }
        }
        true
    }
    /// CLO-1: an inferred local closure result is local to the environment,
    /// which a closure literal binds when it is created, as a call binds the
    /// arguments of an `in x` result. When the body's result can depend on
    /// nothing but the environment (no borrowed non-plain parameter) and every
    /// captured source is global, the closure's result is global.
    fn environment_result(
        &mut self,
        result: TypeId,
        f: &FunctionType,
        captures: &[CapturedName],
        views: &[(BindingKey, TypeId, TypeId)],
    ) -> TypeId {
        let Some(row) = self.out.body.types.get(result).cloned() else {
            return result;
        };
        if !self.lifetime(&row.modes.relation).is_environment() {
            return result;
        }
        let borrowed_parameter = f
            .parameters
            .iter()
            .any(|p| p.passing != PassingMode::Take && !self.property(p.ty, Bound::Plain));
        let sources = captures
            .iter()
            .map(|c| {
                views
                    .iter()
                    .find(|(_, view, _)| *view == c.ty)
                    .map_or(c.ty, |(_, _, source)| *source)
            })
            .collect::<Vec<_>>();
        let global = !borrowed_parameter
            && !views.is_empty()
            && sources.iter().all(|t| {
                hirsel_types::structurally_global(&self.out.body.types, *t, &|o| {
                    self.arena_place(o)
                })
            });
        if !global {
            return result;
        }
        let mut row = row;
        row.modes.relation = self.placement(&row.modes.relation);
        self.out.body.types.push(row)
    }
    /// PROV-4: "a value in a cannot outlive a". A value owns the blocks of
    /// its parts, so an environment that outlives its creating call cannot
    /// hold a non-Plain row placed in an Arena or Region at any depth of the
    /// captured value. A callable's header describes what it receives, not
    /// what it holds, so function types are not entered.
    fn retains_scoped_placement(&mut self, ty: TypeId) -> bool {
        let mut pending = vec![ty];
        let mut seen = BTreeSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let Some(row) = self.out.body.types.get(ty) else {
                continue;
            };
            if matches!(row.kind, TypeKind::Error | TypeKind::Function(_)) {
                continue;
            }
            let scoped = self.placed(&row.modes.relation);
            let edges = row.kind.value_parts();
            if scoped && !self.property(ty, Bound::Plain) {
                return true;
            }
            pending.extend(edges);
        }
        false
    }
    /// Brands are identity evidence: a data value borrowed or taken by a
    /// parameter that names no brand is valid at any depth, the callee simply
    /// does not know it. A `mut` parameter hands the storage back, and a
    /// value that is not data has interior storage, so either could come back
    /// holding an unbranded part where the caller's type promises a brand.
    fn keeps_nested_brands(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        passing: PassingMode,
        argument: ExprId,
    ) -> bool {
        let types = &self.out.body.types;
        let mut pending: Vec<_> = types
            .get(actual)
            .unwrap()
            .kind
            .value_parts()
            .into_iter()
            .zip(types.get(expected).unwrap().kind.value_parts())
            .collect();
        let mut seen = BTreeSet::new();
        let mut forgotten = false;
        while let Some((a, b)) = pending.pop() {
            if !seen.insert((a, b)) {
                continue;
            }
            let (Some(a), Some(b)) = (types.get(a), types.get(b)) else {
                continue;
            };
            if matches!(a.kind, TypeKind::Error) || matches!(b.kind, TypeKind::Error) {
                continue;
            }
            forgotten |= b.brand.alternatives.is_empty() && !a.brand.alternatives.is_empty();
            pending.extend(a.kind.value_parts().into_iter().zip(b.kind.value_parts()));
        }
        let data = forgotten && passing != PassingMode::Mut && {
            let store = self.out.body.types.clone();
            let generics = self.declarations();
            structural_equality(&store, actual, Bound::Data, &generics, &mut |path| {
                Ok::<_, hirsel_types::QueryError>(self.read(path))
            })
            .unwrap_or_else(|error| self.proof_failure(error, false))
        };
        if forgotten && !data {
            let receiver = self
                .receiving
                .as_ref()
                .map_or(self.span(argument), |r| r.span);
            self.explained(
                "PROV-2",
                self.span(argument),
                "the parameter could hand this storage back without the brand its type names",
                Explanation::default()
                    .provided(Fact::new(
                        Cause::BrandForgotten,
                        self.value_site(argument),
                        &[],
                    ))
                    .required(Fact::new(Requirement::UnbrandedParameter, receiver, &[])),
            );
            return false;
        }
        true
    }
    /// MODE-2: a readonly/taken view may forget owned payload evidence. Mut
    /// hand-back must preserve the caller's complete nested storage contract.
    fn keeps_nested_uniqueness(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        view: bool,
        span: Span,
    ) -> bool {
        let store = &self.out.body.types;
        // Callable headers have their own exact check above. Inferred literal
        // body evidence is intentionally separate from the callable promise.
        let mut pending: Vec<_> = store
            .get(actual)
            .unwrap()
            .kind
            .value_parts()
            .into_iter()
            .zip(store.get(expected).unwrap().kind.value_parts())
            .collect();
        let mut seen = BTreeSet::new();
        let mut different = false;
        while let Some((a, b)) = pending.pop() {
            if !seen.insert((a, b)) {
                continue;
            }
            let (Some(a), Some(b)) = (store.get(a), store.get(b)) else {
                continue;
            };
            if matches!(a.kind, TypeKind::Error) || matches!(b.kind, TypeKind::Error) {
                continue;
            }
            different |= a.modes.uniqueness != b.modes.uniqueness;
            pending.extend(a.kind.value_parts().into_iter().zip(b.kind.value_parts()));
        }
        if !different {
            return true;
        }
        let valid = view && {
            let mut store = self.out.body.types.clone();
            // This check concerns nested contracts. Root judgments are made
            // independently by I and the existing authority/placement checks.
            let mut row = store.get(expected).unwrap().clone();
            row.kind = store.get(actual).unwrap().kind.clone();
            let actual = store.push(row);
            let generics = self.declarations();
            compatible_view_type(&store, expected, actual, &generics, &mut |path| {
                Ok::<_, hirsel_types::QueryError>(self.read(path))
            })
            .unwrap_or_else(|error| self.proof_failure(error, false))
        };
        if !valid {
            let value = self.received_span(span);
            let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
            self.explained(
                "MODE-2",
                span,
                "the receiving type cannot change this nested uniqueness contract",
                Explanation::default()
                    .provided(Fact::new(Cause::Aliased, value, &[]))
                    .required(Fact::new(Requirement::Unique, receiver, &[])),
            );
        }
        valid
    }
    /// A value kept in storage, a result or a constructed part.
    fn kept(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        span: Span,
        expression: ExprId,
        sibling: bool,
        receiving: Receiving,
        local_owner_part: bool,
    ) -> bool {
        let saved = self.receiving.replace(Receiving {
            value: Some(expression),
            ..receiving
        });
        let kept = self.kept_by(
            actual,
            expected,
            span,
            expression,
            sibling,
            local_owner_part,
        );
        self.receiving = saved;
        kept
    }
    fn kept_by(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        span: Span,
        expression: ExprId,
        sibling: bool,
        local_owner_part: bool,
    ) -> bool {
        let place = self.source_place(expression);
        let view = place.access != SourceAccess::Copy
            && (matches!(place.access, SourceAccess::Take | SourceAccess::Unique)
                || matches!(place.root, SourceRoot::Value(_)) && place.projections.is_empty()
                || place.projections.is_empty()
                    && self
                        .binding(expression)
                        .is_some_and(|binding| binding.usage == UseKind::Take));
        self.keeps_placement(actual, expected, span, local_owner_part)
            && self.compatible(actual, expected, span, None, sibling)
            && self.keeps_nested_uniqueness(actual, expected, view, span)
    }
    /// Only the root of a value weakens to its use. Parameter and result types
    /// inside a function type are exact, so a callable header that differs from
    /// the required one in a mode, brand or placement is another type. A
    /// callable inside the header keeps its own promise in the direction it
    /// is read (CLO-2, `hirsel_types::Variance`): a result may promise more
    /// than required, a parameter may ask for less than it is given.
    fn header_difference(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        variance: Variance,
    ) -> Option<(&'static str, &'static str)> {
        let types = &self.out.body.types;
        let (a, b) = (types.get(actual)?.clone(), types.get(expected)?.clone());
        if matches!(a.kind, TypeKind::Error) || matches!(b.kind, TypeKind::Error) {
            return None;
        }
        if a.modes.relation != b.modes.relation {
            return Some((
                "MODE-1",
                "a function header's relation differs from the required function type",
            ));
        }
        if a.modes.uniqueness != b.modes.uniqueness {
            return Some((
                "MODE-2",
                "a function header's uniqueness differs from the required function type",
            ));
        }
        if a.modes.multiplicity != b.modes.multiplicity {
            return Some((
                "MODE-3",
                "a function header's multiplicity differs from the required function type",
            ));
        }
        // MODE-4: a callable's crossing mode is part of its type, read in
        // the direction of the edge, as `hirsel_types::nested_promise` does.
        {
            let (ap, bp) = (crossing_requirement(&a), crossing_requirement(&b));
            let differs = match variance {
                Variance::Covariant => !Self::crossing_satisfies(&ap, &bp),
                Variance::Contravariant => !Self::crossing_satisfies(&bp, &ap),
                Variance::Exact => ap != bp,
            };
            if differs {
                return Some((
                    "MODE-4",
                    "a function header's sendability differs from the required function type",
                ));
            }
        }
        if a.brand.alternatives.is_empty() != b.brand.alternatives.is_empty() {
            return Some((
                "PROV-1",
                "a function header's brand differs from the required function type",
            ));
        }
        if let (TypeKind::Function(af), TypeKind::Function(bf)) = (&a.kind, &b.kind) {
            let supplied = variance == Variance::Contravariant || self.promises(af, bf);
            let demanded = variance == Variance::Covariant || self.promises(bf, af);
            if !supplied {
                return Some((
                    "CLO-2",
                    "a function in the header promises less than the required function type",
                ));
            }
            if !demanded {
                return Some((
                    "CLO-2",
                    "a function in the header requires more than the required function type gives",
                ));
            }
        }
        let nested = |kind: &TypeKind| match kind {
            TypeKind::Function(f) => f
                .parameters
                .iter()
                .map(|p| p.ty)
                .chain([f.result])
                .collect(),
            kind => kind.value_parts(),
        };
        nested(&a.kind)
            .into_iter()
            .zip(nested(&b.kind))
            .zip(variance.edges(&b.kind))
            .find_map(|((a, b), variance)| self.header_difference(a, b, variance))
    }
    /// CLO-2: the callee writes a `mut` argument back through its parameter,
    /// so the argument's storage and the parameter state the same callable
    /// promises, at the root and at every depth: storage that promised less
    /// would return holding what the parameter admits, and storage that
    /// promised more would be read as what it never held.
    fn keeps_handback_promises(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        span: Span,
        argument: ExprId,
        callee: ExprId,
    ) -> bool {
        let root = (actual, expected);
        let mut pending = vec![(actual, expected)];
        let mut seen = BTreeSet::new();
        while let Some((actual, expected)) = pending.pop() {
            if !seen.insert((actual, expected)) {
                continue;
            }
            let types = &self.out.body.types;
            let (Some(a), Some(b)) = (types.get(actual).cloned(), types.get(expected).cloned())
            else {
                continue;
            };
            // MODE-3: a mutable alias receives and hands back the same
            // callable contract; owned many-to-once reception may weaken.
            if a.modes.multiplicity != b.modes.multiplicity {
                self.explained(
                    "MODE-3",
                    span,
                    "a `mut` callable argument must have exactly the parameter's once/many multiplicity: the callee may replace it with any function of the parameter's type. Match the binding and parameter types, or take the value and return the new one",
                    Explanation::default()
                        .provided(Fact::new(Cause::Header, self.received_span(span), &[]))
                        .required(Fact::new(Requirement::Header, self.receiving.as_ref().map_or(span, |r| r.span), &[])),
                );
                if let Some(diagnostic) = self.out.diagnostics.last_mut()
                    && diagnostic.primary == span
                    && diagnostic.rule.as_str() == "MODE-3"
                {
                    diagnostic
                        .obligations
                        .push(hirsel_diag::Obligation::Multiplicity);
                }
                return false;
            }
            if let (TypeKind::Function(af), TypeKind::Function(bf)) = (&a.kind, &b.kind) {
                if !(self.promises(af, bf) && self.promises(bf, af)) {
                    let value = self.received_span(span);
                    let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                    let message = self.handback_message(
                        argument,
                        callee,
                        ((actual, expected) == root).then_some(root),
                    );
                    self.explained(
                        "CLO-2",
                        span,
                        &message,
                        Explanation::default()
                            .provided(Fact::new(Cause::Header, value, &[]))
                            .required(Fact::new(Requirement::Header, receiver, &[])),
                    );
                    return false;
                }
            }
            // MODE-4: sendable storage handed to a parameter that is not
            // would come back holding what crosses no task, still typed as
            // crossing; storage that is not sendable would be read as
            // sendable inside. So a callable's crossing mode is exact too.
            if !self.property(actual, Bound::Plain)
                && crossing_requirement(&a) != crossing_requirement(&b)
            {
                let mut place = argument;
                while let ExprKind::Unary {
                    op: UnaryOp::Mut,
                    value,
                } = self.tree.expressions[place.0 as usize].kind
                {
                    place = value;
                }
                let name = self.value_name(Some(place));
                let callee = self.spelled(callee).unwrap_or_else(|| "the callee".into());
                let (value, receiver) = (
                    self.received_span(span),
                    self.receiving.as_ref().map_or(span, |r| r.span),
                );
                let (provided, required) = if b.modes.sendability == Sendability::Sendable {
                    (Cause::NotSendable, Requirement::SendableType)
                } else {
                    (Cause::Sendable, Requirement::NotSendableType)
                };
                let description = format!("the parameter of `{callee}`");
                self.explained(
                    "MODE-4",
                    span,
                    &format!(
                        "a `mut` argument's crossing mode must be exactly the parameter's: `{callee}` may store in `{name}` any value of the parameter's type, which is then used as the argument's. Write the binding's type as the parameter states it, or take the value and return the new one"
                    ),
                    Explanation::default()
                        .provided(Fact::new(provided, value, &[]))
                        .required(Fact::new(required, receiver, &[&description])),
                );
                return false;
            }
            // The erased state's captures (CLO-2) and the cleanup summary
            // (MEM-5) are written back with the value, so they are exact too.
            // A core operation that only advances an erased state declares
            // its parameter at the argument's own captures (spec #91).
            let differs = self.state_difference(&a, &b);
            if let Some((rule, what)) = differs {
                let name = self.value_name(Some(argument));
                let callee = self.spelled(callee).unwrap_or_else(|| "the callee".into());
                self.error(
                    rule,
                    span,
                    &format!(
                        "a `mut` argument's {what} must be exactly the parameter's: `{callee}` may store in `{name}` any value of the parameter's type, which is then used as the argument's. Write the binding's type as the parameter states it, or take the value and return the new one"
                    ),
                );
                return false;
            }
            pending.extend(
                a.kind
                    .graph_edges()
                    .into_iter()
                    .zip(b.kind.graph_edges())
                    .map(|(a, b)| (a.ty, b.ty)),
            );
        }
        true
    }
    /// Whether an erased state's captures (CLO-2) or a cleanup summary
    /// (MEM-5) differ between two rows that must state the same contract.
    fn state_difference(&mut self, a: &Type, b: &Type) -> Option<(&'static str, &'static str)> {
        let state = |row: &Type| {
            row.erased_captures
                .as_ref()
                .map_or(AuthoritySummary::Unknown, ErasedCaptures::authority)
        };
        let dependent =
            |row: &Type| matches!(row.erased_captures, Some(ErasedCaptures::FromArguments(_)));
        let same_state = if dependent(a) || dependent(b) {
            a.erased_captures == b.erased_captures
        } else {
            let held = self.scoped_authority(state(a));
            let allowed = self.scoped_authority(state(b));
            held == allowed || (subset(&held, &allowed) && subset(&allowed, &held))
        };
        if !same_state {
            Some(("CLO-2", "erased state's captured authority"))
        } else if a.drop_allocation != b.drop_allocation {
            Some((
                "MEM-5",
                "cleanup summary (whether destroying it may allocate)",
            ))
        } else {
            None
        }
    }
    /// A nominal type's argument, or a non-sequence builtin's, is exact
    /// (`hirsel_types::Variance`): the declaration may use it in either
    /// direction, so a callable, an erased state or a cleanup summary in it
    /// promises exactly what the expected type states, at every depth. v22
    /// is silent here; this is the conservative reading (spec #92).
    fn keeps_exact_promises(&mut self, actual: TypeId, expected: TypeId, span: Span) -> bool {
        let mut pending = vec![(actual, expected)];
        let mut seen = BTreeSet::new();
        while let Some((actual, expected)) = pending.pop() {
            if !seen.insert((actual, expected)) {
                continue;
            }
            let types = &self.out.body.types;
            let (Some(a), Some(b)) = (types.get(actual).cloned(), types.get(expected).cloned())
            else {
                continue;
            };
            let differs = if !self.property(actual, Bound::Plain)
                && crossing_requirement(&a) != crossing_requirement(&b)
            {
                Some(("MODE-4", "crossing receiving contract"))
            } else {
                match (&a.kind, &b.kind) {
                    (TypeKind::Function(af), TypeKind::Function(bf))
                        if !(self.promises(af, bf) && self.promises(bf, af)) =>
                    {
                        Some(("CLO-2", "callable promise (`uses` and `@noalloc`)"))
                    }
                    _ => self.state_difference(&a, &b),
                }
            };
            if let Some((rule, what)) = differs {
                let value = self.received_span(span);
                let receiver = self.receiving.as_ref().map_or(span, |r| r.span);
                self.explained(
                    rule,
                    span,
                    &format!(
                        "a type argument's {what} must be exactly the expected type's: a generic type may use its argument in either direction. Write the value's type as the expected type states it"
                    ),
                    Explanation::default()
                        .provided(Fact::new(Cause::Header, value, &[]))
                        .required(Fact::new(Requirement::Header, receiver, &[])),
                );
                return false;
            }
            pending.extend(
                a.kind
                    .graph_edges()
                    .into_iter()
                    .zip(b.kind.graph_edges())
                    .map(|(a, b)| (a.ty, b.ty)),
            );
        }
        true
    }
    /// Both sides of a refused `mut` hand-back and the two ways to make them
    /// agree; the types are quoted where they have a short spelling.
    fn handback_message(
        &self,
        argument: ExprId,
        callee: ExprId,
        root: Option<(TypeId, TypeId)>,
    ) -> String {
        let headline =
            "a `mut` argument's function type must promise exactly what the parameter's does";
        let mut place = argument;
        while let ExprKind::Unary {
            op: UnaryOp::Mut,
            value,
        } = self.tree.expressions[place.0 as usize].kind
        {
            place = value;
        }
        let whole = self
            .source_rows
            .places
            .get(&place)
            .is_some_and(|i| self.out.body.places[*i].projections.is_empty());
        let binding = self.binding(place).filter(|_| whole);
        let name = self.value_name(Some(place));
        let callee = self.spelled(callee).unwrap_or_else(|| "the callee".into());
        let annotated = binding.as_ref().is_some_and(|b| match b.site {
            NameSite::Pattern(pattern) => self
                .pattern_row(pattern)
                .is_some_and(|p| p.annotation.is_some()),
            _ => true,
        });
        let spelled = root.and_then(|(actual, expected)| {
            Some((
                self.function_spelling(actual, false)?,
                self.function_spelling(expected, true)?,
            ))
        });
        let stores = format!(
            "`{callee}` may store in it any function of the parameter's type, which is then used as the argument's"
        );
        match (binding.is_some(), annotated, spelled) {
            (true, false, Some((inferred, parameter))) => format!(
                "{headline}: the inferred type of `{name}` is `{inferred}` and the `mut` parameter is `{parameter}`; {stores}. Write the binding's type, `let mut {name}: {parameter} = ...`, or narrow the parameter to the inferred type"
            ),
            (true, true, Some((declared, parameter))) => format!(
                "{headline}: `{name}` is declared `{declared}` and the `mut` parameter is `{parameter}`; {stores}. Declare both with the same `uses` and `@noalloc`"
            ),
            (true, false, None) => format!(
                "{headline}: the inferred type of `{name}` keeps the `uses` and `@noalloc` its initializer proves, and the `mut` parameter promises differently; {stores}. Write the binding's type as the parameter states it, or narrow the parameter"
            ),
            _ => format!("{headline}: {stores}. Declare both with the same `uses` and `@noalloc`"),
        }
    }
    /// CLO-2: `value` keeps every promise `place` makes: no allocation where
    /// none is promised, and no authority beyond the place's `uses`.
    /// CLO-4: a callable type's `@captures` selection is a promise about its
    /// result's environment. A value promises it when its own selection keeps
    /// no input the required one leaves out; an unknown environment does not.
    fn captures_promised(&self, value: &FunctionType, place: &FunctionType) -> bool {
        let selection = |result: TypeId| match &self.out.body.types.get(result)?.erased_captures {
            Some(ErasedCaptures::FromArguments(selectors)) => Some(selectors.clone()),
            _ => None,
        };
        match selection(place.result) {
            None => true,
            Some(required) => selection(value.result)
                .is_some_and(|held| held.iter().all(|selector| required.contains(selector))),
        }
    }
    fn promises(&mut self, value: &FunctionType, place: &FunctionType) -> bool {
        state_promises_satisfy(&value.state_promises, &place.state_promises)
            && self.captures_promised(value, place)
            && (place.allocation != Allocation::NoAlloc || value.allocation == Allocation::NoAlloc)
            && match (&value.uses, &place.uses) {
                (_, Uses::Unrestricted) => true,
                (Uses::Only(held), Uses::Only(allowed)) => {
                    let allowed = self.scoped_authority(allowed.clone());
                    subset(&self.scoped_authority(held.clone()), &allowed)
                }
                (Uses::Unrestricted, Uses::Only(_)) => false,
            }
    }
    /// MEM-5 evidence that the body of an inferred closure literal cannot
    /// allocate, on any path. The proof is by construction over a closed set
    /// of forms: no block is constructed outside an arena (a String or Bytes
    /// literal, a sequence, `box`, an escaping closure), nothing is deep-copied
    /// (`unique`), nothing but a Plain binding is written (no detach), every
    /// call is `@noalloc` after the CLO-2 join, no `for` advances a cursor,
    /// and every value the frame owns is destroyed without allocating. A
    /// body outside this set has no evidence; writing `@noalloc` on the
    /// receiving type asks for the complete audit of that body instead.
    fn noalloc_evidence(&mut self, owner: &NameSite, body: ExprId) -> bool {
        let generics = self.declarations();
        let silent = |row: &Type| match &row.drop_allocation {
            DropAllocation::NoAlloc => true,
            DropAllocation::MayAllocate => false,
            DropAllocation::DependsOn(parameters) => parameters.iter().all(|p| {
                let bounds = generics.bounds(*p);
                bounds.contains(&Bound::NoallocDrop) || bounds.contains(&Bound::Plain)
            }),
        };
        // A closure literal builds its environment in this frame (CLO-1)
        // only where it cannot leave it: called on the spot, or bound by an
        // unannotated `let` whose every use is a call of it.
        let mut scoped = BTreeSet::new();
        let mut callees = BTreeSet::new();
        let mut called_only = BTreeSet::new();
        let mut stack = vec![body];
        while let Some(e) = stack.pop() {
            let row = self
                .ty(e)
                .and_then(|ty| self.out.body.types.get(ty))
                .cloned();
            let row = row.as_ref();
            match &self.tree.expressions[e.0 as usize].kind {
                // A block placed in an arena is not a heap allocation.
                ExprKind::Literal(
                    hirsel_syntax::Literal::String(_) | hirsel_syntax::Literal::Bytes(_),
                )
                | ExprKind::Sequence(_)
                | ExprKind::Unary {
                    op: UnaryOp::Box, ..
                } if row.is_none_or(|row| self.placement(&row.modes.relation).is_global()) => {
                    return false;
                }
                ExprKind::Unary {
                    op: UnaryOp::Unique,
                    ..
                }
                | ExprKind::Comptime(_)
                | ExprKind::Shape(_)
                | ExprKind::Heap
                | ExprKind::Todo(_)
                | ExprKind::Error => return false,
                ExprKind::Closure { .. } => {
                    // Its body runs in its own frame; only the environment
                    // is built here, and a global one is a counted block.
                    if !scoped.contains(&e)
                        || row.is_none_or(|row| self.lifetime(&row.modes.relation).is_global())
                    {
                        return false;
                    }
                    continue;
                }
                ExprKind::Path(_)
                    if !callees.contains(&e)
                        && self
                            .binding(e)
                            .is_some_and(|binding| called_only.contains(&binding.site)) =>
                {
                    return false;
                }
                ExprKind::Call { callee, .. } => {
                    callees.insert(*callee);
                    scoped.insert(*callee);
                    // A constructor builds its value inline; one that
                    // checks a fact (FACT-2) calls that check.
                    let constructor = self
                        .call_row(e)
                        .is_some_and(|call| matches!(call.target, CallTarget::Constructor(_)))
                        && !row.is_some_and(|row| match &row.kind {
                            TypeKind::Named { definition, .. } => {
                                self.read(definition).is_none_or(|signature| {
                                    !matches!(&signature.kind,
                                        SignatureKind::Type(d) if d.fact.is_none())
                                })
                            }
                            _ => false,
                        });
                    let noalloc = self
                        .out
                        .call_allocations
                        .iter()
                        .any(|(site, a)| *site == e && *a == Allocation::NoAlloc);
                    if !(constructor || noalloc) {
                        return false;
                    }
                }
                _ => (),
            }
            // Every value computed here is destroyed here on some path (a
            // discarded result, a failing later operand): only a read of a
            // binding's storage, which borrows, and a callee make none.
            let borrows = !callees.contains(&e)
                && self.source_rows.places.get(&e).is_some_and(|i| {
                    matches!(self.out.body.places[*i].root, SourceRoot::Binding { .. })
                        && matches!(
                            self.tree.expressions[e.0 as usize].kind,
                            ExprKind::Path(_) | ExprKind::Field { .. } | ExprKind::Index { .. }
                        )
                });
            if !callees.contains(&e) && !borrows && !row.is_some_and(silent) {
                return false;
            }
            match &self.tree.expressions[e.0 as usize].kind {
                ExprKind::Block { statements, .. } => {
                    for statement in statements {
                        match self.tree.statements[statement.0 as usize].kind {
                            StmtKind::For { .. } => return false,
                            StmtKind::Assign { place, .. } => {
                                let root = match &self.source_place(place).root {
                                    SourceRoot::Binding { owner, binding } => {
                                        self.root_binding(*owner, binding).map(|b| {
                                            self.binding_types
                                                .get(&binding_key(b))
                                                .copied()
                                                .unwrap_or(b.ty)
                                        })
                                    }
                                    _ => None,
                                };
                                if !root.is_some_and(|ty| self.property(ty, Bound::Plain)) {
                                    return false;
                                }
                            }
                            StmtKind::Let {
                                pattern,
                                ty: None,
                                value,
                                ..
                            } if matches!(
                                self.tree.expressions[value.0 as usize].kind,
                                ExprKind::Closure { .. }
                            ) =>
                            {
                                scoped.insert(value);
                                called_only.insert(NameSite::Pattern(pattern));
                            }
                            _ => (),
                        }
                    }
                }
                _ => (),
            }
            let nested = matches!(
                self.tree.expressions[e.0 as usize].kind,
                ExprKind::Block { .. }
            )
            .then(|| self.local_function_bodies(e))
            .unwrap_or_default();
            stack.extend(
                self.expression_children(e)
                    .into_iter()
                    .filter(|child| !nested.contains(child)),
            );
        }
        let taken = |parameter: ParamId| match owner {
            NameSite::Expression(e) => match &self.tree.expressions[e.0 as usize].kind {
                ExprKind::Closure { signature, .. } => signature
                    .parameters
                    .get(parameter.0 as usize)
                    .is_some_and(|p| p.passing == PassingMode::Take),
                _ => true,
            },
            _ => true,
        };
        self.out.body.bindings.iter().all(|binding| {
            binding.owner != *owner
                || matches!(binding.binding, ResolvedName::Parameter(p) if !taken(p))
                || self
                    .out
                    .body
                    .types
                    .get(
                        self.binding_types
                            .get(&binding_key(binding))
                            .copied()
                            .unwrap_or(binding.ty),
                    )
                    .is_some_and(silent)
        })
    }
    /// The bodies of the local functions a block declares: other frames.
    fn local_function_bodies(&self, block: ExprId) -> Vec<ExprId> {
        let ExprKind::Block { statements, .. } = &self.tree.expressions[block.0 as usize].kind
        else {
            return vec![];
        };
        statements
            .iter()
            .filter_map(|s| match self.tree.statements[s.0 as usize].kind {
                StmtKind::Function { body, .. } => Some(body),
                _ => None,
            })
            .collect()
    }
    /// A callable promise may weaken its result; bounds inspect the independently
    /// recorded complete body result instead of strengthening that promise.
    fn complete_value_type(&mut self, ty: TypeId) -> TypeId {
        let Some(result) = self.closures.by_type(ty).and_then(|c| c.result_evidence) else {
            return ty;
        };
        let mut row = self.out.body.types.get(ty).unwrap().clone();
        if let TypeKind::Function(f) = &mut row.kind {
            f.result = result;
        }
        self.out.body.types.push(row)
    }
    fn generic_obligations(
        &mut self,
        e: ExprId,
        f: &FunctionType,
        args: &[Argument],
        intrinsic: Option<Intrinsic>,
    ) {
        let sharing = sharing_type_parameters(&self.out.body.types, f);
        let substitutions = self
            .out
            .body
            .calls
            .iter()
            .find(|call| call.expression == e)
            .map(|call| call.generic_arguments.clone())
            .unwrap_or_default();
        let store = self.out.body.types.clone();
        let declarations = self.declarations();
        for parameter in sharing {
            if let Some(GenericArgument::Type(actual)) = substitutions.get(parameter.0 as usize) {
                let forbidden =
                    has_owned_crossing_requirement(&store, *actual, &declarations, &mut |path| {
                        Ok(self.read(path))
                    })
                    .unwrap_or_else(|error| self.proof_failure(error, true));
                if forbidden {
                    let site = self.span(e);
                    self.explained("MODE-4", site, "a generic parameter with copying permission cannot carry this implicit sendable receiving requirement; write sendable on its required occurrences or remove the copying permission",
                        Explanation::default().provided(Fact::new(Cause::NotSendable, site, &[]))
                            .required(Fact::new(Requirement::SendableType, site, &["the receiving type"])));
                    return;
                }
            }
        }
        fn evidence_children(kind: &TypeKind) -> Vec<TypeId> {
            if let TypeKind::Function(f) = kind {
                return f
                    .parameters
                    .iter()
                    .map(|p| p.ty)
                    .chain([f.result])
                    .collect();
            }
            kind.value_parts()
        }
        fn occurrences(
            store: &TypeStore,
            root: TypeId,
            param: TypeParamId,
            depth: u32,
            active: &mut BTreeSet<TypeId>,
        ) -> Vec<Vec<usize>> {
            if !active.insert(root) {
                return vec![];
            }
            let Some(t) = store.get(root) else {
                return vec![];
            };
            let result = if t.kind
                == TypeKind::Parameter(TypeParameterRef::Bound {
                    depth,
                    parameter: param,
                }) {
                vec![vec![]]
            } else {
                evidence_children(&t.kind)
                    .iter()
                    .enumerate()
                    .flat_map(|(i, t)| {
                        occurrences(
                            store,
                            *t,
                            param,
                            depth
                                + u32::from(matches!(
                                    store.get(root).unwrap().kind,
                                    TypeKind::Function(_)
                                )),
                            active,
                        )
                        .into_iter()
                        .map(move |mut p| {
                            p.insert(0, i);
                            p
                        })
                    })
                    .collect()
            };
            active.remove(&root);
            result
        }
        for (i, g) in f.generics.iter().enumerate() {
            let GenericParameter::Type { bounds, .. } = g else {
                continue;
            };
            let dependent_arguments: Vec<_> = f
                .parameters
                .iter()
                .zip(args)
                .filter_map(|(p, a)| {
                    (!occurrences(
                        &self.out.body.types,
                        p.ty,
                        TypeParamId(i as u32),
                        0,
                        &mut BTreeSet::new(),
                    )
                    .is_empty())
                    .then(|| Label {
                        span: self.span(a.value),
                        message: "generic parameter instantiation failed".into(),
                    })
                })
                .collect();
            for (p, arg) in f.parameters.iter().zip(args) {
                for path in occurrences(
                    &self.out.body.types,
                    p.ty,
                    TypeParamId(i as u32),
                    0,
                    &mut BTreeSet::new(),
                ) {
                    let Some(mut actual) = self.ty(arg.value) else {
                        continue;
                    };
                    let primary = if intrinsic == Some(Intrinsic::MutexWith)
                        && matches!(
                            self.out.body.types.get(actual).unwrap().kind,
                            TypeKind::Function(_)
                        ) {
                        self.callback_result_span(e)
                    } else {
                        self.span(e)
                    };
                    actual = self.complete_value_type(actual);
                    let mut valid = true;
                    for index in path {
                        if let Some(child) =
                            evidence_children(&self.out.body.types.get(actual).unwrap().kind)
                                .get(index)
                        {
                            actual = *child
                        } else {
                            valid = false;
                            break;
                        }
                    }
                    if !valid {
                        continue;
                    }
                    if bounds.contains(&Bound::Serializable)
                        && !self.property(actual, Bound::Serializable)
                    {
                        self.error("SER-1", self.span(e),
                            "actual generic argument needs a transparent serializable schema without brands or authority");
                        return;
                    }
                    let owner = match intrinsic {
                        Some(Intrinsic::MutexWith | Intrinsic::MutexCreate) => "CONC-5",
                        Some(Intrinsic::ChannelOpen | Intrinsic::ChannelSend) => "CONC-6",
                        _ => "MODE-4",
                    };
                    if bounds.contains(&Bound::Sendable) && !self.sendable(actual) {
                        self.explained(
                            owner,
                            primary,
                            "each actual generic occurrence must be sendable before parameter weakening",
                            Explanation::default()
                                .provided(Fact::new(Cause::NotSendable, self.span(arg.value), &[]))
                                .required(Fact::new(Requirement::Sendable, primary, &[])),
                        );
                        if self.out.failure.is_none()
                            && self.signatures.values().all(Option::is_some)
                        {
                            let bound = self.out.diagnostics.last_mut().unwrap();
                            bound.secondary.extend(dependent_arguments.clone());
                            bound.obligations.push(Obligation::GenericBound {
                                arguments: dependent_arguments.iter().map(|l| l.span).collect(),
                            });
                        }
                        return;
                    }
                    if bounds.contains(&Bound::Inert)
                        && self.authority(actual) != AuthoritySummary::inert()
                    {
                        self.explained(
                            if intrinsic == Some(Intrinsic::SharedFreeze) {
                                "MEM-11"
                            } else {
                                "CAP-2"
                            },
                            self.span(e),
                            "inert requires an empty declared authority summary",
                            Explanation::default()
                                .provided(Fact::new(Cause::Authority, self.span(arg.value), &[]))
                                .required(Fact::new(Requirement::Inert, self.span(e), &[])),
                        );
                        if self.out.failure.is_none()
                            && self.signatures.values().all(Option::is_some)
                        {
                            let bound = self.out.diagnostics.last_mut().unwrap();
                            bound.secondary.extend(dependent_arguments.clone());
                            bound.obligations.push(Obligation::GenericBound {
                                arguments: dependent_arguments.iter().map(|l| l.span).collect(),
                            });
                        }
                        return;
                    }
                }
            }
        }
    }
    fn static_intrinsic(
        &mut self,
        e: ExprId,
        intrinsic: Option<Intrinsic>,
        args: &[TypeId],
        f: &FunctionType,
    ) {
        let span = self.span(e);
        match intrinsic {
            Some(Intrinsic::SharedFreeze) => {
                if let Some(t) = args.first() {
                    if self.contains_resource(*t) {
                        self.explained(
                            "MEM-11",
                            span,
                            "Shared construction cannot hold resources",
                            Explanation::default()
                                .provided(Fact::new(Cause::Resource, span, &[]))
                                .required(Fact::new(Requirement::NoResources, span, &[])),
                        );
                    }
                    if !self.property(*t, Bound::Plain)
                        && self.out.body.types.get(*t).unwrap().modes.uniqueness
                            != Uniqueness::Unique
                    {
                        self.explained(
                            "MEM-11",
                            span,
                            "Shared construction requires a unique root",
                            Explanation::default()
                                .provided(Fact::new(Cause::Aliased, span, &[]))
                                .required(Fact::new(Requirement::Unique, span, &[])),
                        );
                    }
                }
            }
            Some(Intrinsic::ArenaSeal) => {
                if args.get(1).is_some_and(|t| self.contains_resource(*t)) {
                    self.explained(
                        "MEM-4",
                        span,
                        "an arena seal cannot hold resources",
                        Explanation::default()
                            .provided(Fact::new(Cause::Resource, span, &[]))
                            .required(Fact::new(Requirement::NoResources, span, &[])),
                    );
                }
            }
            Some(Intrinsic::MutexCreate) => {
                if let Some(t) = args.get(1) {
                    if !self.property(*t, Bound::Plain)
                        && self.out.body.types.get(*t).unwrap().modes.uniqueness
                            != Uniqueness::Unique
                        || !self.sendable(*t)
                    {
                        let cause = if !self.property(*t, Bound::Plain)
                            && self.out.body.types.get(*t).unwrap().modes.uniqueness
                                != Uniqueness::Unique
                        {
                            Cause::Aliased
                        } else {
                            Cause::NotSendable
                        };
                        let required = if cause == Cause::Aliased {
                            Requirement::Unique
                        } else {
                            Requirement::Sendable
                        };
                        self.explained(
                            "CONC-5",
                            span,
                            "mutex payload must be unique and sendable",
                            self.payload_explanation(e, 1, cause, required),
                        );
                    }
                }
            }
            Some(Intrinsic::ChannelSend) => {
                if let Some(t) = args.get(2) {
                    if !self.sendable(*t) {
                        self.explained("CONC-6", span, "channel payload must be sendable; Receivers cross only by task capture",
                            self.payload_explanation(e, 2, Cause::NotSendable, Requirement::Sendable));
                    }
                }
            }
            Some(Intrinsic::ChannelOpen) => {
                let ts = self
                    .out
                    .body
                    .instantiations
                    .iter()
                    .find(|i| {
                        i.expression == e && self.intrinsics.get(&i.callee).copied() == intrinsic
                    })
                    .map(|i| i.arguments.clone())
                    .unwrap_or_default();
                for t in ts {
                    if let GenericArgument::Type(t) = t {
                        if !self.sendable(t) {
                            self.explained(
                                "CONC-6",
                                span,
                                "channel payload type must be sendable",
                                self.boundary_explanation(
                                    None,
                                    span,
                                    Cause::NotSendable,
                                    Requirement::Sendable,
                                ),
                            );
                            break;
                        }
                    }
                }
            }
            Some(Intrinsic::AtomicCreate) => {
                if let Some(t) = args.get(1) {
                    if !matches!(self.out.body.types.get(*t).map(|t| &t.kind), Some(TypeKind::Primitive(p)) if !matches!(p, Primitive::Bytes | Primitive::String))
                    {
                        self.error("CONC-7", span, "atomic payload must be scalar");
                    }
                }
            }
            Some(Intrinsic::TaskAsync | Intrinsic::TaskIsolate | Intrinsic::MutexWith) => {
                if let Some(t) = args.last() {
                    if let TypeKind::Function(actual) =
                        self.out.body.types.get(*t).unwrap().kind.clone()
                    {
                        let result = self
                            .closures
                            .by_type(*t)
                            .and_then(|c| c.result_evidence)
                            .unwrap_or(actual.result);
                        if !self.sendable(result) {
                            let rule = if intrinsic == Some(Intrinsic::MutexWith) {
                                "CONC-5"
                            } else {
                                "MODE-4"
                            };
                            let result_span = if intrinsic == Some(Intrinsic::MutexWith) {
                                self.callback_result_span(e)
                            } else {
                                span
                            };
                            self.explained(
                                rule,
                                result_span,
                                "the callback result must be sendable",
                                Explanation::default()
                                    .provided(Fact::new(Cause::NotSendable, result_span, &[]))
                                    .required(Fact::new(Requirement::Sendable, span, &[])),
                            );
                        }
                    }
                }
            }
            _ => (),
        }
        let _ = f;
    }
    fn invalidate(&mut self, e: ExprId, span: Span, mutation: bool) {
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
    fn invalidate_origin(&mut self, origin: &Origin, span: Span) {
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
    fn end_temporaries(&mut self, from: usize, span: Span) {
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

    fn weaken_binding(&mut self, binding: &BindingDecision) {
        let key = binding_key(binding);
        let identity = self.identities.get(&key).cloned();
        let keys: Vec<_> = self
            .binding_types
            .keys()
            .filter(|candidate| {
                **candidate == key
                    || identity.as_ref().is_some_and(|origin| {
                        self.identities
                            .get(*candidate)
                            .is_some_and(|other| other.whole.root == origin.whole.root)
                    })
            })
            .cloned()
            .collect();
        for key in keys {
            let ty = self.binding_types[&key];
            let mut row = self.out.body.types.get(ty).unwrap().clone();
            row.modes.uniqueness = Uniqueness::Aliased;
            row.modes.sendability = Sendability::NotSendable;
            let ty = self.out.body.types.push(row);
            self.binding_types.insert(key, ty);
        }
    }
    fn weaken(&mut self, expression: ExprId) {
        let binding = self
            .out
            .body
            .places
            .iter()
            .find(|p| {
                work::count(Counter::AuthoritySourceRows);
                p.expression == expression
            })
            .and_then(|place| match &place.root {
                SourceRoot::Binding { owner, binding } => {
                    self.root_binding(*owner, binding).cloned()
                }
                _ => None,
            })
            .or_else(|| self.binding(expression));
        if let Some(binding) = binding {
            self.weaken_binding(&binding);
        }
        let captures = self
            .identity(expression)
            .and_then(|origin| match origin.root {
                OriginRoot::Expression(expression) => self
                    .captures
                    .iter()
                    .find(|capture| capture.expression == expression)
                    .map(|capture| capture.captures.clone()),
                _ => None,
            })
            .unwrap_or_default();
        for capture in captures {
            if matches!(capture.usage, UseKind::Borrow | UseKind::Mut) {
                if let Some(binding) = self
                    .out
                    .body
                    .bindings
                    .iter()
                    .find(|binding| {
                        binding.owner == capture.owner && binding.binding == capture.binding
                    })
                    .cloned()
                {
                    self.weaken_binding(&binding);
                }
            }
        }
    }
    /// OWN-8/OWN-1: a non-plain part of a borrowed capture is in the
    /// capture's locality, as a part of a borrowed parameter is.
    fn localize_view_part(&mut self, expression: ExprId, root: Option<BindingKey>) {
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
    fn refine_projection(&mut self, expression: ExprId, declared: TypeId) {
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
    fn projection_fields(
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
    fn fields(&mut self, root: TypeId) -> Option<Vec<Field>> {
        let mut fields = match self.layout(root)? {
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
        for field in &mut fields {
            field.ty = self.placed_item(root, field.ty);
        }
        Some(fields.to_vec())
    }
    fn layout(&mut self, root: TypeId) -> Option<TypeKind> {
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
    fn annotation(&mut self, ty: TypeId, syntax: hirsel_syntax::TypeExprId) -> TypeId {
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
    fn within_bindings(&mut self, ty: TypeId, syntax: hirsel_syntax::TypeExprId) -> TypeId {
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
    fn syntax_uses(&mut self, site: NameSite, paths: &[hirsel_syntax::Path]) -> Uses {
        let mut authority = AuthoritySummary::inert();
        let resolved: Vec<_> = self
            .out
            .body
            .resolution
            .paths
            .iter()
            .filter(|(s, _)| s == &site)
            .map(|(_, n)| n.clone())
            .collect();
        let scopes: Vec<_> = self
            .out
            .body
            .resolution
            .reference_scopes
            .iter()
            .filter(|(s, _)| *s == site)
            .map(|(_, owner)| *owner)
            .collect();
        let mut scopes = scopes.into_iter();
        for (index, path) in paths.iter().enumerate() {
            let name = resolved.get(index).cloned();
            match name {
                Some(ResolvedName::Definition(d)) => {
                    authority = authority.join(AuthoritySummary::Known(
                        vec![AuthorityTerm::Capability(d)].into(),
                    ));
                }
                Some(n) => {
                    let owner = scopes.next();
                    if let Some(b) = self
                        .out
                        .body
                        .bindings
                        .iter()
                        .find(|b| {
                            work::count(Counter::AuthoritySourceRows);
                            b.binding == n
                                && owner.map_or(
                                    matches!(n, ResolvedName::Local(_)) || b.owner == site,
                                    |owner| b.owner == owner,
                                )
                        })
                        .cloned()
                    {
                        authority = authority.join(self.authority(b.ty));
                    } else {
                        authority = AuthoritySummary::Unknown;
                    }
                }
                None => {
                    let _ = path;
                    authority = AuthoritySummary::Unknown;
                }
            }
        }
        Uses::Only(authority)
    }
    fn constructor(
        &mut self,
        e: ExprId,
        _callee: ExprId,
        arguments: &[Argument],
        expected: Option<TypeId>,
    ) -> bool {
        let Some(mut root) = self.ty(e) else {
            return false;
        };
        if self.capability_definition(root).is_some_and(|path| self.read(&path).is_some_and(|s|
            matches!(&s.kind, SignatureKind::Type(d) if d.capability == Some(CapabilityKind::Scoped))))
        {
            let mut row = self.out.body.types.get(root).unwrap().clone();
            // A local seal is bounded by this invocation even when every
            // represented field is physically Plain.
            row.modes.relation = row.modes.relation.union(&Relation::environment());
            root = self.out.body.types.push(row);
            self.set_ty(e, root);
        }
        // The binding places the destination's relation names: a field is
        // within the owner's relation, so it admits what they admit (v22 R3).
        let owner_places = expected.map(|expected| {
            self.out
                .body
                .types
                .get(expected)
                .unwrap()
                .modes
                .relation
                .retain(|r| match r {
                    Reach::Place(o) => !self.arena_place(o),
                    Reach::Environment => true,
                    Reach::Region(_) => false,
                })
        });
        if let Some(expected) = expected {
            let goal = self.out.body.types.get(expected).unwrap().clone();
            let mut row = self.out.body.types.get(root).unwrap().clone();
            // Expected placement is a construction requirement; operand modes
            // and nested generic contracts remain independently supplied.
            let placement = self.placement(&goal.modes.relation);
            if self.placement(&row.modes.relation).is_global() && !placement.is_global() {
                row.modes.relation = row.modes.relation.union(&placement);
            }
            if self.out.body.types.get(root) != Some(&row) {
                root = self.out.body.types.push(row);
                self.set_ty(e, root);
            }
        }
        let Some(mut fields) = self.constructor_fields(e, root) else {
            return false;
        };
        if let Some(goal) = expected.filter(|goal| self.brandless_variant(e, *goal).is_some()) {
            root = self.part_brands(root, goal);
            self.set_ty(e, root);
            fields = self.constructor_fields(e, root).unwrap();
        }
        self.verdicts.last_mut().unwrap().call_prerequisites = Some(true);
        let Some(call) = self.call_row(e) else {
            self.out
                .failure
                .get_or_insert(QueryError::Operational(OperationalFailure {
                    operation: "authority constructor".into(),
                    message: "missing H call mapping".into(),
                }));
            return true;
        };
        let supplied: BTreeMap<FieldId, ExprId> = call
            .arguments
            .iter()
            .filter_map(|a| {
                a.parameter
                    .as_ref()
                    .map(|(i, _)| (FieldId(*i), a.expression))
            })
            .collect();
        let mut used = AuthoritySummary::inert();
        for a in arguments {
            let expected = supplied
                .iter()
                .find(|(_, e)| **e == a.value)
                .and_then(|(f, _)| fields.get(f.0 as usize))
                .map(|f| f.ty);
            self.expr(a.value, Some((CaptureContext::Escaping, expected, "CLO-4")));
            used = used.join(self.used(a.value));
        }
        // a constructed value holds its parts' loans, so it lives
        // where their arena content lives, and each field is within it. The
        // owner is checked where it is kept (#79), as a tuple is. A
        // destination that names an arena already placed the owner, and its
        // fields are checked against it.
        let holdings = supplied
            .iter()
            .filter_map(|(field, part)| {
                let ty = self.ty(*part)?;
                if self.property(ty, Bound::Plain) {
                    return Some((*field, Relation::global()));
                }
                let relation = &self.out.body.types.get(ty).unwrap().modes.relation;
                Some((*field, self.placement(relation)))
            })
            .collect();
        self.part_holdings.insert(e, holdings);
        let mut parts = Relation::global();
        let supplied_types: Vec<_> = supplied.values().filter_map(|p| self.ty(*p)).collect();
        for part in supplied_types {
            if !self.property(part, Bound::Plain) {
                let relation = &self.out.body.types.get(part).unwrap().modes.relation;
                parts = parts.union(&self.placement(relation));
            }
        }
        let mut row = self.out.body.types.get(root).unwrap().clone();
        if self.placement(&row.modes.relation).is_global() && !parts.is_global() {
            row.modes.relation = row.modes.relation.union(&parts);
            root = self.out.body.types.push(row);
            self.set_ty(e, root);
            let Some(placed) = self.constructor_fields(e, root) else {
                return false;
            };
            fields = placed;
        }
        let siblings: BTreeMap<_, _> = supplied
            .iter()
            .filter_map(|(f, e)| self.identity(*e).map(|o| (*f, o)))
            .collect();
        if self.verdicts.last().unwrap().call_prerequisites.unwrap() {
            for (&field, &earg) in &supplied {
                let Some(f) = fields.get(field.0 as usize) else {
                    continue;
                };
                let has_sibling = self
                    .out
                    .body
                    .types
                    .get(f.ty)
                    .unwrap()
                    .brand
                    .alternatives
                    .iter()
                    .any(|o| matches!(o.root, OriginRoot::Sibling(_)));
                let mut expected =
                    super::facts::siblings(&mut self.out.body.types, f.ty, &siblings);
                if let Some(places) = owner_places.as_ref().filter(|p| !p.is_global()) {
                    let mut row = self.out.body.types.get(expected).unwrap().clone();
                    if !self.placement(&row.modes.relation).is_global() {
                        row.modes.relation = row.modes.relation.union(places);
                        expected = self.out.body.types.push(row);
                    }
                }
                if let Some(actual) = self.ty(earg) {
                    self.kept(
                        actual,
                        expected,
                        self.span(e),
                        earg,
                        has_sibling,
                        Receiving::new("the constructed field", self.span(e)),
                        // A field's locality follows its owner, whose row
                        // carries its parts' placement and is checked where
                        // it is kept.
                        true,
                    );
                }
            }
        }
        self.constructor_argument_brands(e, root);
        self.hold_moved_fields(e, &supplied);

        let parts: Vec<_> = arguments.iter().map(|argument| argument.value).collect();
        self.construction_sendability(e, &parts);
        if let Some(root) = self.ty(e) {
            let evidence = self
                .out
                .body
                .types
                .get(root)
                .unwrap()
                .modes
                .sendability
                .clone();
            let received = declared_receipt_evidence(&mut self.out.body.types, root)
                .expect("checked constructor receipt");
            let mut row = self.out.body.types.get(received).unwrap().clone();
            row.modes.sendability = evidence;
            let received = self.out.body.types.push(row);
            self.set_ty(e, received);
        }
        if arguments
            .iter()
            .all(|argument| self.unique_construction_part(argument.value))
        {
            self.set_unique(e);
        }
        self.arena_construction(e);
        // Preserve nominal provider attempts and their failure policy.
        let _ = self.authority(root);
        self.facts.insert(e, used);
        true
    }
    /// A field moved into a fresh value carries its owner identity there
    /// (PROV-2: Take transports existing evidence), so the value holds it
    /// until the value is transferred or written. A Plain or copied field
    /// leaves its source owning its identity.
    fn hold_moved_fields(&mut self, e: ExprId, supplied: &BTreeMap<FieldId, ExprId>) {
        let mut held = BTreeSet::new();
        for (field, part) in supplied {
            let copied = matches!(
                self.tree.expressions[self.transparent_value(*part).0 as usize].kind,
                ExprKind::Unary {
                    op: UnaryOp::Copy | UnaryOp::Unique,
                    ..
                }
            );
            if copied
                || self
                    .ty(*part)
                    .is_none_or(|ty| self.property(ty, Bound::Plain))
            {
                continue;
            }
            let Some(partition) = self.value_partition(*part) else {
                continue;
            };
            let at = |path: &[FieldId]| [&[*field][..], path].concat();
            held.insert((at(&[]), partition.whole.clone()));
            held.extend(partition.parts.iter().map(|(p, o)| (at(p), o.clone())));
            held.extend(partition.held.iter().map(|(p, o)| (at(p), o.clone())));
        }
        if held.is_empty() {
            return;
        }
        if let Some(mut partition) = self.value_partition(e) {
            partition.held.extend(held);
            self.expression_partitions.insert(e, partition);
        }
    }
    // Infer payload brands from supplied fields, using the declaration's exact
    // type-parameter occurrences rather than copying a receiving annotation.
    fn constructor_argument_brands(&mut self, e: ExprId, root: TypeId) {
        let Some(call) = self.call_row(e).cloned() else {
            return;
        };
        let CallTarget::Constructor(ContextualName::Variant { variant, .. }) = call.target else {
            return;
        };
        let mut row = self.out.body.types.get(root).unwrap().clone();
        let TypeKind::Named {
            definition,
            arguments,
        } = &mut row.kind
        else {
            return;
        };
        let Some(signature) = self.read(definition) else {
            return;
        };
        let SignatureKind::Type(declaration) = &signature.kind else {
            return;
        };
        let Some(representation) = declaration.representation else {
            return;
        };
        let Some(TypeKind::Variant(variants)) =
            signature.types.get(representation).map(|t| &t.kind)
        else {
            return;
        };
        let Some(selected) = variants.get(variant.0 as usize) else {
            return;
        };
        let mut pending = Vec::new();
        for argument in &call.arguments {
            if let (Some((field, _)), Some(actual)) =
                (argument.parameter.as_ref(), self.ty(argument.expression))
            {
                if let Some((_, formal)) = selected.fields.get(*field as usize) {
                    pending.push((*formal, actual, 0));
                }
            }
        }
        let mut seen = BTreeSet::new();
        let mut occurrences: BTreeMap<u32, Vec<TypeId>> = BTreeMap::new();
        while let Some((formal, actual, depth)) = pending.pop() {
            if !seen.insert((formal, actual, depth)) {
                continue;
            }
            let formal = signature.types.get(formal).unwrap();
            let actual_row = self.out.body.types.get(actual).unwrap();
            if let TypeKind::Parameter(hirsel_types::TypeParameterRef::Bound {
                depth: d,
                parameter,
            }) = formal.kind
            {
                if d == depth {
                    occurrences.entry(parameter.0).or_default().push(actual);
                }
                continue;
            }
            if std::mem::discriminant(&formal.kind) != std::mem::discriminant(&actual_row.kind) {
                continue;
            }
            let next = depth + u32::from(matches!(formal.kind, TypeKind::Function(_)));
            pending.extend(
                formal
                    .kind
                    .value_parts()
                    .into_iter()
                    .zip(actual_row.kind.value_parts())
                    .map(|(a, b)| (a, b, next)),
            );
        }
        for (parameter, parts) in occurrences {
            if let Some(GenericArgument::Type(actual)) = arguments.get_mut(parameter as usize) {
                let joined = self.join_types(parts[0], &parts);
                let refined = self.part_brands(*actual, joined);
                let mut part = self.out.body.types.get(refined).unwrap().clone();
                part.brand = self.out.body.types.get(joined).unwrap().brand.clone();
                *actual = self.out.body.types.push(part);
            }
        }
        let refined = self.out.body.types.push(row);
        self.set_ty(e, refined);
    }
    /// A constructor's fields placed in its owner: a record's, or the
    /// selected variant's.
    fn constructor_fields(&mut self, e: ExprId, root: TypeId) -> Option<Vec<Field>> {
        self.fields(root).or_else(|| {
            let call = self.call_row(e)?;
            let CallTarget::Constructor(ContextualName::Variant { variant, .. }) = &call.target
            else {
                return None;
            };
            let variant = *variant;
            // A Packed construction builds its payload variant (MEM-13).
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
            Some(
                variants
                    .get(variant.0 as usize)?
                    .fields
                    .iter()
                    .map(|(name, ty)| Field {
                        name: name.clone().unwrap_or_default(),
                        ty: self.placed_item(root, *ty),
                        growable: false,
                    })
                    .collect(),
            )
        })
    }
    fn expression_children(&self, e: ExprId) -> Vec<ExprId> {
        crate::expression_children(self.tree, e)
    }
    fn free_uses(&self, owner: NameSite, body: ExprId) -> Vec<FreeUse> {
        let mut owners = BTreeSet::from([owner.clone()]);
        let mut stack = vec![body];
        let mut all = Vec::new();
        while let Some(e) = stack.pop() {
            if matches!(
                self.tree.expressions[e.0 as usize].kind,
                ExprKind::Closure { .. }
            ) {
                owners.insert(NameSite::Expression(e));
            }
            if let ExprKind::Block { statements, .. } = &self.tree.expressions[e.0 as usize].kind {
                for s in statements {
                    if matches!(
                        self.tree.statements[s.0 as usize].kind,
                        StmtKind::Function { .. }
                    ) {
                        owners.insert(NameSite::Statement(*s));
                    }
                }
            }
            all.push(e);
            let children = self.expression_children(e);
            stack.extend(children.into_iter().rev());
        }
        let mut uses: Vec<FreeUse> = vec![];
        for e in all {
            let (reference, explicit_take) = match self.tree.expressions[e.0 as usize].kind {
                ExprKind::Unary {
                    op: UnaryOp::Take,
                    value,
                } => (value, true),
                _ => (e, false),
            };
            let Some(mut b) = self.binding(reference) else {
                continue;
            };
            if owners.contains(&b.owner)
                || (b.site == owner && matches!(b.binding, ResolvedName::Local(_)))
            {
                continue;
            }
            b.ty = self
                .binding_types
                .get(&binding_key(&b))
                .copied()
                .unwrap_or(b.ty);
            // The capture mode is chosen from syntax, not the inner elaborated use.
            if let Some(prior) = uses
                .iter_mut()
                .find(|u| u.binding.owner == b.owner && u.binding.binding == b.binding)
            {
                prior.take |= explicit_take;
            } else {
                uses.push(FreeUse {
                    binding: b,
                    first: reference,
                    take: explicit_take,
                });
            }
        }
        uses
    }
    fn closure(
        &mut self,
        e: ExprId,
        body: ExprId,
        context: Option<(CaptureContext, Option<TypeId>, &'static str)>,
    ) {
        let actual = self.ty(e).unwrap();
        let original = self.out.body.types.get(actual).unwrap().clone();
        let syntax = match self.tree.expressions[e.0 as usize].kind.clone() {
            ExprKind::Closure { signature, .. } => Some(signature),
            _ => None,
        };
        self.function_value(e, body, NameSite::Expression(e), original, syntax, context);
    }
    fn set_function_type(&mut self, key: ExprId, owner: &NameSite, ty: TypeId) {
        if matches!(owner, NameSite::Expression(_)) {
            self.set_ty(key, ty);
        } else if let Some(binding) = self.out.body.bindings.iter_mut().find(|b| {
            work::count(Counter::AuthoritySourceRows);
            &b.site == owner && matches!(b.binding, ResolvedName::Local(_))
        }) {
            binding.ty = ty;
            self.binding_types.insert(binding_key(binding), ty);
        }
    }
    fn function_value(
        &mut self,
        e: ExprId,
        body: ExprId,
        owner: NameSite,
        original: Type,
        syntax: Option<hirsel_syntax::FunctionSyntax>,
        context: Option<(CaptureContext, Option<TypeId>, &'static str)>,
    ) {
        let TypeKind::Function(mut f) = original.kind.clone() else {
            self.out.failure.get_or_insert(QueryError::NotImplemented);
            return;
        };
        let inferred_result = context
            .as_ref()
            .is_none_or(|(_, expected, _)| expected.is_none())
            && syntax.as_ref().is_some_and(|s| s.result.is_none());
        let scopes: Vec<_> = std::iter::once(owner)
            .chain(self.frames.iter().rev().map(|f| f.owner))
            .collect();
        for p in &mut f.parameters {
            p.ty = bind_parameter_scopes(&mut self.out.body.types, p.ty, &scopes)
                .expect("checked lexical parameter scope");
        }
        if !inferred_result {
            f.result = bind_parameter_scopes(&mut self.out.body.types, f.result, &scopes)
                .expect("checked lexical parameter scope");
        }
        if let Some(signature) = &syntax {
            for (parameter, syntax) in f.parameters.iter_mut().zip(&signature.parameters) {
                parameter.ty = self.annotation(parameter.ty, syntax.ty);
                parameter.ty = declared_receipt_evidence(&mut self.out.body.types, parameter.ty)
                    .expect("declared closure entry");
            }
            // H may erase dependent provenance on a nested binding while the
            // function header retains it. Apply the same written annotation to
            // that binding, preserving its elaborated structure and modes.
            let parameters: Vec<_> = self
                .source_rows
                .parameters
                .get(&owner)
                .into_iter()
                .flatten()
                .copied()
                .filter_map(|index| {
                    work::count(Counter::AuthoritySourceRows);
                    let binding = &self.out.body.bindings[index];
                    let ResolvedName::Parameter(parameter) = binding.binding else {
                        return None;
                    };
                    let syntax = signature.parameters.get(parameter.0 as usize)?;
                    Some((index, binding.ty, syntax.ty))
                })
                .collect();
            for (index, ty, syntax) in parameters {
                let annotated = self.annotation(ty, syntax);
                let annotated = declared_receipt_evidence(&mut self.out.body.types, annotated)
                    .expect("declared closure parameter");
                let binding = &mut self.out.body.bindings[index];
                binding.ty = annotated;
                self.binding_types.insert(binding_key(binding), annotated);
            }
            if let Some(result) = signature.result {
                f.result = self.annotation(f.result, result);
            }
            if let Some(uses) = &signature.uses {
                f.uses = self.syntax_uses(owner.clone(), uses);
            }
        }
        let (ctx, expected, restriction) =
            context.unwrap_or((CaptureContext::Scoped, None, "CAP-3"));
        let expected_type = expected.and_then(|t| self.out.body.types.get(t)).cloned();
        let expected_function = expected_type.as_ref().and_then(|t| match &t.kind {
            TypeKind::Function(f) => Some(f.clone()),
            _ => None,
        });
        let capture_limit = expected_function
            .as_ref()
            .map(|f| f.uses.clone())
            .unwrap_or_else(|| f.uses.clone());
        let capture_limit = match capture_limit {
            Uses::Only(a) => Uses::Only(self.scoped_authority(a)),
            other => other,
        };
        // CLO-4/CAP-3: the contextual restriction is checked for captures and
        // the body below, so the published callable must retain that promise.
        if matches!(capture_limit, Uses::Only(_)) {
            f.uses = capture_limit.clone();
        }
        let free = self.free_uses(owner.clone(), body);
        let mut captures = Vec::new();
        let mut authority = AuthoritySummary::inert();
        let mut modes = original.modes.clone();
        modes.relation = if matches!(ctx, CaptureContext::Scoped | CaptureContext::OwningScoped) {
            Relation::environment()
        } else {
            Relation::global()
        };
        modes.multiplicity = if ctx == CaptureContext::Task {
            Multiplicity::Once
        } else {
            Multiplicity::Many
        };
        let mut sendable = matches!(ctx, CaptureContext::Escaping | CaptureContext::Task)
            || expected_type
                .as_ref()
                .is_some_and(|t| crossing_requirement(t) == Sendability::Sendable);
        // Borrowed captures as the body sees them, with their source rows.
        let mut views = Vec::new();
        for free_use in free {
            let mut b = free_use.binding;
            b.ty = self
                .binding_types
                .get(&binding_key(&b))
                .copied()
                .unwrap_or(b.ty);
            let usage = if matches!(ctx, CaptureContext::Scoped) {
                UseKind::Borrow
            } else if free_use.take {
                UseKind::Take
            } else if ctx == CaptureContext::OwningScoped {
                UseKind::Borrow
            } else {
                UseKind::Copy
            };
            let mut capture_type = self.out.body.types.get(b.ty).unwrap().clone();
            let plain = self.property(b.ty, Bound::Plain);
            let flat = self.property(b.ty, Bound::Flat);
            // The environment's locality and uniqueness are the ownership
            // pass's, from current content; these rows feed sendability.
            let shared = matches!(
                capture_type.kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Shared,
                    ..
                }
            );
            // A copied Shared root is an independently counted handle (MEM-11).
            if usage == UseKind::Copy && (plain || flat || shared) {
                capture_type.modes.relation = self.placement(&capture_type.modes.relation);
            }
            if usage == UseKind::Copy && !plain && !flat {
                capture_type.modes.uniqueness = Uniqueness::Aliased;
                capture_type.sendability_requirement = SendabilityRequirement::None;
                let root = self.out.body.types.push(capture_type.clone());
                let store = self.out.body.types.clone();
                let generics = self.declarations();
                capture_type.modes.sendability =
                    sendability_after_sharing(&store, root, &generics, &mut |path| {
                        Ok(self.read(path))
                    })
                    .unwrap_or_else(|error| self.proof_failure(error, Sendability::NotSendable));
            }
            // CLO-1/OWN-1: a closure body knows a borrowed capture exactly as a
            // function body knows a borrowed parameter of the same type. It is
            // local, and anything non-plain derived from it is local to the
            // environment; placement and brands stay those of the source.
            // The source row is a receiving contract. When a source's current
            // content is local (a finalizer's value, a record holding local
            // content), the ownership pass makes the result local again.
            let view = matches!(usage, UseKind::Borrow | UseKind::Mut) && !plain;
            if view {
                capture_type.modes.relation =
                    Relation::environment().union(&self.placement(&capture_type.modes.relation));
            }
            let capture_type_id = self.out.body.types.push(capture_type.clone());
            if view {
                views.push((binding_key(&b), capture_type_id, b.ty));
            }
            let a = self.authority(b.ty);
            authority = authority.join(a.clone());
            if let Uses::Only(allowed) = &capture_limit {
                if !subset(&a, allowed) {
                    let rule = if matches!(ctx, CaptureContext::Escaping | CaptureContext::Task) {
                        "CLO-4"
                    } else {
                        restriction
                    };
                    let provided = if rule == "CONC-5" {
                        let (name, site) = self.sites().binding(&b);
                        Fact::new(Cause::AuthorityCapture, site, &[&name])
                    } else {
                        Fact::new(Cause::Authority, self.span(free_use.first), &[])
                    };
                    let seal = self.capability_definition(b.ty);
                    let message = seal.as_ref().map_or_else(
                        || "captured authority exceeds the callback's declared uses".to_owned(),
                        |path| {
                            let limit = match allowed {
                                AuthoritySummary::Known(terms) => format!(
                                    "{{{}}}",
                                    terms
                                        .iter()
                                        .map(|term| match term {
                                            AuthorityTerm::Capability(path) => path.0.name.clone(),
                                            _ => "parameter authority".into(),
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                                AuthoritySummary::Unknown => "unknown".into(),
                            };
                            format!("captures {}; allowed {limit}", path.0.name)
                        },
                    );
                    let before = self.out.diagnostics.len();
                    self.explained(
                        rule,
                        self.span(free_use.first),
                        &message,
                        Explanation::default()
                            .provided(provided)
                            .required(Fact::new(Requirement::Uses, self.span(e), &[])),
                    );
                    if self.out.diagnostics.len() > before {
                        if let Some(path) = seal {
                            let diagnostic = self.out.diagnostics.last_mut().unwrap();
                            diagnostic.notes.push(format!(
                                "capability declaration: {}@{}/{}{}",
                                path.0.package.name,
                                path.0.package.version,
                                if path.0.modules.is_empty() {
                                    String::new()
                                } else {
                                    format!("{}.", path.0.modules.join("."))
                                },
                                path.0.name
                            ));
                            if let Some(declaration) =
                                self.tree.items.iter().find(|item| item.definition == path)
                            {
                                diagnostic.secondary.push(Label {
                                    span: declaration.name_span,
                                    message: "declared capability".into(),
                                });
                            }
                        }
                    }
                }
            }
            if self.scoped(b.ty) && matches!(ctx, CaptureContext::Escaping | CaptureContext::Task) {
                self.explained(
                    "MODE-6",
                    self.span(e),
                    "a scoped capability cannot enter an escaping capture",
                    Explanation::default()
                        .provided(Fact::new(
                            Cause::ScopedCapability,
                            self.span(free_use.first),
                            &[],
                        ))
                        .required(Fact::new(Requirement::EscapingCapture, self.span(e), &[])),
                );
            } else if ctx == CaptureContext::Task {
                let receiver = matches!(
                    capture_type.kind,
                    TypeKind::Builtin {
                        kind: BuiltinType::Receiver,
                        ..
                    }
                );
                if b.mutable
                    || matches!(usage, UseKind::Borrow | UseKind::Mut)
                    || (receiver && usage != UseKind::Take)
                    || (!receiver && !self.sendable(capture_type_id))
                    // The Receiver exception waives sendability, not
                    // locality: a local value cannot cross into a task.
                    || (receiver && !self.lifetime(&capture_type.modes.relation).is_global())
                {
                    let (name, site) = self.sites().binding(&b);
                    let cause = if b.mutable {
                        Cause::MutableCapture
                    } else if matches!(usage, UseKind::Borrow | UseKind::Mut) {
                        Cause::BorrowedCapture
                    } else if receiver {
                        Cause::ReceiverCapture
                    } else {
                        Cause::NotSendable
                    };
                    self.explained(
                        "CLO-3",
                        self.span(e),
                        "task captures must copy or take sendable values; Receivers must be taken",
                        Explanation::default()
                            .provided(Fact::new(cause, site, &[&name]))
                            .required(Fact::new(Requirement::TaskCapture, self.span(e), &[])),
                    );
                }
            }
            if matches!(ctx, CaptureContext::Escaping | CaptureContext::Task)
                && self.retains_scoped_placement(capture_type_id)
            {
                self.explained(
                    "PROV-4",
                    self.span(e),
                    "a closure cannot outlive the storage of its captured value",
                    Explanation::default()
                        .provided(Fact::new(
                            Cause::ScopedPlacement,
                            self.span(free_use.first),
                            &[],
                        ))
                        .required(Fact::new(Requirement::EscapingCapture, self.span(e), &[])),
                );
            }
            if usage == UseKind::Take || capture_type.modes.multiplicity == Multiplicity::Once {
                modes.multiplicity = Multiplicity::Once;
            }
            let captured_sendable = if usage == UseKind::Copy && !plain && !flat {
                // Use the post-share proof just derived above. Re-entering
                // structural proof would consume stale descendant summaries.
                capture_type.modes.sendability == Sendability::Sendable
            } else {
                self.sendable(capture_type_id)
            };
            if !captured_sendable {
                sendable = false;
            }
            captures.push(CapturedName {
                owner: b.owner,
                binding: b.binding,
                usage,
                ty: capture_type_id,
            });
        }
        modes.sendability = if sendable {
            Sendability::Sendable
        } else {
            Sendability::NotSendable
        };
        // Spec #58: inferred literal values carry capture-derived uses evidence.
        // An explicit header or receiving function type retains its contract.
        if expected.is_none() && matches!(f.uses, Uses::Unrestricted) {
            f.uses = Uses::Only(authority.clone());
        }
        // Capture authority is distinct from authority passed to the closure itself.
        f.authority = authority.clone();
        let mut ty = original.clone();
        ty.kind = TypeKind::Function(f.clone());
        ty.modes = modes;
        // MEM-5: an erased closure's cleanup summary is fixed where it is
        // built, by the values its environment owns; a borrowed capture
        // destroys nothing. Only an unannotated binding's own literal proves
        // it: a receiving type, written or a constructor's, keeps the
        // summary it states, as it keeps its `uses`.
        if expected_function.is_none()
            && matches!(owner, NameSite::Expression(_))
            && self.tree.statements.iter().any(|statement| {
                matches!(statement.kind,
                    StmtKind::Let { ty: None, value, .. } if value == e)
            })
        {
            ty.drop_allocation = captures
                .iter()
                .filter(|capture| matches!(capture.usage, UseKind::Copy | UseKind::Take))
                .fold(DropAllocation::NoAlloc, |summary, capture| {
                    summary.join(
                        self.out
                            .body
                            .types
                            .get(capture.ty)
                            .unwrap()
                            .drop_allocation
                            .clone(),
                    )
                });
        }
        let refined = self.out.body.types.push(ty);
        let refined = abstract_parameter_scope(&mut self.out.body.types, refined, owner)
            .expect("checked callable binder");
        self.set_function_type(e, &owner, refined);

        self.closures.insert(
            e,
            ClosureInfo {
                result_evidence: None,
                expression: e,
                captures: captures
                    .iter()
                    .map(|c| Capture {
                        binding: c.binding.clone(),
                        usage: c.usage,
                        ty: c.ty,
                    })
                    .collect(),
                ty: refined,
            },
        );

        self.captures.insert(
            e,
            CaptureTable {
                expression: e,
                context: ctx,
                captures: captures.clone(),
                authority: authority.clone(),
            },
        );
        let before_types = self.binding_types.clone();
        let before_views = self.capture_views.clone();
        for (key, view, _) in &views {
            self.binding_types.insert(key.clone(), *view);
            self.capture_views.insert(key.clone());
        }
        let before_invalid = self.invalid.clone();
        let before_pending = std::mem::take(&mut self.pending_tasks);
        let before_task_bindings = self.task_bindings.clone();
        let before_loop_exits = std::mem::take(&mut self.loop_task_exits);
        self.frames.push(Frame {
            owner: owner.clone(),
            allowed: Uses::Unrestricted,
            result: (!inferred_result).then_some(f.result),
            restriction,
        });

        let mut allowed = match capture_limit {
            Uses::Only(a) => Some(a),
            Uses::Unrestricted => None,
        };
        // A restricted callback may use the capabilities it receives as parameters.
        if let Some(a) = &mut allowed {
            for p in &f.parameters {
                *a = a.clone().join(self.authority(p.ty));
            }
        }
        self.frames.last_mut().unwrap().allowed = if self.in_clause {
            Uses::Only(AuthoritySummary::inert())
        } else {
            allowed.map(Uses::Only).unwrap_or(Uses::Unrestricted)
        };
        let temporaries = self.temporaries.len();
        self.expr(
            body,
            Some((
                CaptureContext::Escaping,
                (!inferred_result).then_some(f.result),
                "CLO-4",
            )),
        );
        self.frame_temporaries(body, temporaries);
        self.return_value(body, self.result_span(body));
        let result_evidence = self.ty(body);
        if let Some(mut closure) = self.closures.get_mut(&e) {
            closure.result_evidence = result_evidence;
        }
        let mut frame_result = f.result;
        let infers_result = expected_function.is_none()
            && syntax
                .as_ref()
                .is_some_and(|signature| signature.result.is_none());
        // MEM-5: like its `uses` (#58), an inferred literal's `@noalloc` is
        // evidence read off its body. A receiving function type or a written
        // header is a contract and keeps the allocation it states; contracts
        // are code of their own, which this proof does not read.
        let proves_noalloc = expected_function.is_none()
            && matches!(owner, NameSite::Expression(_))
            && syntax
                .as_ref()
                .is_some_and(|signature| signature.contracts.is_empty())
            && f.allocation == Allocation::MayAllocate
            && self.noalloc_evidence(&owner, body);
        if proves_noalloc {
            f.allocation = Allocation::NoAlloc;
        }
        if infers_result || proves_noalloc {
            if let Some(mut result) = self.ty(body).filter(|_| infers_result) {
                // CLO-1: an inferred result is not a uniqueness promise. A
                // borrowed capture read in the body shares its value (MODE-2),
                // whatever the captured binding's row says, so the frame
                // result and the callable's result claim no uniqueness.
                let mut row = self.out.body.types.get(result).unwrap().clone();
                // A literal's inferred result describes a returned value.
                // Its root operand qualifier does not become a written result contract.
                row.sendability_requirement = SendabilityRequirement::None;
                if row.modes.uniqueness == Uniqueness::Unique
                    && !self.property(result, Bound::Plain)
                {
                    row.modes.uniqueness = Uniqueness::Aliased;
                }
                result = self.out.body.types.push(row);
                let returned = self.transparent_value(body);
                let place = self.source_place(returned);
                let source_binding = match &place.root {
                    SourceRoot::Binding { owner, binding } => {
                        self.root_binding(*owner, binding).cloned()
                    }
                    _ => self.binding(returned),
                };
                let borrowed_capture = source_binding
                    .as_ref()
                    .is_some_and(|binding| self.capture_views.contains(&binding_key(binding)));
                let owning = matches!(place.access, SourceAccess::Take | SourceAccess::Unique)
                    || matches!(place.root, SourceRoot::Value(_)) && place.projections.is_empty()
                    || !borrowed_capture
                        && place.projections.is_empty()
                        && source_binding.is_some_and(|binding| binding.usage == UseKind::Take);
                if place.access == SourceAccess::Copy || !owning {
                    let mut row = self.out.body.types.get(result).unwrap().clone();
                    row.sendability_requirement = SendabilityRequirement::None;
                    let root = self.out.body.types.push(row.clone());
                    let store = self.out.body.types.clone();
                    let generics = self.declarations();
                    row.modes.sendability =
                        sendability_after_sharing(&store, root, &generics, &mut |path| {
                            Ok(self.read(path))
                        })
                        .unwrap_or_else(|error| {
                            self.proof_failure(error, Sendability::NotSendable)
                        });
                    result = self.out.body.types.push(row);
                    if let Some(mut closure) = self.closures.get_mut(&e) {
                        closure.result_evidence = Some(result);
                    }
                }
                frame_result = result;
                f.result = self.environment_result(result, &f, &captures, &views);
            }
            f.error = result_error(
                &self.out.body.types,
                f.result,
                &self.signature.metadata.roles,
            );
            let mut row = self.out.body.types.get(refined).unwrap().clone();
            row.kind = TypeKind::Function(f.clone());
            let updated = self.out.body.types.push(row);
            let updated = abstract_parameter_scope(&mut self.out.body.types, updated, owner)
                .expect("checked inferred callable binder");
            self.set_function_type(e, &owner, updated);
            if let Some(mut c) = self.closures.get_mut(&e) {
                c.ty = updated;
            }
        }
        let owner_site = owner.clone();
        let mut owned_brand = false;
        let mut stack = vec![f.result];
        let mut seen = BTreeSet::new();
        while let Some(t) = stack.pop() {
            if !seen.insert(t) {
                continue;
            }
            let ty = self.out.body.types.get(t).unwrap();
            owned_brand |= ty
                .brand
                .alternatives
                .iter()
                .any(|o| matches!(o.root, OriginRoot::Parameter(ParameterRef::Body { scope, .. }) if scope == owner_site))
                || ty.modes.relation.reaches().iter().any(|r| matches!(r, Reach::Place(o) if self.arena_place(o) && matches!(o.root, OriginRoot::Parameter(ParameterRef::Body { scope, .. }) if scope == owner_site)));
            stack.extend(ty.kind.value_parts());
        }
        if owned_brand {
            self.explained(
                "PROV-2",
                self.span(e),
                "a closure result cannot be branded or placed in its own parameters",
                Explanation::default()
                    .provided(Fact::new(Cause::ClosureOwn, self.span(e), &[]))
                    .required(Fact::new(Requirement::NoOwnPlacement, self.span(e), &[])),
            );
        }
        self.check_pending_tasks();
        self.out.result_types.retain(|(site, _)| *site != owner);
        self.out.result_types.push((owner.clone(), frame_result));
        self.pending_tasks = before_pending;
        self.task_bindings = before_task_bindings;
        self.loop_task_exits = before_loop_exits;
        self.frames.pop();
        self.binding_types = before_types;
        self.capture_views = before_views;
        self.invalid = before_invalid;
        // Capture creation happens in the caller even though body evaluation has
        // its own flow snapshot. Counted copies also weaken the retained source.
        for capture in &captures {
            if capture.usage != UseKind::Copy
                || self.property(capture.ty, Bound::Plain)
                || self.property(capture.ty, Bound::Flat)
            {
                continue;
            }
            if let Some(binding) = self.out.body.bindings.iter().find(|b| {
                work::count(Counter::AuthoritySourceRows);
                b.owner == capture.owner && b.binding == capture.binding
            }) {
                let key = binding_key(binding);
                if let Some(ty) = self.binding_types.get(&key).copied() {
                    let mut row = self.out.body.types.get(ty).unwrap().clone();
                    row.modes.uniqueness = Uniqueness::Aliased;
                    row.modes.sendability = Sendability::NotSendable;
                    let ty = self.out.body.types.push(row);
                    self.binding_types.insert(key, ty);
                }
            }
        }
        self.facts.insert(e, authority);
    }
}

impl super::cursors::CursorRefinement for Checker<'_> {
    fn body(&self) -> &CheckedBody {
        &self.out.body
    }
    fn body_mut(&mut self) -> &mut CheckedBody {
        &mut self.out.body
    }
    fn call_result(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
        origins: &BTreeMap<ParameterRef, OriginArgument>,
        actuals: &BTreeMap<ParameterRef, TypeId>,
    ) -> Result<TypeId, QueryError> {
        self.cursor_result(path, arguments, origins, actuals)
            .ok_or_else(|| {
                self.out
                    .failure
                    .clone()
                    .unwrap_or(QueryError::NotImplemented)
            })
    }
    fn item(&mut self, owner: TypeId, item: TypeId) -> Result<TypeId, QueryError> {
        Ok(self.projected_item(owner, item))
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;
    use crate::authority::test_support::Fixture;

    #[test]
    fn resource_free_siblings_do_not_spend_depth_and_a_real_resource_still_rejects() {
        for width in [128, 129] {
            for resource in [false, true] {
                let mut f = Fixture::new();
                let scalar = f.ty(TypeKind::Primitive(Primitive::U8));
                let fields = (0..width)
                    .map(|n| {
                        f.nominal(
                            &format!("N{n}"),
                            None,
                            vec![],
                            AuthoritySummary::inert(),
                            Some(scalar),
                            resource && n == width - 1,
                        )
                    })
                    .collect::<Vec<_>>();
                let root = f.ty(TypeKind::Tuple(fields.into()));
                let mut c = Checker::new(
                    &f.tree,
                    ItemId(0),
                    &f.signature,
                    f.body,
                    &mut f.provider,
                    &f.intrinsics,
                    &f.symbols,
                );
                assert_eq!(c.contains_resource(root), resource);
                assert!(c.out.failure.is_none());
            }
        }
    }

    #[test]
    fn finite_root_layouts_and_resource_walks_have_no_nominal_depth_cutoff() {
        for depth in [128, 129, 200] {
            let mut f = Fixture::new();
            let mut root = f.ty(TypeKind::Primitive(Primitive::U8));
            for n in 0..depth {
                root = f.nominal(
                    &format!("D{n}"),
                    None,
                    vec![],
                    AuthoritySummary::inert(),
                    Some(root),
                    false,
                );
            }
            let mut c = Checker::new(
                &f.tree,
                ItemId(0),
                &f.signature,
                f.body,
                &mut f.provider,
                &f.intrinsics,
                &f.symbols,
            );
            assert!(!c.contains_resource(root));
            assert_eq!(c.layout(root), Some(TypeKind::Primitive(Primitive::U8)));
            assert!(c.out.failure.is_none());
        }
    }
}
