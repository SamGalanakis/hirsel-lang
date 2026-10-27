use super::facts::*;
use super::identities::Identities;
use super::type_bindings::BindingTypes;
use super::*;
use hirsel_diag::{Label, Obligation, RuleId};
use hirsel_modes::reader::Reader;
use hirsel_syntax::{Argument, ExprKind, ItemKind, StmtKind, UnaryOp};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
type HashMap<K, V> = std::collections::HashMap<K, V, hirsel_intern::IdHasher>;
type HashSet<K> = std::collections::HashSet<K, hirsel_intern::IdHasher>;
mod brands;
mod calls;
mod causes;
mod contextual_contracts;
mod crossing;
mod patterns;
mod promises;
use hirsel_modes::explain::{Cause, Explanation, Fact, Requirement};

#[cfg(test)]
thread_local! { static GENERIC_REFINEMENT_ROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(super) fn generic_refinement_rows() -> usize {
    GENERIC_REFINEMENT_ROWS.with(std::cell::Cell::get)
}

/// A labelled block being walked: its consumer's context and each break
/// that leaves it, with that path's state (LANG-11).
struct BlockExit {
    label: hirsel_intern::Symbol,
    context: Option<(CaptureContext, Option<TypeId>, RuleId)>,
    paths: Vec<PathState>,
}
/// The flow state of one path that reaches a join, with the value it yields.
#[derive(Clone)]
struct PathState {
    value: Option<ExprId>,
    binding_types: BindingTypes,
    identities: Identities,
    identity_changes: BTreeMap<Origin, Span>,
    invalid: HashMap<TypeId, Span>,
    pending_tasks: BTreeMap<ExprId, Span>,
}
#[derive(Clone)]
struct Frame {
    owner: NameSite,
    allowed: Uses,
    receipts: BTreeSet<ParamId>,
    result: Option<TypeId>,
    restriction: RuleId,
}
impl Frame {
    fn parameter_receipts(uses: &Uses, owner: SiteRef) -> BTreeSet<ParamId> {
        let Uses::Only(AuthoritySummary::Known(terms)) = uses else {
            return BTreeSet::new();
        };
        terms
            .iter()
            .filter_map(|term| match term {
                AuthorityTerm::Parameter(ParameterRef::Bound {
                    depth: 0,
                    parameter,
                }) => Some(*parameter),
                AuthorityTerm::Parameter(ParameterRef::Body { scope, parameter })
                    if *scope == owner =>
                {
                    Some(*parameter)
                }
                _ => None,
            })
            .collect()
    }
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
    borrowed: bool,
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
            borrowed: false,
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
    /// The session interner every row of this body lives in.
    pub(super) t: &'a TypeInterner,
    pub(super) m: &'a TypeMemo,
    /// The rows this body knows: its checked tables' graphs and every row
    /// this pass interns.
    rows: hirsel_modes::rows::BodyRows,
    syntax: &'a ItemSyntax,
    tree: &'a SyntaxTree,
    /// The owner rows qualify this item's lexical sites with.
    lexical: LexicalOwner,
    signature: &'a Signature,
    /// Every signature read of this query, the memo's included.
    reader: &'a RefCell<Reader<'a>>,
    /// The query's contextual proofs and comparisons.
    memo: ContextMemo<'a>,
    intrinsics: &'a Intrinsics,
    receiving: Option<Receiving>,
    expression_origins: BTreeMap<ExprId, Origin>,
    out: AuthorityOutput,
    /// Some signature read failed: its own diagnostics stand for the body.
    failed_read: bool,
    elaborations: hirsel_modes::rows::ExprRows<usize>,
    resolved: BTreeMap<ExprId, ResolvedName>,
    source_rows: super::source_rows::SourceRows,
    closures: super::closure_rows::ClosureRows,
    captures: super::closure_rows::OrderedRows<CaptureTable>,
    facts: BTreeMap<ExprId, AuthoritySummary>,
    /// Receipts retain their value derivation in the creating frame until a
    /// write or nonpreserving hand-back. Captures still charge their own type.
    derivations: BTreeMap<BindingKey, (NameSite, AuthoritySummary, ExprId)>,
    binding_types: BindingTypes,
    /// Borrowed captures the closure bodies being walked see as borrowed
    /// parameters (CLO-1).
    capture_views: BTreeSet<BindingKey>,
    identities: Identities,
    /// A mutable yielded view has a fresh value name but writes its source
    /// collection. Mutable aliases keep that storage path separately.
    iteration_sources: BTreeMap<BindingKey, ExprId>,
    identity_paths: BTreeSet<Vec<FieldId>>,
    /// The identity paths a brand or relation names, directly or under a
    /// projected prefix.
    branded_paths: BTreeSet<Vec<FieldId>>,
    /// Layouts expanded so far into the append-only body store: expanding a
    /// nominal type again only appends an equal copy of its representation,
    /// once per field read of a wide record (GAP 5).
    layouts: HashMap<TypeId, TypeKind>,
    expression_partitions: BTreeMap<ExprId, Identity>,
    /// The arena placement of each part a constructor received.
    part_holdings: BTreeMap<ExprId, BTreeMap<FieldId, Relation>>,
    /// The known authority of the erased carrier a place projection reads
    /// through (CAP-2, CLO-4); a write there must stay within it.
    pub(super) carrier_places: BTreeMap<ExprId, AuthoritySummary>,
    /// Per-field arena holdings of a record binding declared without `mut`
    /// from a constructor; such a binding is never written or lent `mut`, so
    /// each field keeps what it was constructed with (#82).
    field_holdings: BTreeMap<BindingKey, BTreeMap<FieldId, Relation>>,
    invalid: HashMap<TypeId, Span>,
    brand_inputs: BTreeMap<OriginRoot, Vec<TypeId>>,
    indexed_brand_rows: usize,
    mut_parameters: Vec<BindingKey>,
    /// Places visited as write targets, bases of larger places, or whole
    /// transfer operands; only reads need live brand evidence.
    unread: BTreeSet<ExprId>,
    /// Reads of ended values, reported unless the enclosing call reports
    /// the ended owner at its own branded use.
    ended_reads: Vec<Diagnostic>,
    verdicts: Vec<Verdict>,
    reported: BTreeSet<(hirsel_span::Anchor, u32, u32, String)>,
    /// Temporaries that lent their identity to a brand in the current statement.
    temporaries: Vec<OriginRoot>,
    /// Mut parameters already reported as handed back with an ended brand.
    returned_ended: BTreeSet<BindingKey>,
    identity_changes: BTreeMap<Origin, Span>,
    frames: Vec<Frame>,
    task_values: BTreeMap<ExprId, BTreeSet<ExprId>>,
    task_bindings: BTreeMap<BindingKey, BTreeSet<ExprId>>,
    pending_tasks: BTreeMap<ExprId, Span>,
    loop_task_exits: Vec<(hirsel_syntax::StmtId, BTreeMap<ExprId, Span>)>,
    block_exits: Vec<BlockExit>,
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
        t: &'a TypeInterner,
        m: &'a TypeMemo,
        syntax: &'a ItemSyntax,
        signature: &'a Signature,
        mut body: CheckedBody,
        reader: &'a RefCell<Reader<'a>>,
        memo: ContextMemo<'a>,
        intrinsics: &'a Intrinsics,
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
        let reads = body.resolution.reads.to_vec();
        let mut identity_paths = BTreeSet::new();
        fn add(paths: &mut BTreeSet<Vec<FieldId>>, fields: &[FieldId]) {
            if !fields.is_empty() {
                paths.insert(fields.to_vec());
            }
        }
        let rows = hirsel_modes::rows::BodyRows::new(t, &body);
        for row in rows.ids().iter().map(|id| t.row(*id)) {
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
            // A deep write may be the only direct use of an intermediate
            // owner. Its copied subrecords still need callable selectors
            // relative to each parent, e.g. inner.a.other beside inner.a.n.
            for end in 1..=prefix.len() {
                prefixes.insert(prefix[..end].to_vec());
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
            t,
            m,
            rows,
            syntax,
            tree: &syntax.tree,
            lexical: syntax.owner(),
            signature,
            reader,
            memo: memo.with_nominal_roles(&signature.metadata.roles),
            intrinsics,
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
            failed_read: false,
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
            iteration_sources: BTreeMap::new(),
            identity_paths,
            branded_paths,
            layouts: HashMap::default(),
            expression_partitions: BTreeMap::new(),
            part_holdings: BTreeMap::new(),
            carrier_places: BTreeMap::new(),
            field_holdings: BTreeMap::new(),
            invalid: HashMap::default(),
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
            block_exits: vec![],
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
        let item = &self.syntax.item;
        let (body, allowed, result) = match (&item.kind, &self.signature.kind) {
            (ItemKind::Function { body, .. }, SignatureKind::Function(f)) => {
                let result =
                    bind_parameter_scopes(self.t, f.result, &[self.lexical.site(NameSite::Item)])
                        .ok();
                // Provision comes from parameters, whereas uses limits captured/used authority.
                if !self.validate_header(f, item.origin.span) {
                    // A declaration identity error invalidates the body result
                    // contract. Do not diagnose uses under that invalid promise.
                    return self.out;
                }
                let allowed = if let Uses::Only(a) = &f.uses {
                    let parameter_types: Vec<_> = f
                        .parameters
                        .iter()
                        .filter_map(|p| {
                            bind_parameter_scopes(
                                self.t,
                                p.ty,
                                &[self.lexical.site(NameSite::Item)],
                            )
                            .ok()
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
                                vec![AuthorityTerm::TypeParameter(TypeParameterRef::body(
                                    self.lexical.site(NameSite::Item),
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
            owner: NameSite::Item,
            allowed,
            receipts: match &self.signature.kind {
                SignatureKind::Function(f) => {
                    Frame::parameter_receipts(&f.uses, self.lexical.site(NameSite::Item))
                }
                _ => BTreeSet::new(),
            },
            result,
            restriction: const { RuleId::of("CAP-1") },
        });

        if let Some(result) = result {
            self.out.result_types.push((NameSite::Item, result));
        }
        if let Some(body) = body {
            self.parameter_tasks();
            let temporaries = self.temporaries.len();
            self.expr(
                body,
                Some((
                    CaptureContext::Escaping,
                    self.frames.last().and_then(|f| f.result),
                    const { RuleId::of("CLO-4") },
                )),
            );
            self.frame_temporaries(body, temporaries);
            self.return_value(body, self.result_span(body));
            self.check_pending_tasks();
        }
        for (root, function) in self.clauses.clone() {
            let TypeKind::Function(function) = self.t.get(function).unwrap().kind.clone() else {
                continue;
            };
            self.frames.push(Frame {
                owner: NameSite::Expression(root),
                allowed: Uses::Only(AuthoritySummary::inert()),
                receipts: BTreeSet::new(),
                result: Some(function.result),
                restriction: const { RuleId::of("FACT-1") },
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
        // Rows have no order; ties keep their production order.
        self.out.identity_fields.sort_by(|a, b| {
            (&a.0, &a.1, &a.2)
                .cmp(&(&b.0, &b.1, &b.2))
                .then_with(|| a.3.iter().map(|(f, _)| f).cmp(b.3.iter().map(|(f, _)| f)))
        });
        self.out.result_types.sort_by(|(a, _), (b, _)| a.cmp(b));
        self.out.captures = self.captures.into_sorted();
        self.out.body.closures = self.closures.into_sorted();
        self.out.dependencies.sort_by_key(|a| a.definition);
        self.out.dependencies.dedup();
        // Authority appends to the H read prefix. Preserve that prefix's
        // payload owners instead of copying it into another phase snapshot.
        let prefix_len = self.out.body.resolution.reads.len();
        std::sync::Arc::make_mut(&mut self.out.body.resolution.reads)
            .extend(self.out.reads[prefix_len..].iter().cloned());
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
    pub(super) fn transparent_value(&self, mut e: ExprId) -> ExprId {
        while let ExprKind::Block {
            label: None,
            tail: Some(tail),
            ..
        } = self.tree.expressions[e.0 as usize].kind
        {
            e = tail;
        }
        e
    }
    fn result_span(&self, e: ExprId) -> Span {
        self.span(self.transparent_value(e))
    }
    fn span(&self, e: ExprId) -> Span {
        self.tree.expressions[e.0 as usize].origin.span
    }
    fn error(&mut self, rule: RuleId, span: Span, message: &str) {
        if self.out.failure.is_none() && !self.failed_read {
            let purity =
                self.in_clause && (rule.as_str().starts_with("CAP-") || rule.as_str() == "CLO-1");
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
            let rule = if purity {
                const { RuleId::of("FACT-1") }
            } else {
                rule
            };
            self.emit(Diagnostic::error(rule, span, message));
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
            span.anchor,
            span.start,
            span.end,
            diagnostic.rule.as_str().to_owned(),
        ));
        self.out.diagnostics.push(diagnostic);
    }
    fn read(&mut self, d: DefId) -> Option<SignatureRef> {
        let result = self.reader.borrow_mut().read(d);
        self.record_reads();
        result.ok()
    }
    /// Record the first reads made since the last call, in read order, the
    /// memo's included: each one's dependency, diagnostics or failure.
    pub(super) fn record_reads(&mut self) {
        let pending = self.reader.borrow_mut().take_pending();
        for (d, result) in pending {
            self.out.reads.push(
                DependencyRead::Signature {
                    definition: d,
                    fingerprint: result.as_ref().ok().map(|s| s.fingerprint),
                }
                .shared(),
            );
            match result {
                Ok(s) => self.out.dependencies.push(SignatureDependency {
                    definition: d,
                    fingerprint: s.fingerprint,
                }),
                Err(QueryError::Diagnostics(ds)) => {
                    self.failed_read = true;
                    for diagnostic in ds {
                        self.emit(diagnostic);
                    }
                }
                Err(e) => {
                    self.failed_read = true;
                    self.out.failure.get_or_insert(e);
                }
            }
        }
    }
    fn ty(&self, e: ExprId) -> Option<TypeId> {
        self.elaborations
            .get(&e)
            .map(|i| self.out.body.expressions[*i].ty)
    }
    /// Intern `row` as one of this body's rows.
    pub(super) fn intern(&mut self, row: Type) -> TypeId {
        let id = self.t.intern(row);
        self.rows.note(self.t, id);
        id
    }
    fn set_ty(&mut self, e: ExprId, ty: TypeId) {
        self.rows.note(self.t, ty);
        if let Some(i) = self.elaborations.get(&e).copied() {
            self.out.body.expressions[i].ty = ty
        }
    }
    /// Preserve a construction's chosen storage before joining its parts.
    fn construction_destination(&mut self, e: ExprId) {
        let Some(ty) = self.ty(e) else { return };
        let destination = if self.property(ty, Bound::Plain) {
            // Plain values have no fresh blocks; written placement is inert.
            Destination::Inferred
        } else {
            destination(&self.t.get(ty).unwrap().modes.relation, |o| {
                self.arena_place(o)
            })
        };
        if let Some(index) = self.elaborations.get(&e).copied() {
            self.out.body.expressions[index].construction_destination = Some(destination);
        }
    }
    /// A fresh or explicitly detached value owns its blocks alone. Plain bits
    /// own none: their rows keep the one canonical spelling.
    fn set_unique(&mut self, e: ExprId) {
        let Some(t) = self.ty(e) else { return };
        if self.property(t, Bound::Plain) {
            return;
        }
        let mut row = self.t.get(t).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let t = self.intern(row.clone());
        row.modes.sendability = self.proof(Sendability::NotSendable, |memo| {
            hirsel_modes::proofs::sendability(memo, t)
        });
        let t = self.intern(row);
        self.set_ty(e, t);
    }
    fn valid(&self, e: ExprId) -> bool {
        self.ty(e).is_some_and(|t| {
            self.t
                .get(t)
                .is_some_and(|t| !matches!(t.kind, TypeKind::Error))
        })
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
    /// CONC-2: a parameter taking a `Task` or `TaskSet` holds its caller's obligation, keyed
    /// by an expression identity past the body's own.
    fn parameter_tasks(&mut self) {
        let (ItemKind::Function { signature, .. }, SignatureKind::Function(f)) =
            (&self.syntax.item.kind, &self.signature.kind)
        else {
            return;
        };
        for (i, (p, syntax)) in f.parameters.iter().zip(&signature.parameters).enumerate() {
            if p.passing != PassingMode::Take || !calls::task_obligation(&self.t, p.ty) {
                continue;
            }
            let task = ExprId(u32::MAX - i as u32);
            self.pending_tasks
                .insert(task, self.tree.types[syntax.ty.0 as usize].origin.span);
            self.task_bindings
                .insert((NameSite::Item, 0, i as u32), BTreeSet::from([task]));
        }
    }
    fn check_pending_tasks(&mut self) {
        for span in self.pending_tasks.values().copied().collect::<Vec<_>>() {
            self.error(
                const { RuleId::of("CONC-2") },
                span,
                "every task handle must be awaited or explicitly detached",
            );
        }
    }
    /// A construction with no placement of its own takes its expected
    /// row's: `[]` and `map.empty()` in arena `a` are allocated in `a`.
    fn place_construction(&mut self, e: ExprId, expected: TypeId) -> bool {
        let placement = self.placement(&self.t.get(expected).unwrap().modes.relation);
        let Some(actual) = self.ty(e) else {
            return false;
        };
        let mut ty = self.t.get(actual).unwrap().clone();
        if placement.is_global() {
            return false;
        }
        if self.placement(&ty.modes.relation).is_global() {
            ty.modes.relation = ty.modes.relation.union(&placement);
            let ty = self.intern(ty);
            self.set_ty(e, ty);
        }
        true
    }
    fn falls_through(&self, e: ExprId) -> bool {
        match &self.tree.expressions[e.0 as usize].kind {
            ExprKind::Return(_) | ExprKind::Break { .. } | ExprKind::Continue(_) => false,
            // A break that leaves a labelled block continues after it (LANG-11).
            ExprKind::Block { label: Some(_), .. } => true,
            ExprKind::Block {
                statements, tail, ..
            } => {
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
    /// Kept out of `walk`, whose frame recursion depth multiplies: the entry
    /// state is boxed.
    #[inline(never)]
    fn enter_block(
        &mut self,
        label: hirsel_intern::Symbol,
        context: Option<(CaptureContext, Option<TypeId>, RuleId)>,
    ) -> Box<PathState> {
        self.block_exits.push(BlockExit {
            label,
            context,
            paths: vec![],
        });
        Box::new(self.path_state(None))
    }
    #[inline(never)]
    fn leave_block(&mut self, target: usize, value: Option<ExprId>) {
        let path = self.path_state(value);
        self.block_exits[target].paths.push(path);
    }
    fn path_state(&self, value: Option<ExprId>) -> PathState {
        PathState {
            value,
            binding_types: self.binding_types.clone(),
            identities: self.identities.clone(),
            identity_changes: self.identity_changes.clone(),
            invalid: self.invalid.clone(),
            pending_tasks: self.pending_tasks.clone(),
        }
    }
    /// A labelled block's result joins its fallthrough path, when `tail` is
    /// `Some`, with each break that leaves it, as a match joins its arms
    /// (LANG-11).
    #[inline(never)]
    fn join_block(
        &mut self,
        e: ExprId,
        before: Box<PathState>,
        tail: Option<Option<ExprId>>,
        breaks: Vec<PathState>,
    ) {
        if breaks.is_empty() {
            return;
        }
        let paths: Vec<_> = tail
            .map(|tail| self.path_state(tail))
            .into_iter()
            .chain(breaks)
            .collect();
        let results: Vec<_> = paths.iter().filter_map(|p| p.value).collect();
        self.pending_tasks = BTreeMap::new();
        self.identity_changes = before.identity_changes.clone();
        self.invalid = before.invalid.clone();
        for path in &paths {
            self.pending_tasks.extend(path.pending_tasks.clone());
            self.identity_changes.extend(path.identity_changes.clone());
            self.invalid.extend(path.invalid.clone());
        }
        let mut paths = paths.into_iter();
        let first = paths.next().expect("a break reaches the join");
        let rest: Vec<_> = paths.collect();
        self.identities = first.identities.clone();
        for path in &rest {
            self.join_identities(
                NameSite::Expression(e),
                &before.identities,
                &path.identities,
            );
        }
        if !results.is_empty() {
            self.join_expression(e, &results);
        }
        self.binding_types = before.binding_types.clone();
        for key in before.binding_types.changed_keys(&[&first.binding_types]) {
            if let Some(actual) = first.binding_types.get(&key) {
                self.binding_types.insert(key, *actual);
            }
        }
        for path in &rest {
            self.join_bindings(&before.binding_types, &path.binding_types);
        }
    }
    fn proof_failure<T>(&mut self, error: QueryError, fallback: T) -> T {
        if self.out.failure.is_none() {
            self.out.failure = Some(error);
        }
        fallback
    }
    /// Whole transfers carry ended brands into owned storage. Reading an
    /// expired value, including a projection, is PROV-1 at the read.
    /// A call that itself reports the ended owner at a branded use owns that
    /// failure (section 23), so its arguments' reads are not reported again.
    fn expr(
        &mut self,
        e: ExprId,
        context: Option<(CaptureContext, Option<TypeId>, RuleId)>,
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
                        let inactive = self.t.get(ty).unwrap().brand.alternatives.is_empty()
                            && self.brandless_variant(e, ty).is_some();
                        if !inactive {
                            let mut diagnostic = Diagnostic::error(
                                const { RuleId::of("PROV-1") },
                                span,
                                "the value's brand owner has ended",
                            );
                            self.ended_explanation(ended, span).render(&mut diagnostic);
                            self.ended_reads.push(diagnostic);
                        }
                    }
                }
            }
            ExprKind::Unary {
                op: UnaryOp::Take | UnaryOp::Copy,
                value,
            } => {
                // PROV-1/PROV-3 (brands.md): an expired File of dir may
                // move whole into local storage, but cannot enter a call.
                // Only taken data graphs can forget expiry at that boundary;
                // branded and generic receiving contracts are checked by call.
                if self.valid(e)
                    && let Some(actual) = self.ty(e)
                    && let Some(ended) = self.ended(actual)
                    && matches!(
                        self.tree.expressions[value.0 as usize].kind,
                        ExprKind::Path(_)
                    )
                    && self.source_place(value).projections.is_empty()
                    && let Some(passing) = self.out.body.calls.iter().find_map(|call| {
                        call.arguments.iter().find_map(|argument| {
                            (self.transparent_value(argument.expression) == e)
                                .then_some(argument.parameter.as_ref())
                                .flatten()
                                .map(|(_, p)| p.passing)
                        })
                    })
                {
                    let data = passing == PassingMode::Take && {
                        let types = self.t;
                        let generics = self.declarations();
                        structural_equality(types, actual, Bound::Data, &generics, &mut |path| {
                            Ok::<_, QueryError>(self.read(path))
                        })
                        .unwrap_or_else(|error| self.proof_failure(error, false))
                    };
                    if !data {
                        let span = self.span(value);
                        let mut diagnostic = Diagnostic::error(
                            const { RuleId::of("PROV-1") },
                            span,
                            "the value's brand owner has ended",
                        );
                        self.ended_explanation(ended, span).render(&mut diagnostic);
                        self.ended_reads.push(diagnostic);
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
        let mut diagnostic = Diagnostic::error(const { RuleId::of("PROV-1") }, span, message);
        self.ended_explanation(ended, span).render(&mut diagnostic);
        self.emit(diagnostic);
    }
    /// A brand whose owner's identity ended; uses of one ended identity fold.
    fn ended_explanation(&self, ended: Span, used: Span) -> Explanation {
        Explanation {
            root: Some(hirsel_modes::explain::root_key(
                self.signature.definition.path(),
                ("ended", ended.start, ended.end),
            )),
            ..Explanation::default()
        }
        .provided(Fact::new(Cause::IdentityEnded, ended, &[]))
        .required(Fact::new(Requirement::BrandedUse, used, &[]))
    }
    fn walk(&mut self, e: ExprId, context: Option<(CaptureContext, Option<TypeId>, RuleId)>) {
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
            let expected = context.and_then(|(_, expected, _)| expected);
            self.call(e, callee, &arguments, expected);
            // PROV-4: `map.empty()`, `set.empty()` and `sorted_map.empty(by)` construct where their
            // relation places them, as a literal does.
            if let (
                Some(expected),
                Some(CheckedCall {
                    target: CallTarget::Definition(path),
                    ..
                }),
            ) = (expected, self.call_row(e))
                && hirsel_types::keyed_constructor(path).is_some()
                && self.place_construction(e, expected)
            {
                // The signature's result is sendable; a placed one is not.
                let mut row = self.t.get(self.ty(e).unwrap()).unwrap().clone();
                row.modes.sendability = Sendability::NotSendable;
                let ty = self.intern(row);
                self.set_ty(e, ty);
            }
            return;
        }
        let mut used = AuthoritySummary::inert();
        if matches!(
            kind,
            ExprKind::Literal(_)
                | ExprKind::Sequence(_)
                | ExprKind::Tuple(_)
                | ExprKind::Unary {
                    op: UnaryOp::Box,
                    ..
                }
        ) {
            if let Some((_, Some(expected), _)) = &context {
                let _ = self.place_construction(e, *expected);
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
        if matches!(
            kind,
            ExprKind::Sequence(_)
                | ExprKind::Tuple(_)
                | ExprKind::Unary {
                    op: UnaryOp::Box,
                    ..
                }
        ) {
            self.construction_destination(e);
        }
        match kind {
            ExprKind::Block {
                label,
                statements,
                tail,
            } => {
                let entry = label.map(|label| self.enter_block(label, context.clone()));
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
                    // A labelled block's value comes from its breaks when its
                    // tail diverges (LANG-11).
                    if label.is_none() || self.falls_through(tail) {
                        self.copy_value_type(e, tail);
                        self.task_values.insert(e, self.task_handles(tail));
                    }
                }
                if let Some(entry) = entry {
                    let exit = self.block_exits.pop().expect("walked labelled block");
                    let falls = reachable && tail.is_none_or(|tail| self.falls_through(tail));
                    self.join_block(e, entry, falls.then_some(tail), exit.paths);
                } else if let Some(tail) = tail.filter(|_| reachable) {
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
                        .filter(|root| matches!(root, OriginRoot::Expression(..)))
                        .filter(|root| !self.identities.held_outside(root, &keys))
                        .cloned()
                        .collect();
                    let own = match b.binding {
                        ResolvedName::Local(l) => Some(OriginRoot::Local(self.lexical, l)),
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
                let mut invalid = HashMap::default();
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
                        self.bind_pattern(arm.pattern, actual, value, false);
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
                        let mut carried = None;
                        for projection in projections {
                            let Some((fields, index)) =
                                self.projection_fields(ty, &projection.kind)
                            else {
                                complete = false;
                                break;
                            };
                            if let Some(bound) = self.carrier_bound(ty) {
                                carried = Some(bound);
                            }
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
                            ty = super::facts::siblings(self.t, fields[index].ty, &replacements);
                            if let Some(o) = &mut origin {
                                o.fields.push(FieldId(index as u32));
                            }
                        }
                        if !projected {
                            self.set_ty(e, ty);
                        } else if complete {
                            self.refine_projection(e, ty);
                            self.localize_view_part(e, Some(binding_key(&b)));
                            if let Some(bound) = carried {
                                self.carrier_places.insert(e, bound);
                            }
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
                if self.direct_callee != Some(e)
                    && self.elaborations.get(&e).is_some_and(|i| {
                        !self.out.body.expressions[*i]
                            .instantiated_arguments
                            .is_empty()
                    })
                {
                    let capture_free = self.capture_free_generic(e);
                    self.instantiate_function_reference(e);
                    if capture_free {
                        let ty = self.ty(e).unwrap();
                        let mut row = self.t.get(ty).unwrap().clone();
                        row.modes.relation = Relation::global();
                        row.modes.sendability = Sendability::Sendable;
                        let ty = self.intern(row);
                        self.set_ty(e, ty);
                        self.forwarding_closure(e);
                    } else {
                        self.closure(
                            e,
                            e,
                            context.map(|(_, expected, rule)| {
                                (CaptureContext::Scoped, expected, rule)
                            }),
                        );
                    }
                }
                used = if matches!(self.resolved.get(&e), Some(ResolvedName::Definition(_)))
                    && self.ty(e).is_some_and(|t| {
                        matches!(self.t.get(t).unwrap().kind, TypeKind::Function(_))
                    }) {
                    // A direct callee names its definition; only a value
                    // use is a closure.
                    if self.direct_callee != Some(e) {
                        self.instantiate_function_reference(e);
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
                    if let Some((owner, derived, _)) = self.derivations.get(&binding_key(&binding))
                    {
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
                    let parts = self.t.get(t).unwrap().kind.value_parts();
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
                            self.t.get(*part).is_some_and(|t| {
                                matches!(t.kind, TypeKind::Function(_))
                                    && crossing_requirement(t) == Sendability::Sendable
                            })
                        })
                        .map(|part| {
                            (
                                CaptureContext::Escaping,
                                Some(part),
                                const { RuleId::of("CLO-4") },
                            )
                        });
                    self.expr(child, contextual);
                    used = used.join(self.used(child));
                    if let (Some((_, parts)), Some(now)) = (&mut refined, self.ty(child)) {
                        let constructed = self.construction_part_type(child).unwrap_or(now);
                        if parts.get(ordinal).copied() == written {
                            parts[ordinal] = constructed;
                        } else if let Some(part) = parts.get(ordinal).copied() {
                            // A part typed by its expectation still holds the
                            // operand's identity: the expected row cannot
                            // give or take a brand (PROV-2).
                            let mut row = self.t.get(part).unwrap().clone();
                            let brand = &self.t.get(constructed).unwrap().brand;
                            if row.brand != *brand {
                                row.brand = brand.clone();
                                parts[ordinal] = self.intern(row);
                            }
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
                                .lifetime(&self.t.get(t).unwrap().modes.relation)
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
                    let mut row = self.t.get(owner).unwrap().clone();
                    row.kind = TypeKind::Tuple(refined.into());
                    let ty = self.intern(row);
                    self.set_ty(e, ty);
                }
                if let Some(actual) = self.ty(e) {
                    let evidence = self.t.get(actual).unwrap().modes.sendability.clone();
                    let receipt =
                        declared_receipt_evidence(self.t, actual).expect("checked literal parts");
                    let mut row = self.t.get(receipt).unwrap().clone();
                    row.modes.sendability = evidence;
                    let receipt = self.intern(row);
                    self.set_ty(e, receipt);
                }
                // PROV-2: a literal is a new value. The expected type it was
                // checked against cannot give it an identity it never had.
                if let Some(ty) = self.ty(e) {
                    let mut row = self.t.get(ty).unwrap().clone();
                    if !row.brand.alternatives.is_empty() {
                        row.brand = Brand::default();
                        let ty = self.intern(row);
                        self.set_ty(e, ty);
                    }
                }
            }
            ExprKind::Unary { op, value } => {
                let inferred_box = op == UnaryOp::Box
                    && self.ty(e).is_some_and(|owner| {
                        matches!(self.t.get(owner).unwrap().kind,
                            TypeKind::Box(part) if Some(part) == self.ty(value))
                    });
                // An ownership marker passes its operand on unchanged, so a
                // marked callback literal is still built for the parameter.
                let marker = matches!(op, UnaryOp::Take | UnaryOp::Copy | UnaryOp::Mut);
                let operand_context = if op == UnaryOp::Box {
                    context.and_then(|(capture, expected, rule)| {
                        let owner = expected?;
                        let TypeKind::Box(part) = self.t.get(owner)?.kind else {
                            return None;
                        };
                        Some((capture, Some(self.placed_item(owner, part)), rule))
                    })
                } else {
                    context.filter(|_| marker)
                };
                // A whole transfer preserves evidence, including expiry. A
                // projection still reads the selected part and is judged below.
                if matches!(op, UnaryOp::Take | UnaryOp::Copy)
                    && matches!(
                        self.tree.expressions[value.0 as usize].kind,
                        ExprKind::Path(_)
                    )
                    && self.source_place(value).projections.is_empty()
                {
                    self.unread.insert(value);
                }
                self.expr(value, operand_context);
                used = self.used(value);
                if inferred_box {
                    let payload = self.construction_part_type(value).unwrap();
                    let mut owner = self.t.get(self.ty(e).unwrap()).unwrap().clone();
                    owner.kind = TypeKind::Box(payload);
                    let owner = self.intern(owner);
                    self.set_ty(e, owner);
                }
                if op == UnaryOp::Box {
                    let owner = self.ty(e).unwrap();
                    if let TypeKind::Box(part) = self.t.get(owner).unwrap().kind {
                        let expected = self.placed_item(owner, part);
                        let actual = self.ty(value).unwrap();
                        self.kept(
                            actual,
                            expected,
                            self.span(value),
                            value,
                            false,
                            Receiving::new("the boxed payload", self.span(e)),
                            false,
                        );
                    }
                }
                // Copies preserve provenance but counted copies lose uniqueness.
                if matches!(op, UnaryOp::Take | UnaryOp::Copy | UnaryOp::Mut) {
                    self.copy_value_type(e, value);
                    if op == UnaryOp::Copy {
                        if let Some(t) = self.ty(e) {
                            let mut row = self.t.get(t).unwrap().clone();
                            row.sendability_requirement = SendabilityRequirement::None;
                            let t = self.intern(row);
                            self.set_ty(e, t);
                        }
                        // Handle-only copies own independent counts on immutable
                        // roots or run-owned cells; they retain no borrowed storage.
                        if let Some(t) = self.ty(e) {
                            let row = self.t.get(t).unwrap().clone();
                            if !self.lifetime(&row.modes.relation).is_global()
                                && self.copy_detaches(t)
                            {
                                let mut row = row;
                                row.modes.relation = self.placement(&row.modes.relation);
                                let t = self.intern(row);
                                self.set_ty(e, t);
                            }
                        }
                        if let Some(t) = self.ty(e) {
                            if !self.property(t, Bound::Plain) && !self.property(t, Bound::Flat) {
                                let mut ty = self.t.get(t).unwrap().clone();
                                ty.modes.uniqueness = Uniqueness::Aliased;
                                let root = self.intern(ty.clone());
                                ty.modes.sendability =
                                    self.proof(Sendability::NotSendable, |memo| {
                                        hirsel_modes::proofs::sendability_after_sharing(memo, root)
                                    });
                                let t = self.intern(ty);
                                self.set_ty(e, t);
                            }
                        }
                    }
                }
                if op == UnaryOp::Unique {
                    self.copy_value_type(e, value);
                    // A unique value is independent of the lifetime of the
                    // operand it was extracted from. An operand local to an
                    // own root or environment is borrowed, so `unique` copies
                    // it deeply, and a deep copy holds no loan on bindings or
                    // own roots (MODE-1). One extracted in an arena or region
                    // is bounded by that placement (MEM-5), which PROV-4
                    // judges where the value is kept. A function value or a
                    // type parameter's value may hold borrowed captures a deep
                    // copy keeps. An owned operand's relation is I's: it moves
                    // when already unique.
                    if let Some(t) = self.ty(e) {
                        let mut ty = self.t.get(t).unwrap().clone();
                        if self.lifetime(&ty.modes.relation).has_environment()
                            && !matches!(ty.kind, TypeKind::Function(_) | TypeKind::Parameter(_))
                        {
                            let placement = self.placement(&ty.modes.relation);
                            if placement != ty.modes.relation {
                                ty.modes.relation = placement;
                                let t = self.intern(ty);
                                self.set_ty(e, t);
                            }
                        }
                    }
                    self.set_unique(e);
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
                                let ty = super::facts::siblings(self.t, field.ty, &siblings);
                                let ty = self.projected_item(base_ty, ty);
                                self.refine_projection(e, ty);
                                if let Some(bound) = self.carrier_bound(base_ty) {
                                    self.carrier_places.insert(e, bound);
                                }
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
                    } = &self.t.get(sequence).unwrap().kind
                    {
                        if let Some(GenericArgument::Type(contract)) = arguments.first() {
                            let contract = *contract;
                            let branded = self.part_brands(element, contract);
                            let row = self.t.get(branded).unwrap();
                            let brand = &self.t.get(contract).unwrap().brand;
                            if row.brand.alternatives.is_empty() && brand.alternatives.len() == 1 {
                                let mut row = row.clone();
                                row.brand = brand.clone();
                                let branded = self.intern(row);
                                self.set_ty(e, branded);
                            } else if branded != element {
                                self.set_ty(e, branded);
                            }
                            // PROV-4, OWN-8: an element lives where its
                            // sequence does, as a field of a record does.
                            let placed = self.projected_item(sequence, contract);
                            let mut row = self.t.get(placed).unwrap().clone();
                            row.brand = Brand::default();
                            let placed = self.intern(row);
                            self.refine_projection(e, placed);
                        }
                    }
                }
                self.localize_view_part(e, self.binding(base).map(|b| binding_key(&b)));
                if self.ty(base).is_some_and(|t| {
                    matches!(&self.t.get(t).unwrap().kind,
                        TypeKind::Builtin { kind: BuiltinType::List | BuiltinType::Array | BuiltinType::Inline, arguments }
                        if arguments.first().is_some_and(|a| matches!(a, GenericArgument::Type(t)
                            if matches!(self.t.get(*t).unwrap().kind,
                                TypeKind::Builtin { kind: BuiltinType::Packed, .. }))))
                }) {
                    self.explained(
                        const { RuleId::of("MEM-13") },
                        self.span(e),
                        "a sequence of Packed values is sequential and cannot be indexed",
                        Explanation::default()
                    .provided(Fact::new(Cause::Packed, self.span(base), &[]))
                    .required(Fact::new(Requirement::Index, self.span(e), &[])),
                    );
                }
            }
            ExprKind::Binary { op, left, right } => {
                self.expr(left, None);
                self.expr(right, None);
                if matches!(
                    op,
                    hirsel_syntax::BinaryOp::Eq | hirsel_syntax::BinaryOp::Ne
                ) {
                    if let Some(peer) = self.ty(left) {
                        self.comparison_variant(right, peer);
                    }
                    if let Some(peer) = self.ty(right) {
                        self.comparison_variant(left, peer);
                    }
                }
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
                    if let TypeKind::Named { arguments, .. } = &self.t.get(source).unwrap().kind {
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
            ExprKind::Break {
                label: Some(label),
                value,
            } if let Some(target) = self.block_exits.iter().rposition(|b| b.label == label) => {
                if let Some(value) = value {
                    let context = self.block_exits[target].context.clone();
                    self.expr(value, context);
                    used = self.used(value);
                }
                if value.is_none_or(|value| self.falls_through(value)) {
                    self.leave_block(target, value);
                }
            }
            ExprKind::Break { label, .. } | ExprKind::Continue(label) => {
                if let Some(target) = self.loop_task_exits.iter().rposition(|(statement, _)| {
                    label.is_none()
                        || self.tree.statements[statement.0 as usize].kind.loop_label() == label
                }) {
                    self.loop_task_exits[target]
                        .1
                        .extend(std::mem::take(&mut self.pending_tasks));
                }
            }
            ExprKind::Return(value) => {
                if let Some(value) = value {
                    let expected = self.frames.last().and_then(|f| f.result);
                    self.expr(
                        value,
                        Some((
                            CaptureContext::Escaping,
                            expected,
                            const { RuleId::of("CLO-4") },
                        )),
                    );
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
            // PROV-4: carry payload placement at every enclosing depth so
            // capture rows and structural comparisons see the same owner.
            if let Some(root) = self.ty(e) {
                let mut row = self.t.get(root).unwrap().clone();
                for part in &parts {
                    if let Some(ty) = self.ty(*part) {
                        if !self.property(ty, Bound::Plain) {
                            row.modes.relation = row
                                .modes
                                .relation
                                .union(&self.placement(&self.t.get(ty).unwrap().modes.relation));
                        }
                    }
                }
                let root = self.intern(row);
                self.set_ty(e, root);
            }
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
    fn used(&self, e: ExprId) -> AuthoritySummary {
        self.facts
            .get(&e)
            .cloned()
            .unwrap_or_else(AuthoritySummary::inert)
    }
    /// Replay the existing transfer rules until the loop-header judgments agree.
    /// Type rows are immutable and may be freshly allocated during replay, so
    /// convergence compares semantic types and expiration, not arena indices.
    fn loop_body(&mut self, site: NameSite, body: ExprId, condition: Option<ExprId>) {
        let NameSite::Statement(target) = site else {
            unreachable!("loop body belongs to a statement")
        };
        let (falls, continues) = self.loop_control(body, target, 0);
        let incoming = self.identities.clone();
        loop {
            // As in Salsa's shallow verification (maybe_changed_after.rs:358-387),
            // skip derived work only while all inputs of that work are unchanged.
            // Rows already supplies persistent snapshots; expiration additionally
            // depends on the complete invalidation map, not just a binding's id.
            let before = self.binding_types.clone();
            let invalid = self.invalid.clone();
            let identities = self.identities.clone();
            let identity_changes = self.identity_changes.clone();
            self.renew_loop_locals(body);
            self.expr(body, None);
            if !falls && !continues {
                break;
            }
            self.join_identities(site.clone(), &incoming, &incoming);
            if let Some(condition) = condition {
                self.expr(condition, None);
            }
            let same_invalid = invalid == self.invalid;
            let unchanged = identities == self.identities
                && identity_changes == self.identity_changes
                && before.len() == self.binding_types.len()
                && before.iter().all(|(key, old)| {
                    self.binding_types.get(key).is_some_and(|new| {
                        self.t.semantic_eq(*old, *new)
                            && ((*old == *new && same_invalid)
                                || self.ended_in(*old, &invalid).is_some()
                                    == self.expired_type(*new))
                    })
                });
            if unchanged {
                break;
            }
        }
    }
    /// Fallthrough and continues selecting this loop reach its header. Nested
    /// loops hide unlabelled exits; labelled continues can cross them. Callable
    /// bodies start independent loop scopes.
    fn loop_control(
        &self,
        expression: ExprId,
        target: hirsel_syntax::StmtId,
        nesting: usize,
    ) -> (bool, bool) {
        fn sequence(
            checker: &Checker<'_>,
            target: hirsel_syntax::StmtId,
            nesting: usize,
            expressions: impl IntoIterator<Item = ExprId>,
        ) -> (bool, bool) {
            let (mut next, mut back) = (true, false);
            for expression in expressions {
                if !next {
                    break;
                }
                let flow = checker.loop_control(expression, target, nesting);
                next = flow.0;
                back |= flow.1;
            }
            (next, back)
        }
        match &self.tree.expressions[expression.0 as usize].kind {
            ExprKind::Continue(label) => (
                false,
                match label {
                    Some(label) => {
                        self.tree.statements[target.0 as usize].kind.loop_label() == Some(*label)
                    }
                    None => nesting == 0,
                },
            ),
            ExprKind::Break { value, .. } | ExprKind::Return(value) => (
                false,
                value.is_some_and(|value| self.loop_control(value, target, nesting).1),
            ),
            ExprKind::Closure { .. } => (true, false),
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let prefix = self.loop_control(*condition, target, nesting);
                if !prefix.0 {
                    return prefix;
                }
                let a = self.loop_control(*then_branch, target, nesting);
                let b = else_branch.map_or((true, false), |branch| {
                    self.loop_control(branch, target, nesting)
                });
                (a.0 || b.0, prefix.1 || a.1 || b.1)
            }
            ExprKind::Match { value, arms } => {
                let prefix = self.loop_control(*value, target, nesting);
                if !prefix.0 {
                    return prefix;
                }
                arms.iter().fold((false, prefix.1), |flow, arm| {
                    let branch = sequence(
                        self,
                        target,
                        nesting,
                        arm.guard.into_iter().chain(std::iter::once(arm.body)),
                    );
                    (flow.0 || branch.0, flow.1 || branch.1)
                })
            }
            ExprKind::Block {
                label,
                statements,
                tail,
            } => {
                let (mut next, mut back) = (true, false);
                for statement in statements {
                    if !next {
                        break;
                    }
                    let flow = match &self.tree.statements[statement.0 as usize].kind {
                        StmtKind::Expr(value) => self.loop_control(*value, target, nesting),
                        StmtKind::Let {
                            value, otherwise, ..
                        } => {
                            let flow = self.loop_control(*value, target, nesting);
                            match otherwise {
                                // A let-else continues past a matched pattern.
                                Some(otherwise) if flow.0 => {
                                    let refused = self.loop_control(*otherwise, target, nesting);
                                    (true, flow.1 || refused.1)
                                }
                                _ => flow,
                            }
                        }
                        StmtKind::Assign { place, value, .. } => {
                            sequence(self, target, nesting, [*place, *value])
                        }
                        StmtKind::For { source, body, .. } => {
                            let prefix = self.loop_control(*source, target, nesting);
                            let inner = self.loop_control(*body, target, nesting + 1);
                            (prefix.0, prefix.1 || (prefix.0 && inner.1))
                        }
                        StmtKind::While {
                            condition, body, ..
                        } => {
                            let prefix = self.loop_control(*condition, target, nesting + 1);
                            let inner = self.loop_control(*body, target, nesting + 1);
                            (prefix.0, prefix.1 || (prefix.0 && inner.1))
                        }
                        StmtKind::Function { .. } => (true, false),
                    };
                    next = flow.0;
                    back |= flow.1;
                }
                if next {
                    if let Some(tail) = tail {
                        let flow = self.loop_control(*tail, target, nesting);
                        next = flow.0;
                        back |= flow.1;
                    }
                }
                // A break that leaves a labelled block continues after it (LANG-11).
                (next || label.is_some(), back)
            }
            _ => sequence(self, target, nesting, self.expression_children(expression)),
        }
    }
    fn statement(&mut self, s: hirsel_syntax::StmtId) -> AuthoritySummary {
        let stmt = self.tree.statements[s.0 as usize].clone();
        match stmt.kind {
            StmtKind::Let {
                pattern,
                ty,
                value,
                otherwise,
                ..
            } => {
                let written_locality = self
                    .pattern_row(pattern)
                    .and_then(|p| p.annotation)
                    .is_some_and(|written| written.relation);
                let expected = self.pattern_row(pattern).map(|p| p.ty);
                let expected = match (expected, ty) {
                    (Some(expected), Some(annotation)) => {
                        let mut row = self.t.get(expected).unwrap().clone();
                        row.brand = Brand::default();
                        let expected = self.intern(row);
                        let expected = self.annotation(expected, annotation);
                        Some(self.within_bindings(expected, annotation))
                    }
                    _ => expected,
                };
                let context = if ty.is_some()
                    && expected.is_some_and(|t| {
                        matches!(self.t.get(t).map(|t| &t.kind), Some(TypeKind::Function(_)))
                    }) {
                    Some((
                        CaptureContext::Escaping,
                        expected,
                        const { RuleId::of("CLO-4") },
                    ))
                } else {
                    // An inferred function binding supplies scope, not a
                    // declared callable/result promise. Keep literal inference
                    // and its own written uses clause independent of that row.
                    let expected = expected
                        .filter(|t| !matches!(self.t.get(*t).unwrap().kind, TypeKind::Function(_)));
                    Some((
                        CaptureContext::Scoped,
                        expected,
                        const { RuleId::of("CAP-3") },
                    ))
                };
                self.expr(value, context);
                // LANG-13: the else block runs before any binding exists.
                if let Some(otherwise) = otherwise {
                    self.expr(otherwise, None);
                }
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
                        let mut row = self.t.get(actual).unwrap().clone();
                        let contract = self.t.get(expected).unwrap();
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
                        let received = self.intern(row);
                        let received = declared_receipt_evidence(self.t, received)
                            .expect("checked binding receipt");
                        let mut row = self.t.get(received).unwrap().clone();
                        row.modes.sendability = evidence;
                        self.intern(row)
                    } else {
                        let mut row = self.t.get(actual).unwrap().clone();
                        row.sendability_requirement = SendabilityRequirement::None;
                        self.intern(row)
                    };
                    self.bind_pattern(pattern, actual, value, false);
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
                match otherwise {
                    Some(otherwise) => self.used(value).join(self.used(otherwise)),
                    None => self.used(value),
                }
            }
            StmtKind::Assign { place, value, .. } => {
                self.unread.insert(place);
                self.expr(place, None);
                self.expr(
                    value,
                    Some((
                        CaptureContext::Escaping,
                        self.ty(place),
                        const { RuleId::of("CLO-4") },
                    )),
                );
                if let Some(t) = self.ty(value) {
                    if self.scoped(t) {
                        self.explained(
                            const { RuleId::of("MODE-6") },
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
                if let (Some(bound), Some(value_ty)) = (self.carrier_place(place), self.ty(value))
                    && !self.authority(value_ty).is_subset_of(&bound)
                {
                    self.explained(
                        const { RuleId::of("CLO-4") },
                        stmt.origin.span,
                        "a write into an erased carrier must stay within the authority it carries",
                        Explanation::default()
                            .provided(Fact::new(Cause::Authority, self.span(value), &[]))
                            .required(Fact::new(Requirement::Uses, self.span(place), &[])),
                    );
                }
                if self.protected_field(place) {
                    let name = self.value_name(Some(place));
                    self.explained(
                        const { RuleId::of("PROV-5") },
                        stmt.origin.span,
                        "a sibling brand owner cannot be replaced separately from its record",
                        Explanation::default()
                            .provided(Fact::new(Cause::SiblingOwner, self.span(place), &[]))
                            .required(Fact::new(Requirement::Write, stmt.origin.span, &[&name])),
                    );
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
                self.forget_receipt(place);
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
                            ResolvedName::Local(local) => OriginRoot::Local(self.lexical, local),
                            ResolvedName::Parameter(parameter) => OriginRoot::Parameter(
                                ParameterRef::body(self.lexical.site(b.owner), parameter),
                            ),
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
                        let mut row = self.t.get(t).unwrap().clone();
                        if let Some(expected) = expected {
                            let contract = self.t.get(expected).unwrap();
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
                        let received = self.intern(row);

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
            StmtKind::While {
                condition, body, ..
            } => {
                self.loop_task_exits.push((s, BTreeMap::new()));
                self.expr(condition, None);
                let before = self.binding_types.clone();
                let pending = self.pending_tasks.clone();
                self.loop_body(NameSite::Statement(s), body, Some(condition));
                self.pending_tasks
                    .extend(self.loop_task_exits.pop().unwrap().1);
                self.pending_tasks.extend(pending);
                self.join_bindings(&before, &before);
                self.used(condition).join(self.used(body))
            }
            StmtKind::For {
                source,
                body,
                pattern,
                ..
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
                            let mut row = self.t.get(pattern_row.ty).unwrap().clone();
                            row.kind = TypeKind::Tuple(fields.to_vec().into());
                            let tuple = self.intern(row);
                            let item = self.projected_item(actual, tuple);
                            self.bind_pattern(pattern, item, source, true);
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
                                self.bind_pattern(pattern, item, source, true);
                            }
                        }
                    }
                }
                let pending = self.pending_tasks.clone();
                self.loop_task_exits.push((s, BTreeMap::new()));
                self.loop_body(NameSite::Statement(s), body, None);
                self.pending_tasks
                    .extend(self.loop_task_exits.pop().unwrap().1);
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
                    let original = self.t.get(binding.ty).unwrap().clone();
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
    /// H's comparison context can be a callee result template. Once J has
    /// substituted the other operand, a contextual nullary variant needs that
    /// caller-side instantiation too. It holds no payload or source lifetime.
    fn comparison_variant(&mut self, expression: ExprId, peer: TypeId) {
        match self.tree.expressions[expression.0 as usize].kind.clone() {
            ExprKind::Block {
                tail: Some(tail), ..
            } => {
                self.comparison_variant(tail, peer);
                self.copy_value_type(expression, tail);
                return;
            }
            ExprKind::Unary {
                op: UnaryOp::Copy | UnaryOp::Take,
                value,
            } => {
                self.comparison_variant(value, peer);
                self.copy_value_type(expression, value);
                return;
            }
            _ => (),
        }
        let variant = match self.tree.expressions[expression.0 as usize].kind {
            ExprKind::Path(_) => match self.resolved.get(&expression) {
                Some(ResolvedName::Variant(variant)) => *variant,
                _ => return,
            },
            ExprKind::Call { ref arguments, .. } if arguments.is_empty() => {
                let Some(CheckedCall {
                    target: CallTarget::Constructor(ContextualName::Variant { variant, .. }),
                    ..
                }) = self.call_row(expression)
                else {
                    return;
                };
                *variant
            }
            _ => return,
        };
        let Some(actual) = self.ty(expression) else {
            return;
        };
        if !self.t.shape_eq(actual, peer).unwrap_or(false) {
            return;
        }
        let Some(TypeKind::Variant(variants)) = self.layout(peer) else {
            return;
        };
        if !variants
            .get(variant.0 as usize)
            .is_some_and(|variant| variant.fields.is_empty())
        {
            return;
        }
        let mut row = self.t.get(peer).unwrap().clone();
        row.brand = Brand::default();
        row.modes.relation = Relation::global();
        let receipt = self.intern(row);
        self.set_ty(expression, receipt);
    }

    fn return_value(&mut self, value: ExprId, span: Span) {
        let Some(t) = self.ty(value) else { return };
        if self.scoped(t) {
            let result = self.sites().result(self.frames.last().unwrap().owner);
            self.explained(
                const { RuleId::of("MODE-6") },
                span,
                "a scoped capability cannot be returned",
                Explanation::default()
                    .provided(Fact::new(Cause::ScopedCapability, self.span(value), &[]))
                    .required(Fact::new(Requirement::ReturnValue, result, &[])),
            );
            return;
        }
        if let Some(frame) = self.frames.last() {
            let relation = self.t.get(t).unwrap().modes.relation.clone();
            for origin in relation.reaches().iter().filter_map(|r| match r {
                Reach::Place(o) if self.arena_place(o) => Some(o),
                _ => None,
            }) {
                if matches!(frame.owner, NameSite::Expression(_))
                    && matches!(origin.root, OriginRoot::Parameter(ParameterRef::Body { scope, .. }) if scope == self.lexical.site(frame.owner))
                {
                    let (arena, site) = self.origin_site(origin);
                    let x = self.value_name(Some(value));
                    let callback = self.sites().owner_span(frame.owner);
                    self.explained(
                        const { RuleId::of("PROV-4") },
                        span,
                        "a value cannot escape the arena introduced by this callback",
                        Explanation::default()
                            .provided(Fact::new(
                                Cause::Placed,
                                site.unwrap_or(span),
                                &[
                                    &x,
                                    &arena,
                                    &hirsel_modes::explain::qualifier(
                                        hirsel_modes::explain::Qualifier::In(&[&arena]),
                                    ),
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
}

impl super::cursors::CursorRefinement for Checker<'_> {
    fn types(&self) -> &TypeInterner {
        self.t
    }
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
    use crate::test_support::Fixture;

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
                let syntax = f.syntax(None);
                let generics = f.body.resolution.generic_declarations.clone();
                let reader = hirsel_modes::reader::Reader::new(&mut f.provider, &f.signature);
                let mut lookup = |d: DefId| Ok(reader.borrow_mut().read(d).ok());
                let memo = ContextMemo::new(f.types, &generics, &mut lookup);
                let mut c = Checker::new(
                    f.types,
                    &f.memo,
                    &syntax,
                    &f.signature,
                    f.body,
                    &reader,
                    memo,
                    &f.intrinsics,
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
            let syntax = f.syntax(None);
            let generics = f.body.resolution.generic_declarations.clone();
            let reader = hirsel_modes::reader::Reader::new(&mut f.provider, &f.signature);
            let mut lookup = |d: DefId| Ok(reader.borrow_mut().read(d).ok());
            let memo = ContextMemo::new(f.types, &generics, &mut lookup);
            let mut c = Checker::new(
                f.types,
                &f.memo,
                &syntax,
                &f.signature,
                f.body,
                &reader,
                memo,
                &f.intrinsics,
            );
            assert!(!c.contains_resource(root));
            assert_eq!(c.layout(root), Some(TypeKind::Primitive(Primitive::U8)));
            assert!(c.out.failure.is_none());
        }
    }
}
